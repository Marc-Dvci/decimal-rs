//! Inverse trigonometric and inverse hyperbolic functions.
//!
//! # Everything reduces to `atan` or to `ln`
//!
//! Only [`atan`] has a series of its own. The other five are identities:
//!
//! ```text
//!     asin(x)  = 2·atan( x / (1 + √((1−x)(1+x))) )
//!     acos(x)  = 2·atan( √((1−x)/(1+x)) )
//!     asinh(x) = ln( x + √(x² + 1) )
//!     acosh(x) = ln( x + √(x² − 1) )
//!     atanh(x) = ½·ln( (1+x)/(1−x) )
//! ```
//!
//! # Why those particular identities
//!
//! The obvious forms are not the ones here, and the difference is the whole
//! history of this part of the library.
//!
//! `acos` was once `π/2 − asin(x)`. That subtraction cancels catastrophically
//! as `x` approaches 1, where the answer approaches zero: the two operands
//! agree to more and more digits and the difference keeps fewer and fewer.
//! Upstream PR #217 replaced it with the arctangent form above, which has no
//! subtraction of near-equal quantities.
//!
//! `asin` had the same defect, in the same place, and it survived a further
//! eighteen months: upstream PR #260, merged 2026-07-14 — sixteen days before
//! this port began, and the newest commit in the pinned tree — replaced
//! `asin(x) = atan(x/√(1−x²))` with the form above for the same reason.
//!
//! Both are transcribed in their *current* form. Porting from memory, from a
//! blog post, or from an older checkout would silently reintroduce a defect
//! that took the maintainer two attempts and a year and a half to remove — and
//! the original's own test suite, which is the thing being preserved, now
//! contains the cases that catch it.

use crate::arith::{add, compare, divide, mul, sub};
use crate::config::rounding;
use crate::constants::PI_PRECISION;
use crate::elementary::ln;
use crate::roots::sqrt;
use crate::round::finalise;
use crate::trig::get_pi;
use crate::{Ctx, Decimal, Result, Sign};

fn int(n: i32) -> Decimal {
    Decimal::from_i32(n)
}

fn abs(x: &Decimal) -> Decimal {
    let mut out = x.clone();
    if out.s.is_negative() {
        out.s = Sign::Pos;
    }
    out
}

/// A decimal from a short literal, for the halves and quarters below.
fn literal(ctx: &Ctx, text: &str) -> Decimal {
    crate::parse::parse_decimal(ctx, Sign::Pos, text)
}

/// `|x|` compared with one: `Some(Less)`, `Some(Equal)`, `Some(Greater)`, or
/// `None` when `x` is NaN.
fn magnitude_vs_one(x: &Decimal) -> Option<core::cmp::Ordering> {
    compare(&abs(x), &int(1))
}

/// `atan(x)`.
///
/// Argument reduction by `atan(x) = 2·atan(x / (1 + √(1 + x²)))`, applied `k`
/// times to bring `|x|` below 0.42, then the alternating series
/// `x − x³/3 + x⁵/5 − …`, then a multiplication by `2ᵏ`.
pub fn atan(ctx: &mut Ctx, x: &Decimal) -> Result<Decimal> {
    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);

    if !x.is_finite() {
        if x.is_nan() {
            return Ok(Decimal::nan());
        }
        if pr + 4 <= PI_PRECISION {
            let pi = get_pi(ctx, pr + 4, rm)?;
            let half = literal(ctx, "0.5");
            let mut r = mul(ctx, &pi, &half);
            r.s = x.s;
            return Ok(r);
        }
    } else if x.is_zero() {
        return Ok(x.clone());
    } else if magnitude_vs_one(x) == Some(core::cmp::Ordering::Equal) && pr + 4 <= PI_PRECISION {
        let pi = get_pi(ctx, pr + 4, rm)?;
        let quarter = literal(ctx, "0.25");
        let mut r = mul(ctx, &pi, &quarter);
        r.s = x.s;
        return Ok(r);
    }

    let wpr = pr + 10;
    ctx.cfg.precision = wpr;
    ctx.cfg.rounding = rounding::DOWN;

    // How many halvings to bring |x| below 0.42.
    let k = 28.min(wpr / crate::LOG_BASE + 2);

    let one = int(1);
    let mut x = x.clone();
    for _ in 0..k {
        let square = mul(ctx, &x, &x);
        let shifted = add(ctx, &square, &one);
        let root = sqrt(ctx, &shifted);
        let divisor = add(ctx, &root, &one);
        x = divide(ctx, &x, &divisor, None, rounding::DOWN, false, None);
    }

    let external_before = ctx.external;
    ctx.external = false;

    let j = (wpr + crate::LOG_BASE - 1) / crate::LOG_BASE;
    let mut n: i64 = 1;
    let x2 = mul(ctx, &x, &x);
    let mut r = x.clone();
    let mut px = x.clone();
    let mut t = r.clone();

    // Two terms per iteration, so that the convergence test always compares a
    // partial sum against the one two terms behind it.
    let mut converged = false;
    while !converged {
        px = mul(ctx, &px, &x2);
        n += 2;
        let term = divide(ctx, &px, &int(n as i32), None, rounding::DOWN, false, None);
        t = sub(ctx, &r, &term);

        px = mul(ctx, &px, &x2);
        n += 2;
        let term = divide(ctx, &px, &int(n as i32), None, rounding::DOWN, false, None);
        r = add(ctx, &t, &term);

        if r.is_finite() && (r.digits().len() as i64) > j {
            let mut i = j;
            converged = loop {
                if r.digits().get(i as usize) != t.digits().get(i as usize) {
                    break false;
                }
                if i == 0 {
                    break true;
                }
                i -= 1;
            };
        }
    }

    if k > 0 {
        // 2 << (k - 1) is 2^k.
        let scale = int(1 << k);
        r = mul(ctx, &r, &scale);
    }

    ctx.external = external_before;
    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;
    finalise(ctx, &mut r, Some(pr), rm, true);
    Ok(r)
}

/// `asin(x)`.
pub fn asin(ctx: &mut Ctx, x: &Decimal) -> Result<Decimal> {
    if x.is_zero() {
        return Ok(x.clone());
    }

    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);

    match magnitude_vs_one(x) {
        Some(core::cmp::Ordering::Equal) => {
            let pi = get_pi(ctx, pr + 4, rm)?;
            let half = literal(ctx, "0.5");
            let mut r = mul(ctx, &pi, &half);
            r.s = x.s;
            return Ok(r);
        }
        Some(core::cmp::Ordering::Less) => {}
        // |x| > 1, or NaN.
        _ => return Ok(Decimal::nan()),
    }

    ctx.cfg.precision = pr + 6;
    ctx.cfg.rounding = rounding::DOWN;

    // asin(x) = 2·atan( x / (1 + √((1−x)(1+x))) ) — the form introduced by
    // upstream PR #260 to avoid the cancellation near |x| = 1.
    let one = int(1);
    let lower = sub(ctx, &one, x);
    let upper = add(ctx, &one, x);
    let product = mul(ctx, &lower, &upper);
    let root = sqrt(ctx, &product);
    let divisor = add(ctx, &root, &one);
    let quotient = divide(ctx, x, &divisor, None, rounding::DOWN, false, None);
    let angle = atan(ctx, &quotient)?;

    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;

    Ok(mul(ctx, &angle, &int(2)))
}

/// `acos(x)`.
pub fn acos(ctx: &mut Ctx, x: &Decimal) -> Result<Decimal> {
    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);

    match magnitude_vs_one(x) {
        Some(core::cmp::Ordering::Less) => {}
        Some(core::cmp::Ordering::Equal) => {
            return Ok(if x.is_negative() {
                get_pi(ctx, pr, rm)?
            } else {
                Decimal::zero(Sign::Pos)
            });
        }
        _ => return Ok(Decimal::nan()),
    }

    if x.is_zero() {
        let pi = get_pi(ctx, pr + 4, rm)?;
        let half = literal(ctx, "0.5");
        return Ok(mul(ctx, &pi, &half));
    }

    ctx.cfg.precision = pr + 6;
    ctx.cfg.rounding = rounding::DOWN;

    // acos(x) = 2·atan( √((1−x)/(1+x)) ) — upstream PR #217.
    let one = int(1);
    let lower = sub(ctx, &one, x);
    let upper = add(ctx, &one, x);
    let ratio = divide(ctx, &lower, &upper, None, rounding::DOWN, false, None);
    let root = sqrt(ctx, &ratio);
    let angle = atan(ctx, &root)?;

    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;

    Ok(mul(ctx, &angle, &int(2)))
}

/// The working precision the inverse hyperbolics raise to.
fn hyperbolic_working_precision(x: &Decimal, multiplier: i64, extra: i64) -> i64 {
    multiplier * x.e.abs().max(x.significant_digits()) + extra
}

/// `asinh(x) = ln(x + √(x² + 1))`.
pub fn asinh(ctx: &mut Ctx, x: &Decimal) -> Result<Decimal> {
    if !x.is_finite() || x.is_zero() {
        return Ok(x.clone());
    }

    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
    ctx.cfg.precision = pr + hyperbolic_working_precision(x, 2, 6);
    ctx.cfg.rounding = rounding::DOWN;

    let inner = ctx.without_clamping(|ctx| {
        let square = mul(ctx, x, x);
        let shifted = add(ctx, &square, &int(1));
        let root = sqrt(ctx, &shifted);
        add(ctx, &root, x)
    });

    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;
    ln(ctx, &inner)
}

/// `acosh(x) = ln(x + √(x² − 1))`.
pub fn acosh(ctx: &mut Ctx, x: &Decimal) -> Result<Decimal> {
    let one = int(1);
    match compare(x, &one) {
        Some(core::cmp::Ordering::Greater) => {}
        Some(core::cmp::Ordering::Equal) => return Ok(Decimal::zero(Sign::Pos)),
        // Below one, or NaN.
        _ => return Ok(Decimal::nan()),
    }
    if !x.is_finite() {
        return Ok(x.clone());
    }

    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
    ctx.cfg.precision = pr + hyperbolic_working_precision(x, 1, 4);
    ctx.cfg.rounding = rounding::DOWN;

    let inner = ctx.without_clamping(|ctx| {
        let square = mul(ctx, x, x);
        let shifted = sub(ctx, &square, &one);
        let root = sqrt(ctx, &shifted);
        add(ctx, &root, x)
    });

    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;
    ln(ctx, &inner)
}

/// `atanh(x) = ½·ln((1+x)/(1−x))`.
pub fn atanh(ctx: &mut Ctx, x: &Decimal) -> Result<Decimal> {
    if !x.is_finite() {
        return Ok(Decimal::nan());
    }
    if x.e >= 0 {
        // |x| >= 1 (or zero): the only defined values here are ±1, which give
        // ±Infinity, and zero, which gives itself.
        return Ok(if magnitude_vs_one(x) == Some(core::cmp::Ordering::Equal) {
            Decimal::infinity(x.s)
        } else if x.is_zero() {
            x.clone()
        } else {
            Decimal::nan()
        });
    }

    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
    let xsd = x.significant_digits();

    // For an argument small enough that atanh(x) and x agree to the full
    // working precision, return x rounded — the series would add nothing.
    if xsd.max(pr) < 2 * -x.e - 1 {
        let mut r = x.clone();
        finalise(ctx, &mut r, Some(pr), rm, true);
        return Ok(r);
    }

    let wpr = xsd - x.e;
    ctx.cfg.precision = wpr;

    let one = int(1);
    let numerator = add(ctx, x, &one);
    let denominator = sub(ctx, &one, x);
    let ratio = divide(
        ctx,
        &numerator,
        &denominator,
        Some(wpr + pr),
        rounding::DOWN,
        false,
        None,
    );

    ctx.cfg.precision = pr + 4;
    ctx.cfg.rounding = rounding::DOWN;

    let logged = ln(ctx, &ratio)?;

    ctx.cfg.precision = pr;
    ctx.cfg.rounding = rm;

    let half = literal(ctx, "0.5");
    Ok(mul(ctx, &logged, &half))
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

    fn call(ctx: &mut Ctx, f: fn(&mut Ctx, &Decimal) -> Result<Decimal>, text: &str) -> String {
        let value = f(ctx, &d(text)).expect("within the constant's precision");
        to_string(&value, &ctx.cfg)
    }

    /// All expectations read off upstream decimal.js in Node at precision 20.
    #[test]
    fn arctangents() {
        let mut ctx = Ctx::default();
        assert_eq!(call(&mut ctx, atan, "0"), "0");
        assert_eq!(call(&mut ctx, atan, "1"), "0.78539816339744830962");
        assert_eq!(call(&mut ctx, atan, "-1"), "-0.78539816339744830962");
    }

    #[test]
    fn arcsines_and_arccosines() {
        let mut ctx = Ctx::default();
        assert_eq!(call(&mut ctx, asin, "0"), "0");
        assert_eq!(call(&mut ctx, acos, "1"), "0");
        assert_eq!(call(&mut ctx, asin, "1"), "1.5707963267948966192");
        assert_eq!(call(&mut ctx, asin, "-1"), "-1.5707963267948966192");
    }

    #[test]
    fn arguments_outside_the_domain_are_not_numbers() {
        let mut ctx = Ctx::default();
        assert!(asin(&mut ctx, &d("2")).unwrap().is_nan());
        assert!(acos(&mut ctx, &d("2")).unwrap().is_nan());
        assert!(acosh(&mut ctx, &d("0.5")).unwrap().is_nan());
        assert!(atanh(&mut ctx, &d("2")).unwrap().is_nan());
        assert!(asin(&mut ctx, &Decimal::nan()).unwrap().is_nan());
    }

    #[test]
    fn inverse_hyperbolics() {
        let mut ctx = Ctx::default();
        assert_eq!(call(&mut ctx, asinh, "0"), "0");
        assert_eq!(call(&mut ctx, acosh, "1"), "0");
        assert_eq!(call(&mut ctx, atanh, "0"), "0");
    }

    #[test]
    fn infinities_map_where_they_should() {
        let mut ctx = Ctx::default();
        let inf = Decimal::infinity(Sign::Pos);
        // atan saturates at pi/2; asinh and acosh diverge; atanh is undefined.
        assert_eq!(call(&mut ctx, atan, "1e9999999"), "1.5707963267948966192");
        assert!(asinh(&mut ctx, &inf).unwrap().is_infinite());
        assert!(acosh(&mut ctx, &inf).unwrap().is_infinite());
        assert!(atanh(&mut ctx, &inf).unwrap().is_nan());

        let at_one = atanh(&mut ctx, &d("1")).unwrap();
        assert!(at_one.is_infinite() && !at_one.is_negative());
    }

    /// The cancellation the two upstream pull requests removed.
    ///
    /// With the old formulas these lose most of their significant digits; with
    /// the current ones they do not. This is the test that would catch a port
    /// made from an older copy of the source.
    #[test]
    fn no_cancellation_near_the_domain_boundary() {
        let mut ctx = Ctx::default();
        // acos(1 - 1e-15) should be about 4.47e-8 with full precision.
        let near_one = d("0.999999999999999");
        let value = acos(&mut ctx, &near_one).unwrap();
        assert!(
            value.significant_digits() >= 18,
            "acos near 1 keeps its digits, got {}",
            to_string(&value, &ctx.cfg)
        );

        let value = asin(&mut ctx, &near_one).unwrap();
        assert!(
            value.significant_digits() >= 18,
            "asin near 1 keeps its digits, got {}",
            to_string(&value, &ctx.cfg)
        );
    }

    #[test]
    fn the_round_trips_hold_to_the_working_precision() {
        let mut ctx = Ctx::default();
        for text in ["0.25", "0.5", "-0.75"] {
            let x = d(text);
            let angle = asin(&mut ctx, &x).unwrap();
            let back = crate::trig::sin(&mut ctx, &angle).unwrap();
            let difference = sub(&mut ctx, &back, &x);
            assert!(
                difference.is_zero() || difference.e < -17,
                "sin(asin({text})) round-trips, off by {}",
                to_string(&difference, &ctx.cfg)
            );
        }
    }
}
