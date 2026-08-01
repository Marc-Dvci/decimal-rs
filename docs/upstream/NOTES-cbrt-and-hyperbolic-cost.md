# Two findings that are cost rather than correctness

Neither of these produces a wrong answer, so neither is filed as a bug. Both are
measured, both are reproducible, and one of them is a denial of service in
practice.

---

## `cbrt` does not return for an operand near the exponent floor

```js
const Decimal = require('decimal.js');
Decimal.set({ precision: 20, minE: -9e15 });
new Decimal('-602e-8999999999999975').cbrt();
```

Unfinished after 45 seconds in the first run that found it, and after 15 seconds
in every run since. The operand is legal: `minE` is −9e15 and this is inside it.

The cause is the working precision. `P.cubeRoot` runs Halley's iteration at a
precision derived from the operand's exponent, and here that is a number in the
quadrillions — so each iteration is manipulating a digit array of that length,
and there is no early exit for the case where the exponent alone tells you the
answer.

For comparison, [decimal-rs](../../README.md) answers
`-1.8191373764994663229e-2999999999999991` in under a millisecond, because it
derives the same working precision but caps the *iteration* width at the
requested precision plus guard digits rather than at the exponent.

Whether upstream should return quickly or raise is a design question. Running
for an unbounded time is the one option that helps nobody.

```
node fuzz/repro-case.js cbrt-exponent-floor reference   # does not return
node fuzz/repro-case.js cbrt-exponent-floor port        # < 1 ms
```

---

## The hyperbolic argument fold is chosen by digit count, not magnitude

`P.hyperbolicSine`, `P.hyperbolicCosine` and `P.hyperbolicTangent` decide how
many times to fold their argument like this:

```js
      k = 1.4 * Math.sqrt(len);        // len = x.d.length
      k = k > 16 ? 16 : k | 0;
```

`len` is the number of *limbs*, i.e. how many digits the operand is written
with — not how large it is. So `cosh(1e6)` and `cosh(1.000001)` get the same
fold, and the Taylor series for the former needs work proportional to |x|.

The maintainer's own comment sits on that line:

```js
      // Estimation reused from cosine() and may not be optimal here.  TODO?
```

It is not optimal. `cos` first reduces modulo π/2, which bounds its argument;
these do not, so nothing bounds the series.

Measured, both at precision 20:

| | decimal.js | decimal-rs |
|---|---:|---:|
| `cosh(1e6)` | 1 141 ms | 274 ms |

Both answer `1.5166076984010437725e+434294`. The port is faster only because it
does the same work in compiled code — it reproduces the fold rule exactly, on
purpose. Neither implementation returns for `cosh(1e10)`.

A fold count derived from `x.e` as well as `x.d.length` would bound the series
for both.

```
node fuzz/repro-case.js cosh-argument-fold-cost reference
node fuzz/repro-case.js cosh-argument-fold-cost port
```
