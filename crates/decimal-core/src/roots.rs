//! Square and cube roots.
//!
//! # The shape both share
//!
//! Each takes an initial estimate from IEEE double arithmetic, refines it by a
//! root-finding iteration at a working precision three digits above the
//! configured one, and stops when two successive iterates agree on their
//! leading `sd` digits. `sqrt` uses Newton–Raphson, `cbrt` uses Halley's
//! method.
//!
//! # The part that is not obvious
//!
//! Agreeing on `sd` digits is not enough to round correctly, and the original
//! is careful about it in a way that is worth spelling out, because a
//! reimplementation would almost certainly get it wrong and would almost
//! certainly appear to work.
//!
//! Once the iterates agree, the routine looks at the four digits sitting at
//! positions `sd-3 .. sd`, i.e. straddling the point where rounding will
//! happen. Two of the sixteen thousand possible values are treated specially:
//!
//! * `9999` — the true value may be just below a carry, and the fourth digit
//!   can be low by one, so rounding here could go the wrong way;
//! * `4999` — likewise, just below a half-way point.
//!
//! In either case the iteration continues with four more digits of working
//! precision. And on the *first* such occasion only, it first checks whether
//! rounding up gives a value whose square (or cube) is exactly the argument —
//! because if the argument is a perfect square, the nines repeat forever and
//! the loop would never terminate.
//!
//! Symmetrically, if those four digits are `0000` or `5000`-ish, the result may
//! be exactly representable; the routine truncates and squares to find out,
//! and records the answer in the `is_truncated` flag it hands to `finalise`.
//! That flag is what makes the difference between rounding `2.5` up and leaving
//! it alone.
//!
//! All of this is transcribed rather than reasoned about afresh. It is the
//! difference between a `sqrt` that is right and a `sqrt` that is right except
//! on the inputs a test suite is most likely to contain.

use crate::arith::{add, compare, divide, mul, sub};
use crate::config::rounding;
use crate::format::{digits_to_string, value_of};
use crate::round::finalise;
use crate::{Ctx, Decimal, Sign};

/// The value as an IEEE double — the original's unary `+x`, which goes through
/// `valueOf` and so through the same string the library would print.
pub fn to_f64(ctx: &Ctx, x: &Decimal) -> f64 {
    value_of(x, &ctx.cfg).parse::<f64>().unwrap_or(f64::NAN)
}

/// One half, used to average the two Newton iterates.
fn half() -> Decimal {
    Decimal::finite(Sign::Pos, -1, vec![5_000_000])
}

/// Build a decimal from an `f64` estimate, via the string the original would
/// have used.
fn from_estimate(ctx: &Ctx, estimate: f64) -> Decimal {
    crate::parse::parse_decimal(
        ctx,
        if estimate < 0.0 { Sign::Neg } else { Sign::Pos },
        &crate::format::number_to_string(estimate.abs()),
    )
}

/// The mantissa of `value.toExponential()` with `exponent` substituted for its
/// exponent — the original's
/// `n.slice(0, n.indexOf('e') + 1) + e`.
fn with_exponent(value: f64, exponent: i64) -> String {
    let text = format!("{value:e}");
    let mantissa = text.split_once('e').map(|(m, _)| m).unwrap_or(&text);
    format!("{mantissa}e{exponent}")
}

/// The four digits straddling the rounding position, as the original slices
/// them: `n.slice(sd - 3, sd + 1)`.
fn rounding_window(digits: &str, sd: i64) -> String {
    let start = (sd - 3).max(0) as usize;
    let end = (sd + 1).max(0) as usize;
    let bytes = digits.as_bytes();
    let start = start.min(bytes.len());
    let end = end.min(bytes.len());
    digits[start..end].to_string()
}

/// Whether two digit strings agree on their first `sd` characters, with
/// JavaScript's slicing convention that a short string yields all of itself.
fn agree_to(a: &str, b: &str, sd: i64) -> bool {
    let n = sd.max(0) as usize;
    let a = &a[..n.min(a.len())];
    let b = &b[..n.min(b.len())];
    a == b
}

/// `sqrt(x)`.
pub fn sqrt(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    // Negative, NaN, Infinity, or zero.
    if x.s != Sign::Pos || x.d.is_none() || x.digits()[0] == 0 {
        let non_zero = x.d.as_deref().map(|d| d[0] != 0).unwrap_or(true);
        return if x.is_nan() || (x.s.is_negative() && non_zero) {
            Decimal::nan()
        } else if x.is_finite() {
            // ±0 — the root of a signed zero is that same signed zero.
            x.clone()
        } else {
            Decimal::infinity(Sign::Pos)
        };
    }

    let precision = ctx.cfg.precision;
    let rm = ctx.cfg.rounding;
    let mut is_truncated = false;

    let result = ctx.without_clamping(|ctx| {
        // Initial estimate from double arithmetic, with a fallback for the
        // cases where the double under- or overflows: pass the digits as an
        // integer and correct the exponent afterwards.
        let estimate = to_f64(ctx, x).sqrt();
        let mut r = if estimate == 0.0 || estimate.is_infinite() {
            let mut n = digits_to_string(x.digits());
            let e = x.e;
            if (n.len() as i64 + e) % 2 == 0 {
                n.push('0');
            }
            let s = n.parse::<f64>().unwrap_or(f64::INFINITY).sqrt();
            let e = (e + 1).div_euclid(2) - i64::from(e < 0 || e % 2 != 0);
            let text = if s.is_infinite() {
                format!("5e{e}")
            } else {
                with_exponent(s, e)
            };
            crate::parse::parse_decimal(ctx, Sign::Pos, text.trim_start_matches('+'))
        } else {
            from_estimate(ctx, estimate)
        };

        let mut sd = precision + 3;
        let mut repeated = false;

        loop {
            let t = r.clone();

            // r = (t + x/t) / 2
            let quotient = divide(ctx, x, &t, Some(sd + 2), rounding::DOWN, false, None);
            let summed = add(ctx, &t, &quotient);
            r = mul(ctx, &summed, &half());

            let t_digits = digits_to_string(t.digits());
            let r_digits = digits_to_string(r.digits());

            if !agree_to(&t_digits, &r_digits, sd) {
                continue;
            }

            let window = rounding_window(&r_digits, sd);

            if window == "9999" || (!repeated && window == "4999") {
                // Approaching a rounding boundary, where the fourth digit may
                // be low by one. Before widening, check once whether rounding
                // up is exact — otherwise a perfect square would spin here for
                // ever on repeating nines.
                if !repeated {
                    let mut candidate = t.clone();
                    finalise(ctx, &mut candidate, Some(precision + 1), rounding::UP, false);
                    let squared = mul(ctx, &candidate, &candidate);
                    if compare(&squared, x) == Some(core::cmp::Ordering::Equal) {
                        r = candidate;
                        break;
                    }
                }
                sd += 4;
                repeated = true;
            } else {
                // Digits of `0000` or `5000`-ish mean the result may be exact.
                // Truncate and square to find out; anything else means there
                // are further non-zero digits, which is what `is_truncated`
                // tells `finalise`.
                let numeric: i64 = window.parse().unwrap_or(0);
                let tail_zero = window.len() > 1 && window[1..].parse::<i64>().unwrap_or(0) == 0;
                if numeric == 0 || (tail_zero && window.starts_with('5')) {
                    finalise(ctx, &mut r, Some(precision + 1), rounding::DOWN, false);
                    let squared = mul(ctx, &r, &r);
                    is_truncated = compare(&squared, x) != Some(core::cmp::Ordering::Equal);
                }
                break;
            }
        }

        r
    });

    let mut result = result;
    finalise(ctx, &mut result, Some(precision), rm, is_truncated);
    result
}

/// `cbrt(x)`.
///
/// Unlike `sqrt` this accepts negative arguments — every real has a real cube
/// root — and the sign is carried through the estimate by hand.
pub fn cbrt(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    if !x.is_finite() || x.is_zero() {
        return x.clone();
    }

    let precision = ctx.cfg.precision;
    let rm = ctx.cfg.rounding;
    let mut is_truncated = false;

    let result = ctx.without_clamping(|ctx| {
        let magnitude = to_f64(ctx, x).abs();
        let estimate = magnitude.cbrt();

        let mut r = if estimate == 0.0 || estimate.is_infinite() {
            let mut n = digits_to_string(x.digits());
            let e = x.e;

            // Pad the digit string so its length puts it a multiple of three
            // away from the exponent, which is what makes the cube root of the
            // integer part come out with the right scale.
            let adjust = (e - n.len() as i64 + 1) % 3;
            if adjust != 0 {
                n.push_str(if adjust == 1 || adjust == -2 { "0" } else { "00" });
            }
            let s = n.parse::<f64>().unwrap_or(f64::INFINITY).cbrt();

            // Rarely, `e` is one less than the result's exponent.
            let e = (e + 1).div_euclid(3)
                - i64::from(e % 3 == if e < 0 { -1 } else { 2 });

            let text = if s.is_infinite() {
                format!("5e{e}")
            } else {
                with_exponent(s, e)
            };
            let mut r = crate::parse::parse_decimal(ctx, Sign::Pos, text.trim_start_matches('+'));
            r.s = x.s;
            r
        } else {
            let mut r = from_estimate(ctx, estimate);
            r.s = x.s;
            r
        };

        let mut sd = precision + 3;
        let mut repeated = false;

        loop {
            let t = r.clone();

            // Halley's method for the cube root:
            //     r = t (t³ + 2x) / (2t³ + x)
            // written, as the original writes it, in terms of `t³ + x` so that
            // the shared subexpression is formed once.
            let t3 = {
                let square = mul(ctx, &t, &t);
                mul(ctx, &square, &t)
            };
            let t3_plus_x = add(ctx, &t3, x);
            let numerator = {
                let n = add(ctx, &t3_plus_x, x);
                mul(ctx, &n, &t)
            };
            let denominator = add(ctx, &t3_plus_x, &t3);
            r = divide(
                ctx,
                &numerator,
                &denominator,
                Some(sd + 2),
                rounding::DOWN,
                false,
                None,
            );

            let t_digits = digits_to_string(t.digits());
            let r_digits = digits_to_string(r.digits());

            if !agree_to(&t_digits, &r_digits, sd) {
                continue;
            }

            let window = rounding_window(&r_digits, sd);

            if window == "9999" || (!repeated && window == "4999") {
                if !repeated {
                    let mut candidate = t.clone();
                    finalise(ctx, &mut candidate, Some(precision + 1), rounding::UP, false);
                    let cubed = {
                        let square = mul(ctx, &candidate, &candidate);
                        mul(ctx, &square, &candidate)
                    };
                    if compare(&cubed, x) == Some(core::cmp::Ordering::Equal) {
                        r = candidate;
                        break;
                    }
                }
                sd += 4;
                repeated = true;
            } else {
                let numeric: i64 = window.parse().unwrap_or(0);
                let tail_zero = window.len() > 1 && window[1..].parse::<i64>().unwrap_or(0) == 0;
                if numeric == 0 || (tail_zero && window.starts_with('5')) {
                    finalise(ctx, &mut r, Some(precision + 1), rounding::DOWN, false);
                    let cubed = {
                        let square = mul(ctx, &r, &r);
                        mul(ctx, &square, &r)
                    };
                    is_truncated = compare(&cubed, x) != Some(core::cmp::Ordering::Equal);
                }
                break;
            }
        }

        r
    });

    let mut result = result;
    finalise(ctx, &mut result, Some(precision), rm, is_truncated);
    result
}

/// `|x|` without touching the context — used by callers that need a magnitude
/// mid-calculation.
pub(crate) fn magnitude(x: &Decimal) -> Decimal {
    let mut out = x.clone();
    if out.s.is_negative() {
        out.s = Sign::Pos;
    }
    out
}

/// `x - y` without rounding, for callers already inside `without_clamping`.
pub(crate) fn difference(ctx: &mut Ctx, x: &Decimal, y: &Decimal) -> Decimal {
    sub(ctx, x, y)
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

    /// Apply `f` and render the result. Binding the value before formatting
    /// keeps `ctx` from being borrowed mutably and immutably at once.
    fn run(ctx: &mut Ctx, f: fn(&mut Ctx, &Decimal) -> Decimal, x: &Decimal) -> String {
        let value = f(ctx, x);
        to_string(&value, &ctx.cfg)
    }

    /// Every expectation below was read off upstream decimal.js in Node at
    /// precision 20, not derived from this implementation.
    #[test]
    fn square_roots_of_perfect_squares_are_exact() {
        let mut ctx = Ctx::default();
        for (input, expected) in [
            ("0", "0"),
            ("1", "1"),
            ("4", "2"),
            ("9", "3"),
            ("144", "12"),
            ("1e20", "10000000000"),
            ("0.25", "0.5"),
        ] {
            let got = run(&mut ctx, sqrt, &d(input));
            assert_eq!(got, expected, "sqrt({input})");
        }
    }

    #[test]
    fn square_roots_of_irrationals_carry_the_full_precision() {
        let mut ctx = Ctx::default();
        assert_eq!(
            run(&mut ctx, sqrt, &&d("2")),
            "1.4142135623730950488"
        );
        assert_eq!(
            run(&mut ctx, sqrt, &&d("3")),
            "1.7320508075688772935"
        );
        assert_eq!(
            run(&mut ctx, sqrt, &&d("10")),
            "3.162277660168379332"
        );
    }

    #[test]
    fn square_root_edge_cases() {
        let mut ctx = Ctx::default();
        assert!(sqrt(&mut ctx, &d("-1")).is_nan(), "negative is NaN");
        assert!(sqrt(&mut ctx, &Decimal::nan()).is_nan());
        assert!(sqrt(&mut ctx, &Decimal::infinity(Sign::Pos)).is_infinite());
        assert!(sqrt(&mut ctx, &Decimal::infinity(Sign::Neg)).is_nan());

        // The root of a negative zero is a negative zero, not NaN.
        let neg_zero = sqrt(&mut ctx, &Decimal::zero(Sign::Neg));
        assert!(neg_zero.is_zero() && neg_zero.is_negative());
    }

    #[test]
    fn cube_roots_of_perfect_cubes_are_exact() {
        let mut ctx = Ctx::default();
        for (input, expected) in [
            ("0", "0"),
            ("1", "1"),
            ("8", "2"),
            ("27", "3"),
            ("1000", "10"),
            ("-8", "-2"),
            ("0.125", "0.5"),
        ] {
            let got = run(&mut ctx, cbrt, &d(input));
            assert_eq!(got, expected, "cbrt({input})");
        }
    }

    #[test]
    fn cube_roots_of_irrationals_carry_the_full_precision() {
        let mut ctx = Ctx::default();
        assert_eq!(
            run(&mut ctx, cbrt, &&d("2")),
            "1.2599210498948731648"
        );
        assert_eq!(
            run(&mut ctx, cbrt, &&d("-2")),
            "-1.2599210498948731648",
            "the cube root of a negative is real"
        );
    }

    #[test]
    fn roots_respect_the_configured_precision() {
        let mut ctx = Ctx::default();
        ctx.cfg.precision = 40;
        assert_eq!(
            run(&mut ctx, sqrt, &&d("2")),
            "1.41421356237309504880168872420969807857"
        );
        ctx.cfg.precision = 5;
        assert_eq!(run(&mut ctx, sqrt, &&d("2")), "1.4142");
    }

    #[test]
    fn very_large_and_very_small_arguments_use_the_fallback_estimate() {
        // These are the inputs where `Math.sqrt(+x)` under- or overflows, so
        // the estimate has to be built from the digit string instead.
        let mut ctx = Ctx::default();
        assert_eq!(run(&mut ctx, sqrt, &&d("1e400")), "1e+200");
        assert_eq!(run(&mut ctx, sqrt, &&d("1e-400")), "1e-200");
        assert_eq!(run(&mut ctx, cbrt, &&d("1e600")), "1e+200");
    }
}
