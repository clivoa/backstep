# 03 - Protocol

> Network terms (*datagram*, *RTT*, *SRTT*, *jitter*, *reordering*, *HMAC*) are
> explained in [00 - Glossary](00-glossary.md).

## The principle

**Only inputs cross the wire.** Never state, never positions, never "your
opponent is at (x, y)". Both peers run the same simulation and reach the same
conclusion on their own. That is what buys a 60 Hz fighting game roughly
35 kbit/s per direction.

## Datagram layout

```
offset  size  field
     0     2  magic "RB"
     2     1  protocol version
     3     1  message kind
     4     4  sequence (u32 LE, per sender, monotonic)
     8     n  payload
   8+n    32  HMAC-SHA256 over bytes [0, 8+n)
```

The decisions behind it:

**Fixed-width little-endian throughout.** No varints, no length prefixes that
could disagree with the actual buffer. The decoder consumes the payload exactly
and **rejects trailing bytes** (`WireError::TrailingBytes`).

**The version lives inside the authenticated region.** Nobody downgrades a peer
to an older parser by flipping byte 2.

**A hard 1200-byte cap.** That sits under a typical 1500-byte Ethernet MTU minus
IPv6 and UDP headers, with room for a tunnel, so a datagram **never fragments**.
A fragmented UDP datagram dies entirely if one fragment is lost, precisely the
failure mode a rollback session can least afford.

**Unknown enum values are rejected, not defaulted.** A `DisconnectReason` of 200
is a parse error, not "normal".

The exact bytes of every message are pinned in
`crates/rollback-net/tests/golden_protocol.rs`. If a vector in that file
changes, the format changed, and `PROTOCOL_VERSION` has to change with it.

## Six messages

| Kind | Code | Carries |
|---|---|---|
| `Hello` | 1 | peer identity |
| `HelloAck` | 2 | identity + refusal reason (0 = accepted) |
| `InputBatch` | 3 | start frame, up to 8 inputs, highest remote frame, ACK |
| `Checksum` | 4 | frame + state checksum |
| `TelemetrySummary` | 5 | 18 counters from the sender |
| `Disconnect` | 6 | reason |

### `InputBatch` is the entire loss-recovery strategy

There is no retransmission. No per-frame ACK. No sliding window.

Every `InputBatch` repeats **the last eight local inputs**. At 60 Hz that gives
each input eight chances to arrive before the peer needs it. At 2% independent
loss, the odds of losing all eight are 0.02⁸ ≈ 2.6 × 10⁻¹⁴.

This is simpler *and* faster than retransmission: a lost input is recovered by
the next datagram, 16.7 ms later, with no negotiation at all. It is also why
`InputBatch` is idempotent by construction: refiling a value already known is a
no-op, and `repeated_delivery_is_idempotent` proves it for arbitrary sequences.

The measurement backs it up. Under the `loss2` profile, **100% of rollbacks were
depth 1**, the shallowest correction possible, and there were only four of them
in 240 seconds. Loss barely becomes rollback at all.

The batch also carries `highest_remote_frame` (how far the sender has confirmed
our inputs, useful for diagnosis) and `ack_sequence` (the highest sequence the
sender has seen from us, which is how RTT gets sampled).

### `Checksum` is desync detection

Every 60 confirmed frames each peer sends the checksum of the state at the start
of that frame. The receiver compares.

The check is **deferred**, not immediate. Two conditions must hold before a
comparison means anything:

1. we have simulated that frame, and
2. our own state at that frame is **final**, with every earlier input confirmed.

Neither is guaranteed on arrival. A peer sends its checksum as soon as the frame
is final *for it*, and whichever side is running a few milliseconds ahead gets
there first. Comparing too early would produce a false desync against a state a
pending rollback is about to rewrite; discarding what arrives early would make
detection work in one direction only.

> That second failure was found by the E2E test: under `natural`, one peer
> compared ten checksums and the other compared zero. Checksums are now parked
> until both conditions hold.

A confirmed disagreement **ends the session immediately**. There is no recovery.
The two games are already different.

## Authentication

Every datagram carries an HMAC-SHA256 over its whole body.

UDP has no connection state. Without this, anyone who guesses the port can
inject an input frame into someone's match, or worse, an `InputBatch` that
contradicts a confirmed frame and kills the session outright.

What it is **not**:

- **Not encryption.** Inputs are not secret.
- **Not replay defence on its own.** The session's own frame bookkeeping makes a
  repeated batch a no-op.

It answers exactly one question: *did the peer holding the session key send
this?*

Verification uses a constant-time comparison (`Mac::verify_slice`). A byte-wise
comparison with early return would leak the tag one byte at a time to anyone who
can time the replies.

### The key

- 32 bytes, generated per session from `/dev/urandom`.
- Stored in SSM Parameter Store as a `SecureString`.
- **Never enters Terraform state**: the resource is created with a placeholder
  and carries `ignore_changes = [value]`; `just aws-up` writes the real value.
- **Never appears on a command line**, because an argument is visible in `ps` to
  every user on the box. It arrives through `ROLLBACK_SESSION_KEY` or
  `ROLLBACK_SESSION_KEY_FILE` (mode 0600).
- `Authenticator`'s `Debug` prints `<redacted>`, and a test enforces that no key
  byte escapes that way.

## Handshake

The handshake is **not** about security. The HMAC already answered "is this the
right peer?". This is about **compatibility**.

`PeerIdentity` carries everything that, if it differed between peers, would make
the simulations diverge:

| Field | Why |
|---|---|
| protocol version | different parsers |
| simulation | different games |
| app commit | different builds can simulate differently |
| config hash | input delay, prediction limit, state history, seed |
| seed | seeds the bots |
| core SHA-256 | a different emulator is a different simulation |
| ROM + BIOS SHA-256 | a different revision is a different game |
| player slot | both cannot be P1 |

The ROM field covers the BIOS too, and that is not pedantry: a Neo Geo game is
only half the code that runs. Two peers with different `neogeo.zip` would pass a
naive handshake and then diverge during boot, before either player touched a
button.

The check is **ordered**, so a refusal names the *first* thing that differs.
"ROM hash mismatch" is infinitely more useful than "incompatible".

Both sides verify independently rather than the client trusting the host's ack,
and both send `Disconnect` before giving up, so the other end gets a reason
instead of a timeout.

Neither the core nor the ROM is transmitted, only 32-byte digests. A peer learns
whether the other has the same file without the lab distributing anything.

## Network emulation

Synthetic delay, jitter, loss, duplication and reordering are applied to
**outgoing** datagrams, not incoming ones.

Delaying on ingress would be easier and would not reproduce the phenomenon that
matters: under real loss the sender's data never exists on the wire at all, and
it is the sender's own `InputBatch` redundancy that has to cover it. Impairing
egress puts the emulator where the real network is.

The consequence: a symmetric experiment means the **same profile configured on
both sides**, and the observed RTT is roughly twice the configured one-way
delay. The report's numbers reflect that.

Reordering adds an extra 25 ms to the chosen datagram. It has to exceed the
inter-packet gap (16.7 ms at 60 Hz), or the datagram would arrive in order
anyway and the profile would do nothing.

Everything is seeded (`NetworkProfile::seed`), so an experiment repeats.

## Measurement

| Metric | How |
|---|---|
| Smoothed RTT (SRTT) | RFC 6298, from `ack_sequence` on `InputBatch` |
| RTT variation (RTTVAR) | RFC 6298 |
| Inferred loss | gaps in the peer's sequence: `(highest_seq + 1) − unique` |
| Duplication | 64-sequence window, bitmask |
| Reordering | a sequence lower than the highest already seen |
| Bitrate | bytes × 8 ÷ elapsed |

### Why there is no one-way latency

Measuring one-way delay requires both clocks to agree. Two machines on opposite
sides of a border with unsynchronised NTP can easily differ by tens of
milliseconds, the same order as the thing being measured.

Reporting `arrival − send` across those clocks would produce a number that looks
precise and means nothing. This lab reports RTT (which needs one clock) and says
**nothing** about one-way latency. That is written at the top of
`crates/rollback-net/src/link.rs` and repeated in the HTML report's caveats.

### Inferred loss self-corrects

A delayed datagram looks like loss until it arrives. When it does, the unique
counter rises and the estimate corrects itself. That is why the metric is called
*inferred* rather than *measured*.

## Not in the MVP

STUN, relay, matchmaking, spectating, reconnection, state sync. A direct path
between peers is a **precondition**, not something the lab negotiates. If the
EC2 instance is not reachable on UDP/7000 from your address, the session does
not start, and the handshake says so with a timeout.
