//! Trigonometric and hyperbolic functions.
//!
//! # Three layers
//!
//! [`taylor_series`] sums the series shared by `sin`, `cos`, `sinh` and
//! `cosh`; the only difference between the circular and hyperbolic cases is
//! whether successive terms alternate in sign, which is the `is_hyperbolic`
//! flag. It stops when two successive partial sums agree limb-for-limb down to
//! position `⌈precision/7⌉`, and truncates to that many limbs on the way out.
//!
//! [`sine`] and [`cosine`] wrap it in an argument reduction, because the
//! series converges usefully only for small arguments. `cos` uses
//! `cos(x) = 8(cos⁴(x/4) − cos²(x/4)) + 1` applied `k` times; `sin` uses
//! `sin(x) = sin(x/5)(5 + sin²(x/5)(16sin²(x/5) − 20))`. How many times is an
//! estimate from the operand's limb count — `⌈len/3⌉` for cosine, `1.4√len`
//! capped at 16 for sine — and those estimates are copied rather than
//! re-derived, because they decide how many guard digits the result carries.
//!
//! [`to_less_than_half_pi`] reduces the argument modulo π/2 and records which
//! quadrant it came from in `ctx.quadrant`, which the callers consult to decide
//! the sign of the answer.
//!
//! # A defect reproduced on purpose
//!
//! `tan` is computed as
//!
//! ```text
//!     tan(x) = sin(x) / √(1 − sin²(x))
//! ```
//!
//! with ten guard digits. Near an odd multiple of π/2, `sin(x)` approaches 1
//! and `1 − sin²(x)` is a subtraction of two nearly equal quantities, so it
//! loses roughly two digits for every decade of proximity to the pole. Ten
//! guard digits are exhausted about 10⁻⁶ away from it, and the result then
//! saturates at `10^(wp/2)/√2` regardless of how much closer the argument
//! actually is — so `tan` of a value very near π/2 returns a number around
//! 7×10¹¹ where the true answer is around 3×10¹⁹, and for a longer argument
//! returns `Infinity` for a finite input.
//!
//! The guard is fixed at ten, so raising `precision` does not move the onset.
//! The library's own `cos` is accurate in that region, so `sin/cos` would fix
//! it.
//!
//! **This port reproduces the defect exactly.** Correcting it here would make
//! the port disagree with the original — which is the one thing it must not do
//! — and the original's own test suite would fail. The finding was written up
//! and reported upstream instead; see `DECISIONS.md`.

use crate::arith::{add, compare, divide, mul, sub};
use crate::config::rounding;
use crate::constants::{PI, PI_PRECISION};
use crate::round::finalise;
use crate::{Ctx, Decimal, Error, Result, Sign, LOG_BASE};

/// π to `sd` digits, rounded with mode `rm`.
pub fn get_pi(ctx: &mut Ctx, sd: i64, rm: u8) -> Result<Decimal> {
    if sd > PI_PRECISION {
        return Err(Error::PrecisionLimitExceeded);
    }
    let mut value = crate::parse::parse_decimal(ctx, Sign::Pos, PI);
    finalise(ctx, &mut value, Some(sd), rm, true);
    Ok(value)
}

/// `b^e` in double arithmetic, for the small positive exponents the argument
/// reductions use. The original's `tinyPow`.
fn tiny_pow(b: f64, e: i64) -> f64 {
    let mut n = b;
    let mut remaining = e;
    while remaining > 1 {
        n *= b;
        remaining -= 1;
    }
    n
}

/// `|x|`.
fn abs(x: &Decimal) -> Decimal {
    let mut out = x.clone();
    if out.s.is_negative() {
        out.s = Sign::Pos;
    }
    out
}

/// Whether the integer `x` is odd — the original's `isOdd`, which looks only
/// at the last limb.
fn is_odd(x: &Decimal) -> bool {
    x.digits().last().is_some_and(|last| last & 1 == 1)
}

/// `x <= y`.
fn lte(x: &Decimal, y: &Decimal) -> bool {
    matches!(
        compare(x, y),
        Some(core::cmp::Ordering::Less) | Some(core::cmp::Ordering::Equal)
    )
}

/// `x / y` truncated towards zero, then rounded to the working precision —
/// the original's `dividedToIntegerBy`.
pub fn div_to_int(ctx: &mut Ctx, x: &Decimal, y: &Decimal) -> Decimal {
    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
    let mut q = divide(ctx, x, y, Some(0), rounding::DOWN, true, None);
    finalise(ctx, &mut q, Some(pr), rm, false);
    q
}

/// A decimal from a small integer constant.
fn int(n: i32) -> Decimal {
    Decimal::from_i32(n)
}

/// The series denominator `a·b`, built the way the original builds it.
///
/// The original writes `new Ctor(n++ * n++)`, and `n` there is a JavaScript
/// number — a double. So the product is exact while it stays under 2⁵³ and
/// *rounds* above it. Both halves are reproduced: an `i64` below the boundary,
/// and a deliberate trip through an `f64` above it, so that the port rounds
/// where the original rounds.
///
/// The upper branch needs about 94 million series terms to reach and so cannot
/// be observed before a run is abandoned. It is here anyway, because the
/// alternative is a silent difference whose unreachability is an argument
/// nobody wrote down.
fn series_denominator(ctx: &Ctx, a: i64, b: i64) -> Decimal {
    let product = a * b;
    if product.unsigned_abs() <= crate::MAX_SAFE_INTEGER as u64 {
        Decimal::from_integer(product)
    } else {
        crate::parse::parse_decimal(
            ctx,
            Sign::Pos,
            &crate::format::number_to_string(product as f64),
        )
    }
}

/// Sum the Taylor series for `cos`, `cosh`, `sin` or `sinh`.
///
/// `n` seeds the factorial denominators — 1 for the cosine family, 2 for the
/// sine family — and `is_hyperbolic` selects addition rather than subtraction
/// of successive terms.
///
/// Two terms are consumed per iteration, so the denominators advance by
/// `n(n+1)` twice. Convergence is tested by comparing the current partial sum
/// with the previous one limb by limb from position `k = ⌈precision/7⌉`
/// downwards; agreement all the way to limb 0 ends the loop.
pub fn taylor_series(
    ctx: &mut Ctx,
    mut n: i64,
    x: &Decimal,
    y: &Decimal,
    is_hyperbolic: bool,
) -> Decimal {
    let pr = ctx.cfg.precision;
    let k = if pr <= 0 {
        0
    } else {
        (pr + LOG_BASE - 1) / LOG_BASE
    };

    // Set, not saved and restored. The original clears the flag on entry and
    // ends with a bare `external = true`, so a caller that had suppressed
    // clamping does not get it back — the same shape as `int_pow`, and
    // observable for the same reason. See the note there.
    ctx.external = false;

    let x2 = mul(ctx, x, x);
    let mut y = y.clone();
    let mut u = y.clone();
    let mut t;

    loop {
        // Two terms per iteration. `n++ * n++` in the original evaluates the
        // pre-increment value twice in succession, so the denominator is
        // n(n+1) and n advances by two.
        let a = n;
        n += 1;
        let b = n;
        n += 1;
        let numerator = mul(ctx, &u, &x2);
        let denominator = series_denominator(ctx, a, b);
        t = divide(
            ctx,
            &numerator,
            &denominator,
            Some(pr),
            rounding::DOWN,
            false,
            None,
        );

        u = if is_hyperbolic {
            add(ctx, &y, &t)
        } else {
            sub(ctx, &y, &t)
        };

        let a = n;
        n += 1;
        let b = n;
        n += 1;
        let numerator = mul(ctx, &t, &x2);
        let denominator = series_denominator(ctx, a, b);
        y = divide(
            ctx,
            &numerator,
            &denominator,
            Some(pr),
            rounding::DOWN,
            false,
            None,
        );

        t = add(ctx, &u, &y);

        // Overflowed. The partial sum is ±Infinity; every remaining term is
        // added to an infinity, so no iteration can bring it back and this is
        // the answer, such as it is.
        //
        // The original has no such test. Its next line is
        // `if (t.d[k] !== void 0)`, and `t.d` is null here, so it raises
        //
        //     TypeError: Cannot read properties of null (reading '30')
        //
        // from inside its own `external = false` — which nothing then restores,
        // so the constructor stops clamping to `minE`/`maxE` for the remaining
        // life of the process. Reported as BUG-005; reachable in four lines,
        // by building a value while `maxE` is wide, narrowing `maxE` below its
        // exponent, and calling `sinh`.
        //
        // Not reproduced, on the same test as D-11 and D-13: it is a way to
        // break the library rather than a way to compute a number. D-16.
        // Without this line the port has the same non-answer in worse clothes —
        // it does not crash, so it simply never leaves this loop.
        if !t.is_finite() {
            break;
        }

        // Converged? Only once the sum actually has a limb at position k.
        if (t.digits().len() as i64) > k {
            let mut j = k;
            let converged = loop {
                let tj = t.digits().get(j as usize).copied();
                let uj = u.digits().get(j as usize).copied();
                if tj != uj {
                    break false;
                }
                if j == 0 {
                    break true;
                }
                j -= 1;
            };
            if converged {
                break;
            }
        }

        // Rotate: the previous `y` becomes the new `u`, and `t` becomes `y`.
        u = y.clone();
        y = t.clone();
    }

    ctx.external = true;

    // Trim to the working width; the digits beyond it are not meaningful.
    if let Some(d) = t.d.as_mut() {
        d.truncate((k + 1) as usize);
    }
    t.strip_trailing_zero_limbs();
    t
}

/// What `sine` and `cosine` answer when the argument reduction has overflowed.
///
/// `to_less_than_half_pi` subtracts a whole multiple of π from its operand, and
/// forms that multiple with the exponent clamps in force. Above `maxE` the
/// multiple is Infinity, so the reduced argument is ∓Infinity and no series can
/// be summed on it. `x.cos()` with `maxE` at 104 and `x` around 1e809 gets
/// there in one call.
///
/// The original does not test for it. `cosine` reads `x.d.length` on its first
/// working line and `sine` on its very first, so both raise
///
/// ```text
/// TypeError: Cannot read properties of null (reading 'length')
/// ```
///
/// which is BUG-006 — the same shape as BUG-003 and BUG-005, in a third place.
/// This port panicked instead, which is worse: a Rust panic unwinding across
/// the Node-API boundary. D-17 declines both, and answers with the rule the
/// original's *own first line* applies to a non-finite argument,
/// `if (!x.d) return new Ctor(NaN)`. The reduction produced no number, so
/// there is no angle to take a sine of.
fn non_finite_after_reduction() -> Decimal {
    Decimal::nan()
}

/// `cos(x)` for an argument already reduced below π/2.
pub fn cosine(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    if x.is_zero() {
        return x.clone();
    }
    if !x.is_finite() {
        return non_finite_after_reduction();
    }

    // Estimate how many times to apply cos(x) = 8(cos⁴(x/4) − cos²(x/4)) + 1.
    let len = x.digits().len() as i64;
    let (k, scale) = if len < 32 {
        let k = (len + 2) / 3; // ceil(len / 3)
        (k, crate::format::number_to_string(1.0 / tiny_pow(4.0, k)))
    } else {
        (16, "2.3283064365386962890625e-10".to_string())
    };

    ctx.cfg.precision += k;

    let scale = crate::parse::parse_decimal(ctx, Sign::Pos, &scale);
    let reduced = mul(ctx, x, &scale);
    let one = int(1);
    let mut x = taylor_series(ctx, 1, &reduced, &one, false);

    // Reverse the reduction.
    let eight = int(8);
    for _ in 0..k {
        let cos2 = mul(ctx, &x, &x);
        let cos4 = mul(ctx, &cos2, &cos2);
        let difference = sub(ctx, &cos4, &cos2);
        let scaled = mul(ctx, &difference, &eight);
        x = add(ctx, &scaled, &one);
    }

    ctx.cfg.precision -= k;
    x
}

/// `sin(x)` for an argument already reduced below π/2.
pub fn sine(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    if !x.is_finite() {
        return non_finite_after_reduction();
    }
    let len = x.digits().len() as i64;

    if len < 3 {
        return if x.is_zero() {
            x.clone()
        } else {
            taylor_series(ctx, 2, x, x, false)
        };
    }

    // Estimate how many times to apply the quintuple-angle reduction.
    let k = {
        let estimate = 1.4 * (len as f64).sqrt();
        if estimate > 16.0 {
            16
        } else {
            estimate as i64
        }
    };

    let scale = crate::parse::parse_decimal(
        ctx,
        Sign::Pos,
        &crate::format::number_to_string(1.0 / tiny_pow(5.0, k)),
    );
    let reduced = mul(ctx, x, &scale);
    let mut x = taylor_series(ctx, 2, &reduced, &reduced, false);

    // Reverse the reduction:
    //     sin(5t) = sin(t)(5 + sin²(t)(16sin²(t) − 20))
    let (five, sixteen, twenty) = (int(5), int(16), int(20));
    for _ in 0..k {
        let sin2 = mul(ctx, &x, &x);
        let inner = {
            let a = mul(ctx, &sixteen, &sin2);
            let b = sub(ctx, &a, &twenty);
            mul(ctx, &sin2, &b)
        };
        let factor = add(ctx, &five, &inner);
        x = mul(ctx, &x, &factor);
    }

    x
}

/// Reduce `|x|` to at most π/2, recording the originating quadrant in
/// `ctx.quadrant`.
pub fn to_less_than_half_pi(ctx: &mut Ctx, x: &Decimal) -> Result<Decimal> {
    let is_negative = x.s.is_negative();
    let pi = get_pi(ctx, ctx.cfg.precision, rounding::DOWN)?;
    let half = crate::parse::parse_decimal(ctx, Sign::Pos, "0.5");
    let half_pi = mul(ctx, &pi, &half);

    let x = abs(x);

    if lte(&x, &half_pi) {
        ctx.quadrant = if is_negative { 4 } else { 1 };
        return Ok(x);
    }

    let t = div_to_int(ctx, &x, &pi);

    // The multiple of π to subtract is itself measured against `maxE`, and
    // above it there is no multiple — only Infinity. The reduction cannot be
    // performed and there is no quadrant to record. See
    // `non_finite_after_reduction`: the original reaches `isOdd(t)`, reads
    // `t.d.length` with `t.d` null, and raises. BUG-006 / D-17.
    if !t.is_finite() {
        return Ok(Decimal::nan());
    }

    let x = if t.is_zero() {
        ctx.quadrant = if is_negative { 3 } else { 2 };
        x
    } else {
        let product = mul(ctx, &t, &pi);
        let reduced = sub(ctx, &x, &product);

        // Now 0 <= reduced < π.
        if lte(&reduced, &half_pi) {
            ctx.quadrant = if is_odd(&t) {
                if is_negative {
                    2
                } else {
                    3
                }
            } else if is_negative {
                4
            } else {
                1
            };
            return Ok(reduced);
        }

        ctx.quadrant = if is_odd(&t) {
            if is_negative {
                1
            } else {
                4
            }
        } else if is_negative {
            3
        } else {
            2
        };
        reduced
    };

    let shifted = sub(ctx, &x, &pi);
    Ok(abs(&shifted))
}

/// The working precision the circular functions raise to before reducing.
fn working_precision(x: &Decimal, extra: i64) -> i64 {
    x.e.max(x.significant_digits()) + extra
}

/// `sin(x)`.
pub fn sin(ctx: &mut Ctx, x: &Decimal) -> Result<Decimal> {
    if !x.is_finite() {
        return Ok(Decimal::nan());
    }
    if x.is_zero() {
        return Ok(x.clone());
    }

    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
    ctx.cfg.precision = pr + working_precision(x, LOG_BASE);
    ctx.cfg.rounding = rounding::DOWN;

    let reduced = to_less_than_half_pi(ctx, x)?;
    let mut value = sine(ctx, &reduced);

    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;

    if ctx.quadrant > 2 {
        value.s = value.s.negated();
    }
    finalise(ctx, &mut value, Some(pr), rm, true);
    Ok(value)
}

/// `cos(x)`.
pub fn cos(ctx: &mut Ctx, x: &Decimal) -> Result<Decimal> {
    if x.d.is_none() {
        return Ok(Decimal::nan());
    }
    // cos(0) = cos(-0) = 1
    if x.digits()[0] == 0 {
        return Ok(int(1));
    }

    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
    ctx.cfg.precision = pr + working_precision(x, LOG_BASE);
    ctx.cfg.rounding = rounding::DOWN;

    let reduced = to_less_than_half_pi(ctx, x)?;
    let mut value = cosine(ctx, &reduced);

    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;

    if ctx.quadrant == 2 || ctx.quadrant == 3 {
        value.s = value.s.negated();
    }
    finalise(ctx, &mut value, Some(pr), rm, true);
    Ok(value)
}

/// `tan(x)`, as `sin(x) / √(1 − sin²(x))`.
///
/// See the module documentation: this formula loses all significance near an
/// odd multiple of π/2, and the loss is reproduced here deliberately.
pub fn tan(ctx: &mut Ctx, x: &Decimal) -> Result<Decimal> {
    if !x.is_finite() {
        return Ok(Decimal::nan());
    }
    if x.is_zero() {
        return Ok(x.clone());
    }

    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
    ctx.cfg.precision = pr + 10;
    ctx.cfg.rounding = rounding::DOWN;

    let mut s = sin(ctx, x)?;
    s.s = Sign::Pos;

    let one = int(1);
    let square = mul(ctx, &s, &s);
    let complement = sub(ctx, &one, &square);
    let root = crate::roots::sqrt(ctx, &complement);
    let mut value = divide(ctx, &s, &root, Some(pr + 10), rounding::UP, false, None);

    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;

    if ctx.quadrant == 2 || ctx.quadrant == 4 {
        value.s = value.s.negated();
    }
    finalise(ctx, &mut value, Some(pr), rm, true);
    Ok(value)
}

/// `sinh(x)`.
pub fn sinh(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    if !x.is_finite() || x.is_zero() {
        return x.clone();
    }

    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
    ctx.cfg.precision = pr + working_precision(x, 4);
    ctx.cfg.rounding = rounding::DOWN;

    let len = x.digits().len() as i64;
    let mut value = if len < 3 {
        taylor_series(ctx, 2, x, x, true)
    } else {
        let k = {
            let estimate = 1.4 * (len as f64).sqrt();
            if estimate > 16.0 {
                16
            } else {
                estimate as i64
            }
        };
        let scale = crate::parse::parse_decimal(
            ctx,
            Sign::Pos,
            &crate::format::number_to_string(1.0 / tiny_pow(5.0, k)),
        );
        let reduced = mul(ctx, x, &scale);
        let mut value = taylor_series(ctx, 2, &reduced, &reduced, true);

        // sinh(5t) = sinh(t)(5 + sinh²(t)(16sinh²(t) + 20)) — the same shape as
        // the circular reduction with one sign flipped.
        let (five, sixteen, twenty) = (int(5), int(16), int(20));
        for _ in 0..k {
            let sinh2 = mul(ctx, &value, &value);
            let inner = {
                let a = mul(ctx, &sixteen, &sinh2);
                let b = add(ctx, &a, &twenty);
                mul(ctx, &sinh2, &b)
            };
            let factor = add(ctx, &five, &inner);
            value = mul(ctx, &value, &factor);
        }
        value
    };

    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;
    finalise(ctx, &mut value, Some(pr), rm, true);
    value
}

/// `cosh(x)`.
pub fn cosh(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    if !x.is_finite() {
        return if x.is_nan() {
            Decimal::nan()
        } else {
            Decimal::infinity(Sign::Pos)
        };
    }
    if x.is_zero() {
        return int(1);
    }

    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
    ctx.cfg.precision = pr + working_precision(x, 4);
    ctx.cfg.rounding = rounding::DOWN;

    let len = x.digits().len() as i64;
    let (k, scale) = if len < 32 {
        let k = (len + 2) / 3;
        (k, crate::format::number_to_string(1.0 / tiny_pow(4.0, k)))
    } else {
        (16, "2.3283064365386962890625e-10".to_string())
    };

    let scale = crate::parse::parse_decimal(ctx, Sign::Pos, &scale);
    let reduced = mul(ctx, x, &scale);
    let one = int(1);
    let mut value = taylor_series(ctx, 1, &reduced, &one, true);

    // cosh(x) = 1 − cosh²(x/4)(8 − 8cosh²(x/4))
    let eight = int(8);
    for _ in 0..k {
        let cosh2 = mul(ctx, &value, &value);
        let inner = {
            let a = mul(ctx, &cosh2, &eight);
            sub(ctx, &eight, &a)
        };
        let product = mul(ctx, &cosh2, &inner);
        value = sub(ctx, &one, &product);
    }

    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;
    finalise(ctx, &mut value, Some(pr), rm, true);
    value
}

/// `tanh(x)`, as `sinh(x) / cosh(x)`.
pub fn tanh(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    if !x.is_finite() {
        return if x.is_nan() {
            Decimal::nan()
        } else {
            // ±1 for ±Infinity.
            let mut one = int(1);
            one.s = x.s;
            one
        };
    }
    if x.is_zero() {
        return x.clone();
    }

    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
    ctx.cfg.precision = pr + 7;
    ctx.cfg.rounding = rounding::DOWN;

    // Both are computed at the raised precision; the *division* then happens at
    // the restored one, because the original writes the restoration into the
    // argument list of `divide` and JavaScript evaluates arguments left to
    // right.
    let numerator = sinh(ctx, x);
    let denominator = cosh(ctx, x);

    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;

    divide(ctx, &numerator, &denominator, Some(pr), rm, false, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::to_string;
    use crate::parse::parse_decimal;

    fn d(text: &str) -> Decimal {
        let ctx = Ctx::default();
        if let Some(rest) = text.strip_prefix('-') {
            parse_decimal(&ctx, Sign::Neg, rest)
        } else {
            parse_decimal(&ctx, Sign::Pos, text)
        }
    }

    /// An argument reduction that overflows answers rather than crashing.
    ///
    /// `to_less_than_half_pi` subtracts ⌊|x|/π⌋·π, and forms that multiple with
    /// the exponent clamps in force — so above `maxE` the multiple is Infinity
    /// and there is nothing to reduce. The original walks straight into it:
    /// `isOdd(t)` reads `t.d.length`, `cosine` reads `x.d.length`, `sine` reads
    /// it on its first line. All three raise `TypeError: Cannot read properties
    /// of null`. BUG-006; D-17 is the decision to answer NaN instead, which is
    /// the rule the original's own first line applies to a non-finite argument.
    ///
    /// This port previously panicked here, which is worse than the TypeError:
    /// a Rust panic unwinding across the Node-API boundary.
    #[test]
    fn an_overflowing_argument_reduction_answers_nan() {
        for (name, f) in [
            ("sin", sin as fn(&mut Ctx, &Decimal) -> Result<Decimal>),
            ("cos", cos),
            ("tan", tan),
        ] {
            let mut ctx = Ctx::default();
            let x = d("-4.9481810070120303e809");

            ctx.cfg.precision = 20;
            ctx.cfg.rounding = 7;
            ctx.cfg.max_e = 104;

            let value = f(&mut ctx, &x).expect("no error is raised");
            assert!(
                value.is_nan(),
                "{name} of a value whose reduction overflows should be NaN, got {}",
                to_string(&value, &ctx.cfg)
            );
        }
    }

    /// A series that overflows must terminate, and must not leave clamping off.
    ///
    /// The operand is built while `maxE` is wide and the hyperbolic is taken
    /// after `maxE` has been narrowed below its exponent, so the very first
    /// argument reduction produces ±Infinity and the series is summing an
    /// infinity from its first term.
    ///
    /// Upstream raises `TypeError: Cannot read properties of null` here, from
    /// inside its own `external = false`, and never restores the flag — so
    /// afterwards nothing clamps. That is BUG-005, and D-16 is the decision not
    /// to reproduce it. Both halves are asserted: an answer, and a context that
    /// still enforces its own limits.
    #[test]
    fn an_overflowing_series_terminates_and_leaves_clamping_on() {
        for (name, f) in [
            ("sinh", sinh as fn(&mut Ctx, &Decimal) -> Decimal),
            ("cosh", cosh),
            ("tanh", tanh),
        ] {
            let mut ctx = Ctx::default();
            let x = d("5.879302975574934568e100");

            ctx.cfg.precision = 100;
            ctx.cfg.max_e = 73;
            let value = f(&mut ctx, &x);

            assert!(
                !value.is_finite(),
                "{name} of a value above maxE should be non-finite, got {}",
                to_string(&value, &ctx.cfg)
            );
            assert!(
                ctx.external,
                "{name} must leave the exponent clamps in force"
            );
        }
    }

    fn call(ctx: &mut Ctx, f: fn(&mut Ctx, &Decimal) -> Result<Decimal>, text: &str) -> String {
        let value = f(ctx, &d(text)).expect("within the constant's precision");
        to_string(&value, &ctx.cfg)
    }

    fn call_h(ctx: &mut Ctx, f: fn(&mut Ctx, &Decimal) -> Decimal, text: &str) -> String {
        let value = f(ctx, &d(text));
        to_string(&value, &ctx.cfg)
    }

    /// All expectations read off upstream decimal.js in Node at precision 20.
    #[test]
    fn sines_and_cosines_of_familiar_arguments() {
        let mut ctx = Ctx::default();
        assert_eq!(call(&mut ctx, sin, "0"), "0");
        assert_eq!(call(&mut ctx, cos, "0"), "1");
        assert_eq!(call(&mut ctx, sin, "1"), "0.84147098480789650665");
        assert_eq!(call(&mut ctx, cos, "1"), "0.5403023058681397174");
        assert_eq!(call(&mut ctx, sin, "-1"), "-0.84147098480789650665");
        assert_eq!(call(&mut ctx, cos, "-1"), "0.5403023058681397174");
    }

    #[test]
    fn the_quadrant_reduction_carries_the_sign() {
        let mut ctx = Ctx::default();
        // Arguments beyond pi/2 exercise `to_less_than_half_pi`.
        assert_eq!(call(&mut ctx, sin, "3"), "0.1411200080598672221");
        assert_eq!(call(&mut ctx, cos, "3"), "-0.98999249660044545727");
        assert_eq!(call(&mut ctx, sin, "10"), "-0.5440211108893698134");
        assert_eq!(call(&mut ctx, cos, "10"), "-0.83907152907645245226");
    }

    #[test]
    fn tangents() {
        let mut ctx = Ctx::default();
        assert_eq!(call(&mut ctx, tan, "0"), "0");
        assert_eq!(call(&mut ctx, tan, "1"), "1.5574077246549022305");
        assert_eq!(call(&mut ctx, tan, "-1"), "-1.5574077246549022305");
    }

    #[test]
    fn hyperbolics() {
        let mut ctx = Ctx::default();
        assert_eq!(call_h(&mut ctx, sinh, "0"), "0");
        assert_eq!(call_h(&mut ctx, cosh, "0"), "1");
        assert_eq!(call_h(&mut ctx, tanh, "0"), "0");
        assert_eq!(call_h(&mut ctx, sinh, "1"), "1.1752011936438014569");
        assert_eq!(call_h(&mut ctx, cosh, "1"), "1.5430806348152437785");
        assert_eq!(call_h(&mut ctx, tanh, "1"), "0.76159415595576488812");
    }

    #[test]
    fn non_finite_arguments() {
        let mut ctx = Ctx::default();
        assert!(sin(&mut ctx, &Decimal::infinity(Sign::Pos))
            .unwrap()
            .is_nan());
        assert!(cos(&mut ctx, &Decimal::infinity(Sign::Pos))
            .unwrap()
            .is_nan());
        assert!(tan(&mut ctx, &Decimal::nan()).unwrap().is_nan());

        assert!(cosh(&mut ctx, &Decimal::infinity(Sign::Neg)).is_infinite());
        assert!(sinh(&mut ctx, &Decimal::infinity(Sign::Neg)).is_infinite());

        // tanh saturates at ±1 rather than diverging.
        let t = tanh(&mut ctx, &Decimal::infinity(Sign::Neg));
        assert_eq!(to_string(&t, &ctx.cfg), "-1");
    }

    #[test]
    fn signed_zero_survives_the_odd_functions() {
        let mut ctx = Ctx::default();
        let neg_zero = Decimal::zero(Sign::Neg);
        assert!(
            sin(&mut ctx, &neg_zero).unwrap().is_negative(),
            "sin(-0) is -0"
        );
        assert!(sinh(&mut ctx, &neg_zero).is_negative(), "sinh(-0) is -0");
        assert!(
            tan(&mut ctx, &neg_zero).unwrap().is_negative(),
            "tan(-0) is -0"
        );
        // cos is even: cos(-0) is +1.
        assert!(!cos(&mut ctx, &neg_zero).unwrap().is_negative());
    }

    #[test]
    fn the_pythagorean_identity_holds_to_the_working_precision() {
        let mut ctx = Ctx::default();
        for text in ["0.5", "1", "2", "3", "-1.25"] {
            let s = sin(&mut ctx, &d(text)).unwrap();
            let c = cos(&mut ctx, &d(text)).unwrap();
            let s2 = mul(&mut ctx, &s, &s);
            let c2 = mul(&mut ctx, &c, &c);
            let total = add(&mut ctx, &s2, &c2);
            let difference = sub(&mut ctx, &total, &int(1));
            assert!(
                difference.is_zero() || difference.e < -17,
                "sin^2 + cos^2 = 1 at {text}, off by {}",
                to_string(&difference, &ctx.cfg)
            );
        }
    }

    /// The defect described in the module documentation, pinned as a test.
    ///
    /// This is not a regression guard on this port — it is a guard on the
    /// port's *fidelity*. If a future change "fixed" `tan` here, the port would
    /// stop matching the original and the original's own suite would fail.
    #[test]
    fn tan_loses_all_significance_near_a_pole_exactly_as_upstream_does() {
        let mut ctx = Ctx::default();
        ctx.cfg.precision = 14;
        // The true value of tan at this argument is about 3.19e19.
        let value = tan(&mut ctx, &d("1.5707963267948966192")).unwrap();
        let text = to_string(&value, &ctx.cfg);
        assert!(
            text.starts_with("70710678118"),
            "the saturated value the original produces, got {text}"
        );
    }

    #[test]
    fn asking_for_more_digits_than_pi_carries_is_an_error() {
        let mut ctx = Ctx::default();
        assert_eq!(
            get_pi(&mut ctx, PI_PRECISION + 1, rounding::DOWN).unwrap_err(),
            Error::PrecisionLimitExceeded
        );
        assert!(get_pi(&mut ctx, PI_PRECISION, rounding::DOWN).is_ok());
    }

    /// The series denominator must not be narrowed to 32 bits.
    ///
    /// `cosh(1e6)` reduces to an argument near 250,000 and needs about that
    /// many terms, so `n` passes 46,340 and `n(n+1)` passes `i32::MAX`. Written
    /// as `from_i32((a * b) as i32)` the product wrapped — to a *negative*
    /// denominator, among others — and a series whose terms stop shrinking
    /// never satisfies the convergence test. The call did not return a wrong
    /// answer; it did not return.
    ///
    /// The expectation is upstream's, and the test is a timeout in disguise:
    /// if the narrowing comes back, this hangs rather than fails.
    #[test]
    fn a_long_series_does_not_overflow_its_denominator() {
        let mut ctx = Ctx::default();
        assert_eq!(
            to_string(&cosh(&mut ctx, &d("1e6")), &ctx.cfg),
            "1.5166076984010437725e+434294"
        );
        // Just below the boundary, which passed even before the fix — so the
        // pair together says the boundary is where it is claimed to be.
        assert_eq!(
            to_string(&cosh(&mut ctx, &d("1e5")), &ctx.cfg),
            "1.4033316802130615897e+43429"
        );
    }

    /// `from_integer` is exact across the range the counters use, including
    /// past the point where an `i32` would have wrapped.
    #[test]
    fn integers_convert_exactly_past_the_thirty_two_bit_boundary() {
        let cfg = crate::Config::default();
        for n in [
            0i64,
            1,
            9_999_999,
            10_000_000,
            2_147_483_647,
            2_147_483_648,
            46_341 * 46_342,
            9_007_199_254_740_991,
        ] {
            assert_eq!(to_string(&Decimal::from_integer(n), &cfg), n.to_string());
            assert_eq!(
                to_string(&Decimal::from_integer(-n), &cfg),
                if n == 0 {
                    "0".to_string()
                } else {
                    (-n).to_string()
                }
            );
        }
    }
}
