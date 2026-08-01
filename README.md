# decimal-rs — a Rust port of decimal.js

**Track F (JavaScript → Rust)** · Port Mortem 2026 · solo entry
Upstream: [MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js) @ [`cd73a7f`](https://github.com/MikeMcl/decimal.js/commit/cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f) (v10.6.0, MIT)

| | |
|---|---|
| **Original test suite** | **exactly one failure**, documented ([D-08](DECISIONS.md)) — of about 22,650 assertions |
| **Test files modified** | **0 of 69** — SHA-256 manifest enforced by the build, and the fuzzing oracle is pinned the same way |
| **Differential fuzzing** | **zero undocumented divergences** over 70 continuous seconds, four independent seeds |
| **Unsafe in `decimal-core`** | **0**, compiler-enforced (`unsafe_code = "forbid"`) |
| **Dependencies of `decimal-core`** | **0** |
| **JavaScript in the port** | **none** — Node's own resolver loads the Rust binary |
| **Defects found in the original** | **8**, four of them crashes, two of them hangs |

## Build and verify — one command

```
docker build -t decimal-rs . && docker run --rm decimal-rs
```

That builds the Rust port from source, verifies that all 69 files of the
original test suite are byte-identical to upstream, and runs that **unmodified**
suite against the compiled Rust artifact.

Without Docker: `make` (requires Rust 1.97.1 — pinned in `rust-toolchain.toml` —
and Node 24).

**Read the failure count, not the ratio.** About 6,000 of the suite's assertions
are generated with `Math.random()`, so the denominator moves from run to run —
22,628 and 22,688 in two consecutive runs here. The number that does not move is
the failures: **one**, always the same one, explained in [D-08](DECISIONS.md).

## How the original tests reach the Rust code

`test/setup.js`, untouched, contains one line:

```js
Decimal = require('../decimal');
```

Node's resolver tries `../decimal.js`, then `../decimal.json`, then
`../decimal.node`. The original `decimal.js` is not present — it is the thing
being replaced — and `decimal.node` is the compiled Rust library.

So the module object the tests receive *is* the addon, and the addon's
`module.exports` *is* the `Decimal` constructor. There is no adapter, no shim,
and no JavaScript file anywhere between the test suite and the Rust code. This
required writing the N-API entry point by hand; the usual derive macros cannot
return a constructor function as the module itself.

**About the JavaScript that is in this repository.** There is none in the port
or in its build graph — `cargo build` produces `decimal.node` and touches no
`.js` file. There is JavaScript under `fuzz/`, `bench/` and `scripts/`, which are
the harnesses that *test and measure* the port, and one file that deserves to be
called out: `fuzz/reference/decimal.js` is a byte-identical copy of upstream,
vendored as the fuzzing oracle. It is not linked, called, or shipped by the
port; nothing in `crates/` can reach it. See
[`fuzz/reference/README.md`](fuzz/reference/README.md).

## Performance, in one paragraph

On CPU-bound arbitrary-precision arithmetic the port is **1.4× to 8.5× faster**,
and the advantage is almost entirely a function of operand size: 2.6× at 100
digits, 8.1× at 1 000, 8.5× at 10 000. Below about **40 significant digits it is
slower**, and at the library's default precision of twenty there is **no
measurable difference** on most operations — the port's per-multiply cost barely
moves from 30 digits to 100, 871 ns to 1.33 µs, because across that range it is
paying to cross the Node-API boundary and the arithmetic underneath is lost in
it. Called one small operation at a time it is slower still, ~900 ns against
~200 ns. Startup is 1.5 ms faster; the compiled artifact is 3.8× larger than the
original's source. Full table, methodology and the losses in
[`bench/`](bench/README.md) — every number in this paragraph is a row of
[`bench/results.json`](bench/results.json).

## Behavioural equivalence

Four instruments, answering different questions.

**The original suite** — some 22,650 assertions, unmodified, hash-pinned. It is
the strongest single piece of evidence and it is also fixed: it can only check
what its author thought of, from one starting configuration.

**A differential campaign** — [`fuzz/campaign.js`](fuzz/campaign.js) — which
checks the other thing: that the two implementations agree on inputs nobody
chose, under configurations nobody wrote down, in *sequences* where each
operation inherits the state the last one left. Both run in one process on the
same values. Every result is compared on every observable channel — sign,
exponent, the digit array itself, all three string renderings, the finiteness
predicates, the precision metadata, negative zero, the exact thrown message —
plus the operands afterwards, to catch a mutation, plus the constructor
configuration, to catch a leak. There is no tolerance anywhere in it.

Each run begins by deliberately corrupting the port's results by one unit in the
last place and refusing to continue until the comparator has caught it. A log
saying "zero divergences" from a harness with no demonstrated ability to see one
proves nothing.

```
node fuzz/campaign.js --seconds 70
```

Log: [`fuzz/log.txt`](fuzz/log.txt). The unbounded pass, which fuzzes the entire
legal input space including `1e9000000000000000`, is
[`fuzz/log-limits.txt`](fuzz/log-limits.txt).

### The watchdog, and what it is for

The campaign runs slices as child processes and watches them make progress. A
slice whose sequence index stops advancing is killed, its input recorded **by
seed**, and the slice resumed at the next sequence.

This exists because the oracle cannot always answer. The corpus deliberately
includes values at the exponent limits, and for a handful of operations upstream
at those magnitudes exhausts the heap, or runs for hours, or does not return at
all. An oracle that cannot answer cannot referee. The first response was to bound
the offending families one at a time, and another family kept appearing — a
losing game that also produces a weak artifact, because a bound names a *family*
and the family is far larger than the set of inputs that actually defeat it.

So instead each such input is named individually and then diagnosed, after the
clock has stopped, by re-running it one implementation at a time. That turns a
line of the log from "something hung" into a verdict:

| | meaning |
|---|---|
| **upstream defect** | the port answered and the oracle did not |
| **intractable** | neither returned — they agree, no answer is available |
| **inconclusive** | neither reproduced the stall in isolation |
| **PORT DEFECT** | the oracle answered and the port did not — **this must be zero** |

That last row is a claim worth stating rather than leaving to be inferred from
an absence, and the campaign prints it whether or not it is zero.

**Every upstream defect below came out of that mechanism.**

### Two targeted conformance checks

Each was written because a campaign found one member of a family and the family
turned out to be far larger than the member. A campaign reaches such a case by
luck, once, after minutes; these reach every case in seconds and name the
method rather than the seed.

```
node scripts/clamp-conformance.js     # 43 methods × 6 operands × 4 limit pairs
node scripts/host-limits.js           # the ceilings the original's host imposes
```

[`scripts/clamp-conformance.js`](scripts/clamp-conformance.js) builds each
operand under wide exponent limits and then **narrows the limits before the
call**. That is the only arrangement in which the original's pervasive
`x = new Ctor(x)` — thirty places, a re-judgement and not a copy — is observable
at all, and it is why some 22,650 assertions have nothing to say about the whole
family: the suite never narrows the limits after building an operand. This was
the largest single family of defect in the port ([D-18](DECISIONS.md)). All
1,032 calls agree; the documented divergences are counted, not hidden.

[`scripts/host-limits.js`](scripts/host-limits.js) covers the opposite hazard —
where the original is stopped by *its host* and Rust would not be. It measures
the array ceiling V8 enforces on this machine, checks it against the constant
compiled into the port, and compares five cases on both implementations
including the type of the error thrown. One of the five is required **not** to
throw, because a ceiling set far too low would otherwise pass the whole file.

[D-19](DECISIONS.md) is the entry to read if you read one: the ceiling had been
set to the specification's 2³² − 1 rather than the 2²⁷ V8 actually enforces, and
the consequence was a divergence three lines long at the library's largest
documented precision. It was live until the unbounded campaign's `PORT DEFECT`
row stopped being zero.

## Eight defects in the original

Six of the eight are crashes or hangs. Three of those leave the library in a
state it cannot recover from: afterwards, either every subsequent operation
takes minutes, or the documented `minE`/`maxE` limits silently stop applying to
anything at all. Each is reproducible in two to five lines with no unusual
operand.

| | Defect | Failure |
|---|---|---|
| [BUG-001](docs/upstream/BUG-001-tan-near-poles.md) | `tan` loses every significant digit near its poles | silently wrong, then `Infinity` for finite input |
| [BUG-002](docs/upstream/BUG-002-configuration-leak.md) | `acosh`/`asinh`/`atanh` leak `precision`, `rounding` and `external` when they throw | library permanently unusable |
| [BUG-003](docs/upstream/BUG-003-topower-null-dereference.md) | `toPower` dereferences null when the clamp made the base infinite | `TypeError` |
| [BUG-004](docs/upstream/BUG-004-tofraction-round-floor.md) | `toFraction` never returns under `ROUND_FLOOR` | infinite loop, **every finite value** |
| [BUG-005](docs/upstream/BUG-005-taylorseries-null-dereference.md) | `taylorSeries` dereferences null, and leaves the exponent clamps off | `TypeError` + silent loss of `minE`/`maxE` |
| [BUG-006](docs/upstream/BUG-006-argument-reduction-null-dereference.md) | the argument reduction of `sin`/`cos`/`tan` dereferences null | `TypeError` |
| [BUG-007](docs/upstream/BUG-007-precision-above-939524081.md) | `precision` is documented to 1e9 and division fails above 939,524,081 | host `RangeError`, not `[DecimalError]` |
| [BUG-008](docs/upstream/BUG-008-atan-infinity-null-dereference.md) | `atan(±Infinity)` dereferences null above the π constant's precision | `TypeError` |

BUG-004 is three lines:

```js
Decimal.set({ rounding: Decimal.ROUND_FLOOR });
new Decimal(1).toFraction();     // never returns
```

Run all of them, on both implementations, each in its own process with a
timeout:

```
node fuzz/repro-upstream.js
```

Five of the eight are one mistake wearing different hats, and two are one
missing `finally`; both families, and the suggested sweeps, are in
[`docs/upstream/README.md`](docs/upstream/README.md). BUG-007 is in neither
family and is the only one that is not a mistake in the arithmetic — it is a
configuration range the documentation promises and the engine will not build the
array for. `node scripts/host-limits.js` reproduces it, from both sides of a
threshold the two implementations agree on to the digit.

## Fidelity, and the six places it is set aside

The rule is **fidelity over correctness**: where the original is wrong, this
port is wrong in the same way, because reproducing the original's answers is the
point. `tan` near its poles is transcribed defect and all, with a test asserting
the wrong answer so that a fix upstream would show up here as a decision rather
than a drift.

There are exactly six deliberate exceptions — [D-11](DECISIONS.md),
[D-13](DECISIONS.md), [D-14](DECISIONS.md), [D-16](DECISIONS.md),
[D-17](DECISIONS.md), [D-20](DECISIONS.md) — each with its own entry, and the
test for setting fidelity aside has been the same every time: **reproducing the
original would hand a caller a way to break the library rather than a way to
compute a number.** A `TypeError` from an unguarded null dereference is that. A
loop with no exit is that. A precision left at nine quadrillion is that.

Every one of the six is *reported*, not quietly corrected: each has a write-up
in [`docs/upstream/`](docs/upstream/README.md), and the differential harness
knows each by name so that a run counts them rather than hiding them. A seventh
divergence, [D-08](DECISIONS.md), is a constraint of the Node-API rather than a
choice, and is the single failing assertion.

## Safety

```
node scripts/unsafe-report.js
```

| crate | lines | unsafe | |
|---|---:|---:|---|
| `decimal-core` | 9,636 | **0** | `unsafe_code = "forbid"`, compiler-enforced |
| `decimal-cli` | 10 | **0** | same |
| `decimal-napi` | 2,531 | 90 | the Node-API boundary; no arithmetic |

`forbid` is not a lint level an inner `allow` can turn off, so `decimal-core`
does not compile if an unsafe block appears anywhere in it, including one
produced by a macro. The declaration is the evidence and a successful build is
the check; the counts are a textual cross-check taken after comments and string
literals are stripped.

`decimal-core` has **no dependencies at all** — the limb arithmetic is written
here rather than taken from a bignum crate, for reasons in
[D-02](DECISIONS.md).

## The port without Node

`decimal-core` is the deliverable and the addon is evidence, so there is a
second consumer of the crate that has never heard of Node:

```
$ decimal-calc 2 sqrt --precision 40
1.41421356237309504880168872420969807857
$ decimal-calc 355 div 113
3.1415929203539823009
$ decimal-calc 0x1.8p3 add 1
13
```

23 unary and 9 binary operations, with `--precision`, `--rounding`, `--min-e`
and `--max-e`. It parses through `parse::from_str` and renders through
`format::to_string` — the same two functions the addon calls — and contains no
arithmetic of its own, because anything it computed itself would be a behaviour
this port has and the original does not. Built by `make` alongside the addon,
and shipped in the Docker image at `bin/decimal-calc`.

## Memory

```
node scripts/soak.js
```

Ten minutes per implementation, RSS sampled every two seconds, verdict from the
least-squares slope and the drift between the first and last quarters.

The script yields to the event loop between batches, and this matters: Node runs
Node-API finalizers from the loop, so a fully synchronous soak measures deferral
rather than footprint. The first version did not yield and reported the port
growing to 2.2 GiB in sixty seconds. That turned out to be half a real defect —
the addon was asking `napi_wrap` for a reference it never released, which kept
every Decimal alive for ever — and half an artefact of the measurement. Both are
worth knowing and neither is visible from a test suite that runs in a quarter of
a second.

## Layout

```
crates/decimal-core   the port: parsing, limb arithmetic, rounding, formatting,
                      transcendentals. Zero dependencies, zero unsafe, no Node.
crates/decimal-napi   the only place that knows Node exists: object protocol,
                      config state, error mapping. All unsafe lives here.
crates/decimal-cli    decimal-calc, a standalone evaluator — the port computes
                      with no Node present, through the same two functions the
                      addon uses to parse and to render.
test/                 the original suite, unmodified and hash-pinned.
tests/                the two manifests: the suite's hashes, and the oracle's.
fuzz/                 the differential campaign, its oracle, and the upstream
                      reproductions.
bench/                the benchmark harness, its methodology, and its results.
scripts/              the two conformance checks, the soak, the unsafe report,
                      test-manifest verification.
docs/upstream/        one filable report per defect found in the original.
```

## Documents

- [DECISIONS.md](DECISIONS.md) — every architectural choice and every deliberate
  divergence, written when the decision was made, with its consequence
- [bench/README.md](bench/README.md) — the benchmark report, losses included
- [bench/methodology.md](bench/methodology.md) — how those numbers were produced
- [docs/upstream/README.md](docs/upstream/README.md) — the six defects
- [fuzz/log.txt](fuzz/log.txt) — the published campaign log

## Licence

MIT, as upstream. `LICENCE.md` is retained verbatim from the original.
