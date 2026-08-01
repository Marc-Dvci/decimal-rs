//! Rounding to a place, the modulus, and the three fixed-format renderings.
//!
//! Nearly everything in this module is a thin wrapper over
//! [`finalise`](crate::round::finalise), and the interest is entirely in
//! *which* arguments each one passes it. Rounding to a whole number is
//! `finalise(x, x.e + 1, mode)`; rounding to `dp` decimal places is
//! `finalise(x, dp + x.e + 1, mode)`; rounding to `sd` significant digits is
//! `finalise(x, sd, mode)`. Once that correspondence is visible the whole
//! family collapses into a page.
//!
//! Two things here are not mechanical, and both are places where an
//! independent reimplementation would quietly differ:
//!
//! * `to_fixed` decides the minus sign by looking at the value **before**
//!   rounding, which is why `(-0.5).toFixed(0)` is `"-0"` and not `"0"`.
//! * `modulo` forms the quotient with the *modulo* rounding mode rather than
//!   the configured rounding mode, and suppresses clamping while it does so,
//!   which is what makes the five modulo modes behave differently from one
//!   another.

use crate::arith::{divide, mul, negated, sub};
use crate::config::rounding;
use crate::error::{check_int32, Error, Result};
use crate::format::{finite_to_string, non_finite_to_string};
use crate::round::finalise;
use crate::{Ctx, Decimal, Sign, MAX_DIGITS};

/// `|x|`.
pub fn abs(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    let mut out = clamped_copy(ctx, x);
    if out.s.is_negative() {
        out.s = Sign::Pos;
    }
    finalise(ctx, &mut out, None, ctx.cfg.rounding, false);
    out
}

/// `-x`. NaN negates to NaN; zero keeps a signed zero.
pub fn neg(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    let mut out = negated(&clamped_copy(ctx, x));
    finalise(ctx, &mut out, None, ctx.cfg.rounding, false);
    out
}

/// Round to a whole number towards +Infinity.
pub fn ceil(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    round_to_integer(ctx, x, rounding::CEIL)
}

/// Round to a whole number towards −Infinity.
pub fn floor(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    round_to_integer(ctx, x, rounding::FLOOR)
}

/// Round to a whole number towards zero.
pub fn trunc(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    round_to_integer(ctx, x, rounding::DOWN)
}

/// Round to a whole number using the configured rounding mode.
pub fn round(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    round_to_integer(ctx, x, ctx.cfg.rounding)
}

/// A copy of `x` as the *constructor* would make one.
///
/// # Why this is not `x.clone()`
///
/// Nine of the original's methods begin `new Ctor(x)` or
/// `new this.constructor(this)` rather than working on the receiver, and that
/// is not merely defensive copying. Passing an existing Decimal through the
/// constructor **clamps it to the current exponent limits**: anything above
/// `maxE` becomes ±Infinity, anything below `minE` becomes ±0. So a value
/// built under one configuration and used under a narrower one is re-judged by
/// the narrower one, at the moment it is used.
///
/// A plain clone skips that, which is invisible until `minE` or `maxE` has been
/// moved. `floor` was the case that exposed it, and only because a fuzz
/// sequence happened to change `minE` between constructing a value and using
/// it — the original test suite never does.
///
/// The clamp applies only when `external` is set, i.e. not to the intermediate
/// values of a calculation in progress; that is the whole purpose of the flag.
pub fn clamped_copy(ctx: &Ctx, x: &Decimal) -> Decimal {
    let mut copy = x.clone();
    if !ctx.external {
        return copy;
    }
    if copy.is_nan() {
        return copy;
    }
    if copy.d.is_none() || copy.e > ctx.cfg.max_e {
        copy = Decimal::infinity(copy.s);
    } else if copy.e < ctx.cfg.min_e {
        copy = Decimal::zero(copy.s);
    }
    copy
}

fn round_to_integer(ctx: &mut Ctx, x: &Decimal, rm: u8) -> Decimal {
    let mut out = clamped_copy(ctx, x);
    // `x.e + 1` is the number of significant digits standing before the point,
    // so rounding to that many discards exactly the fractional ones.
    //
    // Note that this reads the exponent of the *original*, not of the clamped
    // copy: the original writes `finalise(new Ctor(x), x.e + 1, rm)`, and `x`
    // there is still the receiver. When the two differ — which is exactly when
    // the clamp fired — the significant-digit count and the value it is applied
    // to come from different places. That is the original's arithmetic and it
    // is transcribed, not corrected; see DECISIONS.md D-12.
    let sd = x.e.saturating_add(1);
    finalise(ctx, &mut out, Some(sd), rm, false);
    out
}

/// Round to `dp` decimal places. `dp == None` returns a plain copy.
pub fn to_decimal_places(
    ctx: &mut Ctx,
    x: &Decimal,
    dp: Option<f64>,
    rm: Option<f64>,
) -> Result<Decimal> {
    let mut out = clamped_copy(ctx, x);
    let Some(dp) = dp else {
        return Ok(out);
    };
    let dp = check_int32(dp, 0, MAX_DIGITS)?;
    let rm = resolve_rounding_mode(ctx, rm)?;

    // `out.e`, not `x.e`, and the difference is the whole of this function.
    //
    // `round` and friends are written `finalise(new Ctor(x), x.e + 1, rm)`, so
    // the digit count comes from the *receiver* while the value it is applied
    // to is the clamped copy — see `round_to_integer`, which says so. `toDP` is
    // written differently:
    //
    // ```js
    //     x = new Ctor(x);              // x is rebound
    //     …
    //     return finalise(x, dp + x.e + 1, rm);
    // ```
    //
    // so here `x.e` is the *clamped* exponent. Two lines apart in the original,
    // opposite in effect, and only distinguishable when the clamp actually
    // fires. Reading the receiver's exponent here made
    // `toDP(0, ROUND_UP)` of a value below `minE` come out as
    // 1e+8999999999999532 where the original gives 0: the clamped copy is zero,
    // but the digit count was computed from an exponent of −9 × 10¹⁵.
    //
    // Found by the differential campaign at 29,472 refereed operations. The
    // eighty argument pairs that expose it are all `ROUND_UP` or `ROUND_CEIL`,
    // which is why `scripts/clamp-conformance.js` — which calls `toDP` at the
    // default `ROUND_HALF_UP`, where a tiny value rounds to zero either way —
    // had nothing to say about it.
    let sd = dp.saturating_add(out.e).saturating_add(1);
    finalise(ctx, &mut out, Some(sd), rm, false);
    Ok(out)
}

/// Round to `sd` significant digits. `sd == None` uses the configured
/// precision and rounding mode.
pub fn to_significant_digits(
    ctx: &mut Ctx,
    x: &Decimal,
    sd: Option<f64>,
    rm: Option<f64>,
) -> Result<Decimal> {
    let (sd, rm) = match sd {
        None => (ctx.cfg.precision, ctx.cfg.rounding),
        Some(sd) => (
            check_int32(sd, 1, MAX_DIGITS)?,
            resolve_rounding_mode(ctx, rm)?,
        ),
    };
    let mut out = clamped_copy(ctx, x);
    finalise(ctx, &mut out, Some(sd), rm, false);
    Ok(out)
}

/// Round to the nearest multiple of `y`.
pub fn to_nearest(
    ctx: &mut Ctx,
    x: &Decimal,
    y: Option<&Decimal>,
    rm: Option<f64>,
) -> Result<Decimal> {
    let mut out = clamped_copy(ctx, x);

    let (y, rm) = match y {
        None => {
            // No modulus given: round to the nearest whole number. A
            // non-finite value is returned untouched.
            if out.d.is_none() {
                return Ok(out);
            }
            (Decimal::from_i32(1), ctx.cfg.rounding)
        }
        Some(y) => {
            let rm = resolve_rounding_mode(ctx, rm)?;
            let y = y.clone();

            if out.d.is_none() {
                // x is non-finite: return it, unless y is NaN, which wins.
                return Ok(if y.is_nan() { y } else { out });
            }
            if y.d.is_none() {
                // y is non-finite: Infinity takes x's sign, NaN stays NaN.
                let mut y = y;
                if !y.is_nan() {
                    y.s = out.s;
                }
                return Ok(y);
            }
            (y, rm)
        }
    };

    if y.digits()[0] != 0 {
        out = ctx.without_clamping(|ctx| {
            let q = divide(ctx, &out, &y, Some(0), rm, true, None);
            mul(ctx, &q, &y)
        });
        finalise(ctx, &mut out, None, ctx.cfg.rounding, false);
    } else {
        // A zero modulus: every multiple is zero, so the answer is a zero
        // carrying x's sign.
        out = Decimal::zero(if out.s == Sign::Nan { Sign::Pos } else { out.s });
    }

    Ok(out)
}

/// `x mod y`, using the configured modulo mode to form the quotient.
///
/// The five useful modes differ only in how the quotient is rounded:
/// truncated division (1) gives JavaScript's `%`, floored division (3) gives
/// Python's, 6 gives the IEEE 754 remainder, and 9 gives Euclidean division
/// whose remainder is always non-negative.
pub fn modulo(ctx: &mut Ctx, x: &Decimal, y: &Decimal) -> Decimal {
    // NaN if x is non-finite, or y is NaN, or y is zero.
    let y_is_zero = y.d.as_deref().map(|d| d[0] == 0).unwrap_or(false);
    if x.d.is_none() || y.is_nan() || y_is_zero {
        return Decimal::nan();
    }

    // x if y is infinite, or x is zero.
    let x_is_zero = x.d.as_deref().map(|d| d[0] == 0).unwrap_or(false);
    if y.d.is_none() || x_is_zero {
        let mut out = x.clone();
        let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
        finalise(ctx, &mut out, Some(pr), rm, false);
        return out;
    }

    let modulo_mode = ctx.cfg.modulo;

    // The intermediate quotient and product must not be rounded to the working
    // precision, or the remainder would be wrong by a rounding error.
    let product = ctx.without_clamping(|ctx| {
        let q = if modulo_mode == 9 {
            // Euclidean: q = sign(y) × floor(x / |y|), so the remainder is
            // never negative.
            let mut y_abs = y.clone();
            y_abs.s = Sign::Pos;
            let mut q = divide(ctx, x, &y_abs, Some(0), rounding::FLOOR, true, None);
            q.s = q.s.product(y.s);
            q
        } else {
            divide(ctx, x, y, Some(0), modulo_mode, true, None)
        };
        mul(ctx, &q, y)
    });

    // `return x.minus(q)` in the original, and `P.minus` opens with
    // `y = new Ctor(y)` — a clamping copy, not a clone (D-12). So the product is
    // measured against `minE`/`maxE` *here*, with the clamps back on, even
    // though it was formed with them off.
    //
    // It routinely exceeds them, because the product is the same size as `x`
    // while the remainder is not. Narrow `maxE` below `x`'s exponent and the
    // subtrahend becomes Infinity, so `x.mod(y)` is ∓Infinity where the true
    // remainder is a perfectly ordinary small number. Reproduced: this is a
    // value the original computes, not a way to break it.
    let product = clamped_copy(ctx, &product);
    sub(ctx, x, &product)
}

/// Clamp to `[min, max]`.
pub fn clamp(ctx: &mut Ctx, x: &Decimal, min: &Decimal, max: &Decimal) -> Result<Decimal> {
    use core::cmp::Ordering;
    if min.is_nan() || max.is_nan() {
        return Ok(Decimal::nan());
    }
    if crate::arith::compare(min, max) == Some(Ordering::Greater) {
        return Err(Error::InvalidArgument(crate::format::interpolated(
            max, &ctx.cfg,
        )));
    }
    Ok(match crate::arith::compare(x, min) {
        Some(Ordering::Less) => min.clone(),
        _ => match crate::arith::compare(x, max) {
            Some(Ordering::Greater) => max.clone(),
            // `new Ctor(x)` in the original, and it is the interesting branch:
            // the receiver is *inside* the requested range and is still
            // re-judged against `minE`/`maxE` on the way out. So
            // `x.clamp(-1e400, 1e400)` with `maxE` at 100 answers Infinity for
            // a receiver of 1.5e300 — the clamp the caller asked for did not
            // apply, and the one they did not ask for did. Reproduced (D-12).
            //
            // Note the asymmetry: the two bounds were already re-judged by the
            // `new Ctor(min)`/`new Ctor(max)` at the top of the function, so
            // returning one of *them* needs no further copy.
            _ => clamped_copy(ctx, x),
        },
    })
}

// ---------------------------------------------------------------------------
// Fixed-format rendering
// ---------------------------------------------------------------------------

/// `toFixed`: fixed-point notation with `dp` decimal places.
///
/// The sign is decided from the value *before* rounding. That is why
/// `(-0.5).toFixed(0)` is `"-0"`: the rounded value is a positive zero, but
/// the original value was negative, and the original comments on this
/// deliberately.
pub fn to_fixed(ctx: &mut Ctx, x: &Decimal, dp: Option<f64>, rm: Option<f64>) -> Result<String> {
    let str = match dp {
        None => finite_to_string(x, false, None),
        Some(dp) => {
            let dp = check_int32(dp, 0, MAX_DIGITS)?;
            let rm = resolve_rounding_mode(ctx, rm)?;

            // `finalise(new Ctor(x), dp + x.e + 1, rm)`. The value rounded is
            // the *clamped* copy and the digit count comes from the *receiver*
            // — the same split as `round_to_integer`, and the same reason: the
            // copy is made inside the call, so `x.e` beside it still refers to
            // the receiver.
            //
            // A plain clone here was invisible under every rounding mode that
            // rounds towards zero, because `finalise` clamps its own result on
            // the way out and both sides then agreed on the zero. `ROUND_UP`
            // on `1.5e-300` with `minE` at −100 is where they part: upstream
            // rounds a zero at 10⁻³⁰⁰ precision and gets a fifty-seven-digit
            // integer, the port rounded the surviving 1.5e-300 and got 0.01.
            let mut y = clamped_copy(ctx, x);
            finalise(
                ctx,
                &mut y,
                Some(dp.saturating_add(x.e).saturating_add(1)),
                rm,
                false,
            );
            if y.is_finite() {
                finite_to_string(&y, false, Some(dp.saturating_add(y.e).saturating_add(1)))
            } else {
                non_finite_to_string(&y).to_string()
            }
        }
    };
    Ok(prefix_sign(x, str))
}

/// `toExponential`: exponential notation with `dp` digits after the point.
pub fn to_exponential(
    ctx: &mut Ctx,
    x: &Decimal,
    dp: Option<f64>,
    rm: Option<f64>,
) -> Result<String> {
    let (value, str) = match dp {
        None => (x.clone(), finite_to_string(x, true, None)),
        Some(dp) => {
            let dp = check_int32(dp, 0, MAX_DIGITS)?;
            let rm = resolve_rounding_mode(ctx, rm)?;
            // `new Ctor(x)` again, as in `to_fixed`. Here the digit count does
            // not depend on the exponent, so the two implementations happen to
            // agree case for case; the copy is clamped anyway, because the
            // agreement is a coincidence of this argument shape and not a
            // property of the function.
            let mut y = clamped_copy(ctx, x);
            finalise(ctx, &mut y, Some(dp + 1), rm, false);
            let str = finite_to_string(&y, true, Some(dp + 1));
            (y, str)
        }
    };
    // Unlike `toFixed`, the sign here comes from the rounded value.
    Ok(prefix_sign(&value, str))
}

/// `toPrecision`: `sd` significant digits, in whichever notation the
/// thresholds select.
pub fn to_precision(
    ctx: &mut Ctx,
    x: &Decimal,
    sd: Option<f64>,
    rm: Option<f64>,
) -> Result<String> {
    let (value, str) = match sd {
        None => {
            let exp = x.e <= ctx.cfg.to_exp_neg || x.e >= ctx.cfg.to_exp_pos;
            (x.clone(), finite_to_string(x, exp, None))
        }
        Some(sd) => {
            let sd = check_int32(sd, 1, MAX_DIGITS)?;
            let rm = resolve_rounding_mode(ctx, rm)?;
            let mut y = clamped_copy(ctx, x); // `new Ctor(x)`, as above.
            finalise(ctx, &mut y, Some(sd), rm, false);
            let exp = sd <= y.e || y.e <= ctx.cfg.to_exp_neg;
            let str = finite_to_string(&y, exp, Some(sd));
            (y, str)
        }
    };
    Ok(prefix_sign(&value, str))
}

/// Attach a minus sign for a negative value, but not for negative zero — the
/// convention `toString`, `toFixed`, `toExponential` and `toPrecision` all
/// share, and which `valueOf` alone departs from.
fn prefix_sign(x: &Decimal, str: String) -> String {
    if x.is_negative() && !x.is_zero() {
        format!("-{str}")
    } else {
        str
    }
}

/// An explicit rounding mode, validated, or the configured default.
fn resolve_rounding_mode(ctx: &Ctx, rm: Option<f64>) -> Result<u8> {
    match rm {
        None => Ok(ctx.cfg.rounding),
        Some(rm) => Ok(check_int32(rm, 0, 8)? as u8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_decimal;

    fn d(text: &str) -> Decimal {
        let ctx = Ctx::default();
        if let Some(rest) = text.strip_prefix('-') {
            parse_decimal(&ctx, Sign::Neg, rest)
        } else {
            parse_decimal(&ctx, Sign::Pos, text)
        }
    }

    fn show(x: &Decimal) -> String {
        crate::format::to_string(x, &crate::Config::default())
    }

    #[test]
    fn the_four_integer_roundings_go_the_right_ways() {
        let mut ctx = Ctx::default();
        for (value, want_ceil, want_floor, want_trunc, want_round) in [
            ("1.5", "2", "1", "1", "2"),
            ("-1.5", "-1", "-2", "-1", "-2"),
            ("1.4", "2", "1", "1", "1"),
            ("-1.4", "-1", "-2", "-1", "-1"),
            ("2", "2", "2", "2", "2"),
        ] {
            let x = d(value);
            assert_eq!(show(&ceil(&mut ctx, &x)), want_ceil, "ceil({value})");
            assert_eq!(show(&floor(&mut ctx, &x)), want_floor, "floor({value})");
            assert_eq!(show(&trunc(&mut ctx, &x)), want_trunc, "trunc({value})");
            assert_eq!(show(&round(&mut ctx, &x)), want_round, "round({value})");
        }
    }

    #[test]
    fn to_fixed_takes_its_sign_from_before_the_rounding() {
        let mut ctx = Ctx::default();
        // The rounded value is a positive zero, but the value that was rounded
        // was negative, so the sign survives. This is the case the original
        // comments on, and the reason the sign is not read off the result.
        assert_eq!(
            to_fixed(&mut ctx, &d("-0.4"), Some(0.0), None).unwrap(),
            "-0"
        );
        // Under the default HALF_UP the tie goes away from zero, so -0.5 does
        // round to a non-zero value and the question does not arise.
        assert_eq!(
            to_fixed(&mut ctx, &d("-0.5"), Some(0.0), None).unwrap(),
            "-1"
        );
        assert_eq!(to_fixed(&mut ctx, &d("0.5"), Some(0.0), None).unwrap(), "1");
        assert_eq!(
            to_fixed(&mut ctx, &d("-1.5"), Some(0.0), None).unwrap(),
            "-2"
        );
    }

    #[test]
    fn to_fixed_pads_to_the_requested_places() {
        let mut ctx = Ctx::default();
        assert_eq!(
            to_fixed(&mut ctx, &d("1"), Some(3.0), None).unwrap(),
            "1.000"
        );
        assert_eq!(
            to_fixed(&mut ctx, &d("1.5"), Some(3.0), None).unwrap(),
            "1.500"
        );
        assert_eq!(
            to_fixed(&mut ctx, &d("1.2345"), Some(2.0), None).unwrap(),
            "1.23"
        );
        assert_eq!(
            to_fixed(&mut ctx, &d("0"), Some(2.0), None).unwrap(),
            "0.00"
        );
    }

    #[test]
    fn to_exponential_and_to_precision() {
        let mut ctx = Ctx::default();
        assert_eq!(
            to_exponential(&mut ctx, &d("12345"), Some(2.0), None).unwrap(),
            "1.23e+4"
        );
        assert_eq!(
            to_exponential(&mut ctx, &d("0.00012345"), Some(3.0), None).unwrap(),
            "1.235e-4"
        );
        assert_eq!(
            to_precision(&mut ctx, &d("12345"), Some(3.0), None).unwrap(),
            "1.23e+4"
        );
        assert_eq!(
            to_precision(&mut ctx, &d("1.2345"), Some(3.0), None).unwrap(),
            "1.23"
        );
    }

    #[test]
    fn out_of_range_arguments_are_rejected_with_the_originals_message() {
        let mut ctx = Ctx::default();
        assert_eq!(
            to_fixed(&mut ctx, &d("1"), Some(-1.0), None)
                .unwrap_err()
                .to_string(),
            "[DecimalError] Invalid argument: -1"
        );
        assert!(to_significant_digits(&mut ctx, &d("1"), Some(0.0), None).is_err());
        assert!(to_decimal_places(&mut ctx, &d("1"), Some(1.0), Some(9.0)).is_err());
    }

    #[test]
    fn modulo_follows_the_configured_mode() {
        let mut ctx = Ctx::default();

        // Mode 1 (DOWN) is JavaScript's %: the remainder takes the dividend's
        // sign.
        ctx.cfg.modulo = rounding::DOWN;
        assert_eq!(show(&modulo(&mut ctx, &d("-7"), &d("3"))), "-1");
        assert_eq!(show(&modulo(&mut ctx, &d("7"), &d("3"))), "1");

        // Mode 3 (FLOOR) is Python's: the remainder takes the divisor's sign.
        ctx.cfg.modulo = rounding::FLOOR;
        assert_eq!(show(&modulo(&mut ctx, &d("-7"), &d("3"))), "2");

        // Mode 9 (Euclidean): the remainder is never negative.
        ctx.cfg.modulo = 9;
        assert_eq!(show(&modulo(&mut ctx, &d("-7"), &d("3"))), "2");
        assert_eq!(show(&modulo(&mut ctx, &d("-7"), &d("-3"))), "2");
    }

    #[test]
    fn modulo_edge_cases() {
        let mut ctx = Ctx::default();
        assert!(modulo(&mut ctx, &d("1"), &d("0")).is_nan(), "y zero is NaN");
        assert!(
            modulo(&mut ctx, &Decimal::infinity(Sign::Pos), &d("3")).is_nan(),
            "x infinite is NaN"
        );
        assert_eq!(
            show(&modulo(&mut ctx, &d("3"), &Decimal::infinity(Sign::Pos))),
            "3",
            "y infinite returns x"
        );
    }

    #[test]
    fn to_nearest_rounds_to_a_multiple() {
        let mut ctx = Ctx::default();
        assert_eq!(
            show(&to_nearest(&mut ctx, &d("9.9"), Some(&d("0.5")), None).unwrap()),
            "10"
        );
        assert_eq!(
            show(&to_nearest(&mut ctx, &d("1.4"), Some(&d("0.5")), None).unwrap()),
            "1.5"
        );
        assert_eq!(
            show(&to_nearest(&mut ctx, &d("1.4"), None, None).unwrap()),
            "1",
            "no modulus means the nearest whole number"
        );
    }

    #[test]
    fn clamp_bounds_and_rejects_an_inverted_range() {
        let mut ctx = Ctx::default();
        assert_eq!(
            show(&clamp(&mut ctx, &d("5"), &d("1"), &d("3")).unwrap()),
            "3"
        );
        assert_eq!(
            show(&clamp(&mut ctx, &d("0"), &d("1"), &d("3")).unwrap()),
            "1"
        );
        assert_eq!(
            show(&clamp(&mut ctx, &d("2"), &d("1"), &d("3")).unwrap()),
            "2"
        );
        assert!(clamp(&mut ctx, &d("2"), &d("3"), &d("1")).is_err());
        assert!(clamp(&mut ctx, &d("2"), &Decimal::nan(), &d("1"))
            .unwrap()
            .is_nan());
    }

    #[test]
    fn absolute_value_and_negation_respect_signed_zero() {
        let mut ctx = Ctx::default();
        assert_eq!(show(&abs(&mut ctx, &d("-3"))), "3");
        assert_eq!(show(&abs(&mut ctx, &d("3"))), "3");
        assert!(abs(&mut ctx, &Decimal::zero(Sign::Neg)).is_zero());
        assert!(!abs(&mut ctx, &Decimal::zero(Sign::Neg)).is_negative());
        assert!(neg(&mut ctx, &Decimal::zero(Sign::Pos)).is_negative());
        assert!(neg(&mut ctx, &Decimal::nan()).is_nan());
    }

    /// The clamp that `new Ctor(x)` applies, and a plain clone does not.
    ///
    /// A value is judged against the exponent limits in force *when it is
    /// used*, not only when it was built. Narrowing `maxE` afterwards makes an
    /// existing value infinite the next time any of these methods touches it.
    ///
    /// Every expectation was read off upstream decimal.js in Node. The port
    /// used to answer `9.87e+300`, `9.87e+300`, `-1.785178753e-8999999999999976`
    /// and `-1` to these four — plausible values, all of them wrong.
    #[test]
    fn the_exponent_limits_are_applied_when_a_value_is_used() {
        let mut ctx = Ctx::default();
        ctx.cfg.max_e = 200;
        ctx.cfg.min_e = -872;

        let big = d("9.87e300");
        assert!(abs(&mut ctx, &big).is_infinite(), "above maxE, so infinite");
        assert!(to_significant_digits(&mut ctx, &big, Some(5.0), None)
            .unwrap()
            .is_infinite());

        // Below minE, so zero — and `neg` of it is a zero too, not the value.
        let tiny = d("-1785178753e-8999999999999985");
        let negated = neg(&mut ctx, &tiny);
        assert!(negated.is_zero(), "below minE, so zero");

        // And the case that exposed it. `floor` rounds the *clamped* copy but
        // takes its significant-digit count from the original's exponent, so
        // the two come from different values and the answer is enormous. That
        // is upstream's arithmetic, reproduced; see D-12.
        let floored = floor(&mut ctx, &tiny);
        assert_eq!(
            crate::format::to_string(&floored, &ctx.cfg),
            "-1e+8999999999999976"
        );
    }

    /// The companion to the case above, and its opposite.
    ///
    /// `ceil` and `floor` take their digit count from the receiver's exponent,
    /// so a value the clamp crushed to zero still rounds as though its digits
    /// stood where they were. `toDP` rebinds `x` before it reads `x.e`, so the
    /// same value rounds as the zero it has become. The two forms sit ten
    /// lines apart upstream, and only a mode that rounds *away* from zero can
    /// tell them apart: under the default `ROUND_HALF_UP` a vanished value
    /// rounds to zero either way. That is why this survived until the
    /// differential campaign put `ROUND_UP` and a narrow `minE` into the same
    /// sequence — 29,472 refereed operations in.
    #[test]
    fn to_decimal_places_rounds_the_value_the_clamp_left_behind() {
        const ROUND_UP: f64 = 0.0;
        const ROUND_CEIL: f64 = 2.0;

        let mut ctx = Ctx::default();
        ctx.cfg.min_e = -872;

        let tiny = d("1785178753e-8999999999999985");
        for rm in [ROUND_UP, ROUND_CEIL] {
            let rounded = to_decimal_places(&mut ctx, &tiny, Some(0.0), Some(rm)).unwrap();
            assert_eq!(
                crate::format::to_string(&rounded, &ctx.cfg),
                "0",
                "toDP(0, {rm}) of a value below minE"
            );
        }

        // The same operand, rounded away from zero by a method of the other
        // form: same clamp, same configuration, an answer nine quadrillion
        // digits wide. Both lines were read off upstream in Node.
        assert_eq!(
            crate::format::to_string(&ceil(&mut ctx, &tiny), &ctx.cfg),
            "1e+8999999999999976"
        );
    }
}
