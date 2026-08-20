# Is Petal fast enough to synthesize audio in realtime?

**Verdict: realtime-viable.** A lean Petal voice produces one 60fps frame of
audio (735 samples at 44100Hz) in **188 µs**, about **1.1% of a 16.6 ms frame**.
`enable_dsp` is worth building. The per-frame budget it should enforce is
**2 ms**, which buys roughly **8 lean voices, 3 full chip voices, or 2 rich
voices** — after which the host fades the DSP bus out, exactly as
[design.md](design.md) already specifies.

Two conditions come with that verdict, and both are load-bearing:

1. **The block buffer must be an `f64_array`, not a boxed list.** Identical
   synthesis math costs 56% more through `append`, and it drags the collector
   into the audio slice.
2. **The host must build `petal` optimized even in its own debug builds.** An
   unoptimized `petal` runs the same block in 4.4 ms — a single voice overruns
   the whole audio budget eight times over.

Everything below is measurement, not estimate: release `petal` on an Apple M4,
minimum of seven runs per figure, with the 5.5 ms interpreter startup subtracted.
The script is [`test/benchmarks/audio_synth.ptl`](../../../test/benchmarks/audio_synth.ptl);
it runs on the plain `petal` CLI with no host natives.

## Per-block cost

One block is 735 mono samples — one 60fps frame at 44100 Hz.

| Voice | µs / block | inst / block | inst / sample | % of 16.6 ms frame |
|---|---:|---:|---:|---:|
| Lean pulse (1 osc, multiplicative envelope) | 188 | 25,405 | 34.6 | 1.1% |
| Chip voice, multiplicative envelope | 430 | 51,195 | 69.7 | 2.6% |
| Chip voice as written (square + triangle + noise, `pow` envelope) | 545 | 59,274 | 80.6 | 3.3% |
| Chip voice returning a boxed list | 849 | 59,274 | 80.6 | 5.1% |
| Rich voice (3 detuned saws, SVF, control-rate LFO) | 733 | 74,828 | 101.8 | 4.4% |

The interpreter sustains **93–115 M instructions/sec** on these loops, roughly
double the ~50 M/s headline in [performance.md](../../../docs/dev/performance.md).
That is the shape of the work, not a faster machine: a synthesis inner loop is
float arithmetic on a handful of registers, with no records to hash, no strings
to slice, and no allocation per iteration — the cases that drag the average
down are absent.

### Stereo

The chip mixer's natural output is mono duplicated to both channels, which costs
nothing extra: a 735-sample block is a 735-sample block regardless of how many
channels the host writes it into. Only genuinely decorrelated stereo — a
separate synthesis pass per channel — doubles the figures above, putting the
chip voice at 1.09 ms/frame and the rich voice at 1.47 ms/frame. Do the cheap
thing: synthesize mono, pan in Rust.

### How many voices fit in 2 ms

| Voice | voices in 2 ms (mono) | in 2 ms (decorrelated stereo) |
|---|---:|---:|
| Lean pulse | 10 | 5 |
| Chip voice | 3 | 1 |
| Rich voice | 2 | 1 |

Round the lean figure down to **8** as the published budget: the table counts
inner-loop time only, and each additional voice also pays a Petal call and a mix
pass over the block.

## Ahead-of-time cost

Rendering a 0.3 s sound effect (13,230 samples) with the full chip voice takes
**9.8 ms** — 0.74 µs/sample, the same rate as the realtime path, confirming
there is no per-block overhead worth naming. A cart registering sixteen such
effects pays ~157 ms at load, once, and again on each hot reload. That is a
visible-but-fine hitch; a cart with a hundred effects would want a progress
frame, but nothing here argues for a cache on disk.

## Language-level friction found

**Boxed lists are the wrong buffer.** `chip_block_list` and `chip_block_arr`
execute the *same* instruction count — `append` and `out[i] = v` are one
instruction each — yet the list version takes 849 µs against 545 µs. The
difference is entirely allocation: 3,100 collections costing 17.9 ms across the
run, against 3 collections costing nothing for the array. Worse than the average
is the distribution: a collection that lands inside the audio slice is a glitch,
not a slowdown. `f64_array` never allocates after `f64_array(count)`.

*Recommendation for the audio natives (they are another task's files):*
`register_sound` and `enable_dsp` should accept an `f64_array` return value in
addition to the list of floats [design.md](design.md) specifies, and
`prelude/nes.ptl` should show the array form in every example. Accepting both
keeps the documented contract honest while making the fast path the obvious one.

**Native calls dominate cheap per-sample work.** Replacing `pow(0.5, t * 6.0)`
and `float(start + i)` — two natives per sample — with a multiplicative envelope
took the chip voice from 545 µs to 430 µs, a 21% saving for math that was never
the point. At ~78 ns per call, a native invocation costs about as much as eight
interpreted instructions, so anything callable per sample should be hoisted to
per-block or expressed as an accumulator. This is the `PetalCxt`-per-call cost
performance.md already names, met head-on.

**No bitwise operators.** The NES noise channel is an LFSR, and Petal has no
`^`, `<<` or `&`, so the benchmark uses a multiply/modulo PRNG sampled and held
at the channel period instead. Audibly this is fine — a sample-and-held LFSR
*is* pseudo-random noise. But a cart author trying to reproduce the exact NES
noise sequence cannot, and neither can they write the packed-2bpp tile helpers
the pattern table invites. Not a blocker for audio; worth knowing.

**Float math itself is not a problem.** Phase accumulation, wrap-by-subtract,
the state-variable filter, and the envelope all lower to plain register
arithmetic. The rich voice costs only 30% more than the chip voice despite doing
three oscillators and a two-pole filter, because everything it adds is
arithmetic rather than calls or allocation.

## What the numbers mean for `enable_dsp`

The audio transport in [design.md](design.md) — `AudioQueue<i16>` filled from
the main thread with a ~3-frame lead — is what makes this work. There is no
callback thread with a hard deadline; a Petal block that runs long steals from
the frame, not from the audio device, and the queue lead absorbs the jitter.

So the budget is a frame-time budget, and the enforcement design already in
design.md is the right one: time the call, and if the DSP function overruns its
2 ms slice, warn and fade the bus out rather than dropping a frame. Two details
worth carrying into the implementation:

- **Measure the whole call, including the marshal.** Converting the returned
  buffer to `i16` is host cost that the script's budget should include, since the
  script chose the buffer type.
- **Fade, do not cut.** The budget will be exceeded occasionally by a cart that
  is fine on average; a one-frame overrun should not silence the bus for good.
  Recover when the running cost drops back under budget.
