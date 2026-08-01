# Benchmark methodology

The numbers are in [`README.md`](README.md) and [`results.json`](results.json).
This file is the protocol they were produced by, written so that a reader who
distrusts the numbers can decide whether to, and reproduce them if they want to.

Everything here is implemented in [`run.js`](run.js), which is the only way any
of it is measured. One command:

```
node bench/run.js            # 11 repetitions, the published run
node bench/run.js --quick    # 5, for a fast check
```

---

## The environment

| | |
|---|---|
| CPU | AMD Ryzen 5 7600, 6 cores / 12 threads |
| OS | Windows 11 Pro, 10.0.26200 |
| Node | v24.13.1 (V8 13.x) |
| rustc | 1.97.1, `--release` |
| Profile | opt-level 3, LTO `fat`, `codegen-units = 1`, `panic = "abort"` |
| Boost clocks | **not** disabled |

That last row matters, so it is in the report's header as well as here. This is
a desktop machine with an unpinned governor and no way to hold the clock steady
from inside a Node process. The interleaving below is what compensates for it,
and the dispersion column is what shows whether it compensated enough.

## The protocol

**Both implementations are measured in the same process, in the same run,
interleaved A/B/A/B.** Not all of one then all of the other. A machine that is
faster in the first thirty seconds of a run than in the last will hand a large
free advantage to whichever implementation went first, and the report will call
it an optimisation. Interleaving cancels the drift; running in a fixed order
merely hides it.

**Warm-up is discarded, and the count is stated.** Three batches per scenario,
per implementation, before anything is recorded. V8 does not compile a function
properly until it has watched it run, so timing cold JavaScript against warm
Rust is the easiest available way to produce a flattering number that is not
true. The Rust side is warmed identically, for symmetry rather than necessity.

**Eleven repetitions; the median is reported, with the interquartile range.**
A single number with no dispersion is not a measurement. The IQR is in
`results.json` for every row.

**The corpus is fixed.** Operands are generated from constant seeds
(`0xBEEF`, `0xCAFE`, `0xF00D`, `0xD00D`) at 20, 200, 1 000 and 10 000 digits, so
the same values are used on both sides, on every machine, on every run.

**Operands are built outside the timed closure**, except in the scenarios that
are explicitly about construction. What is measured is the operation.

## The verdict column, and when there is no verdict

A ratio of two medians is not by itself a result. Where the gap between the two
medians is smaller than their mean interquartile range, `run.js` prints
**`no measurable difference`** rather than a ratio, and records that in
`results.json` beside the numbers.

The test is crude and conservative on purpose. Six of the twenty throughput rows
fail it, and that is not a defect in the report — it is the single most useful
thing the report says. At twenty significant digits these two implementations
are the same speed. Everything the port gains, it gains from operand size.

The rule is applied mechanically to every row, including the ones where it
erases a win.

## The latency table, and its instrument

Per-call latency is sampled 100 000 times, one `plus` per sample, with
`process.hrtime.bigint()` around each call.

**The instrument is measured too, and reported on its own line.** On Windows the
clock's granularity is 100 ns, and decimal.js's own p50 is 100 ns — one tick. So
the correct reading of that row is not "decimal.js takes 100 ns"; it is
"decimal.js is at or below the resolution of the timer, and decimal-rs is not."
The port's p50 of ~900 ns is nine ticks and is measured properly. The comparison
is sound in the direction that matters — the port is slower per call, by roughly
the cost of a Node-API boundary crossing — and unsound if read as a precise
multiple.

`max` is reported and is dominated by garbage collection on both sides. It is
not a tail-latency claim about the arithmetic.

## Startup and memory

Startup is nine fresh child processes per implementation, each doing
`require()` plus one construction, timed from outside. It includes process
creation on both sides, which is common to both and does not cancel out of the
absolute figures — read the difference, not the totals.

Peak RSS is `process.memoryUsage().rss` sampled over 200 000 mixed operations at
precision 200, in a fresh child process per implementation.

## What is not measured

- **Threading.** decimal.js is single-threaded with global mutable constructor
  state, and the port reproduces that model exactly; there is no concurrency to
  benchmark. The soak test in [`../scripts/soak.js`](../scripts/soak.js) covers
  the related question, which is whether long single-threaded operation leaks
  across the FFI boundary.
- **Anything in `fuzz/`.** The vendored `fuzz/reference/decimal.js` is the
  oracle for both the fuzzer and these benchmarks. It is not linked into the
  port, and the port does not call it.
