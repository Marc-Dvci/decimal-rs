# Benchmark report

decimal-rs against the original decimal.js (v10.6.0, `cd73a7f`), measured by
[`run.js`](run.js) under the protocol in [`methodology.md`](methodology.md).
Raw data, including the interquartile range for every row, in
[`results.json`](results.json).

Run: `node bench/run.js` · Host: AMD Ryzen 5 7600 / Windows 11 / Node v24.13.1
/ rustc 1.97.1 `--release`, LTO fat, one codegen unit.

---

## The short version

> On CPU-bound arbitrary-precision arithmetic the Rust port is **1.2× to 8.6×
> faster**, and the advantage is almost entirely a function of operand size:
> 2.9× at 100 digits, 8.2× at 1 000, 8.6× at 10 000. Below about **40
> significant digits it is slower**, and at the library's default precision of
> twenty there is **no measurable difference** on most operations. Called one
> small operation at a time from JavaScript it is slower still — ~900 ns against
> ~100 ns — because crossing the Node-API boundary costs more than a twenty-digit
> addition. Startup is 0.9 ms faster. Resident memory is within 10% of the
> original's, but a *synchronous* burst of work peaks at 3.9× it, for a reason
> worth understanding. The compiled artifact is 4.7× larger than the original's
> source.
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
| parse, 20 digits | 1.10 µs | 710.9 ns | no measurable difference |
| parse, 200 digits | 1.63 µs | 1.32 µs | **1.23× faster** |
| parse, 1000 digits | 7.65 µs | 3.31 µs | **2.31× faster** |
| toString, 20 digits | 459.4 ns | 645.3 ns | no measurable difference |
| toString, 1000 digits | 2.47 µs | 4.73 µs | **1.92× SLOWER** |
| add, precision 20 | 795.2 ns | 1.14 µs | no measurable difference |
| multiply, precision 20 | 885.7 ns | 960.3 ns | no measurable difference |
| divide, precision 20 | 2.13 µs | 1.63 µs | **1.31× faster** |
| add, precision 200 | 541.9 ns | 1.14 µs | **2.11× SLOWER** |
| multiply, precision 200 | 11.99 µs | 2.62 µs | **4.57× faster** |
| divide, precision 200 | 19.61 µs | 7.06 µs | **2.78× faster** |
| sqrt, precision 20 | 5.75 µs | 5.22 µs | no measurable difference |
| sqrt, precision 200 | 105.67 µs | 44.54 µs | **2.37× faster** |
| pow (integer exponent) | 6.27 µs | 2.41 µs | **2.60× faster** |
| ln, precision 20 | 35.52 µs | 22.86 µs | **1.55× faster** |
| exp, precision 20 | 80.14 µs | 37.79 µs | **2.12× faster** |

## Where the crossover is

The most useful rows in the report. One multiplication, operands of the stated
size, everything else held constant.

| Operand size | decimal.js | decimal-rs | Verdict |
|---|---:|---:|---|
| 10 digits | 406.5 ns | 1.06 µs | **2.61× SLOWER** |
| 30 digits | 654.8 ns | 925.8 ns | **1.41× SLOWER** |
| 50 digits | 1.22 µs | 961.3 ns | **1.27× faster** |
| 60 digits | 1.56 µs | 1.03 µs | **1.52× faster** |
| 100 digits | 3.37 µs | 1.17 µs | **2.88× faster** |
| 1 000 digits | 263.51 µs | 32.07 µs | **8.22× faster** |
| 10 000 digits | 25.67 ms | 2.98 ms | **8.60× faster** |

**The port's column is nearly flat from 10 to 60 digits** — 1.06 µs, 926 ns,
961 ns, 1.03 µs — and only then starts to climb. That flatness is the finding.
Across that whole range the port is not doing enough arithmetic for it to show;
it is paying a fixed cost of roughly 900 ns to cross the Node-API boundary, and
the multiplication underneath is lost in it. decimal.js has no boundary to cross
and its column climbs from the first row.

So the crossover, **between 30 and 50 digits**, is not where the limb arithmetic
becomes better. It is where the operand finally becomes large enough for the
arithmetic to matter at all.

## Reading the other losses

**`toString, 1000 digits` — 1.92× slower.** decimal.js builds its output by
JavaScript string concatenation, and V8 implements that with ropes: each append
is O(1) and the flattening happens once, in C++, at the end. The port pushes
digits into a `String`. At twenty digits this is invisible; at a thousand it is
most of the time. A genuine loss to a better data structure, and closing it
would take a rope or a preallocated digit buffer.

**`add, precision 200` — 2.11× slower.** The oddity in the table: decimal.js
adds *faster* at precision 200 (542 ns) than at precision 20 (795 ns). The two
scenarios use different operands, and the precision-200 pair happens to share an
exponent — so the original's alignment loop does nothing and the call reduces to
a limb-wise add of two short arrays. Against something that cheap, the port's
per-call setup is the whole measurement. The boundary again, in the row where
the operation underneath is closest to free.

## Per-call latency

One small `plus` at a time, 100 000 samples.

| | p50 | p90 | p99 | max |
|---|---:|---:|---:|---:|
| the clock itself | 0.0 ns | 100.0 ns | 100.0 ns | 142.40 µs |
| decimal.js | 100.0 ns | 200.0 ns | 300.0 ns | 312.50 µs |
| decimal-rs | 900.0 ns | 1.40 µs | 1.90 µs | 6.78 ms |

**The instrument is in the table because it has to be.** Windows' clock ticks at
100 ns, and decimal.js's whole distribution spans three of those ticks — so that
row does not say "decimal.js takes 200 ns", it says "decimal.js is near the
resolution of this timer". The port's 900 ns is nine ticks and is measured
properly. Read it as: the port is slower per call by about the cost of one
boundary crossing, and the precise multiple is not knowable from this
instrument.

`max` on both sides is garbage collection, not arithmetic.

## Memory

200 000 chained add/multiply steps at precision 200, in a fresh process each time.

| | synchronous burst | yielding |
|---|---:|---:|
| decimal.js | 43.0 MiB | 42.8 MiB |
| decimal-rs | 165.3 MiB | 46.9 MiB |

Two numbers because for a native addon they measure genuinely different things,
and either alone would mislead.

Node runs Node-API finalizers **from the event loop**, not from the allocation
that triggered collection. So a tight synchronous loop defers every finalizer to
the end of it, and the addon's native allocations pile up behind a JavaScript
heap that looks perfectly comfortable — 165 MiB, against the original's 43. Give
the loop a turn and the same work settles at 47 MiB, ten per cent above the
original. The right-hand column is the resident footprint; the gap between the
columns is the price of the deferral, and it is a real cost for anyone who does
run a tight loop.

The ten-minute [soak](../scripts/soak.js) confirms the second column: across a
million rounds of mixed operations, RSS held flat around 60 MiB with a slope of
−238 MiB/h — no upward trend at all, against the original's +281 MiB/h on the
same workload.

The first version of the soak did *not* yield, reported the port growing to
2.2 GiB in sixty seconds, and was right to be alarming: half of that was a real
defect, an addon asking `napi_wrap` for a reference it never released, which
kept every Decimal alive for ever. The other half was this artefact. The
distinguishing test is whether growth survives a turn of the loop.

## Startup

Nine fresh processes each, `require()` plus one construction. Timing starts
inside each child immediately before `require()`, so process creation is excluded.

| | median | IQR |
|---|---:|---:|
| decimal.js | 4.65 ms | 46.00 µs |
| decimal-rs | 3.74 ms | 218.50 µs |

**0.91 ms faster.** Loading a 4 952-line JavaScript file costs more than loading
a 609 KiB DLL: V8 must parse and compile the source, while the addon is mapped
and its relocations resolved. These figures exclude child-process creation.

## Artifact size

| | |
|---|---:|
| `decimal.js` (source) | 128.7 KiB |
| `decimal.node` (compiled, release + LTO) | 609.0 KiB |
| | **4.73× larger** |

Expected, and not really improvable: the binary carries its own parsing,
formatting and transcendental code where the original leans on V8's.

## The original test suite

Not a benchmark of the arithmetic so much as of everything at once — 22,658
assertions, each one at least one call across the boundary.

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
