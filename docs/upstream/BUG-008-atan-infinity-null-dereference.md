# BUG-008 — `atan(±Infinity)` dereferences null above the π constant's precision

**Project:** [MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js)
**Commit:** `cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f` (v10.6.0)
**Location:** `decimal.js:1104–1112`, in `P.inverseTangent`
**Severity:** `TypeError` from inside the library, on an in-domain argument at a
documented precision
**Found by:** the differential campaign for [decimal-rs](../../README.md)

---

## Summary

At any `precision` above 1021, `atan(Infinity)` raises a `TypeError` instead of
returning π/2.

```js
const Decimal = require('decimal.js');
Decimal.set({ precision: 1022 });      // documented range is 1 to 1e9
new Decimal(Infinity).atan();
// TypeError: Cannot read properties of null (reading '147')
```

Three lines. The argument is in the domain, the precision is inside the
documented range, and `atan(±Infinity)` has an exact and obvious answer.

| `precision` | `new Decimal(Infinity).atan()` |
|---|---|
| 1021 | `1.5707963267948966…` (π/2 to 1021 digits) |
| **1022** | **`TypeError: Cannot read properties of null`** |
| 1e9 — the documented maximum | the same `TypeError` |

## Mechanism

`inverseTangent` handles a non-finite argument first, and the handling is
conditional:

```js
    if (!x.isFinite()) {
      if (!x.s) return new Ctor(NaN);
      if (pr + 4 <= PI_PRECISION) {
        r = getPi(Ctor, pr + 4, rm).times(0.5);
        r.s = x.s;
        return r;
      }
    } else if (x.isZero()) {
      …
```

`PI_PRECISION` is 1025, the length of the built-in π. When `pr + 4` exceeds it,
**neither** branch returns and control reaches the series below with `x` still
infinite. From there:

- `x2 = x.times(x)` is `Infinity`;
- every term `px.div(n)` is `Infinity`;
- `r` and `t` are both `Infinity`, so no two partial sums ever differ;
- the convergence test is `if (r.d[j] !== void 0)`, and `r.d` is `null`.

`TypeError`. The loop's only exit is a property read on a value that has no
digit array.

This is the same mistake as [BUG-003](BUG-003-topower-null-dereference.md),
[BUG-005](BUG-005-taylorseries-null-dereference.md) and
[BUG-006](BUG-006-argument-reduction-null-dereference.md): a value with no `d`
is used as though it had one. It is the fourth report in that family, and the
first where the infinity is the caller's own argument rather than something the
exponent clamps produced.

## Suggested fix

Let the branch always return, and let `getPi` refuse:

```js
     if (!x.isFinite()) {
       if (!x.s) return new Ctor(NaN);
-      if (pr + 4 <= PI_PRECISION) {
-        r = getPi(Ctor, pr + 4, rm).times(0.5);
-        r.s = x.s;
-        return r;
-      }
+      r = getPi(Ctor, pr + 4, rm).times(0.5);
+      r.s = x.s;
+      return r;
     } else if (x.isZero()) {
```

`getPi` already throws `[DecimalError] Precision limit exceeded` above
`PI_PRECISION`, so this replaces a `TypeError` from an unrelated line with the
library's own error naming the actual cause — and it makes `atan` agree with the
rest of its family, which reaches that error today:

| at precision 1130 | result |
|---|---|
| `asin(1)`, `asin(-1)` | `[DecimalError] Precision limit exceeded` |
| `acos(0)`, `acos(-1)` | `[DecimalError] Precision limit exceeded` |
| `Decimal.atan2(1, -1)` | `[DecimalError] Precision limit exceeded` |
| `new Decimal('1e9').sin()` | `[DecimalError] Precision limit exceeded` |
| `atan(Infinity)` | `TypeError` |

The guard on the neighbouring `|x| == 1` branch should **not** be removed with
it. That one falls through on purpose: the series converges for `|x| = 1`, and
`atan(1)` at precision 1130 returns correctly today.

## Why the test suite does not catch it

`test/modules/atan.js` calls `atan(Infinity)` at the default precision of 20,
where the guard holds. Nothing in the suite raises the precision above 1021 and
then calls an inverse trigonometric function on an infinity.

## Reproduction in this repository

```
node fuzz/repro-case.js atan-infinity-above-pi reference   # TypeError
node fuzz/repro-case.js atan-infinity-above-pi port        # Precision limit exceeded
node fuzz/repro-upstream.js                                # all findings, both sides
```
