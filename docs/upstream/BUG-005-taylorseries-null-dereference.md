# BUG-005 — `taylorSeries` dereferences null, and leaves the exponent clamps disabled for the life of the process

**Project:** [MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js)
**Commit:** `cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f` (v10.6.0)
**Location:** `decimal.js:3737`, in `taylorSeries` — reached from `P.sinh`,
`P.cosh`, `P.tanh`, `sine` and `cosine`
**Severity:** `TypeError` from a null dereference, **and** unrecoverable
corruption of module state that survives the call
**Found by:** the differential campaign for [decimal-rs](../../README.md)

---

## Summary

Build a value while `maxE` is wide, narrow `maxE` below its exponent, take a
hyperbolic function of it:

```js
const Decimal = require('decimal.js');

Decimal.set({ precision: 20, maxE: 9e15 });
const x = new Decimal('5.879302975574934568e100');

Decimal.set({ precision: 100, maxE: 73 });
x.sinh();
// TypeError: Cannot read properties of null (reading '30')
```

The `TypeError` is the smaller half. The larger half is what it leaves behind:

```js
Decimal.set({ maxE: 100 });
new Decimal('1e500').isFinite();   // true  — should be false, 500 > maxE
```

**`maxE` and `minE` stop being enforced, for every value, for the rest of the
process.** Nothing restores them, and nothing reports it.

## Mechanism

`sinh` raises the working precision and reduces its argument before summing the
series. With `maxE` at 73 and `x.e` at 100, that first reduction —
`x.times(1 / tinyPow(5, k))` — is measured against `maxE` and comes back as
Infinity. The series is then summing an infinity from its first term.

`taylorSeries` tests for convergence like this:

```js
  external = false;
  ...
  for (;;) {
    ...
    t = u.plus(y);

    if (t.d[k] !== void 0) {      // <-- t is Infinity, so t.d is null
      ...
    }
  }

  external = true;                // <-- never reached
```

`t.d` is `null`, so `t.d[k]` raises. And it raises from between the two
assignments to `external`, so the second never runs. `external` is the
module-level flag that suppresses exponent clamping around computations whose
intermediates may legitimately leave the representable range — it is left
`false`, and the constructor's `if (external)` guard silently stops firing.

There is no `try`/`finally` anywhere in the function.

This is the same shape as [BUG-003](BUG-003-topower-null-dereference.md) and
[BUG-006](BUG-006-argument-reduction-null-dereference.md): a value that the
exponent clamps turned into Infinity is then used as though it still had digits.
It is the same shape as [BUG-002](BUG-002-configuration-leak.md) in its
after-effect, but worse — BUG-002 leaks `precision` and `rounding`, which are at
least visible as properties. `external` has no accessor at all, so from outside
the library the only symptom is that documented limits have quietly stopped
applying.

## Suggested fix

Two independent changes; the second matters more.

**1. Give the series an exit for a non-finite partial sum.** Once `t` is
±Infinity every remaining term is added to an infinity and no iteration can
bring it back, so it is the answer such as it is:

```js
      t = u.plus(y);

+     if (!t.d) break;
      if (t.d[k] !== void 0) {
```

**2. Restore `external` on the way out, not just on the happy path.** The
library already knows this is the right thing to do — `getLn10` restores its
state *before* throwing, with a comment saying so deliberately. `taylorSeries`,
`sinh`, `cosh`, `tanh`, `acosh`, `asinh` and `atanh` do not. A `try`/`finally`
around each of these bodies would close the whole family at once, including
BUG-002.

## Verification

The [decimal-rs](../../README.md) port takes fix 1 and answers ±Infinity
(D-16). Its context still enforces its limits afterwards. Asserted in
`crates/decimal-core/src/trig.rs::an_overflowing_series_terminates_and_leaves_clamping_on`,
which checks both halves for `sinh`, `cosh` and `tanh`.

Note that the port had the *same* non-answer in a different costume before this
was found: it does not crash there, so it simply never left the loop. A crash
and a hang are the same bug wearing different clothes, and the fix is the same
line.

## Reproduction in this repository

```
node fuzz/repro-case.js sinh-overflowing-series reference
node fuzz/repro-case.js sinh-overflowing-series port
```

Each prints the outcome and then whether the constructor still clamps.
