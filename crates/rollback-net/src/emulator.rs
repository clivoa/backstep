//! Synthetic network impairment.
//!
//! Applied to *outgoing* datagrams, on purpose. Delaying inbound datagrams
//! would be easier, but it would not reproduce the thing we want to study:
//! under real loss the sender's data never exists on the wire, so the peer's
//! `InputBatch` redundancy has to cover it. Impairing the send side puts the
//! emulator in the same position as the real network.
//!
//! Both peers apply their own profile, so a symmetric experiment means the same
//! profile configured on both ends, and the round trip sees roughly twice the
//! configured one-way delay.

use std::collections::VecDeque;

use rollback_core::{DeterministicRng, NetworkProfile};

/// Extra delay applied to a datagram chosen for reordering.
///
/// Must exceed the inter-packet gap (16.7 ms at 60 Hz) or the datagram would
/// still arrive in order and the profile would silently do nothing.
const REORDER_EXTRA_MS: u64 = 25;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EmulatorStats {
    pub submitted: u64,
    pub dropped: u64,
    pub duplicated: u64,
    pub reordered: u64,
    pub delivered: u64,
}

struct Pending {
    release_at_ms: u64,
    /// Submission order, so datagrams released in the same millisecond keep a
    /// deterministic sequence.
    order: u64,
    bytes: Vec<u8>,
}

pub struct NetworkEmulator {
    profile: NetworkProfile,
    rng: DeterministicRng,
    queue: VecDeque<Pending>,
    next_order: u64,
    stats: EmulatorStats,
}

impl NetworkEmulator {
    pub fn new(profile: NetworkProfile) -> Self {
        Self {
            profile,
            rng: DeterministicRng::new(profile.seed),
            queue: VecDeque::new(),
            next_order: 0,
            stats: EmulatorStats::default(),
        }
    }

    pub fn profile(&self) -> &NetworkProfile {
        &self.profile
    }

    pub fn stats(&self) -> &EmulatorStats {
        &self.stats
    }

    /// True when datagrams pass through untouched and `submit` returns them
    /// immediately.
    pub fn is_transparent(&self) -> bool {
        self.profile.is_transparent()
    }

    /// Hand a datagram to the emulator.
    ///
    /// Returns the datagrams to send *right now* -- for a transparent profile
    /// that is the datagram itself, which keeps the `natural` profile free of
    /// any emulator overhead at all.
    pub fn submit(&mut self, now_ms: u64, bytes: Vec<u8>) -> Vec<Vec<u8>> {
        self.stats.submitted += 1;

        if self.is_transparent() {
            self.stats.delivered += 1;
            return vec![bytes];
        }

        if self.rng.chance_permille(self.profile.loss_permille) {
            self.stats.dropped += 1;
            return Vec::new();
        }

        let mut delay = u64::from(self.profile.delay_ms);
        if self.profile.jitter_ms > 0 {
            let span = self.profile.jitter_ms * 2 + 1;
            let offset = i64::from(self.rng.below(span)) - i64::from(self.profile.jitter_ms);
            delay = delay.saturating_add_signed(offset);
        }
        if self.rng.chance_permille(self.profile.reorder_permille) {
            delay += REORDER_EXTRA_MS;
            self.stats.reordered += 1;
        }

        self.enqueue(now_ms + delay, bytes.clone());
        if self.rng.chance_permille(self.profile.duplicate_permille) {
            self.stats.duplicated += 1;
            self.enqueue(now_ms + delay, bytes);
        }

        self.drain_due(now_ms)
    }

    fn enqueue(&mut self, release_at_ms: u64, bytes: Vec<u8>) {
        self.queue.push_back(Pending {
            release_at_ms,
            order: self.next_order,
            bytes,
        });
        self.next_order += 1;
    }

    /// Datagrams whose delay has elapsed, oldest release time first.
    ///
    /// Call this every frame even when nothing is being sent, otherwise a
    /// delayed datagram would sit in the queue until the next `submit`.
    pub fn drain_due(&mut self, now_ms: u64) -> Vec<Vec<u8>> {
        if self.queue.is_empty() {
            return Vec::new();
        }
        let mut due: Vec<Pending> = Vec::new();
        let mut kept = VecDeque::with_capacity(self.queue.len());
        for pending in self.queue.drain(..) {
            if pending.release_at_ms <= now_ms {
                due.push(pending);
            } else {
                kept.push_back(pending);
            }
        }
        self.queue = kept;
        due.sort_by_key(|p| (p.release_at_ms, p.order));
        self.stats.delivered += due.len() as u64;
        due.into_iter().map(|p| p.bytes).collect()
    }

    /// Datagrams still waiting to be released.
    pub fn in_flight(&self) -> usize {
        self.queue.len()
    }

    /// Release everything immediately, for a clean shutdown.
    pub fn flush(&mut self) -> Vec<Vec<u8>> {
        self.drain_due(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(mut p: NetworkProfile, seed: u64) -> NetworkProfile {
        p.seed = seed;
        p
    }

    #[test]
    fn a_transparent_profile_passes_datagrams_straight_through() {
        let mut e = NetworkEmulator::new(NetworkProfile::NATURAL);
        assert!(e.is_transparent());
        let out = e.submit(0, vec![1, 2, 3]);
        assert_eq!(out, vec![vec![1, 2, 3]]);
        assert_eq!(e.in_flight(), 0);
    }

    #[test]
    fn a_fixed_delay_holds_the_datagram_for_exactly_that_long() {
        let p = profile(NetworkProfile::named("delay20").unwrap().1, 1);
        let mut e = NetworkEmulator::new(p);

        assert!(e.submit(1_000, vec![9]).is_empty(), "must not send immediately");
        assert!(e.drain_due(1_019).is_empty(), "still early");
        assert_eq!(e.drain_due(1_020), vec![vec![9]]);
        assert_eq!(e.in_flight(), 0);
    }

    #[test]
    fn jitter_stays_inside_the_configured_band() {
        let p = profile(NetworkProfile::named("jitter30").unwrap().1, 7);
        let mut e = NetworkEmulator::new(p);
        // 30 +/- 15 ms, and reordering is off for this profile.
        for i in 0..500u64 {
            e.submit(i * 100, vec![i as u8]);
        }
        // Nothing may be delivered before the minimum delay.
        let mut e2 = NetworkEmulator::new(p);
        for i in 0..200u64 {
            let now = i * 100;
            e2.submit(now, vec![0]);
            assert!(
                e2.drain_due(now + u64::from(p.min_delay_ms()) - 1).is_empty(),
                "delivered earlier than {} ms",
                p.min_delay_ms()
            );
            assert_eq!(
                e2.drain_due(now + u64::from(p.max_delay_ms())).len(),
                1,
                "not delivered by {} ms",
                p.max_delay_ms()
            );
        }
    }

    #[test]
    fn loss_drops_roughly_the_configured_fraction() {
        let p = profile(NetworkProfile::named("loss2").unwrap().1, 99);
        let mut e = NetworkEmulator::new(p);
        for i in 0..20_000u64 {
            e.submit(i, vec![0]);
        }
        let dropped = e.stats().dropped;
        // 2% of 20 000 is 400; allow a generous band for a 20k sample.
        assert!((250..=560).contains(&dropped), "dropped {dropped} of 20000");
    }

    #[test]
    fn reordering_actually_swaps_the_delivery_order() {
        let p = NetworkProfile {
            delay_ms: 5,
            reorder_permille: 1000, // reorder everything, deterministically
            seed: 3,
            ..NetworkProfile::NATURAL
        };
        let mut e = NetworkEmulator::new(p);
        e.submit(0, vec![1]); // held for 5 + 25 ms
        let mut e2 = NetworkEmulator::new(NetworkProfile {
            reorder_permille: 0,
            ..p
        });
        e2.submit(0, vec![2]); // held for 5 ms only

        assert!(e.drain_due(10).is_empty());
        assert_eq!(e2.drain_due(10), vec![vec![2]]);
        assert_eq!(e.drain_due(30), vec![vec![1]]);
    }

    #[test]
    fn duplication_delivers_the_datagram_twice() {
        let p = NetworkProfile {
            delay_ms: 1,
            duplicate_permille: 1000,
            seed: 5,
            ..NetworkProfile::NATURAL
        };
        let mut e = NetworkEmulator::new(p);
        e.submit(0, vec![7]);
        assert_eq!(e.drain_due(10), vec![vec![7], vec![7]]);
        assert_eq!(e.stats().duplicated, 1);
    }

    #[test]
    fn the_same_seed_produces_the_same_impairment_pattern() {
        let p = profile(NetworkProfile::named("combined").unwrap().1, 4242);
        let run = || {
            let mut e = NetworkEmulator::new(p);
            let mut delivered = Vec::new();
            for i in 0..2000u64 {
                e.submit(i * 16, vec![(i % 251) as u8]);
                delivered.extend(e.drain_due(i * 16));
            }
            (delivered, *e.stats())
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn flush_releases_everything_still_queued() {
        let p = profile(NetworkProfile::named("delay20").unwrap().1, 1);
        let mut e = NetworkEmulator::new(p);
        for i in 0..5 {
            e.submit(0, vec![i]);
        }
        assert_eq!(e.in_flight(), 5);
        assert_eq!(e.flush().len(), 5);
        assert_eq!(e.in_flight(), 0);
    }
}
