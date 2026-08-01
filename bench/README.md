# Benchmark report

decimal-rs against the original decimal.js (v10.6.0, `cd73a7f`), measured by
[`run.js`](run.js) under the protocol in [`methodology.md`](methodology.md).
Raw data, including the interquartile range for every row, in
[`results.json`](results.json).

Run: `node bench/run.js` · Host: AMD Ryzen 5 7600 / Windows 11 / Node v24.13.1
/ rustc 1.97.1 `--release`, LTO fat, one codegen unit.

---

## The short version

> On CPU-bound arbitrary-precision arithmetic the Rust port is **1.4× to 9.1×
> faster**, and the advantage is almost entirely a function of operand size:
> 2.9× at 100 digits, 8.4× at 1 000, 9.1× at 10 000. Below about **40
> significant digits it is slower**, and at the library's default precision of
> twenty there is **no measurable difference** on most operations. Called one
> small operation at a time from JavaScript it is slower still — ~800 ns against
> ~100 ns — because crossing the Node-API boundary costs more than a twenty-digit
> addition. Startup is 1.3 ms faster; peak RSS is **3.8× worse**; the compiled
> artifact is **3.8× larger** than the original's source.
>
> If your operands are twenty digits and you call one at a time, this port is
> not worth its boundary. If they run to hundreds of digits, it is worth several
> times its boundary.

## Throughput

Median of 11 interleaved repetitions. `no measurable difference` means the gap
between the two medians was smaller than their mean interquartile range — see
[methodology](methodology.md#the-verdict-column-and-when-there-is-no-verdict).
It is applied mechanically, including where it erases a win.

| Operation | decimal.js | decimal-rs | Verdict |
|---|---:|---:|---|
| parse, 20 digits | 1.26 µs | 893.8 ns | no measurable difference |
| parse, 200 digits | 1.84 µs | 1.33 µs | **1.38× faster** |
| parse, 1000 digits | 11.04 µs | 4.45 µs | **2.48× faster** |
| toString, 20 digits | 592.2 ns | 587.5 ns | no measurable difference |
| toString, 1000 digits | 3.20 µs | 4.84 µs | **1.51× SLOWER** |
| add, precision 20 | 1.06 µs | 1.03 µs | no measurable difference |
| multiply, precision 20 | 1.31 µs | 1.26 µs | no measurable difference |
| divide, precision 20 | 2.19 µs | 1.55 µs | **1.41× faster** |
| add, precision 200 | 535.5 ns | 1.18 µs | **2.20× SLOWER** |
| multiply, precision 200 | 12.62 µs | 2.27 µs | **5.56× faster** |
| divide, precision 200 | 20.41 µs | 7.40 µs | **2.76× faster** |
| sqrt, precision 20 | 6.74 µs | 5.00 µs | **1.35× faster** |
| sqrt, precision 200 | 105.67 µs | 41.22 µs | **2.56× faster** |
| pow (integer exponent) | 8.02 µs | 2.79 µs | **2.88× faster** |
| ln, precision 20 | 43.79 µs | 20.66 µs | **2.12× faster** |
| exp, precision 20 | 82.05 µs | 32.63 µs | **2.51× faster** |

## Where the crossover is

The most useful rows in the report. One multiplication, operands of the stated
size, everything else held constant.

| Operand size | decimal.js | decimal-rs | Verdict |
|---|---:|---:|---|
| 10 digits | 393.5 ns | 951.6 ns | **2.42× SLOWER** |
| 30 digits | 651.6 ns | 858.1 ns | **1.32× SLOWER** |
| 50 digits | 1.23 µs | 951.6 ns | **1.29× faster** |
| 60 digits | 1.57 µs | 996.8 ns | **1.58× faster** |
| 100 digits | 3.42 µs | 1.17 µs | **2.92× faster** |
| 1 000 digits | 261.71 µs | 31.15 µs | **8.40× faster** |
| 10 000 digits | 26.58 ms | 2.91 ms | **9.12× faster** |

**The port's column is flat from 10 to 60 digits** — 952, 858, 952, 997 ns — and
then starts to climb. That flatness is the finding. Across that whole range the
port is not doing arithmetic in any quantity that shows up; it is paying a fixed
cost of roughly 850 ns to cross the Node-API boundary, and the multiplication
underneath is lost in it. decimal.js has no boundary to cross and its column
starts climbing immediately.

So the crossover, **between 30 and 50 digits**, is not where the limb arithmetic
becomes better. It is where the operand finally becomes large enough for the
arithmetic to matter at all.

## Reading the other losses

**`toString, 1000 digits` — 1.51× slower.** decimal.js builds its output by
JavaScript string concatenation, and V8 implements that with ropes: each append
is O(1) and the flattening happens once, in C++, at the end. The port pushes
digits into a `String`. At twenty digits this is invisible; at a thousand it is
half the time. A genuine loss to a better data structure, and it would take a
rope or a preallocated digit buffer to close.

**`add, precision 200` — 2.20× slower.** The oddity in the table: decimal.js
adds *faster* at precision 200 (536 ns) than at precision 20 (1.06 µs). The two
scenarios use different operands, and the precision-200 pair happens to share an
exponent — so the original's alignment loop does nothing and the call reduces to
a limb-wise add of two short arrays. Against something that cheap, the port's
per-call setup is the whole measurement. The boundary again, in the row where
the operation underneath is closest to free.

**Peak RSS — 3.8× worse.** 166 MiB against 43 MiB over 200 000 mixed operations
at precision 200. Every Decimal the port returns is a JavaScript object with a
Rust allocation hanging off it, and the two are freed on different schedules:
the Rust side goes when V8 finalises the wrapper, and V8 finalises when *its*
heap is under pressure, not when the native heap is. Native memory therefore
accumulates behind a JavaScript heap that looks comfortable. This is a cost of
the FFI design rather than a tuning oversight, and it is the strongest argument
in this report for using `decimal-core` directly from Rust.

## Per-call latency

One small `plus` at a time, 100 000 samples.

| | p50 | p90 | p99 | max |
|---|---:|---:|---:|---:|
| the clock itself | 0 ns | 100 ns | 100 ns | 58.6 µs |
| decimal.js | 100 ns | 200 ns | 200 ns | 199.4 µs |
| decimal-rs | 800 ns | 900 ns | 1.20 µs | 4.97 ms |

**The instrument is in the table because it has to be.** Windows' clock ticks at
100 ns, and decimal.js's p50 *is* one tick — so that row does not say "decimal.js
takes 100 ns", it says "decimal.js is at or below what this timer can resolve".
The port's 800 ns is eight ticks and is measured properly. Read it as: the port
is slower per call by about the cost of one boundary crossing, and the precise
multiple is not knowable from this instrument.

`max` on both sides is garbage collection, not arithmetic.

## Startup

Nine fresh processes each, `require()` plus one construction, timed externally.

| | median | IQR |
|---|---:|---:|
| decimal.js | 5.00 ms | 207 µs |
| decimal-rs | 3.70 ms | 337 µs |

**1.30 ms faster.** Loading a 4 952-line JavaScript file costs more than loading
a 494 KiB DLL: V8 must parse and compile the source, while the addon is mapped
and its relocations resolved. Both figures include process creation, which is
common to both — the difference is the number to read, not the totals.

## Artifact size

| | |
|---|---:|
| `decimal.js` (source) | 128.7 KiB |
| `decimal.node` (compiled, release + LTO) | 494.5 KiB |
| | **3.84× larger** |

Expected, and not really improvable: the binary carries its own parsing,
formatting and transcendental code where the original leans on V8's.

## The original test suite

Not a benchmark of the arithmetic so much as of everything at once — 22,658
assertions, each one a call across the boundary.

| | |
|---|---:|
| decimal.js | 0.595 s |
| decimal-rs | 0.236 s |
| | **2.5× faster** |

## Reproducing

```
cargo build --release
cp target/release/decimal.dll ./decimal.node    # .so on Linux, .dylib on macOS
node bench/run.js
```

The corpus is generated from fixed seeds inside `run.js`, so the operands are
identical on any machine. The absolute times will not be, and the rows marked
`no measurable difference` may resolve either way between runs — that is what
the marking means. The shape of the table, and the crossover, should hold.
