//! Authenticated, non-blocking UDP transport.
//!
//! One socket, one peer, no connection state beyond the peer address. The local
//! client dials the EC2 instance's public address on UDP/7000; the headless bot
//! binds that port and learns the peer address from the first `Hello` that
//! authenticates. There is no STUN, no relay and no matchmaking in this MVP --
//! a direct path is a precondition, not something the lab negotiates.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::Instant;

use rollback_core::NetworkProfile;

use crate::auth::{AuthError, Authenticator};
use crate::emulator::{EmulatorStats, NetworkEmulator};
use crate::link::{LinkMonitor, LinkStats};
use crate::wire::{Message, Packet, WireError, MAX_DATAGRAM};

/// The lab's fixed UDP port.
pub const DEFAULT_PORT: u16 = 7_000;

/// Receive buffer. Larger than the datagram cap on purpose: an oversized
/// datagram must be *observed* and rejected, not silently truncated into
/// something that might still parse.
const RECV_BUFFER: usize = 2_048;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("socket error: {0}")]
    Io(#[from] io::Error),
    #[error("encoding error: {0}")]
    Wire(#[from] WireError),
    #[error("no peer address yet: nothing to send to")]
    NoPeer,
}

/// A datagram that arrived, authenticated and parsed.
#[derive(Clone, Debug)]
pub struct Received {
    pub from: SocketAddr,
    pub packet: Packet,
}

pub struct UdpTransport {
    socket: UdpSocket,
    peer: Option<SocketAddr>,
    auth: Authenticator,
    emulator: NetworkEmulator,
    monitor: LinkMonitor,
    next_sequence: u32,
    started: Instant,
    last_authenticated_ms: Option<u64>,
}

impl UdpTransport {
    /// Bind a non-blocking socket.
    pub fn bind(
        addr: SocketAddr,
        auth: Authenticator,
        profile: NetworkProfile,
    ) -> Result<Self, TransportError> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(UdpTransport {
            socket,
            peer: None,
            auth,
            emulator: NetworkEmulator::new(profile),
            monitor: LinkMonitor::new(),
            next_sequence: 0,
            started: Instant::now(),
            last_authenticated_ms: None,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        self.peer
    }

    pub fn set_peer(&mut self, addr: SocketAddr) {
        self.peer = Some(addr);
    }

    /// Milliseconds since the transport was created. One clock, used for every
    /// timing decision on this side.
    pub fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn stats(&self) -> &LinkStats {
        self.monitor.stats()
    }

    pub fn emulator_stats(&self) -> &EmulatorStats {
        self.emulator.stats()
    }

    pub fn profile(&self) -> &NetworkProfile {
        self.emulator.profile()
    }

    /// Highest sequence seen from the peer, to be echoed back as the ACK.
    pub fn highest_received_sequence(&self) -> u32 {
        self.monitor.stats().highest_sequence.max(0) as u32
    }

    /// Milliseconds since the last datagram that passed authentication.
    ///
    /// `None` until the first one arrives. The session turns this into the
    /// three-second timeout; datagrams that fail the HMAC deliberately do not
    /// count, or an attacker could keep a dead session alive with garbage.
    pub fn ms_since_authenticated(&self) -> Option<u64> {
        self.last_authenticated_ms
            .map(|at| self.now_ms().saturating_sub(at))
    }

    /// Encode, sign and hand a message to the impairment emulator.
    ///
    /// Returns the sequence number assigned to it.
    pub fn send(&mut self, message: Message) -> Result<u32, TransportError> {
        let peer = self.peer.ok_or(TransportError::NoPeer)?;
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);

        let body = Packet {
            sequence,
            message,
        }
        .encode_unsigned()?;
        let datagram = self.auth.seal(body);
        debug_assert!(datagram.len() <= MAX_DATAGRAM);

        let now = self.now_ms();
        self.monitor.on_sent(sequence, datagram.len(), now);
        let due = self.emulator.submit(now, datagram);
        self.transmit(peer, due)?;
        Ok(sequence)
    }

    /// Release any datagrams whose emulated delay has elapsed.
    ///
    /// Must be called every frame: without it a delayed datagram would sit in
    /// the emulator until the next `send`, which under stall conditions is
    /// exactly when no `send` is happening.
    pub fn pump(&mut self) -> Result<(), TransportError> {
        let Some(peer) = self.peer else {
            return Ok(());
        };
        let now = self.now_ms();
        let due = self.emulator.drain_due(now);
        self.transmit(peer, due)
    }

    /// Send everything still queued, for a clean shutdown.
    pub fn flush(&mut self) -> Result<(), TransportError> {
        let Some(peer) = self.peer else {
            return Ok(());
        };
        let due = self.emulator.flush();
        self.transmit(peer, due)
    }

    fn transmit(&self, peer: SocketAddr, datagrams: Vec<Vec<u8>>) -> Result<(), TransportError> {
        for datagram in datagrams {
            match self.socket.send_to(&datagram, peer) {
                Ok(_) => {}
                // A full send buffer is congestion, not a session failure: the
                // input redundancy will carry the dropped frame anyway.
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(TransportError::Io(e)),
            }
        }
        Ok(())
    }

    /// Drain the socket. Never blocks.
    ///
    /// Datagrams that fail authentication, fail to parse, or exceed the size
    /// cap are counted and discarded. Duplicates are counted and discarded too,
    /// so the caller only ever sees each sequence once.
    pub fn receive(&mut self) -> Result<Vec<Received>, TransportError> {
        let mut out = Vec::new();
        let mut buffer = [0u8; RECV_BUFFER];

        loop {
            let (len, from) = match self.socket.recv_from(&mut buffer) {
                Ok(v) => v,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                // A peer that has gone away makes Linux surface an ICMP port
                // unreachable on the next recv. That is not fatal for a session
                // that is allowed to have gaps -- keep draining.
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => continue,
                Err(e) => return Err(TransportError::Io(e)),
            };

            if len > MAX_DATAGRAM {
                self.monitor.on_malformed();
                continue;
            }

            let body = match self.auth.open(&buffer[..len]) {
                Ok(body) => body,
                Err(AuthError::TooShort) | Err(AuthError::BadTag) => {
                    self.monitor.on_auth_failure();
                    continue;
                }
                Err(_) => {
                    self.monitor.on_auth_failure();
                    continue;
                }
            };

            let packet = match Packet::decode(body) {
                Ok(p) => p,
                Err(_) => {
                    self.monitor.on_malformed();
                    continue;
                }
            };

            // Authenticated: the timeout clock restarts here, before any
            // duplicate filtering, because a duplicate still proves liveness.
            let now = self.now_ms();
            self.last_authenticated_ms = Some(now);

            if !self.monitor.on_received(packet.sequence, len) {
                continue;
            }
            if let Message::InputBatch { ack_sequence, .. } = &packet.message {
                self.monitor.on_ack(*ack_sequence, now);
            }
            if self.peer.is_none() {
                self.peer = Some(from);
            }
            out.push(Received { from, packet });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{DisconnectReason, PROTOCOL_VERSION};
    use rollback_core::{PlayerHandle, PlayerInput, SimulationKind};
    use std::net::{IpAddr, Ipv4Addr};

    fn loopback() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
    }

    fn pair(profile: NetworkProfile) -> (UdpTransport, UdpTransport) {
        let auth = Authenticator::from_passphrase("test");
        let mut a = UdpTransport::bind(loopback(), auth.clone(), profile).unwrap();
        let mut b = UdpTransport::bind(loopback(), auth, profile).unwrap();
        let (aa, ba) = (a.local_addr().unwrap(), b.local_addr().unwrap());
        a.set_peer(ba);
        b.set_peer(aa);
        (a, b)
    }

    fn batch(start_frame: i32, ack: u32) -> Message {
        Message::InputBatch {
            start_frame,
            inputs: vec![PlayerInput(1), PlayerInput(2)],
            highest_remote_frame: start_frame - 1,
            ack_sequence: ack,
        }
    }

    /// Poll until `f` yields something or the deadline passes. Loopback is fast
    /// but not instantaneous, and a fixed sleep would be a flaky test.
    fn wait_for<T>(mut f: impl FnMut() -> Vec<T>) -> Vec<T> {
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let got = f();
            if !got.is_empty() {
                return got;
            }
            if Instant::now() > deadline {
                return Vec::new();
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn a_message_survives_the_round_trip() {
        let (mut a, mut b) = pair(NetworkProfile::NATURAL);
        a.send(batch(10, 0)).unwrap();

        let got = wait_for(|| b.receive().unwrap());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].packet.sequence, 0);
        assert_eq!(got[0].packet.message, batch(10, 0));
        assert_eq!(b.stats().unique_received, 1);
    }

    #[test]
    fn sequence_numbers_increment_per_send() {
        let (mut a, _b) = pair(NetworkProfile::NATURAL);
        assert_eq!(a.send(batch(0, 0)).unwrap(), 0);
        assert_eq!(a.send(batch(1, 0)).unwrap(), 1);
        assert_eq!(a.send(batch(2, 0)).unwrap(), 2);
        assert_eq!(a.stats().packets_sent, 3);
    }

    #[test]
    fn sending_without_a_peer_is_refused() {
        let auth = Authenticator::from_passphrase("test");
        let mut t = UdpTransport::bind(loopback(), auth, NetworkProfile::NATURAL).unwrap();
        assert!(matches!(
            t.send(Message::Disconnect(DisconnectReason::Normal)),
            Err(TransportError::NoPeer)
        ));
    }

    #[test]
    fn a_datagram_signed_with_the_wrong_key_is_counted_and_dropped() {
        let profile = NetworkProfile::NATURAL;
        let mut victim =
            UdpTransport::bind(loopback(), Authenticator::from_passphrase("real"), profile).unwrap();
        let mut attacker = UdpTransport::bind(
            loopback(),
            Authenticator::from_passphrase("guessed"),
            profile,
        )
        .unwrap();
        attacker.set_peer(victim.local_addr().unwrap());
        attacker.send(batch(0, 0)).unwrap();

        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while victim.stats().auth_failures == 0 && Instant::now() < deadline {
            assert!(victim.receive().unwrap().is_empty(), "must not be delivered");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(victim.stats().auth_failures, 1);
        assert_eq!(victim.stats().packets_received, 0);
        assert!(
            victim.ms_since_authenticated().is_none(),
            "a forged datagram must not keep the session alive"
        );
    }

    #[test]
    fn garbage_that_is_not_even_a_datagram_is_counted_as_an_auth_failure() {
        let (mut a, mut b) = pair(NetworkProfile::NATURAL);
        let junk = [0xFFu8; 64];
        a.socket.send_to(&junk, b.local_addr().unwrap()).unwrap();

        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while b.stats().auth_failures == 0 && Instant::now() < deadline {
            b.receive().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(b.stats().auth_failures, 1);
    }

    #[test]
    fn a_server_learns_the_peer_address_from_the_first_datagram() {
        let auth = Authenticator::from_passphrase("test");
        let mut server =
            UdpTransport::bind(loopback(), auth.clone(), NetworkProfile::NATURAL).unwrap();
        let mut client =
            UdpTransport::bind(loopback(), auth, NetworkProfile::NATURAL).unwrap();
        client.set_peer(server.local_addr().unwrap());
        assert!(server.peer().is_none());

        client
            .send(Message::Hello(crate::wire::PeerIdentity {
                protocol_version: PROTOCOL_VERSION,
                simulation: SimulationKind::Arena,
                player: PlayerHandle::P1,
                app_commit: [0; 20],
                config_hash: 1,
                seed: 2,
                core_hash: [0; 32],
                rom_hash: [0; 32],
            }))
            .unwrap();

        let got = wait_for(|| server.receive().unwrap());
        assert_eq!(got.len(), 1);
        assert_eq!(server.peer(), Some(client.local_addr().unwrap()));
    }

    #[test]
    fn an_input_batch_ack_produces_an_rtt_sample() {
        let (mut a, mut b) = pair(NetworkProfile::NATURAL);
        let seq = a.send(batch(0, 0)).unwrap();
        wait_for(|| b.receive().unwrap());

        b.send(batch(0, seq)).unwrap();
        wait_for(|| a.receive().unwrap());
        assert_eq!(a.stats().rtt_samples, 1);
    }

    #[test]
    fn the_timeout_clock_only_moves_on_authenticated_datagrams() {
        let (mut a, mut b) = pair(NetworkProfile::NATURAL);
        assert!(b.ms_since_authenticated().is_none());
        a.send(batch(0, 0)).unwrap();
        wait_for(|| b.receive().unwrap());
        assert!(b.ms_since_authenticated().is_some());
    }

    #[test]
    fn an_impaired_profile_holds_datagrams_until_pumped() {
        let profile = NetworkProfile {
            delay_ms: 200,
            seed: 11,
            ..NetworkProfile::NATURAL
        };
        let (mut a, mut b) = pair(profile);
        a.send(batch(0, 0)).unwrap();
        // Nothing can have been transmitted yet.
        assert!(b.receive().unwrap().is_empty());

        let deadline = Instant::now() + std::time::Duration::from_secs(3);
        let mut got = Vec::new();
        while got.is_empty() && Instant::now() < deadline {
            a.pump().unwrap();
            got = b.receive().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got.len(), 1, "the delayed datagram must eventually arrive");
    }

    #[test]
    fn flush_releases_queued_datagrams_immediately() {
        let profile = NetworkProfile {
            delay_ms: 5_000,
            seed: 3,
            ..NetworkProfile::NATURAL
        };
        let (mut a, mut b) = pair(profile);
        a.send(batch(0, 0)).unwrap();
        a.flush().unwrap();
        assert_eq!(wait_for(|| b.receive().unwrap()).len(), 1);
    }
}
