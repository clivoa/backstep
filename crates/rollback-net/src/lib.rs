//! Authenticated UDP transport for rollback sessions.
//!
//! Four layers, each testable on its own:
//!
//! * [`wire`] -- the versioned packet format;
//! * [`auth`] -- HMAC-SHA256 over every datagram;
//! * [`emulator`] -- synthetic delay, jitter, loss, duplication and reordering;
//! * [`link`] -- RTT, loss, duplication and bitrate measurement;
//! * [`transport`] -- the socket that glues them together.

#![forbid(unsafe_code)]

pub mod auth;
pub mod cursor;
pub mod emulator;
pub mod link;
pub mod transport;
pub mod wire;

pub use auth::{key_to_hex, AuthError, Authenticator, KEY_LEN};
pub use emulator::{EmulatorStats, NetworkEmulator};
pub use link::{LinkMonitor, LinkStats};
pub use transport::{Received, TransportError, UdpTransport, DEFAULT_PORT};
pub use wire::{
    DisconnectReason, Incompatibility, Message, Packet, PeerIdentity, TelemetrySummary, WireError,
    HEADER_LEN, INPUT_REDUNDANCY, MAX_DATAGRAM, PROTOCOL_VERSION, TAG_LEN,
};
