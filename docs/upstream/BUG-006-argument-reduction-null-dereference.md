# BUG-006 — the argument reduction of `sin`/`cos`/`tan` dereferences null

**Project:** [MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js)
**Commit:** `cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f` (v10.6.0)
**Location:** `decimal.js`, in `toLessThanHalfPi` (`isOdd(t)`), `cosine`
(`x.d.length`) and `sine` (`x.d.length`)
**Severity:** `TypeError` from a null dereference
**Found by:** the differential campaign for [decimal-rs](../../README.md)

---

## Summary

```js
const Decimal = require('decimal.js');

Decimal.set({ precision: 34, maxE: 9e15 });
const x = new Decimal('-4.9481810070120303e809');

Decimal.set({ precision: 20, rounding: 7, maxE: 104 });
x.cos();
// TypeError: Cannot read properties of null (reading 'length')
```

`sin` and `tan` fail the same way on the same input.

## Mechanism

`toLessThanHalfPi` reduces `|x|` by subtracting a whole multiple of π:

```js
    t = x.divToInt(pi);

    if (t.isZero()) {
      quadrant = isNeg ? 3 : 2;
    } else {
      x = x.minus(t.times(pi));
      ...
      quadrant = isOdd(t) ? ... : ...;      // <-- isOdd reads t.d.length
```

`divToInt` ends in `finalise(…, Ctor.precision, Ctor.rounding)`, which applies
the exponent clamps. When `x.e` is above `maxE` there is no representable
multiple of π to subtract, so `t` is Infinity and `t.d` is `null`.

`isOdd(n)` is `n.d[n.d.length - 1] & 1`. It raises.

Two more sites downstream have the same exposure, and are reached whenever the
reduction returns a non-finite value by any other route: `cosine` reads
`x.d.length` on its first working line, and `sine` reads it on its very first.

The `quadrant` module variable is also left holding whatever the previous call
put there — harmless while the exception propagates, but it is global state
being abandoned mid-update.

This is one of three places where a value the clamps turned into Infinity is
subsequently used as though it still had digits; see
[BUG-003](BUG-003-topower-null-dereference.md) in `toPower` and
[BUG-005](BUG-005-taylorseries-null-dereference.md) in `taylorSeries`. The
pattern is worth a sweep rather than three patches: `grep` for `.d.length` and
`.d[` and check each site against the possibility that the clamps have just
fired.

## Suggested fix

Answer NaN when the reduction cannot be performed, which is the rule the
function's own caller already applies to a non-finite argument — `P.cos` opens
with `if (!x.d) return new Ctor(NaN);`. There is no angle to take a cosine of.

```js
    t = x.divToInt(pi);

+   if (!t.d) return new Ctor(NaN);
    if (t.isZero()) {
```

with the same guard at the top of `cosine` and `sine` for the other two entry
points.

## Verification

The [decimal-rs](../../README.md) port returns NaN for `sin`, `cos` and `tan` on
this input (D-17). Asserted in
`crates/decimal-core/src/trig.rs::an_overflowing_argument_reduction_answers_nan`.

The port previously *panicked* here — a Rust panic unwinding across the Node-API
boundary, which is strictly worse than the `TypeError`. It was found by the same
campaign run that found the upstream defect, in the same slice, and the two are
the same missing guard on two sides of a port.

## Reproduction in this repository

```
node fuzz/repro-case.js cos-overflowing-reduction reference
node fuzz/repro-case.js cos-overflowing-reduction port
```
