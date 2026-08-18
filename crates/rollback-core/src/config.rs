//! Session and network-emulation configuration.
//!
//! Every field here is part of the handshake compatibility check: two peers
//! that disagree on any of it would diverge, so the session refuses to start
//! rather than desync later.

use serde::{Deserialize, Serialize};

/// Frame index. `-1` (`NULL_FRAME`) means "no frame".
pub type Frame = i32;

/// Sentinel for "no frame yet".
pub const NULL_FRAME: Frame = -1;

/// Which simulation the session drives.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SimulationKind {
    /// The instrumented deterministic 2D arena.
    Arena,
    /// The Last Blade 2 hosted through the same libretro core.
    LastBlade2,
}

impl SimulationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            SimulationKind::Arena => "arena",
            SimulationKind::LastBlade2 => "lastblade2",
        }
    }
}

impl std::str::FromStr for SimulationKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "arena" => Ok(SimulationKind::Arena),
            "lastblade2" | "lastbld2" => Ok(SimulationKind::LastBlade2),
            other => Err(format!(
                "unknown simulation '{other}' (expected arena|lastblade2)"
            )),
        }
    }
}

/// Synthetic impairment applied to the local end of the UDP link.
///
/// Both peers apply their own profile to *outgoing* datagrams, so a symmetric
/// experiment means configuring the same profile on both sides.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct NetworkProfile {
    /// Base one-way delay added to each datagram, in milliseconds.
    pub delay_ms: u32,
    /// Uniform jitter, in milliseconds: actual delay is `delay_ms ± jitter_ms`.
    pub jitter_ms: u32,
    /// Datagram loss, in per-mille (20 = 2%).
    pub loss_permille: u32,
    /// Probability a datagram is delivered twice, in per-mille.
    pub duplicate_permille: u32,
    /// Probability a datagram is pushed behind the next one, in per-mille.
    pub reorder_permille: u32,
    /// Seed for the impairment PRNG, so an experiment can be replayed exactly.
    pub seed: u64,
}

impl NetworkProfile {
    /// No synthetic impairment: whatever the real WAN does.
    pub const NATURAL: NetworkProfile = NetworkProfile {
        delay_ms: 0,
        jitter_ms: 0,
        loss_permille: 0,
        duplicate_permille: 0,
        reorder_permille: 0,
        seed: 0x5EED_0000_0000_0001,
    };

    /// The five profiles the lab runs its 180-second sessions on.
    pub fn named(name: &str) -> Option<(&'static str, NetworkProfile)> {
        let profile = match name {
            "natural" => ("natural", NetworkProfile::NATURAL),
            "delay20" => (
                "delay20",
                NetworkProfile {
                    delay_ms: 20,
                    ..NetworkProfile::NATURAL
                },
            ),
            "jitter30" => (
                "jitter30",
                NetworkProfile {
                    delay_ms: 30,
                    jitter_ms: 15,
                    ..NetworkProfile::NATURAL
                },
            ),
            "loss2" => (
                "loss2",
                NetworkProfile {
                    loss_permille: 20,
                    ..NetworkProfile::NATURAL
                },
            ),
            "combined" => (
                "combined",
                NetworkProfile {
                    delay_ms: 40,
                    jitter_ms: 20,
                    loss_permille: 20,
                    reorder_permille: 5,
                    ..NetworkProfile::NATURAL
                },
            ),
            // Madrid to São Paulo and Madrid to Tokyo both measured about
            // 267 ms round trip with only a few milliseconds of variance, so
            // one profile stands in for both. Delay is one-way and applied at
            // each end, hence half the round trip here.
            //
            // This is the only profile that exceeds the default 8-frame
            // prediction window: 133 ms is roughly 8 frames at 60 Hz, so the
            // window fills before an input can possibly answer. It exists so
            // the tuning question can be asked without paying for an instance
            // on another continent. See docs/08-experiments.md.
            "transcontinental" => (
                "transcontinental",
                NetworkProfile {
                    delay_ms: 133,
                    jitter_ms: 5,
                    ..NetworkProfile::NATURAL
                },
            ),
            _ => return None,
        };
        Some(profile)
    }

    /// Names of all built-in profiles, in the order the report presents them.
    ///
    /// `just bench` and the E2E run the first five. `transcontinental` is not
    /// in that set because it is a tuning tool rather than a baseline, and
    /// including it would change every historical benchmark row.
    pub const NAMES: [&'static str; 6] = [
        "natural",
        "delay20",
        "jitter30",
        "loss2",
        "combined",
        "transcontinental",
    ];

    /// Lowest delay this profile can produce, in milliseconds.
    pub fn min_delay_ms(&self) -> u32 {
        self.delay_ms.saturating_sub(self.jitter_ms)
    }

    /// Highest delay this profile can produce, in milliseconds.
    pub fn max_delay_ms(&self) -> u32 {
        self.delay_ms + self.jitter_ms
    }

    /// True when the profile leaves datagrams completely untouched.
    pub fn is_transparent(&self) -> bool {
        *self == NetworkProfile::NATURAL
            || (self.delay_ms == 0
                && self.jitter_ms == 0
                && self.loss_permille == 0
                && self.duplicate_permille == 0
                && self.reorder_permille == 0)
    }
}

/// Everything both peers must agree on for the session to be well defined.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SessionConfig {
    pub simulation: SimulationKind,
    /// Seed handed to the simulation. Not used by the arena physics, but it
    /// seeds the bots, so both peers must agree.
    pub seed: u64,
    /// Simulation rate. Fixed at 60 for every simulation in this lab.
    pub tick_rate_hz: u32,
    /// Frames of local input delay before an input reaches the simulation.
    pub input_delay: u8,
    /// How far ahead of the last confirmed remote frame we may speculate.
    pub prediction_limit: u8,
    /// How many saved states the ring buffer holds.
    pub state_history: u8,
    /// Confirmed-frame interval between checksum exchanges.
    pub checksum_interval: u32,
    /// How long to wait for an authenticated datagram before giving up.
    pub peer_timeout_ms: u32,
    pub network: NetworkProfile,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            simulation: SimulationKind::Arena,
            seed: 0x1234_5678_9ABC_DEF0,
            tick_rate_hz: 60,
            input_delay: 1,
            prediction_limit: 8,
            state_history: 16,
            checksum_interval: 60,
            peer_timeout_ms: 3_000,
            network: NetworkProfile::NATURAL,
        }
    }
}

impl SessionConfig {
    /// Duration of one simulated frame, in nanoseconds.
    pub fn frame_duration(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(1_000_000_000 / u64::from(self.tick_rate_hz))
    }

    /// Reject configurations the session cannot honour.
    ///
    /// The state history must be strictly deeper than the prediction limit:
    /// a rollback can reach back `prediction_limit` frames, and we need the
    /// state *at* that frame still to be in the buffer.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.tick_rate_hz == 0 {
            return Err(ConfigError::Invalid("tick_rate_hz must be non-zero"));
        }
        if self.prediction_limit == 0 {
            return Err(ConfigError::Invalid("prediction_limit must be non-zero"));
        }
        if u16::from(self.state_history) <= u16::from(self.prediction_limit) {
            return Err(ConfigError::Invalid(
                "state_history must exceed prediction_limit",
            ));
        }
        if self.checksum_interval == 0 {
            return Err(ConfigError::Invalid("checksum_interval must be non-zero"));
        }
        if self.peer_timeout_ms == 0 {
            return Err(ConfigError::Invalid("peer_timeout_ms must be non-zero"));
        }
        Ok(())
    }

    /// Stable hash of the fields both peers must agree on.
    ///
    /// The network profile is deliberately excluded: impairment is applied
    /// locally to outgoing datagrams and asymmetric profiles are a legitimate
    /// experiment, not a desync risk.
    pub fn compatibility_hash(&self) -> u64 {
        let mut h = Fnv1a::new();
        h.write(self.simulation.as_str().as_bytes());
        h.write(&self.seed.to_le_bytes());
        h.write(&self.tick_rate_hz.to_le_bytes());
        h.write(&[self.input_delay, self.prediction_limit, self.state_history]);
        h.write(&self.checksum_interval.to_le_bytes());
        h.finish()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("invalid session config: {0}")]
    Invalid(&'static str),
}

/// FNV-1a, used wherever we need a stable hash that is identical across
/// platforms and Rust versions (`DefaultHasher` guarantees neither).
pub struct Fnv1a(u64);

impl Fnv1a {
    pub const fn new() -> Self {
        Fnv1a(0xCBF2_9CE4_8422_2325)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x1000_0000_01B3);
        }
    }

    pub const fn finish(&self) -> u64 {
        self.0
    }
}

impl Default for Fnv1a {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_and_matches_the_spec() {
        let c = SessionConfig::default();
        c.validate().unwrap();
        assert_eq!(c.tick_rate_hz, 60);
        assert_eq!(c.input_delay, 1);
        assert_eq!(c.prediction_limit, 8);
        assert_eq!(c.state_history, 16);
    }

    #[test]
    fn state_history_must_exceed_prediction_limit() {
        let c = SessionConfig {
            prediction_limit: 16,
            state_history: 16,
            ..Default::default()
        };
        assert!(c.validate().is_err());
    }

    #[test]
    fn compatibility_hash_ignores_network_profile() {
        let a = SessionConfig::default();
        let b = SessionConfig {
            network: NetworkProfile::named("combined").unwrap().1,
            ..Default::default()
        };
        assert_eq!(a.compatibility_hash(), b.compatibility_hash());
    }

    #[test]
    fn compatibility_hash_reacts_to_every_agreed_field() {
        let base = SessionConfig::default();
        let mutations = [
            SessionConfig {
                simulation: SimulationKind::LastBlade2,
                ..base
            },
            SessionConfig { seed: 1, ..base },
            SessionConfig {
                tick_rate_hz: 30,
                ..base
            },
            SessionConfig {
                input_delay: 2,
                ..base
            },
            SessionConfig {
                prediction_limit: 7,
                ..base
            },
            SessionConfig {
                state_history: 20,
                ..base
            },
            SessionConfig {
                checksum_interval: 30,
                ..base
            },
        ];
        for m in mutations {
            assert_ne!(
                base.compatibility_hash(),
                m.compatibility_hash(),
                "hash ignored a change in {m:?}"
            );
        }
    }

    #[test]
    fn all_named_profiles_resolve() {
        for name in NetworkProfile::NAMES {
            let (resolved, _) = NetworkProfile::named(name).expect("profile must exist");
            assert_eq!(resolved, name);
        }
        assert!(NetworkProfile::named("nope").is_none());
    }

    #[test]
    fn combined_profile_matches_the_experiment_matrix() {
        let (_, p) = NetworkProfile::named("combined").unwrap();
        assert_eq!((p.delay_ms, p.jitter_ms), (40, 20));
        assert_eq!(p.loss_permille, 20);
        assert_eq!(p.reorder_permille, 5);
        assert_eq!((p.min_delay_ms(), p.max_delay_ms()), (20, 60));
    }

    #[test]
    fn simulation_kind_round_trips_through_str() {
        for kind in [SimulationKind::Arena, SimulationKind::LastBlade2] {
            assert_eq!(kind.as_str().parse::<SimulationKind>().unwrap(), kind);
        }
        assert!("tekken".parse::<SimulationKind>().is_err());
    }
}
