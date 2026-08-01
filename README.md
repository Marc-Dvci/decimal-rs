# decimal-rs — a Rust port of decimal.js

**Track F (JavaScript → Rust)** · Port Mortem 2026 · solo entry
Upstream: [MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js) @ [`cd73a7f`](https://github.com/MikeMcl/decimal.js/commit/cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f) (v10.6.0, MIT)

## Judge path — one page

1. **Standalone port, no JavaScript runtime required:** `decimal-core` has zero
   dependencies and zero unsafe; `cargo run --release -p decimal-cli -- 2 sqrt
   --precision 40` exercises it through the standalone `decimal-calc` consumer.
2. **One-command compatibility proof:** `docker build -t decimal-rs . && docker
   run --rm decimal-rs` builds from source, verifies all 69 upstream test files
   byte-for-byte, requires every one of roughly 22,650 assertions to pass, then
   runs the Node-API lifecycle/re-entry regression and unsafe report.
3. **Literal strict fuzz bonus:** [`fuzz/log-strict.txt`](fuzz/log-strict.txt)
   records 807,231 exact comparisons over 70 continuous seconds and 65 shared
   API entry points: **0 actual divergences, 0 known/waived divergences, 0 port
   defects**, with all nine rounding modes and a fixed replay seed.
4. **Hostile-domain evidence:** [`fuzz/log.txt`](fuzz/log.txt) and
   [`fuzz/log-limits.txt`](fuzz/log-limits.txt) retain extreme exponents, known
   upstream failures, watchdog attribution, and explicit unrefereeable accounting.
5. **The boundary is under test, not merely asserted:**
   [`scripts/adapter-regression.js`](scripts/adapter-regression.js) proves calls
   without `new`, shared clone prototypes, re-entry safety, rejected clone
   handling, and collection of discarded constructors.
6. **Balanced benchmark:** the port wins by 2.9× at 100 digits and 8.6× at
   10,000, but loses on tiny per-call latency; protocol and raw results are in
   [`bench/`](bench/README.md).
7. **Bug bounty status:** eight upstream bugs were found and documented in
   [`docs/upstream/`](docs/upstream/README.md). GitHub issue creation is
   restricted in the upstream repository, which is why they were not filed.

[![verify](https://github.com/Marc-Dvci/decimal-rs/actions/workflows/verify.yml/badge.svg)](https://github.com/Marc-Dvci/decimal-rs/actions/workflows/verify.yml)
— the documented command, on a machine that has never seen this code, on every
push. [`.github/workflows/verify.yml`](.github/workflows/verify.yml) runs
`docker build && docker run` with nothing installed on the runner, and
separately runs Rust tests and lints, strict upstream and adapter gates, both
conformance checks, six differential defect reproducers plus two cost findings,
the unsafe report, and a 70-second strict differential campaign.

| | |
|---|---|
| **Original test suite** | **all assertions pass** — about 22,650 per run; the random denominator is parsed and strictly gated |
| **Test files modified** | **0 of 69** — SHA-256 manifest enforced by the build, and the fuzzing oracle is pinned the same way |
| **Strict differential fuzzing** | **0 actual · 0 known/waived · 0 port defects** — 807,231 operations, 70 seconds, 65 shared API entries |
| **Unsafe in `decimal-core`** | **0**, compiler-enforced (`unsafe_code = "forbid"`) |
| **Dependencies of `decimal-core`** | **0** |
| **JavaScript in the port** | **none** — Node's own resolver loads the Rust binary |
| **Defects found in the original** | **8**, four of them crashes, two of them hangs |
| **Decisions documented** | architectural and fidelity decisions are recorded with consequences in [`DECISIONS.md`](DECISIONS.md) |

## The demo film

**[film/output/decimal-rs-port-mortem-2026.mp4](film/output/decimal-rs-port-mortem-2026.mp4)**
— 4 minutes 1 second, 1080p, in the repository rather than behind a link.

**No terminal in it was typed.** Every command shown was run by
[`film/scripts/capture.ts`](film/scripts/capture.ts), which records each line of
output with the millisecond it arrived; the scenes read that recording and
cannot supply text of their own — a scene quoting a line the command no longer
prints fails the render rather than showing something plausible. Where a
recording is replayed faster than it ran, the factor and the real elapsed time
are both on screen; where a command finished in 300 ms, the film scrolls it at a
readable pace and says so.

The recording it was built from is committed at
[`film/artifacts/capture.json`](film/artifacts/capture.json) — the same suite
run, the same campaign, the same conformance output the sections below describe,
with the host, the commit and the exit code of each command beside it.

## Build and verify — one command

```
docker build -t decimal-rs . && docker run --rm decimal-rs
```

That builds the Rust port from source, verifies that all 69 files of the
original test suite are byte-identical to upstream, runs that **unmodified**
suite against the compiled Rust artifact, and runs the adapter lifecycle and
re-entry regression. The strict wrapper fails if even one assertion fails.

Without Docker: `make` (requires Rust 1.97.1 — pinned in `rust-toolchain.toml` —
and Node 24).

The image carries the vendored oracle and the standalone binary too, so the rest
of the evidence is reproducible in it without a local toolchain:

```
docker run --rm decimal-rs node scripts/clamp-conformance.js
docker run --rm decimal-rs node scripts/host-limits.js
docker run --rm decimal-rs ./bin/decimal-calc 2 sqrt --precision 40
```

**Read the count, not a hard-coded denominator.** About 6,000 assertions are
generated with `Math.random()`, so the total moves from run to run. The required
relation does not: passed must equal asserted.

That is also why the suite is run through
[`scripts/expect-zero-failures.js`](scripts/expect-zero-failures.js) rather than
directly. Upstream's runner **exits 0 whatever happens** — it prints its
failures and returns success — so a container or a CI job that gated on its exit
code would gate on nothing, and would stay green through a five-thousand-failure
regression. The wrapper parses the summary and fails on any difference.

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

On CPU-bound arbitrary-precision arithmetic the port is **1.2× to 8.6× faster**,
and the advantage is almost entirely a function of operand size: 2.9× at 100
digits, 8.2× at 1 000, 8.6× at 10 000. Below about **40 significant digits it is
slower**, and at the library's default precision of twenty there is **no
measurable difference** on most operations — the port's per-multiply cost barely
moves from 30 digits to 100, 926 ns to 1.17 µs, because across that range it is
paying to cross the Node-API boundary and the arithmetic underneath is lost in
it. Called one small operation at a time it is slower still, ~900 ns against
~100 ns. Startup is 0.9 ms faster; the compiled artifact is 4.7× larger than the
original's source. Full table, methodology and the losses in
[`bench/`](bench/README.md) — every number in this paragraph is a row of
[`bench/results.json`](bench/results.json).

## Behavioural equivalence

Five instruments, answering different questions.

**The original suite** — some 22,650 assertions, unmodified, hash-pinned. It is
the strongest single piece of evidence and it is also fixed: it can only check
what its author thought of, from one starting configuration.

**The strict differential artifact** — a predefined shared-API domain with
moderate generated exponents, default wide `minE`/`maxE`, all nine rounding
modes, all constructor representations, stateful configuration and cloning,
and 65 API entry points. `toFraction` is excluded because upstream BUG-004
loops for every finite input under `ROUND_FLOOR`; `random` has no shared answer
because the implementations intentionally use different generators. Nothing is
compared and then waived. The published fixed-seed run is literal:

```
node fuzz/differential.js --strict --seconds 70 --seed 1592594996 --log fuzz/log-strict.txt
```

Result: 807,231 operations, **0 actual divergences, 0 known/waived divergences,
0 port defects**. The comparator first proves itself by detecting an injected
one-ulp fault. Full artifact: [`fuzz/log-strict.txt`](fuzz/log-strict.txt).

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
node fuzz/campaign.js --seconds 70 --log fuzz/log.txt
```

An omitted `--log` writes only to stdout, so an exploratory run cannot
overwrite published evidence. Log: [`fuzz/log.txt`](fuzz/log.txt). The unbounded pass, which fuzzes the entire
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

So instead every such sequence is counted and recorded. After the clock has
stopped, up to the explicit `--diagnose` cap are named and re-run one
implementation at a time; larger full-range runs print the remaining count
rather than extending wall-clock time without limit. A diagnosed record turns
"something did not finish" into a verdict:

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
node scripts/clamp-conformance.js     # 3,528 calls across four axes
node scripts/host-limits.js           # the ceilings the original's host imposes
```

[`scripts/clamp-conformance.js`](scripts/clamp-conformance.js) builds each
operand under wide exponent limits and then **narrows the limits before the
call**. That is the only arrangement in which the original's pervasive
`x = new Ctor(x)` — a re-judgement against the current limits, not a copy — is
observable at all, and it is why some 22,650 assertions have nothing to say about the whole
family: the suite never narrows the limits after building an operand. This was
the largest single family of defect in the port ([D-18](DECISIONS.md)). The
check attempts 3,528 cases: zero unexpected mismatches, 18 named intentional
exceptions, and five methods with a case on which neither implementation
returns; those timeouts are reported rather than counted as agreement.

It varies four axes — 67 methods × 6 operands × 4 exponent-limit pairs, the ten
methods that take a rounding mode across all nine of them, and each binary
method with the extreme operand in the *argument* position as well as the
receiver. Two of those axes were added after the campaign found a defect the
check should have caught and did not, and the second of them found another
within a minute of being written: [D-21](DECISIONS.md) is the entry on what an
instrument is silent about.

[`scripts/host-limits.js`](scripts/host-limits.js) covers the opposite hazard —
where the original is stopped by *its host* and Rust would not be. It measures
the array ceiling V8 enforces on this machine, checks it against the constant
compiled into the port, and compares seven cases on both implementations
including the type of the error thrown — two of them the precisions immediately
either side of the threshold at which division stops working, which the two
implementations turn over between *to the digit*. One case is required **not**
to throw, because a ceiling set far too low would otherwise pass the whole file.

It then sweeps fifteen routines above that threshold, where the requirement is
weaker and more important: that the port **stops**. Abandoning a calculation
without an exception to unwind means the caller keeps running on a placeholder,
and getting that wrong does not produce a wrong answer — it produces a dead
process, or a live one that never returns. Three of these hung and nine aborted
before the protocol in [D-19](DECISIONS.md) was written down.

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

Run the six timeout-safe campaign defects (BUG-002 through BUG-006 and BUG-008),
plus two cost findings, on both implementations:

```
node fuzz/repro-upstream.js
```

BUG-007 has its dedicated `node scripts/host-limits.js` probe; BUG-001 requires
the high-precision mpmath analysis in its write-up. Five of the eight are one
mistake wearing different hats, and two are one
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

Every one of the six is *documented*, not quietly corrected: each has a write-up
in [`docs/upstream/`](docs/upstream/README.md), and the differential harness
knows each by name so that a hostile run counts them rather than hiding them.
None of the six is reachable from the original suite, which is why it passes in
full.

All eight upstream bugs were found during the competition, but GitHub issue
creation is restricted in the upstream repository. That restriction is why no
issues were filed; the reports are preserved in filing-ready form.

## Safety

```
node scripts/unsafe-report.js
```

| crate | lines | unsafe | |
|---|---:|---:|---|
| `decimal-core` | 9,931 | **0** | `unsafe_code = "forbid"`, compiler-enforced |
| `decimal-cli` | 296 | **0** | same |
| `decimal-napi` | 2,853 | 92 | the Node-API boundary; no arithmetic |

`forbid` is not a lint level an inner `allow` can turn off, so `decimal-core`
does not compile if an unsafe block appears anywhere in it, including one
produced by a macro. The declaration is the evidence and a successful build is
the check; the counts are a textual cross-check taken after comments and string
literals are stripped.

`decimal-core` has **no dependencies at all** — the limb arithmetic is written
here rather than taken from a bignum crate, for reasons in
[D-02](DECISIONS.md).

The adapter's ownership proof is short enough to audit: each constructor owns
one native `ConstructorData` box through `napi_wrap`; its finalizer deletes the
constructor's weak self-reference and drops the box. Decimal instances own their
payloads the same way. Native references never escape the closure-scoped unwrap
helpers, and no configuration borrow spans a property read, conversion,
construction, or function call—every operation that can invoke JavaScript.
[D-23](DECISIONS.md) records the failure mode, design, and executable tests.

A panic cannot take the host down either. Every callback is a plain Rust
function wrapped in one `extern "C"` shim that catches unwinds and throws a
JavaScript `Error` instead — deliberately *not* wearing the library's
`[DecimalError]` prefix, so a `catch` written for the library's own errors
cannot swallow a bug in the port. Verified by injecting a panic and watching it
arrive as a catchable error with the process still running; the first version of
the guard failed that test, and [D-22](DECISIONS.md) says why.

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

Constructor churn is tested separately because the arithmetic soak does not
create clones. The adapter regression discards 1,000 cloned constructors,
forces collection across event-loop turns, and requires at least half to reach a
`FinalizationRegistry`; the previous strong-reference cycle finalized none.

## Layout

```
crates/decimal-core   the port: parsing, limb arithmetic, rounding, formatting,
                      transcendentals. Zero dependencies, zero unsafe, no Node.
crates/decimal-napi   the only place that knows Node exists: object protocol,
                      config state, error mapping. All unsafe lives here.
crates/decimal-cli    decimal-calc, a standalone evaluator — the port computes
                      with no Node present, through the same two functions the
                      addon uses to parse and to render.
test/                 the original suite, unmodified and hash-pinned — at
                      upstream's own paths, for the reason in tests/README.md.
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
- [tests/README.md](tests/README.md) — where the original suite is, why it is at
  upstream's paths rather than `tests/original/`, and how the pinning is checked
- [bench/README.md](bench/README.md) — the benchmark report, losses included
- [bench/methodology.md](bench/methodology.md) — how those numbers were produced
- [docs/upstream/README.md](docs/upstream/README.md) — the eight defects
- [fuzz/log.txt](fuzz/log.txt) — the published campaign log
- [fuzz/log-strict.txt](fuzz/log-strict.txt) — the literal 70-second strict-parity artifact

## Licence

MIT, as upstream. `LICENCE.md` is retained verbatim from the original.
