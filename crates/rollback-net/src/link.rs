//! Link measurement: RTT, loss, duplication, reordering, bitrate.
//!
//! # Why there is no one-way latency here
//!
//! Measuring one-way delay requires the two clocks to agree, and two machines
//! on opposite sides of the Atlantic running unsynchronised NTP can easily be
//! tens of milliseconds apart -- the same order as the thing being measured.
//! Reporting `arrival_time - send_time` across those clocks would produce a
//! number that looks precise and is meaningless, so the lab reports round-trip
//! time (which needs only one clock) and says nothing about one-way delay.

use std::collections::BTreeMap;

/// Window used for duplicate detection, in packets.
const WINDOW: u32 = 64;
/// How long an unacknowledged send is kept for RTT matching.
const RTT_SAMPLE_TTL_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LinkStats {
    pub packets_sent: u64,
    pub bytes_sent: u64,
    /// Datagrams that authenticated and parsed, including duplicates.
    pub packets_received: u64,
    pub bytes_received: u64,
    /// Distinct sequence numbers received.
    pub unique_received: u64,
    pub duplicates_received: u64,
    pub reordered_received: u64,
    /// Datagrams rejected by the authenticator.
    pub auth_failures: u64,
    /// Datagrams that authenticated but did not parse.
    pub malformed: u64,
    /// Highest sequence number seen from the peer.
    pub highest_sequence: i64,
    /// Smoothed round-trip time, in microseconds (RFC 6298 SRTT).
    pub srtt_micros: u32,
    /// Round-trip time variation, in microseconds (RFC 6298 RTTVAR).
    pub rttvar_micros: u32,
    pub rtt_samples: u64,
}

impl LinkStats {
    /// Datagrams the peer must have sent that never arrived.
    ///
    /// Sequence numbers are contiguous per sender, so the peer sent at least
    /// `highest_sequence + 1` datagrams. Anything missing from that run is
    /// either lost or still in flight; the figure self-corrects as late
    /// datagrams land.
    pub fn inferred_lost(&self) -> u64 {
        let expected = (self.highest_sequence + 1).max(0) as u64;
        expected.saturating_sub(self.unique_received)
    }

    /// Inferred inbound loss as a fraction in `[0, 1]`.
    pub fn loss_ratio(&self) -> f64 {
        let expected = (self.highest_sequence + 1).max(0) as f64;
        if expected <= 0.0 {
            return 0.0;
        }
        self.inferred_lost() as f64 / expected
    }

    pub fn srtt_ms(&self) -> f64 {
        f64::from(self.srtt_micros) / 1000.0
    }

    pub fn rttvar_ms(&self) -> f64 {
        f64::from(self.rttvar_micros) / 1000.0
    }

    /// Outbound bitrate over `elapsed_ms`, in bits per second.
    pub fn send_bitrate(&self, elapsed_ms: u64) -> f64 {
        rate(self.bytes_sent, elapsed_ms)
    }

    /// Inbound bitrate over `elapsed_ms`, in bits per second.
    pub fn receive_bitrate(&self, elapsed_ms: u64) -> f64 {
        rate(self.bytes_received, elapsed_ms)
    }
}

fn rate(bytes: u64, elapsed_ms: u64) -> f64 {
    if elapsed_ms == 0 {
        return 0.0;
    }
    (bytes as f64) * 8.0 * 1000.0 / (elapsed_ms as f64)
}

/// Tracks sequence numbers to classify arrivals and to sample RTT.
pub struct LinkMonitor {
    stats: LinkStats,
    /// Bitmask of the `WINDOW` sequences at or below `highest_sequence`.
    window: u64,
    /// Send timestamps awaiting an acknowledgement.
    sent_at_ms: BTreeMap<u32, u64>,
}

impl Default for LinkMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkMonitor {
    pub fn new() -> Self {
        LinkMonitor {
            stats: LinkStats {
                highest_sequence: -1,
                ..Default::default()
            },
            window: 0,
            sent_at_ms: BTreeMap::new(),
        }
    }

    pub fn stats(&self) -> &LinkStats {
        &self.stats
    }

    pub fn on_sent(&mut self, sequence: u32, bytes: usize, now_ms: u64) {
        self.stats.packets_sent += 1;
        self.stats.bytes_sent += bytes as u64;
        self.sent_at_ms.insert(sequence, now_ms);
        // Drop samples too old to ever be acknowledged, so the map cannot grow
        // without bound over a long session.
        let cutoff = now_ms.saturating_sub(RTT_SAMPLE_TTL_MS);
        self.sent_at_ms.retain(|_, &mut sent| sent >= cutoff);
    }

    pub fn on_auth_failure(&mut self) {
        self.stats.auth_failures += 1;
    }

    pub fn on_malformed(&mut self) {
        self.stats.malformed += 1;
    }

    /// Classify an authenticated, parsed datagram.
    ///
    /// Returns false when the datagram is a duplicate the caller should ignore.
    pub fn on_received(&mut self, sequence: u32, bytes: usize) -> bool {
        self.stats.packets_received += 1;
        self.stats.bytes_received += bytes as u64;

        let seq = i64::from(sequence);
        let highest = self.stats.highest_sequence;

        if seq > highest {
            let shift = (seq - highest) as u32;
            self.window = if shift >= WINDOW {
                0
            } else {
                self.window << shift
            };
            self.window |= 1;
            self.stats.highest_sequence = seq;
            self.stats.unique_received += 1;
            return true;
        }

        let back = (highest - seq) as u32;
        if back >= WINDOW {
            // Older than the window: too late to tell duplicate from reorder.
            // Count it as reordered and do not credit it as unique, so the loss
            // figure stays conservative rather than swinging negative.
            self.stats.reordered_received += 1;
            return true;
        }

        let bit = 1u64 << back;
        if self.window & bit != 0 {
            self.stats.duplicates_received += 1;
            return false;
        }
        self.window |= bit;
        self.stats.unique_received += 1;
        self.stats.reordered_received += 1;
        true
    }

    /// Fold in an RTT sample from a peer's acknowledgement, RFC 6298 style.
    pub fn on_ack(&mut self, ack_sequence: u32, now_ms: u64) {
        let Some(sent_at) = self.sent_at_ms.remove(&ack_sequence) else {
            return;
        };
        let sample_us = now_ms.saturating_sub(sent_at).saturating_mul(1000) as f64;
        self.stats.rtt_samples += 1;

        if self.stats.rtt_samples == 1 {
            self.stats.srtt_micros = sample_us as u32;
            self.stats.rttvar_micros = (sample_us / 2.0) as u32;
            return;
        }
        let srtt = f64::from(self.stats.srtt_micros);
        let rttvar = f64::from(self.stats.rttvar_micros);
        let new_rttvar = 0.75 * rttvar + 0.25 * (srtt - sample_us).abs();
        let new_srtt = 0.875 * srtt + 0.125 * sample_us;
        self.stats.rttvar_micros = new_rttvar as u32;
        self.stats.srtt_micros = new_srtt as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_in_order_run_is_all_unique_and_loses_nothing() {
        let mut m = LinkMonitor::new();
        for seq in 0..100 {
            assert!(m.on_received(seq, 40));
        }
        assert_eq!(m.stats().unique_received, 100);
        assert_eq!(m.stats().duplicates_received, 0);
        assert_eq!(m.stats().reordered_received, 0);
        assert_eq!(m.stats().inferred_lost(), 0);
        assert_eq!(m.stats().loss_ratio(), 0.0);
    }

    #[test]
    fn a_duplicate_is_detected_and_told_to_be_ignored() {
        let mut m = LinkMonitor::new();
        assert!(m.on_received(0, 40));
        assert!(m.on_received(1, 40));
        assert!(!m.on_received(1, 40), "the caller must be told to drop it");
        assert_eq!(m.stats().duplicates_received, 1);
        assert_eq!(m.stats().unique_received, 2);
    }

    #[test]
    fn a_late_arrival_counts_as_reordered_and_repairs_the_loss_estimate() {
        let mut m = LinkMonitor::new();
        m.on_received(0, 40);
        m.on_received(2, 40);
        assert_eq!(m.stats().inferred_lost(), 1, "frame 1 looks lost");

        assert!(m.on_received(1, 40));
        assert_eq!(m.stats().reordered_received, 1);
        assert_eq!(m.stats().inferred_lost(), 0, "and then it turned up");
    }

    #[test]
    fn a_genuine_gap_shows_up_as_loss() {
        let mut m = LinkMonitor::new();
        for seq in [0, 1, 2, 7, 8, 9] {
            m.on_received(seq, 40);
        }
        assert_eq!(m.stats().inferred_lost(), 4);
        assert!((m.stats().loss_ratio() - 0.4).abs() < 1e-9);
    }

    #[test]
    fn a_huge_jump_forward_resets_the_window_without_panicking() {
        let mut m = LinkMonitor::new();
        m.on_received(0, 40);
        m.on_received(10_000, 40);
        assert_eq!(m.stats().highest_sequence, 10_000);
        assert!(m.on_received(9_999, 40), "still inside the window");
        assert!(
            m.on_received(5, 40),
            "far outside the window, accepted once"
        );
    }

    #[test]
    fn rtt_starts_at_the_first_sample_and_then_smooths() {
        let mut m = LinkMonitor::new();
        m.on_sent(1, 40, 0);
        m.on_ack(1, 100);
        assert_eq!(m.stats().srtt_micros, 100_000);
        assert_eq!(m.stats().rttvar_micros, 50_000);

        m.on_sent(2, 40, 200);
        m.on_ack(2, 300);
        // A second identical sample must not move SRTT.
        assert_eq!(m.stats().srtt_micros, 100_000);
        assert!(m.stats().rttvar_micros < 50_000, "variation should shrink");
    }

    #[test]
    fn an_ack_for_something_we_never_sent_is_ignored() {
        let mut m = LinkMonitor::new();
        m.on_ack(12_345, 1_000);
        assert_eq!(m.stats().rtt_samples, 0);
        assert_eq!(m.stats().srtt_micros, 0);
    }

    #[test]
    fn stale_send_timestamps_are_forgotten() {
        let mut m = LinkMonitor::new();
        m.on_sent(1, 40, 0);
        m.on_sent(2, 40, RTT_SAMPLE_TTL_MS + 1);
        m.on_ack(1, RTT_SAMPLE_TTL_MS + 2);
        assert_eq!(m.stats().rtt_samples, 0, "sequence 1 should have aged out");
    }

    #[test]
    fn bitrate_is_bits_per_second() {
        let mut m = LinkMonitor::new();
        for seq in 0..100 {
            m.on_sent(seq, 100, u64::from(seq) * 10);
        }
        // 100 datagrams x 100 bytes over 1 second.
        assert!((m.stats().send_bitrate(1_000) - 80_000.0).abs() < 1e-6);
        assert_eq!(m.stats().send_bitrate(0), 0.0);
    }

    #[test]
    fn auth_and_parse_failures_are_counted_separately() {
        let mut m = LinkMonitor::new();
        m.on_auth_failure();
        m.on_malformed();
        assert_eq!(m.stats().auth_failures, 1);
        assert_eq!(m.stats().malformed, 1);
        assert_eq!(m.stats().packets_received, 0, "neither counts as received");
    }
}
