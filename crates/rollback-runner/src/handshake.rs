//! The handshake: prove both peers will simulate the same thing.
//!
//! Nothing here is about security -- the HMAC already answered "is this the
//! right peer?". This is about *compatibility*: a session between two builds
//! that disagree about the input delay, the seed, or the ROM will desync, and
//! it is far better to refuse with "ROM hash mismatch" than to play forty
//! seconds and then die on a checksum.

use std::time::{Duration, Instant};

use rollback_net::{
    DisconnectReason, Incompatibility, Message, PeerIdentity, TransportError, UdpTransport,
};

/// Which side of the connection this process is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Binds the well-known port and waits to be dialled. The EC2 bot.
    Host,
    /// Dials the host's public address. The local client.
    Client,
}

#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error("peer refused the session: {0}")]
    Refused(&'static str),
    #[error("this peer refused the session: {0}")]
    Rejected(&'static str),
    #[error("no compatible peer answered within {0:?}")]
    Timeout(Duration),
    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// How often the client repeats its `Hello` while waiting for an ack.
const HELLO_INTERVAL: Duration = Duration::from_millis(200);
/// How often the loop wakes to drain the socket.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Run the handshake to completion, returning the peer's identity.
///
/// A `Host` waits for a `Hello` and answers it. A `Client` sends `Hello` until
/// a `HelloAck` comes back. Both sides validate the peer's identity against
/// their own, and both sides send a `Disconnect` before erroring out, so the
/// other end gets a reason rather than a timeout.
pub fn handshake(
    transport: &mut UdpTransport,
    role: Role,
    local: PeerIdentity,
    timeout: Duration,
) -> Result<PeerIdentity, HandshakeError> {
    let deadline = Instant::now() + timeout;
    let mut next_hello = Instant::now();

    loop {
        if Instant::now() > deadline {
            return Err(HandshakeError::Timeout(timeout));
        }

        if role == Role::Client && Instant::now() >= next_hello {
            transport.send(Message::Hello(local))?;
            next_hello = Instant::now() + HELLO_INTERVAL;
        }
        transport.pump()?;

        for received in transport.receive()? {
            match received.packet.message {
                Message::Hello(remote) if role == Role::Host => {
                    let verdict = local.compatible_with(&remote);
                    let rejection = verdict.err();
                    transport.send(Message::HelloAck {
                        identity: local,
                        rejection,
                    })?;
                    transport.pump()?;
                    match rejection {
                        Some(reason) => {
                            transport.send(Message::Disconnect(DisconnectReason::Incompatible))?;
                            transport.flush()?;
                            return Err(HandshakeError::Rejected(reason.reason()));
                        }
                        None => return Ok(remote),
                    }
                }
                Message::HelloAck {
                    identity,
                    rejection,
                } if role == Role::Client => {
                    if let Some(reason) = rejection {
                        return Err(HandshakeError::Refused(reason.reason()));
                    }
                    // Check independently rather than trusting the ack: the two
                    // peers must each be satisfied on their own terms.
                    if let Err(reason) = local.compatible_with(&identity) {
                        transport.send(Message::Disconnect(DisconnectReason::Incompatible))?;
                        transport.flush()?;
                        return Err(HandshakeError::Rejected(reason.reason()));
                    }
                    return Ok(identity);
                }
                Message::Disconnect(_) => {
                    return Err(HandshakeError::Refused("peer disconnected during handshake"))
                }
                // A stray `Hello` at the client, or an ack at the host: the peer
                // is retrying and we have not answered yet. Ignore and continue.
                _ => {}
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Human-readable summary of what the two peers agreed on, for the log header.
pub fn describe(local: &PeerIdentity, remote: &PeerIdentity) -> String {
    format!(
        "protocol v{}, simulation {:?}, seed {:#018x}, config {:#018x}, local {:?} vs remote {:?}",
        local.protocol_version,
        local.simulation,
        local.seed,
        local.config_hash,
        local.player,
        remote.player,
    )
}

/// The rejection an identity would produce against itself, for tests and for
/// the `--check` path of the CLIs.
pub fn self_check(identity: &PeerIdentity) -> Option<Incompatibility> {
    identity.compatible_with(identity).err()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rollback_core::{NetworkProfile, PlayerHandle, SimulationKind};
    use rollback_net::{Authenticator, PROTOCOL_VERSION};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn identity(player: PlayerHandle) -> PeerIdentity {
        PeerIdentity {
            protocol_version: PROTOCOL_VERSION,
            simulation: SimulationKind::Arena,
            player,
            app_commit: *b"deadbeefdeadbeefdead",
            config_hash: 0x1111,
            seed: 0x2222,
            core_hash: [0; 32],
            rom_hash: [0; 32],
        }
    }

    fn transport() -> UdpTransport {
        UdpTransport::bind(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            Authenticator::from_passphrase("handshake-test"),
            NetworkProfile::NATURAL,
        )
        .unwrap()
    }

    /// Run both ends, host on a thread, and return their results.
    fn run(
        client_identity: PeerIdentity,
        host_identity: PeerIdentity,
    ) -> (
        Result<PeerIdentity, HandshakeError>,
        Result<PeerIdentity, HandshakeError>,
    ) {
        let mut host = transport();
        let host_addr = host.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            handshake(&mut host, Role::Host, host_identity, Duration::from_secs(5))
        });

        let mut client = transport();
        client.set_peer(host_addr);
        let client_result = handshake(
            &mut client,
            Role::Client,
            client_identity,
            Duration::from_secs(5),
        );
        (client_result, handle.join().unwrap())
    }

    #[test]
    fn compatible_peers_exchange_identities() {
        let (client, host) = run(identity(PlayerHandle::P1), identity(PlayerHandle::P2));
        assert_eq!(client.unwrap().player, PlayerHandle::P2);
        assert_eq!(host.unwrap().player, PlayerHandle::P1);
    }

    #[test]
    fn a_seed_mismatch_is_refused_by_both_sides_with_a_reason() {
        let mut host_identity = identity(PlayerHandle::P2);
        host_identity.seed = 0x9999;
        let (client, host) = run(identity(PlayerHandle::P1), host_identity);

        let client_err = client.unwrap_err();
        assert!(
            matches!(client_err, HandshakeError::Refused(r) if r.contains("seed")),
            "client got {client_err:?}"
        );
        let host_err = host.unwrap_err();
        assert!(
            matches!(host_err, HandshakeError::Rejected(r) if r.contains("seed")),
            "host got {host_err:?}"
        );
    }

    #[test]
    fn a_rom_hash_mismatch_is_refused() {
        let mut host_identity = identity(PlayerHandle::P2);
        host_identity.rom_hash = [0xAB; 32];
        let (client, _host) = run(identity(PlayerHandle::P1), host_identity);
        assert!(
            matches!(client.unwrap_err(), HandshakeError::Refused(r) if r.contains("ROM")),
        );
    }

    #[test]
    fn two_peers_asking_for_the_same_slot_are_refused() {
        let (client, _host) = run(identity(PlayerHandle::P1), identity(PlayerHandle::P1));
        assert!(
            matches!(client.unwrap_err(), HandshakeError::Refused(r) if r.contains("player slot")),
        );
    }

    #[test]
    fn a_client_with_nobody_listening_times_out() {
        let mut client = transport();
        // Port 1 on loopback: nothing will ever answer.
        client.set_peer(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1));
        let result = handshake(
            &mut client,
            Role::Client,
            identity(PlayerHandle::P1),
            Duration::from_millis(300),
        );
        assert!(matches!(result, Err(HandshakeError::Timeout(_))));
    }

    #[test]
    fn an_identity_is_never_compatible_with_itself() {
        assert_eq!(
            self_check(&identity(PlayerHandle::P1)),
            Some(Incompatibility::PlayerSlot)
        );
    }

    #[test]
    fn describe_names_both_slots() {
        let text = describe(&identity(PlayerHandle::P1), &identity(PlayerHandle::P2));
        assert!(text.contains("P1"));
        assert!(text.contains("P2"));
        assert!(text.contains("Arena"));
    }
}
