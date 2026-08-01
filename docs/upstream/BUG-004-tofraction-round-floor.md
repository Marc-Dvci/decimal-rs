# BUG-004 — `toFraction()` never returns under `ROUND_FLOOR`

**Project:** [MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js)
**Commit:** `cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f` (v10.6.0)
**Location:** `decimal.js:2091–2103`, in `P.toFraction`
**Severity:** unbounded loop — no exception, no return, no way out but killing
the process
**Found by:** the differential campaign for [decimal-rs](../../README.md), which
recorded it as an input its oracle would not answer

---

## Summary

Setting the rounding mode to `ROUND_FLOOR` makes `toFraction()` loop for ever,
for **every finite value**, including `0` and `1`.

```js
const Decimal = require('decimal.js');
Decimal.set({ rounding: Decimal.ROUND_FLOOR });   // rounding: 3
new Decimal(1).toFraction();                      // never returns
```

Three lines. No unusual operand, no extreme exponent, no unusual precision. The
other eight rounding modes are unaffected.

| value | rm 0 | rm 1 | rm 2 | **rm 3** | rm 4 | rm 5 | rm 6 | rm 7 | rm 8 |
|---|---|---|---|---|---|---|---|---|---|
| `0` | ok | ok | ok | **hangs** | ok | ok | ok | ok | ok |
| `1` | ok | ok | ok | **hangs** | ok | ok | ok | ok | ok |
| `0.5` | ok | ok | ok | **hangs** | ok | ok | ok | ok | ok |
| `3.14159` | ok | ok | ok | **hangs** | ok | ok | ok | ok | ok |
| `1e21` | ok | ok | ok | **hangs** | ok | ok | ok | ok | ok |
| `-4` | ok | ok | ok | **hangs** | ok | ok | ok | ok | ok |

(Twelve values were tested across all nine modes, each in its own process with a
3-second timeout. Every value hangs under mode 3 and no value hangs under any
other mode.)

## Mechanism

The continued-fraction search terminates when the next convergent's denominator
exceeds the bound:

```js
for (;;)  {
  q = divide(n, d, 0, 1, 1);
  d2 = d0.plus(q.times(d1));
  if (d2.cmp(maxD) == 1) break;      // <-- the only exit
  ...
  d = n.minus(q.times(d2));          // <-- cancels exactly when the expansion ends
  n = d2;
}
```

Every value this library holds is a terminating decimal, so the expansion always
ends exactly, and it ends at `d = n.minus(q.times(d2))` returning zero.

**Which zero is a rounding-mode question.** `ROUND_FLOOR` rounds towards
−Infinity, so an exact cancellation returns `-0`; every other mode returns `+0`.
From there:

| iteration | `n` | `d` | `q = n/d` | `d2` | `d2.cmp(maxD)` |
|---|---|---|---|---|---|
| 0 | 1.2345…e28 | 0.1 | 1.2345…e29 | 1 | 0 — continue |
| 1 | 0.1 | **−0** | **−Infinity** | **−Infinity** | −1 — continue |
| 2 | −0 | NaN | NaN | NaN | NaN — continue |
| 3+ | NaN | NaN | NaN | NaN | NaN — continue for ever |

Under any other mode, iteration 1 has `d = +0`, so `q` is `+Infinity`, `d2` is
`+Infinity`, `cmp` is `1`, and the loop breaks — which is why the bug is
invisible everywhere else.

The defect is in the **termination test**. It is written as "has the denominator
grown past the bound" and it is standing in for "has the expansion finished".
Those coincide for eight rounding modes by accident, because `+Infinity > maxD`
happens to be the right answer to the wrong question. A signed zero is enough to
separate them.

## Suggested fix

Test for the degenerate convergent directly, before comparing it to the bound:

```js
  for (;;)  {
    q = divide(n, d, 0, 1, 1);
    d2 = d0.plus(q.times(d1));
+   if (!d2.d) break;              // the expansion has terminated
    if (d2.cmp(maxD) == 1) break;
```

In the eight modes that currently work this changes nothing at all: it breaks at
the same iteration, on the same convergents, because `+Infinity` was already
failing the comparison one line below. In the ninth it returns what the other
eight return.

That is the right answer and not merely *an* answer. The nearest fraction is a
property of the value; the recurrence is deliberately run unrounded (`external`
is cleared and the working precision is set to twice the operand's digit count);
and the rounding mode has no business reaching the result. It reaches it here
only through the sign of a zero.

## Verification

The [decimal-rs](../../README.md) port applies exactly the fix above. Across 702
calls — thirteen values × six denominator bounds × nine rounding modes — its
answer under `ROUND_FLOOR` is identical to upstream's under each of the eight
modes that terminate. Asserted in
`crates/decimal-core/src/fraction.rs::the_search_terminates_under_every_rounding_mode`.

## Why the test suite does not catch it

`test/modules/toFraction.js` has 200 assertions and every one of them runs at
the default rounding mode. Nothing in the suite calls `toFraction` after a
`Decimal.set({ rounding: … })`, and the sequence is only two calls long.

## Reproduction in this repository

```
node fuzz/repro-case.js tofraction-round-floor reference   # does not return
node fuzz/repro-case.js tofraction-round-floor port        # returns 1,1
node fuzz/repro-upstream.js                                # all findings, both sides
```
