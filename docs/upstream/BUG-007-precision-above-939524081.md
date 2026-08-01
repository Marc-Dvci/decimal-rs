# BUG-007 — `precision` is documented up to 1e9, and division fails above 939,524,081

**Project:** [MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js)
**Commit:** `cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f` (v10.6.0)
**Location:** `decimal.js:2793`, in `divide`; validated by `P.config` at `decimal.js:4139`
**Severity:** a documented configuration the implementation cannot honour —
`RangeError` from the host, not `[DecimalError]` from the library
**Found by:** the differential campaign for [decimal-rs](../../README.md), while
reproducing the host's array ceiling in Rust

---

## Summary

`Decimal.config` accepts any `precision` from 1 to `1e9`, and the documentation
states that range. Division throws for every value above **939,524,081**.

```js
const Decimal = require('decimal.js');
Decimal.set({ precision: 1e9 });   // accepted, and documented as the maximum
new Decimal(1).div(3);             // RangeError: Invalid array length
```

Two lines. No unusual operand, no extreme exponent, no `minE`/`maxE` games. The
same throw reaches `sqrt`, `ln`, `exp`, `pow` and everything else built on
`divide`.

## The exact threshold

| `precision` | `new Decimal(1).div(3)` |
|---|---|
| 939,524,081 | returns, 939,524,081 significant digits |
| **939,524,082** | **`RangeError: Invalid array length`** |
| 1e9 — the documented maximum | `RangeError: Invalid array length` |

Bisected, one child process per probe. About 6% of the documented range of
`precision` — every value from 939,524,082 to 1,000,000,000 — is accepted by
`config` and rejected by the first division that follows.

## Mechanism

`divide` converts the significant-digit target into limbs and then fills the
quotient one index at a time:

```js
sd = sd / logBase + 2 | 0;
…
sd++;
for (; (i < xL || k) && sd--; i++) {
  t = k * base + (xd[i] || 0);
  qd[i] = t / yd | 0;        // <-- decimal.js:2793
  k = t % yd | 0;
}
```

`qd` therefore grows to `⌊pr/7 + 2⌋ + 1` elements for a non-terminating
quotient. A dense array in a 64-bit V8 keeps its elements in a backing store
capped at one gigabyte of eight-byte slots, so growth by assignment stops at
2²⁷ = 134,217,728 elements and throws `RangeError: Invalid array length` there.

Setting the two equal gives the threshold exactly:

```
⌊pr/7 + 2⌋ + 1 > 134_217_728   ⟺   pr ≥ 7 × 134_217_726 = 939_524_082
```

which is the measured boundary, to the digit.

Note that 2²⁷ is *not* the specification's array limit of 2³² − 1. Assigning
`arr[i]` for increasing `i` is stopped by the engine's backing store long before
`length` could reach its specified maximum:

```js
const a = []; for (let i = 0; ; i++) a[i] = 0;   // RangeError at a.length === 134217728
```

## Why the test suite does not catch it

`test/modules/config.js` checks that `precision: 1e9` is *accepted* and that
`1e9 + 1` is rejected. It does not then perform an operation. No assertion in
the suite divides at a precision above a few hundred.

## Suggested fixes

Either would do; the first is a one-line documentation change and the second is
a behaviour change, so they are not equivalent in cost.

1. **State the real maximum.** Document `precision` as 1 to 939,524,081 and
   reject above it in `config`, so the failure arrives from `[DecimalError]
   Invalid argument: precision: …` at the point of configuration rather than as
   a host `RangeError` from an unrelated line several calls later.

2. **Raise the ceiling in `divide`.** A typed array, or a chunked quotient,
   would carry the documented range. That is a much larger change and probably
   not worth it: 939 million significant digits is far past any use the library
   is put to, and the value of fixing this is mostly that the failure should be
   the library's own and should name the setting that caused it.

Whichever is chosen, the 32-bit truncation on the line above is worth a second
look on its own account. `sd / logBase + 2 | 0` silently wraps once the working
precision passes about 1.5 × 10¹⁰ — which the transcendental functions reach on
their own, since they raise the precision by the operand's exponent — and a
negative limb target does not terminate the loop, because `sd--` is truthy for
every non-zero value. `sinh` one exponent below `maxE` arrives at that line with
a target of −1,354,212,501; what stops it is the array ceiling and nothing else.

## Reproduction in this repository

```
node scripts/host-limits.js
```

Seven cases on both implementations, in separate processes, including the two
precisions either side of the threshold above. It also measures the host's array
ceiling live and compares it with the constant the port compiles in.
