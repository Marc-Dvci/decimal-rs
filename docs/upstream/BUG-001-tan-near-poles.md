# BUG-001 — `tan()` loses all significant digits near its poles

**Project:** [MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js)
**Commit:** `cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f` (v10.6.0 + PR #260, current `master`)
**Location:** `decimal.js:1838` / `decimal.mjs:1839`, in `P.tangent = P.tan`
**Severity:** silently wrong results — no throw, no NaN, no warning
**Status:** reproduced, mechanism confirmed, fix identified and verified

---

## Summary

`tan(x)` computes

```js
x = x.sin();
x.s = 1;
x = divide(x, new Decimal(1).minus(x.times(x)).sqrt(), pr + 10, 0);
```

i.e. `tan = sin / sqrt(1 - sin²)`, with a fixed 10 guard digits.

As `x` approaches a pole `(2n+1)·π/2`, `sin(x) → ±1`, so `1 - sin²(x)` is a
subtraction of two nearly equal quantities. The cancellation eats roughly **two
significant digits per decade** of proximity to the pole. With only 10 guard
digits available, the result is completely wrong once `|x − pole|` drops below
about `10⁻⁵`, and eventually collapses to a fixed constant or to `Infinity`.

This is the same failure mode as the two cancellation bugs already fixed in this
library — [PR #217](https://github.com/MikeMcl/decimal.js/pull/217) (`acos` near
1) and [PR #260](https://github.com/MikeMcl/decimal.js/pull/260) (`asin` near 1)
— in a function neither PR touched. `acos` and `asin` were both repaired by
replacing a cancelling expression with an algebraically equivalent stable one;
`tan` still has the original pattern.

## Reproduction

```js
const Decimal = require('decimal.js');
Decimal.set({ precision: 14 });

new Decimal('1.5707963267948966192').tan().toString();
// decimal.js  ->  707106781186.55
// correct     ->  31926755792809398050.6...   (3.19e19)

new Decimal('1.5707963267948966192313216916397514').tan().toString();
// decimal.js  ->  Infinity
// correct     ->  2.3753767665435e+34
```

The first result is wrong by **eight orders of magnitude** with all 14
significant digits incorrect. The second returns `Infinity` for a finite input —
no finite `Decimal` has a tangent of infinity, so this value is unreachable in a
correct implementation and is not among the documented `Infinity` cases in the
function's own header comment.

## Measured error profile

`x = π/2 − 3.7182818284590452·10⁻ᵏ`, error in ulps of the requested precision
(a correctly rounded result has ≤ 0.5 ulp):

| k | precision 14 | precision 20 | precision 40 |
|---:|---:|---:|---:|
| 5 | 0.24 | 0.03 | 0.50 |
| 6 | **1.6** | **1.0** | **2.7** |
| 8 | 4.0e3 | 5.8e3 | 2.0e4 |
| 12 | 1.7e11 | 2.4e11 | 2.0e12 |
| 16 | 2.7e13 (saturated) | 9.8e19 | 9.8e19 |
| 20+ | 2.7e13 (saturated) | 2.7e19 (saturated) | 1.5e28 |

Two things to note:

1. **The onset does not move with `precision`.** The first result outside 0.5 ulp
   is at `k = 6` for precision 14, 20 and 40 alike, because the guard is a fixed
   `+10` digits rather than a function of how close the argument is to the pole.
   A user cannot buy their way out of this by raising `precision`.
2. **The result saturates.** Past a certain point `tan` returns a constant —
   `707106781186.55` at precision 14, `7.0710678118654752440084436210484903928483593768847e+29`
   at precision 50 — regardless of how much closer to the pole the input gets.
   The constant is `1/√2 × 10^(working precision / 2)`, the signature of
   `1 - sin²` bottoming out at the smallest representable value at working
   precision.

## Mechanism (measured, not inferred)

Instrumenting the actual steps of `P.tan` at precision 14 (working precision 24),
for `x = π/2 − 3.7182818284590452·10⁻ᵏ`:

| k | `1 - sin(x)²` computed | `1 - sin(x)²` true | surviving digits |
|---:|---|---|---:|
| 4 | `1.38256191186895467e-7` | `1.3825619118689546736103105e-7` | 18 |
| 8 | `1.382561976e-15` | `1.3825619755848734063399919e-15` | 10 |
| 12 | `1.4e-23` | `1.382561975584874043499191e-23` | **2** |
| 16 | `0` | `1.3825619755848740434991974e-31` | **0** |

At `k = 16` the denominator is **exactly zero**, so `sqrt(0) = 0` and the
division produces `Infinity`. That is the whole of the failure.

Meanwhile, in the same run, `cos(x)` is computed essentially perfectly:

| k | `cos(x)` computed | `cos(x)` true |
|---:|---|---|
| 12 | `3.71828182845904519999999e-12` | `3.7182818284590451999999914e-12` |
| 16 | `3.7182818284590452e-16` | `3.7182818284590452e-16` |

**The library already computes the quantity `tan` needs, accurately. `tan` just
doesn't use it.**

## Scope

The bug applies at **every** pole, not only `π/2`. Because it follows the
argument reduction, it reproduces identically at `(2n+1)·π/2` for all tested `n`
up to `10⁸` — the affected input set is unbounded, and at precision 14 each pole
carries a neighbourhood roughly `10⁻⁵` wide in which results are wrong.

Verified **not** affected (all within 0.5 ulp across targeted boundary sweeps):
`atanh` at |x|→1 and →0, `atan2` near the axes and at extreme ratios, `ln`,
`log2`, `log10` and `log(base)` approaching 1, `pow` with fractional exponents,
`asin`/`acos` (post-#260), `acosh` near 1, `sinh`/`cosh`/`tanh`, `sqrt`, `cbrt`.
`P.tanh` uses the stable `sinh/cosh` form and is fine; `tan` is the only
remaining site of the cancelling pattern.

## Suggested fix

Use the quotient form, which is well conditioned everywhere except at the pole
itself (where the true result genuinely diverges):

```js
  x = x.sin();
  x.s = 1;
- x = divide(x, new Ctor(1).minus(x.times(x)).sqrt(), pr + 10, 0);
+ x = divide(x, this.cos(), pr + 10, 0);
```

(as with #217/#260, the change is to replace a cancelling expression with an
equivalent stable one; the exact call shape needs to respect the existing
`quadrant` handling and the `external` flag, since `P.cos` performs its own
argument reduction.)

**Verified:** computing `x.sin().div(x.cos())` with the library's own primitives
at the same working precision gives ≤ 3·10⁻¹¹ ulp error across `k = 6 … 23` —
correct to far beyond the requested precision at every point where `tan()`
currently returns garbage.

| k | `tan()` error | `sin()/cos()` error |
|---:|---:|---:|
| 8 | 4.0e3 ulp | 6.4e-11 ulp |
| 12 | 1.7e11 ulp | 1.2e-12 ulp |
| 16 | 2.7e13 ulp | 2.5e-11 ulp |
| 23 | saturated | 2.5e-11 ulp |

## Severity relative to the accepted fixes

The harness was calibrated against the bug PR #260 fixed, by running it on the
genuine pre-fix tree (commit `1a6e845`) and on HEAD:

| | worst `asin` error near 1 |
|---|---:|
| `1a6e845` (before #260) | **137.6 ulp** — 2.4 of 14 digits lost |
| `cd73a7f` (HEAD, #260 merged) | 0.48 ulp — correctly rounded |

So the harness detects the known bug and clears the fixed build. Against that
calibration, `tan` near a pole loses **all 14** significant digits (2.7e13 ulp)
and then returns a constant or `Infinity` — a substantially larger error than
the one that motivated #260.

## How it was found

Differential testing against mpmath at 500+ bit precision, scoring **error in
ulps** rather than exact-match equality. Upstream's own
`test/hypothesis/error_hunt.py` does sweep `tan`, but draws inputs as
`mantissa(|·| ≤ 1) × 10^k`, which has no gradient toward `π/2`; finding a pole
neighbourhood that way is a needle in a `10¹⁴` haystack. Sampling *relative to
the poles* finds it immediately.

This is the one finding that needed an oracle outside the pair. A port built for
fidelity reproduces the defect exactly, so the two implementations agree and any
comparison between them is satisfied; catching it required a third opinion —
mpmath at 500 bits — and a question the campaign does not ask, which is not
*"do these two agree"* but *"how many digits are left"*. The error profile, the
mechanism, the pole sweep and the positive control above are all reproducible
from the figures in this document.

The port pins the behaviour in `crates/decimal-core/src/trig.rs` with a test
that asserts the *wrong* answer, so a fix upstream surfaces here as a deliberate
choice rather than a silent drift.
