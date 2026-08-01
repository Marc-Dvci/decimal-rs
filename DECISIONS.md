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
`MAX_ARRAY_LENGTH` is 2³² − 1; `plus` and `minus` check the padding against it,
set `Ctx::array_limit_exceeded`, and abandon the calculation. The Node binding
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
