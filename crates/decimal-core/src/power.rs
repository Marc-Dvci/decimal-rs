//! Exponentiation and logarithms to an arbitrary base.
//!
//! Both are built on `ln` and `exp`:
//!
//! ```text
//!     log_b(x) = ln(x) / ln(b)
//!     x^y      = exp(y · ln(x))
//! ```
//!
//! and both spend most of their length deciding how many digits that
//! composition needs in order to round correctly at the end. Dividing two
//! inexact logarithms, or exponentiating an inexact product, loses digits in a
//! way that depends on the arguments, so neither routine can pick a working
//! precision in advance: each computes, inspects the digits around the
//! rounding position, and recomputes at higher precision if they are
//! ambiguous.
//!
//! # The case that cannot be decided
//!
//! The original records an example where this defeats it, and it is worth
//! keeping in view because it is the reason the code stops where it does
//! rather than looping for ever:
//!
//! ```text
//!     log[1048576](4503599627370502) = 2.60000000000000009610279511444746…
//! ```
//!
//! Rounded to one decimal place with ROUND_CEIL the answer is 2.7, but there
//! are fifteen zeros immediately after the requested place, so any finite
//! inspection concludes the value is exactly 2.6 and returns 2.6.
//!
//! The original's response is a rule, not a fix: if the result is known to
//! have a non-terminating expansion, keep widening until the digits become
//! unambiguous; otherwise, after ten more digits, treat fourteen consecutive
//! nines as an exact value and round up. That rule is transcribed here. It is
//! wrong on the example above, and it is wrong here too — deliberately, since
//! the object is to reproduce this library's answers rather than to compute
//! better ones.

use crate::arith::{compare, divide, int_pow, mul};
use crate::config::rounding;
use crate::elementary::{check_rounding_digits, get_ln10, natural_exponential, natural_logarithm};
use crate::format::{digits_to_string, number_to_string};
use crate::round::finalise;
use crate::roots::to_f64;
use crate::{Ctx, Decimal, Result, Sign, LOG_BASE, MAX_SAFE_INTEGER};

/// Whether `x` equals the small integer `n`.
fn equals_int(x: &Decimal, n: i32) -> bool {
    compare(x, &Decimal::from_i32(n)) == Some(core::cmp::Ordering::Equal)
}

/// The fourteen digits starting just past position `at`, read as a number, are
/// all nines.
///
/// The original writes this as `+digitsToString(r.d).slice(at+1, at+15) + 1 ==
/// 1e14`. A short or empty slice parses as zero and so fails the test, which
/// is the behaviour wanted.
fn fourteen_nines_from(x: &Decimal, at: i64) -> bool {
    let digits = digits_to_string(x.digits());
    let start = (at + 1).max(0) as usize;
    let end = (at + 15).max(0) as usize;
    if start >= digits.len() {
        return false;
    }
    let slice = &digits[start..end.min(digits.len())];
    slice.parse::<u64>().map(|n| n + 1 == 100_000_000_000_000).unwrap_or(false)
}

/// `log_base(arg)`, with base 10 when `base` is `None`.
pub fn logarithm(ctx: &mut Ctx, arg: &Decimal, base: Option<&Decimal>) -> Result<Decimal> {
    let pr = ctx.cfg.precision;
    let rm = ctx.cfg.rounding;
    let guard: i64 = 5;

    let (base, is_base10) = match base {
        None => (Decimal::from_i32(10), true),
        Some(b) => {
            // A negative, non-finite, zero or unit base has no logarithm.
            if b.s.is_negative() || b.d.is_none() || b.digits()[0] == 0 || equals_int(b, 1) {
                return Ok(Decimal::nan());
            }
            let is_ten = equals_int(b, 10);
            (b.clone(), is_ten)
        }
    };

    // Negative, non-finite, zero or one argument.
    if arg.s.is_negative() || arg.d.is_none() || arg.digits()[0] == 0 || equals_int(arg, 1) {
        return Ok(if arg.is_finite() && arg.digits()[0] == 0 {
            Decimal::infinity(Sign::Neg)
        } else if arg.s != Sign::Pos {
            Decimal::nan()
        } else if arg.is_finite() {
            Decimal::zero(Sign::Pos)
        } else {
            Decimal::infinity(Sign::Pos)
        });
    }

    // In base 10 the expansion terminates only for an exact power of ten.
    let mut non_terminating = false;
    if is_base10 {
        let d = arg.digits();
        if d.len() > 1 {
            non_terminating = true;
        } else {
            let mut k = d[0];
            while k % 10 == 0 {
                k /= 10;
            }
            non_terminating = k != 1;
        }
    }

    let external_before = ctx.external;
    ctx.external = false;

    let outcome = (|ctx: &mut Ctx| -> Result<Decimal> {
        let mut sd = pr + guard;
        let mut numerator = natural_logarithm(ctx, arg, Some(sd))?;
        let mut denominator = if is_base10 {
            get_ln10(ctx, sd + 10)?
        } else {
            natural_logarithm(ctx, &base, Some(sd))?
        };
        let mut r = divide(ctx, &numerator, &denominator, Some(sd), rounding::DOWN, false, None);

        // Five rounding digits were computed; if they sit on a boundary the
        // last digit cannot yet be decided.
        let mut k = pr;
        if check_rounding_digits(r.digits(), k, rm, None) {
            loop {
                sd += 10;
                numerator = natural_logarithm(ctx, arg, Some(sd))?;
                denominator = if is_base10 {
                    get_ln10(ctx, sd + 10)?
                } else {
                    natural_logarithm(ctx, &base, Some(sd))?
                };
                r = divide(ctx, &numerator, &denominator, Some(sd), rounding::DOWN, false, None);

                if !non_terminating {
                    // Fourteen nines from the second rounding digit — the
                    // first may legitimately be a 4 — is taken as an exact
                    // value that should round up.
                    if fourteen_nines_from(&r, k) {
                        finalise(ctx, &mut r, Some(pr + 1), rounding::UP, false);
                    }
                    break;
                }

                k += 10;
                if !check_rounding_digits(r.digits(), k, rm, None) {
                    break;
                }
            }
        }

        Ok(r)
    })(ctx);

    ctx.external = external_before;
    let mut r = outcome?;
    finalise(ctx, &mut r, Some(pr), rm, false);
    Ok(r)
}

/// `Math.pow(base, exponent)` — ECMAScript's, which is **not** `f64::powf`.
///
/// Rust's `powf` implements IEEE 754's `pow`, and C99's before it. ECMAScript
/// deliberately departs from both in two places, and its own specification
/// spells the departures out (`Number::exponentiate`, steps 1 and 8–9):
///
/// ```text
///     Math.pow(1, NaN)       is NaN      IEEE pow(1, NaN)       is 1
///     Math.pow(±1, ±Infinity) is NaN     IEEE pow(±1, ±Infinity) is 1
/// ```
///
/// IEEE makes `pow(1, anything)` be 1 on the reasoning that 1 raised to any
/// power is 1 whatever the exponent turns out to be. ECMAScript takes the
/// opposite view — that an indeterminate exponent gives an indeterminate
/// result — and the original inherits it wholesale, because this branch of
/// `toPower` is literally `mathpow(+x, yn)`. The test suite checks all four:
/// `Decimal(1).pow(Infinity)` is NaN, and so is `Decimal(-1).pow(-Infinity)`.
///
/// Everything else agrees, including the signed zeros and the odd-integer
/// rules for a negative base, so those are left to `powf`.
fn math_pow(base: f64, exponent: f64) -> f64 {
    if exponent.is_nan() {
        return f64::NAN;
    }
    if base.abs() == 1.0 && exponent.is_infinite() {
        return f64::NAN;
    }
    base.powf(exponent)
}

/// `x^y`.
pub fn to_power(ctx: &mut Ctx, x: &Decimal, y: &Decimal) -> Result<Decimal> {
    let yn = to_f64(ctx, y);

    // Any non-finite or zero operand: defer to double arithmetic, which
    // already carries the whole table of special cases. It has to be
    // *ECMAScript's* table, though, not IEEE's — see `math_pow`.
    if x.d.is_none() || y.d.is_none() || x.digits()[0] == 0 || y.digits()[0] == 0 {
        let xn = to_f64(ctx, x);
        let result = math_pow(xn, yn);
        return Ok(if result.is_nan() {
            Decimal::nan()
        } else if result.is_infinite() {
            Decimal::infinity(if result < 0.0 { Sign::Neg } else { Sign::Pos })
        } else {
            crate::parse::parse_decimal(
                ctx,
                if result.is_sign_negative() { Sign::Neg } else { Sign::Pos },
                &number_to_string(result.abs()),
            )
        });
    }

    // `x = new Ctor(x)` in the original: a clamping copy, not a clone.
    let mut x = crate::ops::clamped_copy(ctx, x);
    if equals_int(&x, 1) {
        return Ok(x);
    }

    let pr = ctx.cfg.precision;
    let rm = ctx.cfg.rounding;

    if equals_int(y, 1) {
        finalise(ctx, &mut x, Some(pr), rm, false);
        return Ok(x);
    }

    let mut e = y.e.div_euclid(LOG_BASE);

    // A small integer exponent goes through exponentiation by squaring, which
    // is both faster and exact.
    if e >= y.digits().len() as i64 - 1 && yn.abs() <= MAX_SAFE_INTEGER as f64 {
        let k = yn.abs() as i64;
        let r = int_pow(ctx, &x, k, pr);
        return Ok(if y.s.is_negative() {
            let one = Decimal::from_i32(1);
            divide(ctx, &one, &r, None, rm, false, None)
        } else {
            let mut r = r;
            finalise(ctx, &mut r, Some(pr), rm, false);
            r
        });
    }

    let mut s = x.s;

    if s.is_negative() {
        // A negative base raised to a non-integer power is not real.
        if e < y.digits().len() as i64 - 1 {
            return Ok(Decimal::nan());
        }
        // The result is positive when the limb holding the exponent's units
        // digit is even — `(y.d[e] & 1) == 0` in the original.
        //
        // `e` can point past the end of the array, and that is not an error:
        // `1e307` is the single limb `[1]` with `e = 43`, because a value
        // stores only its significant digits. JavaScript answers `undefined`
        // for the missing limb, and `undefined & 1` is `0` — even. So an
        // exponent whose units digit lies in a limb that was never stored is
        // even, which is right, since every such digit is a trailing zero.
        //
        // A missing limb must therefore read as 0, not as "no answer": with
        // `is_some_and` this returned false and `(-1)^1e307` came out −1.
        let units_limb = y.digits().get(e as usize).copied().unwrap_or(0);
        if units_limb & 1 == 0 {
            s = Sign::Pos;
        }
        // (-1)^y is just ±1.
        if x.e == 0 && x.digits() == [1] {
            x.s = s;
            return Ok(x);
        }
    }

    // Estimate the result's exponent: x^y = 10^(y·log₁₀ x).
    let k = to_f64(ctx, &x).powf(yn);
    e = if k == 0.0 || !k.is_finite() {
        let mantissa = format!("0.{}", digits_to_string(x.digits()))
            .parse::<f64>()
            .unwrap_or(0.0);
        (yn * (mantissa.ln() / core::f64::consts::LN_10 + x.e as f64 + 1.0)).floor() as i64
    } else {
        crate::parse::parse_decimal(ctx, Sign::Pos, &number_to_string(k.abs())).e
    };

    // The estimate can be off by one, hence the ±1 slack on the limits.
    if e > ctx.cfg.max_e + 1 || e < ctx.cfg.min_e - 1 {
        return Ok(if e > 0 {
            Decimal::infinity(s)
        } else {
            Decimal::zero(s)
        });
    }

    let external_before = ctx.external;
    ctx.external = false;
    ctx.cfg.rounding = rounding::DOWN;
    x.s = Sign::Pos;

    // Extra guard digits so that `ln(x)` yields five correct rounding digits.
    // The original records the failure this prevents, at precision 10:
    //   2.32456 ^ 2087987436534566.46411
    //     should be 1.162377823e+764914905173815
    //     but is    1.162355823e+764914905173815
    let k = 12.min(e.to_string().len() as i64);

    let outcome = (|ctx: &mut Ctx| -> Result<Decimal> {
        let logged = natural_logarithm(ctx, &x, Some(pr + k))?;
        let scaled = mul(ctx, y, &logged);
        let mut r = natural_exponential(ctx, &scaled, Some(pr));

        // The exponential can still overflow, e.g. 0.9999999999999999 ^ -1e40.
        if r.is_finite() {
            finalise(ctx, &mut r, Some(pr + 5), rounding::DOWN, false);

            if check_rounding_digits(r.digits(), pr, rm, None) {
                let wider = pr + 10;
                let logged = natural_logarithm(ctx, &x, Some(wider + k))?;
                let scaled = mul(ctx, y, &logged);
                r = natural_exponential(ctx, &scaled, Some(wider));
                finalise(ctx, &mut r, Some(wider + 5), rounding::DOWN, false);

                if fourteen_nines_from(&r, pr) {
                    finalise(ctx, &mut r, Some(pr + 1), rounding::UP, false);
                }
            }
        }

        Ok(r)
    })(ctx);

    ctx.cfg.rounding = rm;
    ctx.external = external_before;

    let mut r = outcome?;
    r.s = s;
    finalise(ctx, &mut r, Some(pr), rm, false);
    Ok(r)
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

    fn pow_str(ctx: &mut Ctx, a: &str, b: &str) -> String {
        let value = to_power(ctx, &d(a), &d(b)).expect("within precision");
        to_string(&value, &ctx.cfg)
    }

    fn log_str(ctx: &mut Ctx, arg: &str, base: Option<&str>) -> String {
        let base = base.map(d);
        let value = logarithm(ctx, &d(arg), base.as_ref()).expect("within precision");
        to_string(&value, &ctx.cfg)
    }

    /// All expectations read off upstream decimal.js in Node at precision 20.
    #[test]
    fn integer_powers_are_exact() {
        let mut ctx = Ctx::default();
        assert_eq!(pow_str(&mut ctx, "2", "10"), "1024");
        assert_eq!(pow_str(&mut ctx, "2", "0"), "1");
        assert_eq!(pow_str(&mut ctx, "2", "1"), "2");
        assert_eq!(pow_str(&mut ctx, "10", "3"), "1000");
        assert_eq!(pow_str(&mut ctx, "-2", "3"), "-8");
        assert_eq!(pow_str(&mut ctx, "-2", "2"), "4");
        assert_eq!(pow_str(&mut ctx, "2", "-2"), "0.25");
    }

    #[test]
    fn fractional_powers_go_through_exp_and_ln() {
        let mut ctx = Ctx::default();
        assert_eq!(pow_str(&mut ctx, "2", "0.5"), "1.4142135623730950488");
        assert_eq!(pow_str(&mut ctx, "4", "0.5"), "2");
        assert_eq!(pow_str(&mut ctx, "8", "0.3333333333333333"), "1.9999999999999998614");
    }

    #[test]
    fn the_documented_guard_digit_counterexample_comes_out_right() {
        // The original records this at precision 10: without the extra guard
        // digits on `ln(x)` the answer is 1.162355823e+764914905173815 — wrong
        // in the fifth significant digit.
        let mut ctx = Ctx::default();
        ctx.cfg.precision = 10;
        assert_eq!(
            pow_str(&mut ctx, "2.32456", "2087987436534566.46411"),
            "1.162377823e+764914905173815"
        );
    }

    #[test]
    fn a_negative_base_needs_an_integer_exponent() {
        let mut ctx = Ctx::default();
        assert!(
            to_power(&mut ctx, &d("-2"), &d("0.5")).unwrap().is_nan(),
            "no real root"
        );
        assert_eq!(pow_str(&mut ctx, "-1", "3"), "-1");
        assert_eq!(pow_str(&mut ctx, "-1", "2"), "1");
    }

    #[test]
    fn the_ieee_special_cases_come_from_double_arithmetic() {
        let mut ctx = Ctx::default();

        // The two places ECMAScript departs from IEEE 754, both of which the
        // original inherits by calling `Math.pow` here. `f64::powf` answers 1
        // to all four of these.
        let infinite = Decimal::infinity(Sign::Pos);
        let negative_infinite = Decimal::infinity(Sign::Neg);
        for (base, exponent) in [
            (d("1"), infinite.clone()),
            (d("1"), negative_infinite.clone()),
            (d("-1"), infinite),
            (d("-1"), negative_infinite),
        ] {
            let value = to_power(&mut ctx, &base, &exponent).unwrap();
            assert!(
                value.is_nan(),
                "ECMAScript makes |1| to an infinite power NaN, got {}",
                to_string(&value, &ctx.cfg)
            );
        }
        assert!(to_power(&mut ctx, &d("1"), &Decimal::nan()).unwrap().is_nan());

        // Everything else is the IEEE table, unmodified.
        assert_eq!(pow_str(&mut ctx, "0", "0"), "1", "0^0 is 1");
        assert!(to_power(&mut ctx, &d("0"), &d("-1")).unwrap().is_infinite());
        assert_eq!(pow_str(&mut ctx, "2", "0"), "1");
    }

    /// `(-1)^y` for an even `y` whose units digit lives in a limb the value
    /// never stored. The original reads `undefined & 1`, which is 0; reading
    /// it as "no limb, so not known to be even" gave −1.
    #[test]
    fn a_huge_even_exponent_is_still_even() {
        let mut ctx = Ctx::default();
        ctx.cfg.precision = 100;
        ctx.cfg.rounding = rounding::DOWN;
        assert_eq!(pow_str(&mut ctx, "-1", "1e307"), "1");
        assert_eq!(pow_str(&mut ctx, "-1", "1e309"), "1");
        // An odd exponent, for contrast: this one's units digit is stored.
        assert_eq!(pow_str(&mut ctx, "-1", "101"), "-1");
    }

    #[test]
    fn base_ten_logarithms_of_exact_powers_are_exact() {
        let mut ctx = Ctx::default();
        for (arg, expected) in [
            ("1", "0"),
            ("10", "1"),
            ("100", "2"),
            ("1000", "3"),
            ("0.1", "-1"),
            ("1e20", "20"),
        ] {
            assert_eq!(log_str(&mut ctx, arg, None), expected, "log10({arg})");
        }
    }

    #[test]
    fn logarithms_to_other_bases() {
        let mut ctx = Ctx::default();
        assert_eq!(log_str(&mut ctx, "8", Some("2")), "3");
        assert_eq!(log_str(&mut ctx, "1024", Some("2")), "10");
        assert_eq!(log_str(&mut ctx, "2", Some("10")), "0.30102999566398119521");
    }

    #[test]
    fn logarithm_edge_cases() {
        let mut ctx = Ctx::default();
        assert!(logarithm(&mut ctx, &d("-1"), None).unwrap().is_nan());
        assert!(logarithm(&mut ctx, &d("1"), Some(&d("1"))).unwrap().is_nan(), "base 1");
        assert!(logarithm(&mut ctx, &d("1"), Some(&d("-2"))).unwrap().is_nan(), "negative base");
        assert!(logarithm(&mut ctx, &d("1"), Some(&d("0"))).unwrap().is_nan(), "zero base");

        let zero = logarithm(&mut ctx, &Decimal::zero(Sign::Pos), None).unwrap();
        assert!(zero.is_infinite() && zero.is_negative());
    }

    #[test]
    fn the_documented_undecidable_case_behaves_as_the_original_does() {
        // log[1048576](4503599627370502) is 2.60000000000000009610…, and no
        // finite inspection can tell it from 2.6. The original returns 2.6 and
        // so does this. Reproducing the limitation is the point.
        let mut ctx = Ctx::default();
        ctx.cfg.precision = 2;
        assert_eq!(log_str(&mut ctx, "4503599627370502", Some("1048576")), "2.6");
    }
}
