//! Constructor configuration, and the transient state that surrounds a
//! calculation.
//!
//! ## Why there are two structs here and not one
//!
//! decimal.js keeps two quite different kinds of mutable state, and it is easy
//! to miss that they are different because JavaScript stores both of them in
//! the same place — properties reachable from the closure.
//!
//! The first kind is *configuration*: `precision`, `rounding`, and friends.
//! These live on the constructor function object, they are set by the user
//! through `Decimal.config`, and `Decimal.clone()` produces a second
//! constructor with an independent copy. That is [`Config`].
//!
//! The second kind is *scratch state for a calculation in progress*:
//! `external`, `inexact`, `quadrant`. These are module-level variables in the
//! original, shared by every clone, and they exist only to carry a flag
//! across a few stack frames — `external` suppresses overflow clamping while
//! an intermediate result is being built, `inexact` reports back from the
//! division routine, `quadrant` from the argument reduction in the
//! trigonometric functions. That is the rest of [`Ctx`].
//!
//! Conflating the two would be a real bug and not a stylistic one: a clone
//! must get its own `precision`, but must *not* get its own half-finished
//! `external` flag, because the flag is only ever set and cleared within a
//! single operation. Keeping the scratch state in the context passed down the
//! call stack, rather than in a global, means a second thread cannot observe
//! another thread's half-finished calculation — which is stricter than the
//! original and cannot break a program that the original would have accepted.

use crate::{EXP_LIMIT, MAX_DIGITS};

/// The nine rounding modes, by the numeric values the original uses.
///
/// They are kept as plain integers rather than promoted to an enum because
/// the rounding decision in [`crate::round::finalise`] is written as
/// arithmetic on them — `rm < 4` separates the directed modes from the
/// half-way modes, and `rm == if negative { 3 } else { 2 }` folds CEIL and
/// FLOOR together. Transcribing those tests literally is worth more than the
/// type safety of an enum would be, and the tests exercise every mode.
pub mod rounding {
    /// Away from zero.
    pub const UP: u8 = 0;
    /// Towards zero.
    pub const DOWN: u8 = 1;
    /// Towards +Infinity.
    pub const CEIL: u8 = 2;
    /// Towards −Infinity.
    pub const FLOOR: u8 = 3;
    /// To nearest; ties away from zero.
    pub const HALF_UP: u8 = 4;
    /// To nearest; ties towards zero.
    pub const HALF_DOWN: u8 = 5;
    /// To nearest; ties to the even neighbour.
    pub const HALF_EVEN: u8 = 6;
    /// To nearest; ties towards +Infinity.
    pub const HALF_CEIL: u8 = 7;
    /// To nearest; ties towards −Infinity.
    pub const HALF_FLOOR: u8 = 8;
}

/// The settings of one `Decimal` constructor.
///
/// `Decimal.clone()` copies this; the two constructors then diverge, and the
/// original's `clone` and `config` test modules check precisely that they do
/// not interfere with one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Significant digits kept in the result of a calculation, 1 to 1e9.
    pub precision: i64,
    /// Which of the nine [`rounding`] modes applies at `precision`.
    pub rounding: u8,
    /// The rounding mode used to form the quotient in a modulo operation.
    /// Only 0, 1, 3, 6 and 9 give useful results; all are permitted.
    pub modulo: u8,
    /// At and below this exponent, `toString` switches to exponential form.
    pub to_exp_neg: i64,
    /// At and above this exponent, `toString` switches to exponential form.
    pub to_exp_pos: i64,
    /// Below this exponent a result underflows to zero.
    pub min_e: i64,
    /// Above this exponent a result overflows to Infinity.
    pub max_e: i64,
    /// Whether `random` should draw from a cryptographic source.
    pub crypto: bool,
}

impl Default for Config {
    /// The defaults of a freshly-required `Decimal`, as listed in the
    /// "EDITABLE DEFAULTS" block at the top of the original.
    fn default() -> Self {
        Config {
            precision: 20,
            rounding: rounding::HALF_UP,
            modulo: rounding::DOWN,
            to_exp_neg: -7,
            to_exp_pos: 21,
            min_e: -EXP_LIMIT,
            max_e: EXP_LIMIT,
            crypto: false,
        }
    }
}

impl Config {
    /// The permitted range of each numeric setting, as a `(min, max)` pair.
    ///
    /// These bounds are part of the observable behaviour: `Decimal.config`
    /// throws `[DecimalError] Invalid argument: …` for anything outside them,
    /// and the `config` test module walks the boundary of every one.
    pub const PRECISION_RANGE: (i64, i64) = (1, MAX_DIGITS);
    /// Permitted range of `rounding`.
    pub const ROUNDING_RANGE: (i64, i64) = (0, 8);
    /// Permitted range of `modulo`.
    pub const MODULO_RANGE: (i64, i64) = (0, 9);
    /// Permitted range of `toExpNeg`.
    pub const TO_EXP_NEG_RANGE: (i64, i64) = (-EXP_LIMIT, 0);
    /// Permitted range of `toExpPos`.
    pub const TO_EXP_POS_RANGE: (i64, i64) = (0, EXP_LIMIT);
    /// Permitted range of `minE`.
    pub const MIN_E_RANGE: (i64, i64) = (-EXP_LIMIT, 0);
    /// Permitted range of `maxE`.
    pub const MAX_E_RANGE: (i64, i64) = (0, EXP_LIMIT);
}

/// A configuration plus the scratch state of a calculation in progress.
///
/// Every routine that can round takes `&mut Ctx`. That is more threading of
/// state than the original needs, because the original reaches these values
/// through a closure; it is the price of not having a mutable global, and it
/// buys the guarantee that no calculation can observe another's scratch flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ctx {
    /// The owning constructor's settings.
    pub cfg: Config,
    /// When false, [`crate::round::finalise`] skips the overflow and underflow
    /// clamps.
    ///
    /// The original clears this around the construction of intermediate values
    /// whose exponents may legitimately stray outside `[minE, maxE]` before
    /// the final result is formed — most visibly in `parseOther`, where a hex
    /// float is built by dividing two values either of which may overflow, and
    /// in the transcendental functions, which compute at raised precision.
    /// Without it, `new Decimal('0x1p-1074')` would underflow to zero halfway
    /// through being parsed.
    pub external: bool,
    /// Set by the division routine when the quotient was truncated rather than
    /// exact. Consumed by base conversion, which needs to know whether a
    /// trailing digit was lost.
    pub inexact: bool,
    /// Which quadrant the argument of a trigonometric function landed in after
    /// reduction modulo π/2. Written by `toLessThanHalfPi`, read by `sin`,
    /// `cos` and `tan`.
    pub quadrant: u8,
    /// Set when an operation needed a digit array longer than JavaScript
    /// permits, i.e. longer than [`crate::MAX_ARRAY_LENGTH`].
    ///
    /// # Why a flag rather than a `Result`
    ///
    /// The condition arises inside `plus` and `minus`, which are infallible in
    /// this crate and are called from everywhere; threading a `Result` out of
    /// them would change several hundred call sites to carry an error that
    /// cannot occur in any of them.
    ///
    /// It is not a hypothetical. `asinh` raises the working precision to
    /// `pr + 2·max(|e|, sd) + 6`, so an argument near the exponent ceiling
    /// asks for a precision around 1.8 × 10¹⁶ — and the alignment inside the
    /// following `plus` then wants to prepend 2.6 × 10¹⁵ zero limbs. The
    /// original attempts it too and JavaScript stops it: `RangeError: Invalid
    /// array length`, catchable, the calculation abandoned. Rust's `Vec` has no
    /// such ceiling, so the port instead asked for ten petabytes and the
    /// allocator aborted the process — a strictly worse outcome than the
    /// original's, and one that no amount of fidelity elsewhere makes up for.
    ///
    /// So the arithmetic sets this and returns early, and the boundary — the
    /// Node binding, or any embedder of this crate — turns it into a thrown
    /// error. Found by the differential fuzzer; see DECISIONS.md D-10.
    pub array_limit_exceeded: bool,
}

impl Default for Ctx {
    fn default() -> Self {
        Ctx::new(Config::default())
    }
}

impl Ctx {
    /// A context for a constructor configured as `cfg`, with clean scratch
    /// state.
    pub fn new(cfg: Config) -> Self {
        Ctx {
            cfg,
            external: true,
            inexact: false,
            quadrant: 0,
            array_limit_exceeded: false,
        }
    }

    /// Report and clear [`Ctx::array_limit_exceeded`].
    ///
    /// Callers at an API boundary use this to decide whether the value they are
    /// holding is a real answer or the placeholder left behind by an abandoned
    /// calculation.
    pub fn take_array_limit_exceeded(&mut self) -> bool {
        core::mem::replace(&mut self.array_limit_exceeded, false)
    }

    /// Run `body` with the overflow and underflow clamps suppressed, and turn
    /// them back **on** afterwards — set, not restored.
    ///
    /// # Why set rather than restore
    ///
    /// The original writes this by hand, eighteen times, always as
    /// `external = false; … external = true;`. An earlier version of this
    /// helper saved the previous value and put it back, on the reasoning that
    /// restoring makes nesting harmless and costs nothing.
    ///
    /// It costs one behaviour, and the nesting is real. `acosh` suppresses the
    /// clamps and then calls `sqrt`, which suppresses them again and sets them
    /// back on — so the `plus` that follows the square root in
    /// `x.times(x).minus(1).sqrt().plus(x)` runs *with* clamping, and
    /// `acosh(1.5e300)` with `maxE` at 100 is Infinity rather than 691.87.
    /// `parseOther` calls `intPow` and gets the same treatment: with restoring
    /// semantics `new Decimal('0x1.8p3')` at precision 1 came out as 20 instead
    /// of 12. Both were found by the differential campaign, months of reading
    /// after the original was transcribed.
    ///
    /// So the sloppiness is load-bearing, and a port that tidies it up is a
    /// port of a different library. `scripts/clamp-conformance.js` checks the
    /// whole family against the oracle in one pass.
    ///
    /// # The error path
    ///
    /// The original is *not* careful here, and that is one of its defects: a
    /// throw between the two assignments leaves the clamps disabled for the
    /// life of the process (BUG-002, BUG-005). `getLn10` alone gets it right,
    /// with a comment saying so deliberately. This helper restores on the error
    /// path structurally, because `body` returning early cannot skip the line
    /// below — a deliberate divergence, recorded as D-11 and D-16.
    pub fn without_clamping<T>(&mut self, body: impl FnOnce(&mut Ctx) -> T) -> T {
        self.external = false;
        let result = body(self);
        self.external = true;
        result
    }
}
