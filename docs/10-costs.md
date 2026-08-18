# 10 - Costs

`eu-central-1` (Frankfurt) on-demand prices in USD, as an order-of-magnitude
reference. Check AWS's current table before planning anything. Prices move and
this file does not.

## A four-hour session

| Resource | Price | 4 h |
|---|---|---|
| EC2 `t3.small` on-demand | ~0.0216 /h | ~0.086 |
| EBS gp3, 20 GB | ~0.0952 /GB-month | ~0.010 |
| Elastic IP (attached to a running instance) | free | 0.00 |
| S3 Standard, ~200 MB | ~0.0245 /GB-month | <0.01 |
| S3 requests (a few hundred) | ~0.005 /1000 PUT | <0.01 |
| SSM Parameter Store (Standard tier) | free | 0.00 |
| SSM Session Manager | free | 0.00 |
| Egress, a 180 s session | ~0.09 /GB | negligible |

**About US$ 0.11 for a four-hour session.**

The measured runs came in well under that: the two Madrid-Frankfurt sessions in
[08 - Experiments](08-experiments.md), including bring-up and teardown, cost
under **US$ 0.05** between them.

Egress deserves a note, because it runs against intuition. A 180-second session
at ~35 kbit/s moves roughly **0.8 MB** per direction. Only inputs travel. You
could not spend meaningful money on bandwidth in this lab if you tried.

Video changes that arithmetic slightly. Recording the remote peer produces a
~50 MB MP4 that `collect` pulls down through S3. Still cents, but it is the one
thing here that moves real bytes.

## What actually costs money

### A forgotten instance

A `t3.small` left running for a month is **~US$ 15**, plus the volume. That is
the only plausible way this lab costs anything you would notice.

Two defences, both on by default:

```hcl
instance_initiated_shutdown_behavior = "terminate"
```

```bash
shutdown -h "+240"    # armed at boot by user-data
```

Because the behaviour is *terminate* rather than *stop*, the four-hour deadline
is real: the instance disappears and the volume goes with it
(`delete_on_termination = true`).

### An unattached Elastic IP

An EIP **attached to a running instance** is free. An allocated, idle one costs
about US$ 3.60/month.

That matters if a `terraform destroy` fails partway: the instance can go and the
EIP remain. Which is why [11 - Cleanup](11-cleanup.md) checks for EIPs
explicitly.

### Orphaned volumes

`delete_on_termination = true` covers the normal case. A snapshot taken by hand,
or a volume from an instance terminated some other way, does not.

### A bucket with objects still in it

The bucket has `force_destroy = true` and a seven-day lifecycle, and
`just aws-down` empties it explicitly before the destroy. But 200 MB sitting
there costs cents a month. The problem is not the money, it is somebody's
**ROM** left in a forgotten bucket.

## Estimating before you build

```bash
just aws-plan
```

The plan lists 21 resources. The ones with ongoing cost are `aws_instance`,
`aws_ebs_volume` (through `root_block_device`), `aws_eip` and `aws_s3_bucket`.

## The other kind of cost

Beyond the bill, there is rollback's CPU cost. It shows up in the dashboard and
in `summary.csv`:

| Metric | Meaning |
|---|---|
| `resimulation_overhead` | Re-simulated frames per presented frame. 0.1 = 10% extra work. |
| `rollback_advance_seconds_total` | Summed time inside `advance_frame`. |
| `rollback_save_state_seconds_total` | Summed time inside `save_state`. |
| `cpu_seconds` | Process CPU, from `/proc/self/stat`. |
| `effective_fps` | Presented frames ÷ duration. Needs to sit near 60. |

In the arena, with a 204-byte state, these are noise: about 2 seconds of CPU for
a 240-second session. On The Last Blade 2 they are the number that decides
between `t3.small` and `t3.medium`: 116 seconds of CPU for a 300-second session,
roughly 39% of a core.

## Why not something bigger

`t3.medium` costs twice as much (~0.0432/h). For the arena it is pure waste. For
the emulated game, measure first: the bottleneck is `retro_serialize`, which is
dominated by memory bandwidth rather than vCPU count, so doubling the instance
type may buy less than it looks like it should.

The measured breakdown is in [15 - Elastic](15-elastic.md): `save_state` alone
is 2 271 µs of a 16 667 µs frame. Look at
`rollback_save_state_seconds_total` before changing anything.

## The rule

Run `just collect && just aws-down` at the end of **every** session. The whole
lab costs less than a coffee for as long as that stays true, and the two
shutdown mechanisms exist to keep it cheap on the days it does not.
