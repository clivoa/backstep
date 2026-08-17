//! Golden tests: byte-exact vectors for every message and for the HMAC.
//!
//! The round-trip tests in `wire.rs` prove the encoder and the decoder agree
//! with each other. They would keep passing if both changed together -- which
//! is exactly the change that silently breaks compatibility with a peer running
//! yesterday's build.
//!
//! These vectors are the fixed point. If one of them changes, the wire format
//! changed, and `PROTOCOL_VERSION` must change with it.

use rollback_core::{PlayerHandle, PlayerInput, SimulationKind};
use rollback_net::{
    Authenticator, DisconnectReason, Incompatibility, Message, Packet, PeerIdentity,
    TelemetrySummary, HEADER_LEN, MAX_DATAGRAM, PROTOCOL_VERSION, TAG_LEN,
};

/// A fixed identity, so the vectors below are reproducible.
fn identity() -> PeerIdentity {
    PeerIdentity {
        protocol_version: PROTOCOL_VERSION,
        simulation: SimulationKind::Sfa3,
        player: PlayerHandle::P1,
        app_commit: *b"0123456789abcdef0123",
        config_hash: 0x0011_2233_4455_6677,
        seed: 0x8899_AABB_CCDD_EEFF,
        core_hash: [0x11; 32],
        rom_hash: [0x22; 32],
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn encoded(sequence: u32, message: Message) -> String {
    hex(&Packet { sequence, message }.encode_unsigned().unwrap())
}

/// The key the sealed-datagram vector is signed with.
const GOLDEN_PASSPHRASE: &str = "golden";

#[test]
fn the_header_layout_is_fixed() {
    let bytes = Packet {
        sequence: 0x0403_0201,
        message: Message::Disconnect(DisconnectReason::Timeout),
    }
    .encode_unsigned()
    .unwrap();

    assert_eq!(&bytes[0..2], b"RB", "magic");
    assert_eq!(bytes[2], PROTOCOL_VERSION, "version");
    assert_eq!(bytes[3], 6, "kind: disconnect");
    assert_eq!(&bytes[4..8], &[0x01, 0x02, 0x03, 0x04], "sequence, LE");
    assert_eq!(bytes.len(), HEADER_LEN + 1);
}

#[test]
fn golden_hello() {
    assert_eq!(
        encoded(1, Message::Hello(identity())),
        "5242010101000000\
         01\
         01\
         00\
         30313233343536373839616263646566303132 33\
         7766554433221100\
         ffeeddccbbaa9988\
         1111111111111111111111111111111111111111111111111111111111111111\
         2222222222222222222222222222222222222222222222222222222222222222"
            .replace([' ', '\n'], "")
    );
}

#[test]
fn golden_hello_ack_accepted_and_rejected() {
    let accepted = encoded(
        2,
        Message::HelloAck {
            identity: identity(),
            rejection: None,
        },
    );
    let rejected = encoded(
        2,
        Message::HelloAck {
            identity: identity(),
            rejection: Some(Incompatibility::RomHash),
        },
    );

    // Identical but for the trailing reason byte: 00 = accepted, 07 = ROM hash.
    assert_eq!(accepted.len(), rejected.len());
    assert_eq!(&accepted[..accepted.len() - 2], &rejected[..rejected.len() - 2]);
    assert!(accepted.ends_with("00"));
    assert!(rejected.ends_with("07"));
    assert!(accepted.starts_with("5242010202000000"), "{accepted}");
}

#[test]
fn golden_input_batch() {
    assert_eq!(
        encoded(
            0x0000_002A,
            Message::InputBatch {
                start_frame: 1_000,
                inputs: vec![
                    PlayerInput(0x0001),
                    PlayerInput(0x0002),
                    PlayerInput(0x0004),
                    PlayerInput(0x0008),
                ],
                highest_remote_frame: 990,
                ack_sequence: 0x0000_0029,
            },
        ),
        concat!(
            "52420103", // magic, version, kind 3
            "2a000000", // sequence 42
            "e8030000", // start_frame 1000
            "de030000", // highest_remote_frame 990
            "29000000", // ack_sequence 41
            "04",       // four inputs
            "0100", "0200", "0400", "0800",
        )
    );
}

#[test]
fn golden_input_batch_with_a_negative_frame() {
    // Frames are signed; -1 is the "no frame" sentinel and must survive
    // round-tripping as two's complement, not as an unsigned reinterpretation.
    let bytes = encoded(
        0,
        Message::InputBatch {
            start_frame: -1,
            inputs: vec![PlayerInput(0)],
            highest_remote_frame: -1,
            ack_sequence: 0,
        },
    );
    assert!(bytes.contains("ffffffffffffffff"), "{bytes}");

    let raw = Packet {
        sequence: 0,
        message: Message::InputBatch {
            start_frame: -1,
            inputs: vec![PlayerInput(0)],
            highest_remote_frame: -1,
            ack_sequence: 0,
        },
    }
    .encode_unsigned()
    .unwrap();
    match Packet::decode(&raw).unwrap().message {
        Message::InputBatch { start_frame, .. } => assert_eq!(start_frame, -1),
        other => panic!("decoded as {other:?}"),
    }
}

#[test]
fn golden_checksum() {
    assert_eq!(
        encoded(
            7,
            Message::Checksum {
                frame: 60,
                value: 0x0123_4567_89AB_CDEF,
            },
        ),
        "5242010407000000 3c000000 efcdab8967452301".replace(' ', "")
    );
}

#[test]
fn golden_disconnect_reasons() {
    for (reason, code) in [
        (DisconnectReason::Normal, "00"),
        (DisconnectReason::Desync, "01"),
        (DisconnectReason::Timeout, "02"),
        (DisconnectReason::Incompatible, "03"),
        (DisconnectReason::LocalError, "04"),
    ] {
        assert_eq!(
            encoded(0, Message::Disconnect(reason)),
            format!("5242010600000000{code}"),
            "{reason:?}"
        );
    }
}

#[test]
fn golden_telemetry_field_order() {
    // Every field is a distinct power of two, so a swapped pair is visible in
    // the hex rather than hiding behind similar-looking numbers.
    let summary = TelemetrySummary {
        frames_presented: 1,
        frames_resimulated: 2,
        rollbacks: 4,
        max_rollback_depth: 8,
        predicted_frames: 16,
        mispredicted_frames: 32,
        stalls: 64,
        checksums_compared: 128,
        state_bytes_last: 256,
        srtt_micros: 512,
        rttvar_micros: 1024,
        packets_sent: 2048,
        packets_received: 4096,
        bytes_sent: 8192,
        bytes_received: 16384,
        inferred_lost: 32768,
        duplicates: 65536,
        reordered: 131072,
    };
    assert_eq!(
        encoded(0, Message::Telemetry(summary)),
        concat!(
            "52420105", "00000000",
            "0100000000000000", // frames_presented
            "0200000000000000", // frames_resimulated
            "0400000000000000", // rollbacks
            "08000000",         // max_rollback_depth (u32)
            "1000000000000000", // predicted_frames
            "2000000000000000", // mispredicted_frames
            "4000000000000000", // stalls
            "8000000000000000", // checksums_compared
            "00010000",         // state_bytes_last (u32)
            "00020000",         // srtt_micros (u32)
            "00040000",         // rttvar_micros (u32)
            "0008000000000000", // packets_sent
            "0010000000000000", // packets_received
            "0020000000000000", // bytes_sent
            "0040000000000000", // bytes_received
            "0080000000000000", // inferred_lost
            "0000010000000000", // duplicates
            "0000020000000000", // reordered
        )
    );
}

#[test]
fn golden_sealed_datagram() {
    // The full thing as it goes on the wire: body plus HMAC-SHA256 tag.
    let auth = Authenticator::from_passphrase(GOLDEN_PASSPHRASE);
    let body = Packet {
        sequence: 7,
        message: Message::Checksum {
            frame: 60,
            value: 0x0123_4567_89AB_CDEF,
        },
    }
    .encode_unsigned()
    .unwrap();
    let sealed = auth.seal(body.clone());

    assert_eq!(sealed.len(), body.len() + TAG_LEN);
    assert_eq!(
        hex(&sealed[body.len()..]),
        "c5af45b9e7840d8b0939ee30fe4c8e7b400a5cdb64220dfa0d4ac8ca4e140d9d",
        "the authentication tag changed; the key derivation or the body did"
    );
    assert_eq!(auth.open(&sealed).unwrap(), &body[..]);
}

#[test]
fn the_passphrase_derivation_is_stable() {
    // `from_passphrase` is used by tests and by local development. If its
    // derivation changed, every recorded vector above would silently shift.
    let auth = Authenticator::from_passphrase(GOLDEN_PASSPHRASE);
    assert_eq!(
        hex(&auth.seal(Vec::new())),
        "02eb47538b9ea0c0a2c6358e6f4c8e99d776b999b8b4e718223419f288bf2758"
    );
}

#[test]
fn every_message_stays_inside_the_datagram_cap() {
    let biggest = [
        Message::Hello(identity()),
        Message::HelloAck {
            identity: identity(),
            rejection: Some(Incompatibility::PlayerSlot),
        },
        Message::InputBatch {
            start_frame: i32::MIN,
            inputs: vec![PlayerInput(u16::MAX); 8],
            highest_remote_frame: i32::MAX,
            ack_sequence: u32::MAX,
        },
        Message::Checksum {
            frame: i32::MIN,
            value: u64::MAX,
        },
        Message::Telemetry(TelemetrySummary {
            frames_presented: u64::MAX,
            frames_resimulated: u64::MAX,
            rollbacks: u64::MAX,
            max_rollback_depth: u32::MAX,
            ..Default::default()
        }),
        Message::Disconnect(DisconnectReason::LocalError),
    ];
    for message in biggest {
        let name = message.kind_name();
        let body = Packet {
            sequence: u32::MAX,
            message,
        }
        .encode_unsigned()
        .unwrap();
        assert!(
            body.len() + TAG_LEN <= MAX_DATAGRAM,
            "{name} is {} bytes sealed, cap is {MAX_DATAGRAM}",
            body.len() + TAG_LEN
        );
    }
}

#[test]
fn a_datagram_from_the_future_is_rejected_rather_than_misparsed() {
    let mut bytes = Packet {
        sequence: 1,
        message: Message::Disconnect(DisconnectReason::Normal),
    }
    .encode_unsigned()
    .unwrap();
    bytes[2] = PROTOCOL_VERSION + 1;
    assert!(
        Packet::decode(&bytes).is_err(),
        "a newer protocol version must be refused, not guessed at"
    );
}
