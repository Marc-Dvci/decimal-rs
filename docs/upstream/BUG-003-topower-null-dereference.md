# BUG-003 — `toPower` dereferences null when the clamp makes the base infinite

**Project:** [MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js)
**Commit:** `cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f` (v10.6.0)
**Location:** `decimal.js:2277` onwards, in `P.toPower = P.pow`
**Severity:** `TypeError` from a null dereference
**Found by:** the differential campaign for [decimal-rs](../../README.md)

---

## Summary

```js
const Decimal = require('decimal.js');

const x = new Decimal('1e10');
Decimal.set({ maxE: 5 });
x.pow(3);
// TypeError: Cannot read properties of null (reading 'length')
```

`pow(2)` and `pow(0.5)` fail identically. `pow(-3)` returns `0`.

## Mechanism

`toPower`'s third line is

```js
    x = new Ctor(x);
```

which is not a copy but a re-judgement: passing an existing Decimal through the
constructor measures it against the *current* `minE` and `maxE`, and a value
built when the limits were wider comes back as ±Infinity. This is deliberate and
documented behaviour — a value is measured against the limits when it is used —
but every line after it in `toPower` assumes `x.d` is an array.

The first one to touch it raises. `intPow` reaches `r.d.length`; the negative
branch reaches `new Ctor(1).div(r)` first, which is why `pow(-3)` gets an
answer (`Infinity^-3` is 0) while the positive exponents do not.

Same shape as [BUG-005](BUG-005-taylorseries-null-dereference.md) and
[BUG-006](BUG-006-argument-reduction-null-dereference.md).

## Suggested fix

The function already has the rule it needs on its own first line:

```js
    // Either ±Infinity, NaN or ±0?
    if (!x.d || !y.d || !x.d[0] || !y.d[0]) return new Ctor(mathpow(+x, yn));
```

That test runs *before* the clamping copy. Repeating it after is enough, and
gives the answer the same table would have given had the base arrived infinite:

```js
    x = new Ctor(x);
+   if (!x.d) return new Ctor(mathpow(+x, yn));

    if (x.eq(1)) return x;
```

It also agrees with upstream wherever upstream currently answers at all:
`pow(-3)` is `0` either way.

## Verification

The [decimal-rs](../../README.md) port applies exactly this and returns
±Infinity or 0 as the exponent's sign dictates (D-13). The port panicked here
before the fix — `digits() called on a non-finite value` — which is a Rust panic
crossing the Node-API boundary and worse than the `TypeError`.

## Reproduction in this repository

```
node fuzz/repro-case.js pow-clamped-base reference
node fuzz/repro-case.js pow-clamped-base port
```
