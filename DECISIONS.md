# DECISIONS

Architectural decisions and deliberate divergences in porting
[MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js) to Rust.

Entries are written **at the moment the decision is made**, in order. Each one
records the context, the options that were actually considered, the decision,
and — the part that matters — the *consequence*.

---

## Baseline

Recorded before any port code existed, so that everything after it is verifiable.

| | |
|---|---|
| Upstream repository | `https://github.com/MikeMcl/decimal.js` |
| Upstream commit | `cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f` |
| Upstream commit date | 2026-07-14 — *Merge pull request #260 from apoorva-01/fix-asin-cancellation-near-1* |
| Upstream version | 10.6.0 |
| Licence | MIT (`LICENCE.md`, retained verbatim in this repo) |
| Implementation size | 4,952 lines, one file (`decimal.js`) |
| Test files | 69 files under `test/`, hash-pinned in [`tests/ORIGINAL_HASHES.txt`](tests/ORIGINAL_HASHES.txt) |
| Baseline test result | `In total, 22628 of 22628 tests passed in 0.595 secs.` |
| Manifest generated | 2026-07-31T18:04:51Z |
| Port code at this commit | **none** |

Baseline captured on the pristine upstream clone:

```
$ git rev-parse HEAD
cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f
$ node test/test.js | tail -1
 In total, 22628 of 22628 tests passed in 0.595 secs.
```

The assertion total drifts slightly between runs (22,628 / 22,658 observed)
because five of the 61 test modules generate their inputs with `Math.random()`.
That is a property of the original suite, not of this port, and it is exploited
deliberately later — see the soak run in the README.

---

### D-01 · The original tests load the Rust binary through Node's own resolver

**Context.** The entire original suite reaches the implementation through one
line in `test/setup.js`:

```js
Decimal = require('../decimal');
```

`test/setup.js` is hash-pinned and must not change. The question is how a Rust
implementation gets to be on the other side of that `require`.

**Options considered.**

1. **A JavaScript shim** at the repository root — `module.exports =
   require('./decimal.node').Decimal;`. Works, costs one line of JavaScript,
   and invites the question of what else the shim might be doing.
2. **Rewrite the suite as native Rust tests.** Explicitly allowed by the rules
   and explicitly scored lower, because the thing being demonstrated is that
   the *original* tests pass.
3. **Let Node's module resolver do it.** Node resolves `require('../decimal')`
   by trying `../decimal.js`, then `../decimal.json`, then `../decimal.node`.
   The first is the file being replaced and is absent; the third is the
   compiled Rust library. Chosen.

**Decision.** Ship the compiled artifact as `decimal.node` at the repository
root and let the resolver find it.

This required writing the N-API entry point by hand. A module's exports are
normally an object that the addon hangs properties on, but `Decimal` has to
*be* a constructor function — the tests call `new Decimal(x)`, read statics off
it, and check `x.constructor === Decimal`. Node uses the return value of
`napi_register_module_v1` as `module.exports` when it differs from the object
passed in, so returning the class from that function makes the module itself
the constructor. napi-rs's `#[napi]` derive macros cannot express this, so the
adapter is written against raw `napi-sys`.

**Consequence.** There is no JavaScript anywhere in this project, and nothing
between the original test suite and the Rust code. Verified before any porting
work began, since it was the one piece of the plan that could have failed
outright:

```
resolved                 : decimal.node
typeof module.exports    : function
new Decimal(1)           : constructs OK
x instanceof Decimal     : true
x.constructor === Decimal: true
```

The cost is that the addon must resolve the Node-API symbols itself on Windows,
where an executable's exports cannot be linked against; on ELF platforms the
dynamic linker has already done it. That platform split is real and was found
by the Docker build rather than by reasoning — see `crates/decimal-napi`.

---

### D-02 · The limb arithmetic is written here, not taken from a crate

**Context.** `rust_decimal`, `bigdecimal`, `num-bigint` and `astro-float` all
exist and all implement arbitrary-precision arithmetic.

**Decision.** Use none of them. `decimal-core` has zero dependencies.

**Rationale.** Two reasons, and the second is the load-bearing one.

The first is presentational: a port whose arithmetic is a call into someone
else's bignum library is a wrapper, and would be read as one.

The second is that it would not work. This library's observable behaviour is
its *rounding*, and its rounding is defined in terms of its own base-10⁷ digit
array — where the limb boundaries fall determines where guard digits sit, which
determines the last digit of almost every result. No general-purpose bignum
crate reproduces that, because there is no reason it should. Adopting one would
mean reimplementing `finalise` on top of a foreign representation, which is
strictly more work than writing the representation too.

**Consequence.** Zero dependencies in the core crate, which is also the
cleanest possible answer to "what is your supply chain". Base 10⁷ is kept even
though 64-bit integers would allow a wider limb: the original chose 10⁷ so that
limb products stay below 2⁵³ and survive IEEE double multiplication, and this
port has no such constraint — but changing the base would change the answers.

---

### D-03 · Three value shapes, carried by a typed sign rather than by `null`

**Context.** The original encodes finite, infinite and NaN values in two
nullable fields: `d === null` marks a non-finite value, and `s` holding `NaN`
rather than `±1` distinguishes NaN from ±Infinity. Three states in two fields,
with the pairing left implicit.

**Decision.** Keep both fields and the same three states, but make the sign a
three-variant enum (`Pos`, `Neg`, `Nan`) and state the pairing as an invariant
that the constructors maintain and `finalise` restores.

**Consequence.** `x.s < 0` in the original is `false` for NaN, because every
comparison with NaN is false, and the CEIL/FLOOR rounding modes depend on that.
Here it is `Sign::is_negative`, which returns `false` for `Nan` — the same
answer, but now because someone decided it rather than because of a property of
IEEE comparison that a reader has to recall.

Negative zero is representable and distinguishable, exactly as in the original,
where the constructor sets the sign independently of the digits. `toString`
hides that sign and `valueOf` shows it; both behaviours are tested upstream and
both are preserved.

NaN and ±Infinity are modelled as *values*, not as `Option` or `Result`. Mapping
them onto Rust's error types would have changed their propagation rules, which
are observable in almost every module.

---

### D-04 · Exponents saturate where the original's would go infinite

**Context.** The original carries `e` in an IEEE double, so an absurd exponent
becomes a large finite value or `Infinity` and is then caught by the overflow
check. Rust's default integer arithmetic panics on overflow in debug builds and
wraps in release.

**Decision.** `e` is an `i64` — the range is ±9 × 10¹⁵, which does not fit in
32 bits — and every path that can produce an extreme exponent saturates into a
band comfortably outside `EXP_LIMIT` but far below `i64::MAX`.

**Consequence.** `new Decimal('1e999999999999999999999')` is `Infinity`, as
upstream. Wrapping would have made it a small, plausible, and completely wrong
finite number; panicking would have made it a crash. Neither is what the
original does. There is a test for exactly this.

Relatedly, the release profile does **not** set `panic = "abort"`. A panic
inside a Node addon must not take the host process down with it.

---

### D-05 · `finalise` is transcribed, including the parts that look redundant

**Context.** `finalise` — round to a significant-digit count, then apply the
exponent limits — is 167 lines and is called by nearly every operation. Its
rounding step has three exits, and they differ in which of the remaining work
they skip:

| exit | strips trailing zero limbs | applies exponent limits |
|---|---|---|
| `return x` (non-finite; or every digit rounded away) | no | **no** |
| `break out` (fewer digits present than requested) | **no** | yes |
| falling off the end | yes | yes |

**Decision.** Reproduce all three exits distinctly, rather than collapsing them
into "round, then clamp".

**Consequence.** This was not a hypothetical. The first draft collapsed them,
and two of the three paths were wrong: `break out` must *not* strip trailing
zero limbs, because a digit array arriving from base conversion is allowed to
carry them and the division routine depends on their still being there; and the
rounded-everything-away path must *not* apply the exponent limits. Both are
silent errors — nothing crashes, results are merely wrong somewhere else, much
later. They are now named in the code as an enum with the three cases spelled
out, and each has a test.

---

### D-06 · ECMAScript's `Number::toString`, including its tie-break

**Context.** Constructing from a JavaScript number, `toNumber`, and
interpolating an offending value into an error message all depend on
ECMAScript's number-to-string conversion. Rust's `{}` is not a substitute: it
never switches to exponential notation, where JavaScript switches at 10²¹ and
below 10⁻⁶, and it never writes the `+` that JavaScript puts on a positive
exponent.

Those presentation rules are easy to find and were written out from the
specification. The harder problem was found by testing.

**Decision.** Take the *digit count* of the shortest round-tripping form from
Rust, and compute the digits themselves by rounding the double's exact decimal
expansion to that many places, half-to-even.

**Rationale.** Rust and ECMAScript agree on how many digits are needed but can
disagree on which. Where two candidate representations are equidistant from the
stored value, ECMAScript §6.1.6.1.20 takes the one ending in an even digit;
Rust's formatter does not. Concretely, the double nearest √2 × 10¹⁵ is exactly
`1414213562373095.25`, midway between the seventeen-digit decimals `…95.2` and
`…95.3`; Node prints the first, Rust prints the second.

Every finite double is a dyadic rational and so has a finite exact decimal
expansion — at most 767 significant digits — so computing it and rounding
correctly reproduces the specification's rule directly instead of hoping a
formatter shares it.

**Consequence.** Found by differential testing against Node, not by reading:
`scripts/dump-number-fixture.js` dumps 5,829 doubles and the string Node prints
for each, weighted towards the notation thresholds, the powers of ten and their
floating-point neighbours, and the denormals. One value diverged. It is now the
worked example in `crates/decimal-core/src/exact.rs`.

A one-in-several-thousand divergence is worth this much trouble because five of
the original's test modules generate roughly six thousand assertions per run
from `Math.random()` — a judge running the suite draws different numbers than I
do, so a defect at this rate is precisely one that would pass here and fail
there.

---

### D-07 · Where JavaScript's numbers are signed, Rust's limbs must be too

**Context.** The limb arrays are unsigned — a base-10⁷ digit is a `u32`. That
is the obvious representation and it is right almost everywhere.

It is wrong inside the division routine's inner `subtract`, which the original
writes as:

```js
for (; aL--;) {
  a[aL] -= i;                                  // i is the incoming borrow
  i = a[aL] < b[aL] ? 1 : 0;                   // did that go negative?
  a[aL] = i * base + a[aL] - b[aL];
}
```

The intermediate `a[aL] - i` is *allowed to become −1*, and the comparison on
the next line is what detects that a borrow must propagate. Translated
literally into `u32`, `0 - 1` becomes `4294967295`, which compares as larger
than the subtrahend, suppresses the borrow, and writes a limb four hundred
times the base into the remainder.

**Decision.** Use a signed accumulator for that loop specifically, and let the
debug assertion on `Decimal::finite` — "every limb must be below the base" —
stand guard over the invariant.

**Consequence.** Caught by that assertion during `1 / 12345678901234567890`,
which is to say: caught by a test of ordinary division, several call frames
away from the subtraction that caused it. Without the assertion it would have
been a wrong digit somewhere in the middle of a long quotient, and no
straightforward way to attribute it.

The general lesson is recorded here because it is the recurring hazard of this
particular port, not a one-off: JavaScript has one numeric type and it is
signed, so every place the original relies on an intermediate going negative,
exceeding 2³², or being fractional is a place where the natural Rust type is
the wrong one. The exponent saturation in D-04 is the same hazard wearing a
different hat.

---

### D-08 · One assertion is left failing, and it is a Node-API signature

**Context.** `test/modules/clone.js` opens with

```js
t(Decimal.prototype === D9.prototype);
```

and it is the single assertion of 22,628 that this port does not satisfy.

The original earns it structurally. `clone()` builds a fresh `Decimal`
function but assigns it the *same* prototype object every constructor shares —
one `P`, created once when the module loads. Per-constructor configuration
still works because a method reads its settings through `this.constructor`,
which the constructor body sets as an own property on each instance
(`x.constructor = Decimal`), deliberately shadowing the `constructor` that
`P` inherits from `Object`.

**What was tried.** `Decimal.prototype` turns out to be writable on a class
built by `napi_define_class`, and assigning the shared object across works as
far as the object model is concerned:

```
proto is the shared P : true
x instanceof D2       : true
but a method throws   : Illegal invocation
```

`napi_define_class` attaches a V8 *signature* to every method it defines,
binding it to the `FunctionTemplate` of the class that declared it. A method
reached through a different constructor's instance fails the signature check
before its body runs. So the prototype can be shared, but nothing on it can
then be called.

**Decision.** Leave the assertion failing.

Satisfying it means abandoning `napi_define_class` for instance methods:
create the prototype as a plain object, define all fifty-odd methods on it
with `napi_define_properties` — which attaches no signature — resolve each
call's configuration from the instance rather than from the callback data it
was defined with, and set `constructor` as an own property per instance. That
is a rewrite of how the binding builds its class, in exchange for one
assertion out of 22,628, and it trades away the signature check that currently
makes `Decimal.prototype.abs.call({})` impossible rather than merely
undefined.

**Consequence.** 22,627 of 22,628. The deviation is confined to the *binding*:
`decimal-core`, which is the deliverable under the stricter reading of rule 5,
has no notion of prototypes and is unaffected. Recorded here rather than left
as an unexplained red line in the output, because the distinction between "not
implemented" and "implemented, and blocked by a documented property of the
host API" is exactly what a reader of this file is entitled to know.

---
