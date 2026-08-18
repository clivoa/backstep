//! The versioned wire protocol.
//!
//! # Datagram layout
//!
//! ```text
//! offset  size  field
//!      0     2  magic  "RB"
//!      2     1  protocol version
//!      3     1  message kind
//!      4     4  sequence (u32 LE, per-sender, monotonic)
//!      8     n  payload
//!    8+n    32  HMAC-SHA256 over bytes [0, 8+n)
//! ```
//!
//! Design notes:
//!
//! * **Everything is fixed-width little-endian.** No varints, no length
//!   prefixes that could disagree with the actual buffer: the decoder consumes
//!   the payload exactly and rejects leftovers.
//! * **The version is inside the authenticated region.** A peer cannot be
//!   downgraded to an older parser by an attacker flipping byte 2.
//! * **1200 bytes is the hard cap.** It sits under the usual 1500-byte Ethernet
//!   MTU minus IPv6 and UDP headers and minus a tunnel's worth of slack, so a
//!   datagram never fragments. Fragmented UDP means one lost fragment kills the
//!   whole datagram, which is exactly the failure mode a rollback session can
//!   least afford.

use rollback_core::{Frame, PlayerHandle, PlayerInput, SimulationKind};

use crate::cursor::{Reader, Writer};

/// Bumped whenever the layout or semantics change incompatibly.
pub const PROTOCOL_VERSION: u8 = 1;
/// Hard cap on a serialised datagram, including the HMAC tag.
pub const MAX_DATAGRAM: usize = 1_200;
/// How many trailing inputs every `InputBatch` repeats.
pub const INPUT_REDUNDANCY: usize = 8;
/// Length of the authentication tag.
pub const TAG_LEN: usize = 32;
/// Length of the fixed header.
pub const HEADER_LEN: usize = 8;

const MAGIC: [u8; 2] = *b"RB";

const KIND_HELLO: u8 = 1;
const KIND_HELLO_ACK: u8 = 2;
const KIND_INPUT_BATCH: u8 = 3;
const KIND_CHECKSUM: u8 = 4;
const KIND_TELEMETRY: u8 = 5;
const KIND_DISCONNECT: u8 = 6;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WireError {
    #[error("datagram too short: {0} bytes")]
    TooShort(usize),
    #[error("datagram too large: {0} bytes (cap {MAX_DATAGRAM})")]
    TooLarge(usize),
    #[error("bad magic: expected 'RB'")]
    BadMagic,
    #[error("unsupported protocol version {got} (this build speaks {PROTOCOL_VERSION})")]
    BadVersion { got: u8 },
    #[error("unknown message kind {0}")]
    UnknownKind(u8),
    #[error("truncated payload: needed {need} more bytes, had {have}")]
    Truncated { need: usize, have: usize },
    #[error("{0} unexpected trailing bytes")]
    TrailingBytes(usize),
    #[error("input batch declares {0} inputs (cap {INPUT_REDUNDANCY})")]
    BatchTooLong(usize),
    #[error("invalid enum value {value} for {field}")]
    BadEnum { field: &'static str, value: u8 },
}

/// Identity a peer presents at handshake time.
///
/// Every field is something that, if the two peers disagreed on it, would make
/// their simulations diverge. Checking them up front turns a mid-match desync
/// into a refused connection with a readable reason.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PeerIdentity {
    pub protocol_version: u8,
    pub simulation: SimulationKind,
    /// Which side the sender intends to drive.
    pub player: PlayerHandle,
    /// Short git commit of the binary, as raw bytes.
    pub app_commit: [u8; 20],
    /// `SessionConfig::compatibility_hash`.
    pub config_hash: u64,
    pub seed: u64,
    /// SHA-256 of the libretro core, or zeroes for the arena.
    pub core_hash: [u8; 32],
    /// SHA-256 of the ROM, or zeroes for the arena.
    pub rom_hash: [u8; 32],
}

impl PeerIdentity {
    /// Bytes an identity occupies on the wire. Asserted in the tests so a
    /// field added without a version bump is caught rather than shipped.
    #[cfg(test)]
    const ENCODED_LEN: usize = 1 + 1 + 1 + 20 + 8 + 8 + 32 + 32;

    /// Everything that must match between the two peers, in order, so the
    /// rejection message can name the first thing that differs.
    pub fn compatible_with(&self, other: &PeerIdentity) -> Result<(), Incompatibility> {
        if self.protocol_version != other.protocol_version {
            return Err(Incompatibility::ProtocolVersion);
        }
        if self.simulation != other.simulation {
            return Err(Incompatibility::Simulation);
        }
        if self.app_commit != other.app_commit {
            return Err(Incompatibility::AppCommit);
        }
        if self.config_hash != other.config_hash {
            return Err(Incompatibility::Config);
        }
        if self.seed != other.seed {
            return Err(Incompatibility::Seed);
        }
        if self.core_hash != other.core_hash {
            return Err(Incompatibility::CoreHash);
        }
        if self.rom_hash != other.rom_hash {
            return Err(Incompatibility::RomHash);
        }
        if self.player == other.player {
            return Err(Incompatibility::PlayerSlot);
        }
        Ok(())
    }

    fn encode(&self, w: &mut Writer) {
        w.u8(self.protocol_version);
        w.u8(match self.simulation {
            SimulationKind::Arena => 0,
            SimulationKind::LastBlade2 => 2,
        });
        w.u8(self.player.index() as u8);
        w.bytes(&self.app_commit);
        w.u64(self.config_hash);
        w.u64(self.seed);
        w.bytes(&self.core_hash);
        w.bytes(&self.rom_hash);
    }

    fn decode(r: &mut Reader<'_>) -> Result<PeerIdentity, WireError> {
        let protocol_version = r.u8()?;
        let simulation = match r.u8()? {
            0 => SimulationKind::Arena,
            // 1 was Street Fighter Alpha 3, which this lab never managed to
            // run: the available romset is missing its CPS-2 decryption key.
            // The discriminant stays reserved so LastBlade2 keeps the byte it
            // has always had on the wire, and a peer still claiming 1 is
            // rejected rather than silently misread.
            2 => SimulationKind::LastBlade2,
            value => {
                return Err(WireError::BadEnum {
                    field: "simulation",
                    value,
                })
            }
        };
        let player = match r.u8()? {
            0 => PlayerHandle::P1,
            1 => PlayerHandle::P2,
            value => {
                return Err(WireError::BadEnum {
                    field: "player",
                    value,
                })
            }
        };
        Ok(PeerIdentity {
            protocol_version,
            simulation,
            player,
            app_commit: r.array::<20>()?,
            config_hash: r.u64()?,
            seed: r.u64()?,
            core_hash: r.array::<32>()?,
            rom_hash: r.array::<32>()?,
        })
    }
}

/// Why a handshake was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Incompatibility {
    ProtocolVersion = 1,
    Simulation = 2,
    AppCommit = 3,
    Config = 4,
    Seed = 5,
    CoreHash = 6,
    RomHash = 7,
    PlayerSlot = 8,
}

impl Incompatibility {
    fn from_u8(v: u8) -> Option<Incompatibility> {
        Some(match v {
            1 => Incompatibility::ProtocolVersion,
            2 => Incompatibility::Simulation,
            3 => Incompatibility::AppCommit,
            4 => Incompatibility::Config,
            5 => Incompatibility::Seed,
            6 => Incompatibility::CoreHash,
            7 => Incompatibility::RomHash,
            8 => Incompatibility::PlayerSlot,
            _ => return None,
        })
    }

    pub const fn reason(self) -> &'static str {
        match self {
            Incompatibility::ProtocolVersion => "protocol version mismatch",
            Incompatibility::Simulation => "peers chose different simulations",
            Incompatibility::AppCommit => "peers run different application builds",
            Incompatibility::Config => "session configuration mismatch",
            Incompatibility::Seed => "session seed mismatch",
            Incompatibility::CoreHash => "libretro core hash mismatch",
            Incompatibility::RomHash => "ROM hash mismatch",
            Incompatibility::PlayerSlot => "both peers asked for the same player slot",
        }
    }
}

/// Why a session is ending.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum DisconnectReason {
    Normal = 0,
    Desync = 1,
    Timeout = 2,
    Incompatible = 3,
    LocalError = 4,
}

impl DisconnectReason {
    fn from_u8(v: u8) -> Option<DisconnectReason> {
        Some(match v {
            0 => DisconnectReason::Normal,
            1 => DisconnectReason::Desync,
            2 => DisconnectReason::Timeout,
            3 => DisconnectReason::Incompatible,
            4 => DisconnectReason::LocalError,
            _ => return None,
        })
    }
}

/// A peer's own view of the session, exchanged so one report can compare both
/// ends without needing to reach the remote log while the match is running.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TelemetrySummary {
    pub frames_presented: u64,
    pub frames_resimulated: u64,
    pub rollbacks: u64,
    pub max_rollback_depth: u32,
    pub predicted_frames: u64,
    pub mispredicted_frames: u64,
    pub stalls: u64,
    pub checksums_compared: u64,
    pub state_bytes_last: u32,
    pub srtt_micros: u32,
    pub rttvar_micros: u32,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub inferred_lost: u64,
    pub duplicates: u64,
    pub reordered: u64,
}

/// One decoded message.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Message {
    Hello(PeerIdentity),
    HelloAck {
        identity: PeerIdentity,
        /// `None` means accepted.
        rejection: Option<Incompatibility>,
    },
    InputBatch {
        start_frame: Frame,
        inputs: Vec<PlayerInput>,
        /// Highest frame the sender has received from *us*.
        highest_remote_frame: Frame,
        /// Highest sequence the sender has seen from us, for RTT sampling.
        ack_sequence: u32,
    },
    Checksum {
        frame: Frame,
        value: u64,
    },
    Telemetry(TelemetrySummary),
    Disconnect(DisconnectReason),
}

impl Message {
    pub const fn kind(&self) -> u8 {
        match self {
            Message::Hello(_) => KIND_HELLO,
            Message::HelloAck { .. } => KIND_HELLO_ACK,
            Message::InputBatch { .. } => KIND_INPUT_BATCH,
            Message::Checksum { .. } => KIND_CHECKSUM,
            Message::Telemetry(_) => KIND_TELEMETRY,
            Message::Disconnect(_) => KIND_DISCONNECT,
        }
    }

    pub const fn kind_name(&self) -> &'static str {
        match self {
            Message::Hello(_) => "hello",
            Message::HelloAck { .. } => "hello_ack",
            Message::InputBatch { .. } => "input_batch",
            Message::Checksum { .. } => "checksum",
            Message::Telemetry(_) => "telemetry",
            Message::Disconnect(_) => "disconnect",
        }
    }
}

/// A message plus its header fields.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Packet {
    pub sequence: u32,
    pub message: Message,
}

impl Packet {
    /// Serialise the packet *without* the authentication tag.
    ///
    /// [`crate::auth::Authenticator::seal`] appends the tag; keeping the two
    /// steps separate means the encoder cannot accidentally emit an unsigned
    /// datagram, because the transport only ever sends sealed buffers.
    pub fn encode_unsigned(&self) -> Result<Vec<u8>, WireError> {
        let mut w = Writer::with_capacity(128);
        w.bytes(&MAGIC);
        w.u8(PROTOCOL_VERSION);
        w.u8(self.message.kind());
        w.u32(self.sequence);

        match &self.message {
            Message::Hello(id) => id.encode(&mut w),
            Message::HelloAck {
                identity,
                rejection,
            } => {
                identity.encode(&mut w);
                w.u8(rejection.map_or(0, |r| r as u8));
            }
            Message::InputBatch {
                start_frame,
                inputs,
                highest_remote_frame,
                ack_sequence,
            } => {
                if inputs.len() > INPUT_REDUNDANCY {
                    return Err(WireError::BatchTooLong(inputs.len()));
                }
                w.i32(*start_frame);
                w.i32(*highest_remote_frame);
                w.u32(*ack_sequence);
                w.u8(inputs.len() as u8);
                for input in inputs {
                    w.u16(input.bits());
                }
            }
            Message::Checksum { frame, value } => {
                w.i32(*frame);
                w.u64(*value);
            }
            Message::Telemetry(t) => {
                w.u64(t.frames_presented);
                w.u64(t.frames_resimulated);
                w.u64(t.rollbacks);
                w.u32(t.max_rollback_depth);
                w.u64(t.predicted_frames);
                w.u64(t.mispredicted_frames);
                w.u64(t.stalls);
                w.u64(t.checksums_compared);
                w.u32(t.state_bytes_last);
                w.u32(t.srtt_micros);
                w.u32(t.rttvar_micros);
                w.u64(t.packets_sent);
                w.u64(t.packets_received);
                w.u64(t.bytes_sent);
                w.u64(t.bytes_received);
                w.u64(t.inferred_lost);
                w.u64(t.duplicates);
                w.u64(t.reordered);
            }
            Message::Disconnect(reason) => w.u8(*reason as u8),
        }

        if w.len() + TAG_LEN > MAX_DATAGRAM {
            return Err(WireError::TooLarge(w.len() + TAG_LEN));
        }
        Ok(w.finish())
    }

    /// Parse an already-authenticated datagram body (tag stripped).
    pub fn decode(bytes: &[u8]) -> Result<Packet, WireError> {
        if bytes.len() < HEADER_LEN {
            return Err(WireError::TooShort(bytes.len()));
        }
        if bytes[0..2] != MAGIC {
            return Err(WireError::BadMagic);
        }
        if bytes[2] != PROTOCOL_VERSION {
            return Err(WireError::BadVersion { got: bytes[2] });
        }
        let kind = bytes[3];
        let mut r = Reader::new(&bytes[4..]);
        let sequence = r.u32()?;

        let message = match kind {
            KIND_HELLO => Message::Hello(PeerIdentity::decode(&mut r)?),
            KIND_HELLO_ACK => {
                let identity = PeerIdentity::decode(&mut r)?;
                let code = r.u8()?;
                let rejection = if code == 0 {
                    None
                } else {
                    Some(Incompatibility::from_u8(code).ok_or(WireError::BadEnum {
                        field: "rejection",
                        value: code,
                    })?)
                };
                Message::HelloAck {
                    identity,
                    rejection,
                }
            }
            KIND_INPUT_BATCH => {
                let start_frame = r.i32()?;
                let highest_remote_frame = r.i32()?;
                let ack_sequence = r.u32()?;
                let count = usize::from(r.u8()?);
                if count > INPUT_REDUNDANCY {
                    return Err(WireError::BatchTooLong(count));
                }
                let mut inputs = Vec::with_capacity(count);
                for _ in 0..count {
                    inputs.push(PlayerInput(r.u16()?));
                }
                Message::InputBatch {
                    start_frame,
                    inputs,
                    highest_remote_frame,
                    ack_sequence,
                }
            }
            KIND_CHECKSUM => Message::Checksum {
                frame: r.i32()?,
                value: r.u64()?,
            },
            KIND_TELEMETRY => Message::Telemetry(TelemetrySummary {
                frames_presented: r.u64()?,
                frames_resimulated: r.u64()?,
                rollbacks: r.u64()?,
                max_rollback_depth: r.u32()?,
                predicted_frames: r.u64()?,
                mispredicted_frames: r.u64()?,
                stalls: r.u64()?,
                checksums_compared: r.u64()?,
                state_bytes_last: r.u32()?,
                srtt_micros: r.u32()?,
                rttvar_micros: r.u32()?,
                packets_sent: r.u64()?,
                packets_received: r.u64()?,
                bytes_sent: r.u64()?,
                bytes_received: r.u64()?,
                inferred_lost: r.u64()?,
                duplicates: r.u64()?,
                reordered: r.u64()?,
            }),
            KIND_DISCONNECT => {
                let code = r.u8()?;
                Message::Disconnect(DisconnectReason::from_u8(code).ok_or(WireError::BadEnum {
                    field: "disconnect",
                    value: code,
                })?)
            }
            other => return Err(WireError::UnknownKind(other)),
        };

        r.finish()?;
        Ok(Packet { sequence, message })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> PeerIdentity {
        PeerIdentity {
            protocol_version: PROTOCOL_VERSION,
            simulation: SimulationKind::Arena,
            player: PlayerHandle::P1,
            app_commit: *b"0123456789abcdef0123",
            config_hash: 0x1122_3344_5566_7788,
            seed: 0x9900_AABB_CCDD_EEFF,
            core_hash: [0xAB; 32],
            rom_hash: [0xCD; 32],
        }
    }

    fn round_trip(message: Message) -> Packet {
        let packet = Packet {
            sequence: 0x0102_0304,
            message,
        };
        let bytes = packet.encode_unsigned().unwrap();
        let decoded = Packet::decode(&bytes).unwrap();
        assert_eq!(decoded, packet);
        decoded
    }

    #[test]
    fn identity_encoding_has_the_documented_length() {
        let mut w = Writer::with_capacity(128);
        identity().encode(&mut w);
        assert_eq!(w.len(), PeerIdentity::ENCODED_LEN);
    }

    #[test]
    fn every_message_round_trips() {
        round_trip(Message::Hello(identity()));
        round_trip(Message::HelloAck {
            identity: identity(),
            rejection: None,
        });
        round_trip(Message::HelloAck {
            identity: identity(),
            rejection: Some(Incompatibility::RomHash),
        });
        round_trip(Message::InputBatch {
            start_frame: 1234,
            inputs: (0..8).map(|i| PlayerInput(i * 7)).collect(),
            highest_remote_frame: 1200,
            ack_sequence: 99,
        });
        round_trip(Message::Checksum {
            frame: -1,
            value: u64::MAX,
        });
        round_trip(Message::Telemetry(TelemetrySummary {
            frames_presented: 10_800,
            rollbacks: 421,
            max_rollback_depth: 8,
            ..Default::default()
        }));
        round_trip(Message::Disconnect(DisconnectReason::Desync));
    }

    #[test]
    fn every_packet_fits_the_datagram_cap() {
        let biggest = Packet {
            sequence: u32::MAX,
            message: Message::Telemetry(TelemetrySummary {
                frames_presented: u64::MAX,
                ..Default::default()
            }),
        };
        assert!(biggest.encode_unsigned().unwrap().len() + TAG_LEN <= MAX_DATAGRAM);

        let batch = Packet {
            sequence: u32::MAX,
            message: Message::InputBatch {
                start_frame: Frame::MAX,
                inputs: vec![PlayerInput(u16::MAX); INPUT_REDUNDANCY],
                highest_remote_frame: Frame::MAX,
                ack_sequence: u32::MAX,
            },
        };
        assert!(batch.encode_unsigned().unwrap().len() + TAG_LEN <= MAX_DATAGRAM);
    }

    #[test]
    fn an_oversized_input_batch_is_refused_at_encode_time() {
        let packet = Packet {
            sequence: 1,
            message: Message::InputBatch {
                start_frame: 0,
                inputs: vec![PlayerInput(1); INPUT_REDUNDANCY + 1],
                highest_remote_frame: 0,
                ack_sequence: 0,
            },
        };
        assert!(matches!(
            packet.encode_unsigned(),
            Err(WireError::BatchTooLong(9))
        ));
    }

    #[test]
    fn bad_headers_are_rejected() {
        let good = Packet {
            sequence: 1,
            message: Message::Disconnect(DisconnectReason::Normal),
        }
        .encode_unsigned()
        .unwrap();

        assert!(matches!(
            Packet::decode(&good[..4]),
            Err(WireError::TooShort(4))
        ));

        let mut wrong_magic = good.clone();
        wrong_magic[0] = b'X';
        assert!(matches!(
            Packet::decode(&wrong_magic),
            Err(WireError::BadMagic)
        ));

        let mut wrong_version = good.clone();
        wrong_version[2] = 99;
        assert!(matches!(
            Packet::decode(&wrong_version),
            Err(WireError::BadVersion { got: 99 })
        ));

        let mut wrong_kind = good.clone();
        wrong_kind[3] = 42;
        assert!(matches!(
            Packet::decode(&wrong_kind),
            Err(WireError::UnknownKind(42))
        ));
    }

    #[test]
    fn truncated_and_padded_payloads_are_rejected() {
        let bytes = Packet {
            sequence: 7,
            message: Message::Checksum {
                frame: 60,
                value: 0xFEED,
            },
        }
        .encode_unsigned()
        .unwrap();

        assert!(Packet::decode(&bytes[..bytes.len() - 1]).is_err());

        let mut padded = bytes.clone();
        padded.push(0);
        assert!(matches!(
            Packet::decode(&padded),
            Err(WireError::TrailingBytes(1))
        ));
    }

    #[test]
    fn unknown_enum_values_are_rejected_rather_than_defaulted() {
        let mut bytes = Packet {
            sequence: 1,
            message: Message::Disconnect(DisconnectReason::Normal),
        }
        .encode_unsigned()
        .unwrap();
        let last = bytes.len() - 1;
        bytes[last] = 200;
        assert!(matches!(
            Packet::decode(&bytes),
            Err(WireError::BadEnum {
                field: "disconnect",
                value: 200
            })
        ));
    }

    #[test]
    fn compatibility_reports_the_first_difference() {
        let a = identity();
        let mut b = identity();
        b.player = PlayerHandle::P2;
        a.compatible_with(&b).unwrap();

        let cases: [(PeerIdentity, Incompatibility); 7] = [
            (
                PeerIdentity {
                    protocol_version: 99,
                    ..b
                },
                Incompatibility::ProtocolVersion,
            ),
            (
                PeerIdentity {
                    simulation: SimulationKind::LastBlade2,
                    ..b
                },
                Incompatibility::Simulation,
            ),
            (
                PeerIdentity {
                    app_commit: [0; 20],
                    ..b
                },
                Incompatibility::AppCommit,
            ),
            (
                PeerIdentity {
                    config_hash: 0,
                    ..b
                },
                Incompatibility::Config,
            ),
            (PeerIdentity { seed: 0, ..b }, Incompatibility::Seed),
            (
                PeerIdentity {
                    core_hash: [0; 32],
                    ..b
                },
                Incompatibility::CoreHash,
            ),
            (
                PeerIdentity {
                    rom_hash: [0; 32],
                    ..b
                },
                Incompatibility::RomHash,
            ),
        ];
        for (other, expected) in cases {
            assert_eq!(a.compatible_with(&other), Err(expected));
        }
    }

    #[test]
    fn two_peers_cannot_claim_the_same_slot() {
        assert_eq!(
            identity().compatible_with(&identity()),
            Err(Incompatibility::PlayerSlot)
        );
    }

    #[test]
    fn every_rejection_has_a_readable_reason() {
        for code in 1..=8u8 {
            let reason = Incompatibility::from_u8(code).unwrap();
            assert!(!reason.reason().is_empty());
        }
        assert!(Incompatibility::from_u8(0).is_none());
        assert!(Incompatibility::from_u8(9).is_none());
    }
}
