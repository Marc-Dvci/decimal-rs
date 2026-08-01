# Defects found in the original

Seven defects in [decimal.js](https://github.com/MikeMcl/decimal.js) v10.6.0
(`cd73a7f`), plus two findings that are cost rather than correctness.

Five of the seven are crashes or hangs. Three of those five leave the library in
a state it cannot recover from: after them, either every subsequent operation
takes minutes, or the documented `minE`/`maxE` limits silently stop applying to
anything.

Every one is reproducible in two to five lines with no unusual operand.

| | Defect | Failure | Reachable in | Found by |
|---|---|---|---|---|
| [BUG-001](BUG-001-tan-near-poles.md) | `tan` loses every significant digit near its poles | silently wrong, then `Infinity` for finite input | 2 lines | ulp-scored comparison against mpmath |
| [BUG-002](BUG-002-configuration-leak.md) | `acosh`/`asinh`/`atanh` leak `precision`, `rounding` and `external` when they throw | library permanently unusable | 2 lines | differential campaign |
| [BUG-003](BUG-003-topower-null-dereference.md) | `toPower` dereferences null when the clamp made the base infinite | `TypeError` | 3 lines | differential campaign |
| [BUG-004](BUG-004-tofraction-round-floor.md) | `toFraction` never returns under `ROUND_FLOOR` | infinite loop, **every finite value** | 3 lines | differential campaign |
| [BUG-005](BUG-005-taylorseries-null-dereference.md) | `taylorSeries` dereferences null, and leaves the exponent clamps off | `TypeError` + silent loss of `minE`/`maxE` | 4 lines | differential campaign |
| [BUG-006](BUG-006-argument-reduction-null-dereference.md) | the argument reduction of `sin`/`cos`/`tan` dereferences null | `TypeError` | 4 lines | differential campaign |
| [BUG-007](BUG-007-precision-above-939524081.md) | `precision` is documented to 1e9 and division fails above 939,524,081 | host `RangeError`, not `[DecimalError]` | 2 lines | reproducing the host's ceiling in Rust |
| [notes](NOTES-cbrt-and-hyperbolic-cost.md) | `cbrt` does not return near the exponent floor | non-termination | 3 lines | differential campaign |
| [notes](NOTES-cbrt-and-hyperbolic-cost.md) | the hyperbolic argument fold ignores magnitude | 1.1 s for `cosh(1e6)` | 2 lines | benchmarking |

## Run them all

```
node fuzz/repro-upstream.js
```

Each case runs in its own process, on both implementations, with a timeout —
three of them do not return, and two more corrupt the state of anything measured
after them. Individual cases:

```
node fuzz/repro-case.js <case> <reference|port>
```

## Two families, not six unrelated defects

**Four of the six are the same mistake.** A value that the exponent clamps
turned into `Infinity` is then used as though it still had a digit array.
BUG-003 in `toPower`, BUG-005 in `taylorSeries`, BUG-006 in `toLessThanHalfPi`
and in `cosine`/`sine`. The clamp is deliberate and documented — a value is
measured against `minE`/`maxE` when it is *used*, not when it is built, so
`new Ctor(x)` is a re-judgement rather than a copy — and the consequence is that
almost any intermediate can become non-finite between one line and the next. The
call sites do not expect it.

A sweep for `.d.length` and `.d[` against that possibility would be worth more
than four separate patches.

**Two of the six are the missing `finally`.** BUG-002 and the second half of
BUG-005: a function raises `Ctor.precision`, sets `external = false`, computes,
and restores both afterwards — with nothing to run the restore if the
computation throws. The same raise/compute/lower shape appears in `sin`, `cos`,
`tan`, `sinh`, `cosh`, `tanh`, `exp`, `ln`, `log`, `pow`, `acosh`, `asinh` and
`atanh`. `getLn10` alone gets it right, and has a comment saying that is
deliberate.

BUG-004 belongs to neither family. It is a termination test standing in for a
different question, and a signed zero is enough to separate them.

BUG-007 belongs to neither either, and is the only one of the seven that is not
a mistake in the arithmetic: it is a range the documentation promises and the
implementation cannot reach, because the quotient array it would need is larger
than the engine will build. It is included because a caller who sets a
documented value and receives a host `RangeError` from an unrelated line has
been told nothing useful, and because the same line carries the 32-bit
truncation described at the end of that write-up.

## How they were found

BUG-001 came from scoring **error in ulps** against mpmath at 500 bits, sampling
trigonometric inputs *relative to the poles*. The port reproduces it exactly, so
no comparison between the two implementations could have seen it.

The other five came from [`fuzz/campaign.js`](../../fuzz/campaign.js), which runs
the differential harness in child processes and kills any slice that stops
making progress. An input the oracle cannot answer is recorded by seed and
re-run afterwards one implementation at a time — so a hang is *attributed*
rather than merely noticed, and "the oracle stopped answering and the port did
not" is a category the campaign reports on its own. That category is where all
five of these came from. See [`fuzz/log-limits.txt`](../../fuzz/log-limits.txt).

## Filing status

BUG-001 is written up for filing by the author of this port. The remaining six
are written here in filable form; each has a runnable reproduction and a
suggested patch, and none has been submitted upstream at the time of writing.
