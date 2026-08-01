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

### D-09 · The narrowing that turned a wrong answer into a hang

**Context.** `taylorSeries` builds each denominator as `new Ctor(n++ * n++)`.
`n` there is a JavaScript number — a double — so the product is exact to 2⁵³.
The port had it as:

```rust
let denominator = Decimal::from_i32((a * b) as i32);
```

Every counter in this crate is an `i64`, so `a * b` is computed correctly; the
`as i32` then discards the top half of it. For `n` above 46,340 the product
exceeds `i32::MAX` and wraps, sometimes to a negative number.

**The symptom was not a wrong digit.** A series whose denominators stop growing
has terms that stop shrinking, so the convergence test — successive partial
sums agreeing limb-for-limb — is never satisfied. `cosh(1e6)` reduces to an
argument near 250,000 and needs about that many terms, which is comfortably
past the boundary. The call did not return. Upstream answers it in two seconds.

**Found by** the differential fuzz harness, within minutes of it first running,
as a sequence that stopped producing output. It was not found by the original
test suite, which passed 22,627 of 22,628 before and after: no assertion in it
drives a series that long.

**Decision.** Add `Decimal::from_integer(i64)`, exact across the whole range,
and remove every narrowing conversion of a counter — six sites, in `trig.rs`,
`elementary.rs` and `inverse.rs`, only one of which was reachable. Where the
product can exceed 2⁵³, `series_denominator` deliberately routes through an
`f64` so that the port rounds where the original rounds; that branch needs
about 94 million terms to reach and exists so that the difference is written
down rather than merely improbable.

**Consequence.** `cosh(1e6)` now returns the same digits as upstream in 470 ms
against upstream's 2,070 ms. Pinned by a test that hangs rather than fails if
the narrowing returns.

This is D-07 for the third time — JavaScript has one numeric type and it is a
double, so every intermediate the original computes is exact to 2⁵³ and signed,
and every place the port picks a narrower type is a place to justify rather
than assume. D-07 was an intermediate going negative; the `pow` fix in the
straggler pass was an array read past its end; this is a product outgrowing its
type. Three symptoms, one cause.

**A note on what the same investigation found upstream.** `cosh`'s argument
reduction chooses how many times to fold by the operand's *digit count*
(`k = ceil(len / 3)`), never by its *magnitude* — the maintainer's own comment
there reads `TODO? Estimation reused from cosine() and may not be optimal
here`. For `cos` that is sound, because the argument is first reduced modulo
π/2; the hyperbolic functions have no periodicity to exploit, so a large
argument stays large and the series needs work proportional to it. Upstream's
`cosh(1e6)` takes two seconds, `cosh(1e8)` is minutes, and the growth is
linear. That is a property of the original, so the port reproduces it, and the
fuzz harness bounds the magnitude of hyperbolic arguments for this reason and
says so in its log rather than silently skipping them.

---

### D-10 · Reproducing a limit that Rust does not have

**Context.** `asinh` raises the working precision to
`pr + 2·max(|x.e|, x.sd()) + 6`. For an argument near the exponent ceiling that
is a precision around 1.8 × 10¹⁶, and the alignment inside the `plus` that
follows then wants to prepend about 2.6 × 10¹⁵ zero limbs to the smaller
operand.

The original attempts this too. JavaScript stops it:

```
RangeError: Invalid array length
    at P.plus (decimal.js:1604)
```

catchable, the calculation abandoned, the process fine.

A Rust `Vec` has no maximum length, so the port instead asked the allocator for
10,285,714,285,714,240 bytes and was killed:

```
memory allocation of 10285714285714240 bytes failed
```

**Found by** the differential fuzz harness on its third sequence, as a process
that stopped existing. The original test suite does not reach it — no assertion
in it calls an inverse hyperbolic function on an argument near the exponent
limit.

**Decision.** Reproduce JavaScript's array-length ceiling explicitly.
`MAX_ARRAY_LENGTH` is that ceiling; `plus` and `minus` check the padding against
it, set `Ctx::array_limit_exceeded`, and abandon the calculation. The Node binding
turns that flag into a thrown `RangeError` carrying the original's exact
message — `napi_throw_range_error`, not `napi_throw_error`, so that
`err instanceof RangeError` is true on both sides and not merely the message.

A flag rather than a `Result` because `plus` and `minus` are infallible in this
crate and are called from several hundred places, none of which could produce
the condition; the flag is read once, at the boundary, in `make`, which every
returned Decimal passes through.

**Consequence.** Identical observable behaviour, including the error's type:

```
port      threw: RangeError: Invalid array length | instanceof RangeError: true
reference threw: RangeError: Invalid array length | instanceof RangeError: true
```

**The general point.** This is the first divergence in the port caused by Rust
being *less* limited than JavaScript rather than differently limited. The
recurring hazard recorded in D-07 and D-09 is that the original's numbers are
doubles and the port's are not; this is its structural counterpart, and it is
worse, because a missing ceiling has no symptom until the day it has a fatal
one. Where the original depends on its host refusing something, the port has to
refuse it too — and refuse it the same way.

**Postscript.** The mechanism here is right and the number was not: this entry
set `MAX_ARRAY_LENGTH` to the specification's 2³² − 1, and the ceiling V8
actually enforces on an array grown one index at a time is 2²⁷. The case above
overshoots both by seven orders of magnitude, so nothing here noticed. D-19 is
what did.

---

### D-11 · The second place the port deliberately differs

**Context.** `acosh`, `asinh` and `atanh` raise the working precision, compute,
and lower it again:

```js
pr = Ctor.precision;
rm = Ctor.rounding;
Ctor.precision = pr + Math.max(Math.abs(x.e), x.sd()) + 4;
Ctor.rounding = 1;
external = false;
x = x.times(x).minus(1).sqrt().plus(x);   // can throw
external = true;
Ctor.precision = pr;                      // then never reached
Ctor.rounding = rm;
return x.ln();
```

There is no `try`/`finally`. For an argument near the exponent ceiling — an
ordinary value, well inside the documented `maxE` of 9e15 — the raised
precision is about 9e15 and the alignment inside `minus` throws `RangeError:
Invalid array length`. The restoring assignments are skipped.

The constructor is then left at `precision = 8999999999999024`, `rounding = 1`,
and `external = false`, permanently. The damage is not confined to the value
that failed:

```
decimal.js  (reference)
  before          precision=20 rounding=4
  acosh(9.87e+8999999999999000)  threw RangeError: Invalid array length
  after           precision=8999999999999024 rounding=1
  then 1/3        threw RangeError: Invalid array length   (2204 ms)

decimal-rs  (this port)
  before          precision=20 rounding=4
  acosh(9.87e+8999999999999000)  threw RangeError: Invalid array length
  after           precision=20 rounding=4
  then 1/3        0.33333333333333333333   (0 ms)
```

`fuzz/repro-upstream-config-leak.js` produces exactly that output.

**Decision.** Do not reproduce the leak. This is, with D-08, one of only two
places where the port knowingly behaves differently from the original, and the
only one that is a choice rather than a constraint.

The standing rule is fidelity, and it is not set aside lightly. Three things
justify it here:

1. **No assertion covers it.** The port passes 22,627 of 22,628 with the leak
   absent, so nothing being preserved depends on it.
2. **The maintainer's own code shows the opposite intent.** `getLn10` restores
   state *before* it throws, with a comment saying that is deliberate, so that
   a caught `[DecimalError]` does not leave the library wedged. The inverse
   hyperbolic functions simply do not implement the intention the rest of the
   library holds.
3. **Reproducing it would make the port destructible.** One call on a value the
   API accepts would leave every later operation computing at a precision of
   9e15 — throwing, or exhausting memory, until the process is restarted. A
   port that faithfully reproduces a way to render itself permanently unusable
   has preserved the wrong thing.

**Consequence.** Reported upstream as BUG-002 (see `docs/upstream/`). The
differential harness bounds the exponent of inverse-hyperbolic arguments and
says so in its log header, because one such call corrupts the *oracle* for the
rest of the process and every comparison after it would be against a reference
in a state no user would ever see.

Note that the port's own `Ctx` makes this class of bug structurally difficult:
`without_clamping` restores rather than sets, and the working precision is
restored on the way out of each routine rather than at a single point that an
early return can skip. That was a choice made for D-06 and it paid for itself
here.

---

### D-12 · `new Ctor(x)` is not a copy, it is a re-judgement

**Context.** Nine of the original's methods begin by passing the receiver back
through the constructor:

```js
P.floor = function () { return finalise(new this.constructor(this), this.e + 1, 3); };
P.abs   = function () { var x = new this.constructor(this); if (x.s < 0) x.s = 1; return finalise(x); };
```

Read as defensive copying — which is how the port first read it — this is
`x.clone()`. It is not. The constructor, given an existing Decimal, **clamps it
to the exponent limits currently in force**:

```js
if (external) {
  if (!v.d || v.e > Decimal.maxE) { x.e = NaN; x.d = null; }   // Infinity
  else if (v.e < Decimal.minE)    { x.e = 0; x.d = [0]; }      // zero
  else { x.e = v.e; x.d = v.d.slice(); }
}
```

So a value is judged against `minE` and `maxE` **when it is used**, not only
when it was built. Narrow `maxE` after constructing a large value and the next
`abs`, `floor`, `round`, `trunc`, `neg`, `toDP`, `toSD`, `toNearest` or `pow`
returns Infinity.

**Found by** the differential fuzzer, on a sequence that set `minE` between
building a value and calling `floor` on it — which the original's own test
suite never does, and which is why 22,627 assertions passed either way.

**Decision.** Transcribe it: `ops::clamped_copy`, used at all nine sites.

Including where the result is absurd. `floor` rounds the *clamped* copy but
takes its significant-digit count from `this.e + 1` — the exponent of the
**original**. When the clamp fires those are different values, and
`floor(-1.785e-8999999999999976)` with `minE` at −872 returns
`-1e+8999999999999976`: a request to floor something smaller than one, answered
with a number of nine quadrillion digits. The port now returns exactly that.

It is tempting to call this the third upstream bug and fix it. It is not
reported as one, because unlike D-09 and D-11 it needs a configuration change
between two operations on the same value to reach — a sequence no ordinary
program performs — and because the rule here is fidelity. D-11 was set aside
only because reproducing it would let a caller render the library permanently
unusable. Returning one wrong number does not meet that bar.

**Consequence.** Four cases the port previously answered plausibly and wrongly,
now matching upstream exactly, including the ordinary one:

```
maxE = 200:  abs(9.87e300)  →  Infinity          (was 9.87e+300)
             toSD(9.87e300, 5) → Infinity        (was 9.87e+300)
minE = -872: neg(-1.78e-8999999999999976) → 0    (was the value)
             floor(same)    →  -1e+8999999999999976   (was -1)
```

The first two are not exotic. Any program that narrows `maxE` after
constructing its values would have seen the difference.

---

### D-13 · The third divergence, and the second crash declined

**Context.** `new Ctor(x)` can turn a finite receiver into ±Infinity — that is
what D-12 is about — and everything after that line in `toPower` assumes a
digit array. The port panicked:

```
thread '<unnamed>' panicked at crates\decimal-core\src\decimal.rs:268:14:
digits() called on a non-finite value
```

A Rust panic unwinding across the Node-API boundary, which is the worst failure
mode available to this crate.

The guard that protects the original is accidental. Infinity carries `e = NaN`
in JavaScript, so `x.e == 0` is false and the `x.d[0]` beside it is never
evaluated. This port stores `e = 0` for Infinity, so the same comparison
succeeds and the digit access happens.

**Upstream does not survive it either.** With `maxE` narrowed after the value
was built:

```
> const x = new Decimal('1e10'); Decimal.config({maxE: 5}); x.pow(3)
TypeError: Cannot read properties of null (reading 'length')
```

raised from inside `intPow`. `pow(-3)` returns 0; `pow(2)` and `pow(0.5)` throw
the same TypeError. Reported as BUG-003.

**Decision.** Do not reproduce the crash. Answer with the rule the original's
*own first line* uses for a base that was already non-finite —
`Math.pow(+x, yn)` — which is the same question reached by a different road.

This is the third deliberate divergence, alongside D-08 (a constraint, not a
choice) and D-11; D-14 is the fourth. The test for setting fidelity aside has
been the same each time: reproducing the original would give a caller a way to
break the library rather than a way to compute a number. A `TypeError` from an unguarded
null dereference is exactly that, and matching it would mean the port throwing
V8's own message for a mistake V8 was merely the reporter of.

It also agrees with upstream wherever upstream answers at all: `pow(-3)` is 0
on both sides, because `Infinity^-3` is 0 by the same table.

**Consequence.** No panic; `pow` of a clamped-to-infinite base returns
±Infinity or 0 as the exponent's sign dictates. The differential harness
classifies the remaining difference as a *known* divergence, counted and named
with this entry in its log rather than filtered out of it.

---

### D-14 · The fourth divergence: a termination test that a signed zero defeats

**Context.** The differential campaign's watchdog reported
`x0.toFraction()` as an input neither implementation would return from. It was
not an oracle limitation and not a large operand — it was `ROUND_FLOOR`.

Three lines, against the pinned upstream tree, hang forever:

```js
const Decimal = require('decimal.js');       // v10.6.0, cd73a7f
Decimal.set({ rounding: Decimal.ROUND_FLOOR });
new Decimal(1).toFraction();                 // never returns
```

Not `1` in particular. Every finite value, `0` included, under that one mode.
The other eight are unaffected. Reported as BUG-004.

**Why.** `toFraction` runs the continued-fraction recurrence until the
denominator of the next convergent exceeds the bound:

```js
d2 = d0.plus(q.times(d1));
if (d2.cmp(maxD) == 1) break;
```

The expansion of a terminating decimal — which is every value this library
holds — ends by cancelling exactly, at `d = n.minus(q.times(d2))`. That
subtraction returns zero, and *which* zero is a rounding-mode question:
`ROUND_FLOOR` rounds towards −Infinity, so it returns `-0`. Everywhere else it
is `+0`.

From there the loop is over as a computation and unbounded as a program. The
next quotient is `n / -0` = `-Infinity`, so `d2` is `-Infinity`, which is not
greater than `maxD`, so there is no break; the iteration after that forms
`-Infinity × -0` = `NaN`, and every comparison involving `NaN` is false
forever. The loop makes no further progress and has no exit.

The termination test is the defect. It is written as "has the denominator grown
past the bound", and it is standing in for "has the expansion finished" — which
is true for eight rounding modes because `+Infinity > maxD` happens to be the
right answer to the wrong question.

**Decision.** Break when the convergent stops being finite, before comparing it
to the bound.

In the eight modes where upstream returns, this changes nothing whatsoever: it
breaks at the same iteration, on the same convergents, because `+Infinity` was
already failing the comparison one line later. In the ninth it returns what the
other eight return. That is the only defensible answer — the nearest fraction is
a property of the value, the recurrence is run unrounded on purpose (`external`
is off and the working precision is twice the operand's digit count), and the
rounding mode has no business reaching the result at all. It reaches it here
only through the sign of a zero.

Verified: across 702 calls — thirteen values × six denominator bounds × nine
modes — the port's answer under `ROUND_FLOOR` is identical to the oracle's under
each of the eight modes that terminate. `fraction.rs` asserts this directly.

**Why this one is not transcribed.** The same test as the previous three
(D-08, D-11, D-13): reproducing the original would hand a caller a way to break
the library rather than a way to compute a number. A non-terminating loop is the
strongest form of that available — no exception to catch, no value to inspect,
no way back except killing the process. The port did reproduce it faithfully
until this change, and the campaign log records the input that proved it.

**Consequence.** `toFraction` terminates under all nine rounding modes. The
divergence is unobservable to the differential harness in the ordinary way,
because the oracle never answers; it appears in `fuzz/log-limits.txt` as an
input the oracle could not referee, with the port's own answer and timing beside
it.

---

### D-15 · Two transcription errors in one line, cancelling at the default `maxE`

**Context.** `new Decimal('0x1p-1074')` with `maxE` at 41 is **0** upstream, and
this port answered 5e-324. The value is 4.94e-324, comfortably inside the
default limits, so nothing about the answer suggests `maxE` should have a say.

**Why upstream gives zero.** `parseOther` applies the binary exponent as

```js
if (p) x = x.times(Math.abs(p) < 54 ? mathpow(2, p) : Decimal.pow(2, p));
```

Two scales, and which one is used is observable. Below 54 the scale is a
*double* — exact for every power of two in that range, and converted exactly.
At 54 and above it is the library's own `pow`, whose negative-exponent branch is
`new Ctor(1).div(r)` with `r = 2^1074`, exponent 323. `div` re-judges its
argument (D-12), so below `maxE = 323` the divisor becomes Infinity and the
quotient is 0.

**The two errors.** This port computed the scale itself, with `int_pow` and a
division, which is neither of upstream's two paths and bypasses `pow`'s
exponent estimate entirely. And `int_pow` used `without_clamping`, which
*restored* the flag where the original ends with a bare `external = true` — so
even routing through `pow` would not have clamped, because `parseOther` clears
the flag around the whole conversion and upstream's `intPow` hands it back.

They cancel at the default `maxE`, which is why 22,658 assertions are silent
about both: every radix test in the suite runs at the default configuration.

**Decision.** Transcribe both. `parse_other` calls `to_power` for `|p| ≥ 54` and
converts a double below it; `int_pow` sets the flag rather than restoring it.

**Consequence.** A third error surfaced immediately and is worth recording,
because it is the same shape one level up: the divisor for the *fractional*
part must be built **before** the suppressed region, where upstream builds it.
Built inside, `int_pow`'s parting `external = true` switches clamping on for the
multiplication that follows, and `new Decimal('0x1.8p3')` at precision 1 comes
out as 20 instead of 12.

---

### D-16 · A series that overflows must stop, even though the original cannot

**Context.** Build a value while `maxE` is wide, narrow `maxE` below its
exponent, take a hyperbolic function. The first argument reduction overflows, so
`taylorSeries` is summing an infinity from its first term. Upstream's next line
is `if (t.d[k] !== void 0)`, `t.d` is null, and it raises

```
TypeError: Cannot read properties of null (reading '30')
```

from between its own `external = false` and `external = true`. Nothing restores
the flag: for the rest of the process the constructor stops clamping to `minE`
and `maxE` at all. Four lines to reach, no recovery. Reported as BUG-005.

**What the port did.** Not crash — and therefore never leave the loop. The
convergence test asks whether the partial sum has a limb at position `k`, and an
infinity has no limbs, so it was false for ever. The same non-answer in worse
clothes.

**Decision.** Break when the partial sum stops being finite. Every remaining
term is added to an infinity and no iteration can bring it back, so it is the
answer such as it is.

**Consequence.** ±Infinity, in microseconds, with the clamps still in force
afterwards. `trig.rs` asserts both halves for `sinh`, `cosh` and `tanh`. The
fourth deliberate divergence, on the same test as D-11 and D-13.

---

### D-17 · The third null dereference, and the panic that was worse than it

**Context.** `to_less_than_half_pi` reduces its operand by subtracting
`⌊|x|/π⌋·π`, and forms that multiple with the clamps in force. Above `maxE`
there is no representable multiple of π, so the multiple is Infinity and there
is nothing to reduce. Upstream walks into it three separate ways — `isOdd(t)`
reads `t.d.length`, `cosine` reads `x.d.length` on its first working line,
`sine` on its very first — and raises the same `TypeError`. BUG-006.

This port **panicked**:

```
thread '<unnamed>' panicked at crates\decimal-core\src\decimal.rs:268:14:
digits() called on a non-finite value
```

A Rust panic unwinding across the Node-API boundary, which is strictly worse
than the exception it was failing to reproduce.

**Decision.** Answer NaN, which is the rule the original's *own first line*
applies to a non-finite argument: `if (!x.d) return new Ctor(NaN);`. The
reduction produced no number, so there is no angle to take a sine of.

**Consequence.** The fifth deliberate divergence. Found by the campaign's
watchdog, in the category it exists to report — *the oracle answered and the
port did not* — and the upstream defect and the port's panic were the same
missing guard on two sides of a port.

---

### D-18 · The flag is set, not restored, and the sloppiness is load-bearing

**Context.** `external` suppresses the exponent clamps around computations whose
intermediates may legitimately leave the representable range. The original
writes it by hand, eighteen times, always as

```js
external = false;  …  external = true;
```

which **sets** the flag rather than putting back what it was. `Ctx::without_clamping`
restored it, and said so in its own doc comment: *"That is safe there only
because the pattern is never nested. Restoring instead of setting makes nesting
harmless, and costs nothing."*

Both halves of that sentence are false.

**It is nested.** `acosh` suppresses the clamps and then calls `sqrt`, which
suppresses them again and turns them back on — so the `.plus(x)` in
`x.times(x).minus(1).sqrt().plus(x)` runs *with* clamping, and `acosh(1.5e300)`
with `maxE` at 100 is Infinity rather than 691.87. `parseOther` calls `intPow`
and gets the same treatment (D-15). `asinh` gets NaN out of it, from a
+Infinity root meeting a −Infinity operand.

**And it costs a behaviour.** Nine methods differed because of this, none of
them reachable from the suite.

**A second mechanism, found alongside it.** `P.plus`, `P.minus` and `P.times`
each open with `y = new Ctor(y)` — a clamping copy of the *argument*. There is
no function form of these in the original; every internal use is a method call,
so the copy happens every time. This port's `add`, `sub` and `mul` did not do
it. `divide` is the exception and stays one: it *is* a function upstream, called
directly by a dozen routines, and it does not re-judge.

**Decision.** Match the original exactly, in both. `without_clamping` sets the
flag on exit; `add`/`sub`/`mul` re-judge their second operand.

**The instrument.** [`scripts/clamp-conformance.js`](scripts/clamp-conformance.js)
checks the whole family in one pass: 43 methods × 6 operands × 4 limit pairs,
each operand built under wide limits and the limits narrowed before the call —
the only arrangement in which any of this is observable. It knows the two
documented divergences and counts them rather than hiding them, and it shards
one child per method-and-operand so that the four cases neither implementation
returns from cost four cases rather than four methods.

**Consequence.** Every method agrees. This was the largest family of defect in
the port and the one that most resembled an improvement: a port that saves and
restores a flag is more careful than one that sets it, and is a port of a
different library.

---

### D-19 · The ceiling is the host's, not the specification's — and `| 0` is not a cast

**Context.** The unbounded campaign ended with a **1** in the column that is
supposed to read zero: *the oracle answered and the port did not*.

```
slice 0x422d0c37  sequence 57  x0.sinh()
  oracle:  returned in 1607 ms
  port:    did NOT return within 2.5 s
  verdict: PORT DEFECT
```

Replayed on its own, the port did worse than not return:

```
memory allocation of 34359738368 bytes failed
```

Thirty-two gigabytes, from `sinh` of an ordinary six-digit value that happens to
sit one exponent below `maxE`. The oracle raises `RangeError: Invalid array
length` — catchable, 1.6 seconds, process fine.

**Two transcription errors, one on top of the other.**

*First.* `divide` sizes its quotient from the working precision:

```js
sd = sd / logBase + 2 | 0;
```

`sinh` has already raised the precision to `pr + max(x.e, x.sd()) + 4`, which for
this operand is 8_999_999_999_999_967. The port computed the limb target in
`i64` and got 1_285_714_285_714_283. The original computes it as a *double* and
then truncates it to **32 bits** — `| 0` is `ToInt32`, not a cast — so upstream's
target is **−1_354_212_501**. Both then run the same loop, whose guard is
`sd--`; a negative count is truthy, so neither implementation is bounded by it,
and what actually stops the original is its host.

*Second.* The port had no host to stop it. `MAX_ARRAY_LENGTH` existed already —
D-10 introduced it — but it was applied only in `plus` and `minus`, and it held
the wrong number.

**2³² − 1 is the wrong ceiling.** It is the largest value an array's `length` may
*hold*; it is not the largest array that can be *built*. A 64-bit V8 keeps a
dense array's elements in a backing store capped at one gigabyte of eight-byte
slots, so growing an array by assigning one index at a time — which is how the
original grows every digit array — stops at **2²⁷** and throws there. Four
billion elements below the specification.

The distinction is not academic, and it is not confined to the exponent limits:

```js
Decimal.set({ precision: 1e9 });   // the largest precision the library documents
new Decimal(1).div(3);
```

is `RangeError: Invalid array length` upstream, in 1.3 seconds. With the
specification's constant the port answered — a billion correct digits, in half
the time, and the wrong behaviour. Three lines, one documented setting, no
exponent games; the kind of divergence that is embarrassing to find late.

**Decision.** `MAX_ARRAY_LENGTH` becomes 2²⁷, checked at the one statement that
can breach it — `push_limb`, in `divide`'s quotient loop — rather than inferred
from the loop bound, which is the very thing that has gone wrong. And
`crate::to_int32` transcribes `ToInt32`, because Rust's `as i32` *saturates*
where JavaScript *wraps*: `1e16 as i32` is 2_147_483_647 and `1e16 | 0` is
1_874_919_424, and a saturating port of a wrapping expression has stopped
computing the same function.

**The instrument.** [`scripts/host-limits.js`](scripts/host-limits.js) measures
the ceiling the host enforces *right now*, reads `MAX_ARRAY_LENGTH` back out of
the Rust source, and fails if the two have drifted apart — a constant nobody
re-measures is a constant that has already gone stale. It then runs five cases
on both implementations in separate processes and compares the outcome
including the error's type, with one case that must **not** throw, because
without it a ceiling set far too low would pass every other case in the file.

**Consequence.** All five agree. The three cases that reach the ceiling through
a raised precision also show the configuration leak the port declines to
reproduce (D-11), and the script names that divergence rather than tolerating a
mismatch.

**What abandoning a calculation actually costs, which took three attempts.**
The original abandons by *throwing*: the stack unwinds and nothing further runs.
This crate has no exception, so `divide` returns and the routine that called it —
`sqrt`'s Newton iteration, a Taylor series, an argument reduction — keeps
running. Each attempt at a placeholder failed differently, and each failure is
worth recording because the next port to reproduce a host limit will meet them
in the same order:

1. **NaN.** `digits()` panics on a non-finite value, and with `panic = "unwind"`
   but no `catch_unwind` at the `extern "C"` boundary, a panic there aborts the
   process. Nine methods died. Strictly worse than the exception being
   reproduced — D-17's lesson, met again from the other side.
2. **Zero.** Finite, so no panic; but the next `x / r` is an infinity and the
   panic arrives one frame later instead.
3. **One, plus a short circuit in every primitive.** Finite, non-zero, and
   because every operation after the abandonment returns the *same* value, no
   convergence test comparing successive iterates can ever fire — so `atan`,
   `asin` and `sinh` stopped aborting and started hanging.
4. **One, plus the short circuit, plus a guard in every loop.** Nine loops in
   `roots`, `elementary`, `trig`, `inverse`, `power`, `fraction` and `radix` now
   break on the flag, exactly as `taylor_series` already broke on a partial sum
   that stopped being finite (D-16). Everything terminates.

And then a fifth thing, which the sweep found only after it was fixed: the flag
was consumed in `make`, and **`make` only handles results that are Decimals.**
`toBinary` returns a string, so at precision 939,524,081 the port rendered the
placeholder and answered `0b1` where the original raises. Every return path
consumes the flag now, not only the one that builds a value.

That last one is worth a sentence on its own. The sweep did not catch it at
first either: it fingerprinted results by calling `sd()` on them, so a string
result and a refusal both came back as the same `TypeError` from the harness and
compared equal. A check whose failure mode is *looking like a pass* is the one
kind worth re-reading.

**The general point, which is D-10's restated and sharpened.** Where the original
depends on its host refusing something, the port has to refuse it too — and it
has to refuse it *at the size the host actually refuses*, which is a property of
V8 and not of ECMA-262. D-10 got the mechanism right and the number wrong, and
the wrong number survived because the only case that had ever exercised it
overshot both ceilings by seven orders of magnitude. A limit that is only ever
tested far beyond itself is not tested.

**Where the two still differ, and why that is left.** Above the threshold neither
implementation can compute anything, and which refusal arrives first depends on
the order in which two different limits are met: the host's array ceiling, and
the library's own 1025-digit constants for π and ln 10. `ln`, `log` and `pow`
reach the constants first here and the array first upstream. Both refuse; the
words differ. Reproducing the order would mean reproducing where V8 runs out of
backing store inside a series, at precisions the original cannot serve at all
(BUG-007) — so the sweep requires termination and an outcome, counts the three
that differ, and says so rather than quietly relaxing the comparison the
threshold cases make.

---

### D-20 · `atan(±∞)` above the π table: the fourth null dereference, and the guard that skipped the error

**Context.** The bounded campaign reported a `PORT DEFECT` at 31,597 refereed
operations: `x1.atan()` where the oracle answered in 554 ms and the port did not
answer in 90 seconds.

What made it hard to see is that the receiver's configuration was not the
current one. `x1` belonged to a *previous* constructor — the sequence had called
`Decimal.clone()` and then `Decimal.set({ precision: 2 })` on the clone — and a
method reads its settings through `this.constructor`, exactly as the original
does. So the call that looked like `atan(∞)` at precision 2 was `atan(∞)` at
**precision 1130**.

**Why that matters.** `inverseTangent` opens with

```js
if (!x.isFinite()) {
  if (!x.s) return new Ctor(NaN);
  if (pr + 4 <= PI_PRECISION) { r = getPi(Ctor, pr + 4, rm).times(0.5); … return r; }
}
```

`PI_PRECISION` is 1025. Above it the guard fails, nothing returns, and control
**falls through to the series** with `x` infinite. Every term is infinite, no two
partial sums ever differ, and the convergence test is `r.d[j] !== void 0` on a
value whose `d` is null:

```
TypeError: Cannot read properties of null (reading '163')
```

The fourth instance of the family already reported as BUG-003, BUG-005 and
BUG-006. This port, which checks finiteness before indexing — the D-16 lesson —
did not crash and therefore never left the loop. The same non-answer in better
clothes, again.

**Decision.** Call `get_pi` unguarded in that branch. `±∞` is `±π/2` and nothing
else; if π is unavailable at the requested precision then the answer is
unavailable, and `get_pi` already says so with the library's own
`[DecimalError] Precision limit exceeded`.

This is not an invention. It is what every other member of the family already
does at these precisions, on *both* implementations — measured, not assumed:

| at precision 1130 | upstream | this port |
|---|---|---|
| `asin(1)`, `asin(-1)` | Precision limit exceeded | Precision limit exceeded |
| `acos(0)`, `acos(-1)` | Precision limit exceeded | Precision limit exceeded |
| `atan2(1, -1)` | Precision limit exceeded | Precision limit exceeded |
| `sin(1e9)` | Precision limit exceeded | Precision limit exceeded |
| `atan(1)`, `acos(0.5)` | answers | answers, identically |
| **`atan(±∞)`** | **TypeError** | **was: no answer at all** |

`atan` of an infinity was the single member that did not raise it, and only
because its guard skipped the call to `getPi` rather than letting the call fail.

**What is deliberately *not* changed.** The identical guard on the
`|x| == 1` branch stays. There the fall-through is not a defect: the series
converges for `|x| = 1`, and `atan(±1)` at precision 1130 returns the same
digits on both sides. Removing that guard too would turn a working computation
into an error, which is the opposite mistake.

**Consequence.** 90 seconds becomes 0 ms. The sixth deliberate divergence,
recognised by name in the harness so that it is counted rather than reported.
Reported upstream as BUG-008, and it is the fourth report in one family — which
is now the strongest argument in `docs/upstream/README.md` for the sweep it
suggests rather than for four separate patches.

**The lesson, which is not about `atan`.** D-16 taught the port to check
finiteness before indexing a digit array, and that check is what converted a
crash into a hang. A guard that makes the port survive a state the original
cannot survive is only half a fix: the other half is deciding what the surviving
code should *answer*, and "keep going" is never that answer.

---

### D-21 · The axis a conformance check does not vary is where the next defect lives

**Context.** The bounded campaign reported a divergence at 29,472 refereed
operations: `toDP(0, ROUND_UP)` on a value below `minE` answered
`1e+8999999999999559` where the oracle answered `0`.

The cause is a two-line asymmetry upstream. `round` and its neighbours are
written

```js
return finalise(new Ctor(x), x.e + 1, rm);
```

— the copy is made *inside* the call, so `x.e` beside it is still the
**receiver's** exponent, and the digit count and the value being rounded come
from different places. That is D-12, and the port reproduces it deliberately.
`toDecimalPlaces` is written differently:

```js
x = new Ctor(x);              // x is rebound
…
return finalise(x, dp + x.e + 1, rm);
```

Here `x.e` is the **clamped** exponent. Ten lines apart in the original,
opposite in effect, and distinguishable only when the clamp actually fires. The
port had transcribed the first form into both.

**Why nothing had caught it.** Two instruments should have. The original suite
never narrows the exponent limits after building an operand, so the whole family
is outside it — that is already D-12. But `scripts/clamp-conformance.js` exists
precisely to cover that family, it had been calling `toDP` since the day it was
written, and it was green.

It called `toDP` at the default `ROUND_HALF_UP`. A value the clamp crushed to
zero rounds to zero under every mode that rounds towards zero, so the default
hides *which* value was rounded. Only `ROUND_UP` and `ROUND_CEIL` — 2 of the 9
modes — make the difference observable, and the check varied operand and
exponent limits while holding the rounding mode constant.

**Decision.** Fix the transcription, and then fix the instrument, in that order,
because the second is the part that generalises. The check now varies four axes
instead of two:

| axis | before | after |
|---|---|---|
| operands | 6 | 6 |
| exponent-limit pairs | 4 | 4 |
| rounding mode | 1 (the default) | **9**, for the 10 methods that take one |
| operand position | receiver only | **receiver and argument** |
| methods | 43 | **67** |
| calls | 1,032 | **3,528** |

**Consequence — the mode axis found a second defect on its first run.**
`toFixed` had no clamping copy at all: upstream's
`finalise(new Ctor(x), dp + x.e + 1, rm)` had been transcribed as a plain clone.
`(1.5e-300).toFixed(2, ROUND_UP)` with `minE` at −100 gave `0.01` where the
original gives a fifty-seven-digit integer — upstream rounds a zero at 10⁻³⁰⁰
precision, the port rounded the operand that should not have survived. Invisible
under the seven modes that round towards zero, because `finalise` clamps its own
result on the way out and both sides then agreed on the answer for the wrong
reason.

`toExponential` and `toPrecision` carried the identical plain clone. Neither is
observable — their digit counts do not depend on the exponent, so the clamp
inside `finalise` covers for the missing one — and both were corrected anyway.
An agreement that holds because of an argument shape is not a property of the
function.

**The position axis found nothing, which is also a result.** `y = new Ctor(y)`
opens every binary method upstream, and no check had ever put a wide-built
extreme on the right-hand side of an operator: the existing entries build their
argument *after* narrowing the limits, so the argument was always ordinary.
Eleven new entries pass the extreme operand as the argument instead. All agree —
because `coerce`, at the Node-API boundary, re-judges every incoming Decimal
before the core sees it. That is the correct place for it, and it now calls
`ops::clamped_copy` rather than restating the rule, so the boundary and the core
cannot drift apart.

**The lesson.** A conformance check is a product of the axes it varies, and it
is silent about every axis it holds constant. This one was written to cover a
family that the original suite could not reach, and it inherited the suite's
blind spot on a different axis: the suite varies operands and holds the
configuration still; the check varied the configuration and held the *arguments*
still. Both defects lived in the axis that was constant. The question worth
asking of any such instrument is not "what does it cover" but **"what does it
hold still, and why is that safe?"** — and the honest answer here was that
nobody had asked.

### D-22 · The unwind backstop the build profile already promised

**Context.** `Cargo.toml` has said this since the first commit:

```toml
# Deliberately NOT panic = "abort". A panic inside a Node addon must not take
# the host process down; the N-API boundary catches unwinds and converts them
# into thrown JavaScript errors.
panic = "unwind"
```

The module documentation said the same thing in prose. Neither was true: no
callback caught anything, and `catch_unwind` appeared nowhere in the crate. A
panic in `decimal-core` aborted the Node process — no stack, no `catch`, no
exit code a caller could act on.

Nothing was known to panic. Every fallible path in the core returns a `Result`,
and its nine `expect`s assert invariants that `finalise` restores on every
exit. But "we believe it cannot happen" is the argument for keeping the
handler cheap, not for not having one, and a *documented* guarantee that does
not exist is worse than an absent one: a reader who greps for the mechanism and
finds prose has been told something false about the artifact.

**Decision.** Install it, at the registration tables rather than inside sixty
callback bodies, so that the guarantee is visible in one place and an entry
added later that lacks it does not read like the sixty that have it:

```rust
(&["absoluteValue\0", "abs\0"], Some(guarded!(m_abs))),
```

**The part that is worth writing down.** The first version of this was wrong in
a way that looked right, compiled, passed the entire suite, and would have
shipped a guarantee that did not hold. `guarded!` wrapped each callback while
the callbacks were themselves `unsafe extern "C" fn`. A panic then escapes an
`extern "C"` frame *before* it reaches the wrapper, and Rust aborts at that
frame. The negative control:

```
$ node -e "require('./probe.node'); new D(-3).abs()"     # panic injected in ops::abs
thread '<unnamed>' panicked at crates/decimal-core/src/ops.rs:30:5
exit=127                                                  # process gone, nothing caught
```

The fix is that the callbacks stop being `extern "C"` — they become plain
`unsafe fn`s, and the wrapper is the only C boundary in the crate, so the
unwind has somewhere to unwind *to*:

```
$ node -e "…"                                             # same injected panic
caught: Error: decimal-rs internal error: negative-control panic from ops::abs
still alive; 1+2 = 3
exit=0
```

**Consequence.** A panic is now a catchable `Error` and the process survives it.
The message says `decimal-rs internal error` and deliberately does *not* wear
the library's `[DecimalError]` prefix: a `try`/`catch` written for the
library's own errors must not silently swallow a bug in the port.
`AssertUnwindSafe` is the honest annotation rather than a workaround — a panic
part-way through a configuration change can leave that constructor's
`precision` inconsistent, and an inconsistent `precision` is recoverable and
observable where an aborted process is neither.

**The lesson, which is the same one as D-21.** A guard nobody has watched fail
is a claim, not a mechanism. This one was tested exactly the way the fuzz
harness tests its own comparator — inject the fault, require the machinery to
catch it, then revert — and the first attempt failed that test. Two of this
port's instruments have now been wrong in the direction that reads as green.
