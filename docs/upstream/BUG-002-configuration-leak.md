# BUG-002 — `acosh`/`asinh`/`atanh` leave `precision` and `rounding` raised when the inner computation throws

**Project:** [MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js)
**Commit:** `cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f` (v10.6.0)
**Location:** `decimal.js`, in `P.inverseHyperbolicCosine`,
`P.inverseHyperbolicSine`, `P.inverseHyperbolicTangent`
**Severity:** the library is left permanently unusable — every later operation,
on any value, is computed at a precision of nine quadrillion
**Found by:** the differential campaign for [decimal-rs](../../README.md)

---

## Summary

```js
const Decimal = require('decimal.js');
Decimal.set({ precision: 20, rounding: 4 });

new Decimal('9.87e8999999999999000').acosh();
// RangeError: Invalid array length      (after ~3 s)

Decimal.precision;        // 8999999999999024   -- was 20
Decimal.rounding;         // 1                  -- was 4

new Decimal(1).div(3);    // RangeError: Invalid array length, after ~1.9 s
```

The operand is an ordinary finite value well inside the documented range —
`maxE` is 9e15 and this is `9.87e8999999999999000`.

## Mechanism

```js
    pr = Ctor.precision;
    rm = Ctor.rounding;
    Ctor.precision = pr + Math.max(Math.abs(x.e), x.sd()) + 4;
    Ctor.rounding = 1;
    external = false;

    x = x.times(x).minus(1).sqrt().plus(x);      //  <-- can throw

    external = true;
    Ctor.precision = pr;                         //  <-- never reached
    Ctor.rounding = rm;

    return x.ln();
```

No `try`/`finally`. For an argument near the exponent limit the raised precision
is around 9e15, and the alignment inside `minus` then asks for an array longer
than JavaScript permits, which throws. All three restoring assignments are
skipped, including `external` — so the exponent clamps are disabled too, the
same after-effect as [BUG-005](BUG-005-taylorseries-null-dereference.md).

The damage is not confined to the value that failed. Every subsequent operation
on every value runs at that precision.

## The leak is wider than the inverse hyperbolics

The differential campaign also observes `cos` leaving `precision` raised from
995 to 1042 and `rounding` changed from 3 to 1, after throwing
`[DecimalError] Precision limit exceeded` from `getPi`. The same pattern —
raise, compute, lower — appears in `sin`, `cos`, `tan`, `sinh`, `cosh`, `tanh`,
`exp`, `ln`, `log` and `pow`, none of them with a `finally`.

Worth fixing as a family rather than one function at a time.

## Suggested fix

The library already contains the correct pattern, and a comment saying it is
deliberate: `getLn10` restores state *before* it throws. Applying the same
discipline with `try`/`finally` closes the whole family:

```js
    pr = Ctor.precision;
    rm = Ctor.rounding;
    Ctor.precision = pr + Math.max(Math.abs(x.e), x.sd()) + 4;
    Ctor.rounding = 1;
    external = false;
+   try {
      x = x.times(x).minus(1).sqrt().plus(x);
+   } finally {
      external = true;
      Ctor.precision = pr;
      Ctor.rounding = rm;
+   }
    return x.ln();
```

## Verification

The [decimal-rs](../../README.md) port throws the same `RangeError` with the
same message, in 1 ms rather than 3 s, and leaves `precision` at 20 — the next
`1/3` is `0.33333333333333333333` and costs nothing (D-11).

## Reproduction in this repository

```
node fuzz/repro-upstream-config-leak.js                        # the annotated version
node fuzz/repro-case.js acosh-configuration-leak reference
node fuzz/repro-case.js acosh-configuration-leak port
```
