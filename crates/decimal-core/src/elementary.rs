//! The natural logarithm and the natural exponential.
//!
//! # Two series, and the machinery around them
//!
//! The series themselves are ordinary:
//!
//! ```text
//!     exp(x) = 1 + x + x²/2! + x³/3! + …
//!     ln(y)  = 2(t + t³/3 + t⁵/5 + …)        where t = (y−1)/(y+1)
//! ```
//!
//! Almost all of the code is the machinery around them, and all of the
//! difficulty is there too.
//!
//! **Argument reduction.** Both series converge quickly only near a particular
//! point. `exp` repeatedly halves the argument five bits at a time — dividing
//! by 2⁵ = 32, hence the `0.03125` — until `|x| < 0.1`, sums the series, and
//! then squares the result back `k` times. `ln` repeatedly *multiplies* the
//! argument by itself until its leading digits land in 7…13, which drives `t`
//! towards zero, and afterwards divides the sum by the number of
//! multiplications and adds back `e·ln(10)`.
//!
//! **Guard digits.** Reduction costs accuracy, so both raise the working
//! precision first. `exp` raises it by `2·log₁₀(2ᵏ) + 5`, a bound the original
//! records as empirical; `ln` raises it by a flat ten. The working precision is
//! written into the configuration for the duration and restored afterwards,
//! because every sub-operation reads it from there.
//!
//! **The restart.** Even at raised precision the sum can land ambiguously
//! close to a rounding boundary — the first four digits beyond the target come
//! out `9999` or `4999`, and the true value could be on either side.
//! [`check_rounding_digits`] detects that, and the summation is then *restarted
//! from scratch* at a higher precision. `exp` allows this three times; `ln`
//! allows it once. The original documents both with a worked counterexample,
//! and both are reproduced rather than reasoned about:
//!
//! ```text
//!   precision 18, rounding 1:
//!     exp(18.404272462595034083567793919843761)
//!       = 98372560.1229999999   without the restart
//!       = 98372560.123          with it
//!
//!   precision 12, rounding 1:
//!     ln(135520028.6126091714265381533)
//!       = 18.7246299999         without the restart
//!       = 18.72463              with it
//! ```
//!
//! **The overflow escape.** `ln`'s argument reduction multiplies `x` by itself
//! up to six times, which would overflow the exponent range for an argument
//! above about 10^1.5e15. Beyond that threshold it instead recurses on the
//! mantissa alone and adds `e·ln(10)` — the same identity, applied before the
//! reduction rather than after.
//!
//! **The truncation flag.** See [`TRUNCATED_BY_ASSIGNMENT`], which is the
//! subtlest thing in this file and is invisible in the original.

use crate::arith::{add, compare, divide, mul, sub};
use crate::config::rounding;
use crate::constants::{LN10, LN10_PRECISION};
use crate::format::digits_to_string;
use crate::round::finalise;
use crate::{digit_count, Ctx, Decimal, Error, Result, Sign, LOG_BASE};

/// The `isTruncated` argument the original passes when a series sum reaches
/// `finalise`, and the reason it is easy to miss.
///
/// All three sites are written like this:
///
/// ```js
/// return finalise(sum, Ctor.precision = pr, rm, external = true);
/// ```
///
/// `finalise(x, sd, rm, isTruncated)` takes four parameters. The fourth
/// argument here is `external = true` — an *assignment expression*, which
/// restores the global clamping flag **and** evaluates to `true`. So
/// `isTruncated` is true, and the line does two jobs while looking like it does
/// one. Read as bookkeeping, it disappears; read as an argument, it is the
/// difference between a right answer and a wrong one.
///
/// And it is the right value on the merits, not an accident of golfing: a
/// series sum is by definition a truncation of something infinite, so digits
/// beyond the rounding position always exist, even when the ones this
/// computation happened to produce are zeros. Without the flag,
///
/// ```text
///   ln(1.000000000000000000000000000000000000001) at precision 46, ROUND_UP
///     = 0.…9995              digits 41…46 came out zero, so nothing rounds up
///     = 0.…9995000001        knowing the tail is non-empty
/// ```
///
/// — the true tail sits around the 79th digit, far past any working precision
/// the routine uses, so the flag is the *only* thing that can carry it.
///
/// Named rather than written as a bare `true` so that the three call sites say
/// which `true` they mean.
const TRUNCATED_BY_ASSIGNMENT: bool = true;

/// `ln(10)` to `sd` digits, truncated.
///
/// Raises `[DecimalError] Precision limit exceeded` beyond the 1025 digits the
/// constant carries. The original is careful to restore the global state
/// *before* throwing, so that a caught exception does not leave the library
/// wedged with a raised precision; here that is structural, since the caller
/// restores on the way out either way.
pub fn get_ln10(ctx: &mut Ctx, sd: i64) -> Result<Decimal> {
    if sd > LN10_PRECISION {
        return Err(Error::PrecisionLimitExceeded);
    }
    let mut value = crate::parse::parse_decimal(ctx, Sign::Pos, LN10);
    finalise(ctx, &mut value, Some(sd), rounding::DOWN, true);
    Ok(value)
}

/// Whether the digits just beyond position `i` are too close to a rounding
/// boundary to decide the last digit.
///
/// Five rounding digits are inspected when `repeating` is `None` — the caller
/// is `log` or `pow` — and four otherwise, when the caller is `ln` or `exp`.
/// The `Option` is not decoration: the original distinguishes an *absent*
/// `repeating` from one that is present and zero, and takes a different branch
/// for each.
pub fn check_rounding_digits(d: &[u32], mut i: i64, rm: u8, repeating: Option<i64>) -> bool {
    // Reading past the end of the digit array yields `undefined` in the
    // original, whose arithmetic then produces NaN and whose `| 0` makes that
    // zero. These two helpers reproduce that rather than panicking.
    let limb = |index: i64| -> u64 {
        if index < 0 {
            0
        } else {
            d.get(index as usize).copied().map_or(0, u64::from)
        }
    };

    let mut k = d[0];
    while k >= 10 {
        k /= 10;
        i -= 1;
    }

    i -= 1;
    let di: i64;
    if i < 0 {
        i += LOG_BASE;
        di = 0;
    } else {
        di = (i + LOG_BASE) / LOG_BASE; // ceil((i + 1) / LOG_BASE)
        i %= LOG_BASE;
    }

    let k = 10u64.pow((LOG_BASE - i) as u32);
    let mut rd = limb(di) % k;
    let truthy = repeating.is_some_and(|r| r != 0);

    match repeating {
        None => {
            if i < 3 {
                if i == 0 {
                    rd /= 100;
                } else if i == 1 {
                    rd /= 10;
                }
                (rm < 4 && rd == 99_999) || (rm > 3 && rd == 49_999) || rd == 50_000 || rd == 0
            } else {
                ((rm < 4 && rd + 1 == k) || (rm > 3 && rd + 1 == k / 2))
                    && limb(di + 1) / k / 100 == 10u64.pow((i - 2) as u32) - 1
                    || (rd == k / 2 || rd == 0) && limb(di + 1) / k / 100 == 0
            }
        }
        Some(_) => {
            if i < 4 {
                if i == 0 {
                    rd /= 1000;
                } else if i == 1 {
                    rd /= 100;
                } else if i == 2 {
                    rd /= 10;
                }
                ((truthy || rm < 4) && rd == 9999) || (!truthy && rm > 3 && rd == 4999)
            } else {
                ((truthy || rm < 4) && rd + 1 == k || (!truthy && rm > 3) && rd + 1 == k / 2)
                    && limb(di + 1) / k / 1000 == 10u64.pow((i - 3) as u32) - 1
            }
        }
    }
}

/// Whether two values agree on their leading `n` digits.
fn agree(a: &Decimal, b: &Decimal, n: i64) -> bool {
    let a = digits_to_string(a.digits());
    let b = digits_to_string(b.digits());
    let n = n.max(0) as usize;
    a[..n.min(a.len())] == b[..n.min(b.len())]
}

/// `exp(x)`, rounded to `sd` digits, or to the configured precision when `sd`
/// is `None`.
pub fn natural_exponential(ctx: &mut Ctx, x: &Decimal, sd: Option<i64>) -> Decimal {
    let pr = ctx.cfg.precision;
    let rm = ctx.cfg.rounding;

    // Zero, non-finite, or so large that the result is certainly saturated:
    // `e > 17` puts the argument above 10¹⁷, whose exponential overflows any
    // representable range.
    if x.d.is_none() || x.digits()[0] == 0 || x.e > 17 {
        return if x.is_finite() {
            if x.is_zero() {
                Decimal::from_i32(1)
            } else if x.s.is_negative() {
                Decimal::zero(Sign::Pos)
            } else {
                Decimal::infinity(Sign::Pos)
            }
        } else if x.is_nan() {
            Decimal::nan()
        } else if x.s.is_negative() {
            Decimal::zero(Sign::Pos)
        } else {
            x.clone()
        };
    }

    let external_before = ctx.external;
    if sd.is_none() {
        ctx.external = false;
    }
    let mut wpr = sd.unwrap_or(pr);

    // Argument reduction: divide by 2⁵ until |x| < 0.1.
    let mut x = x.clone();
    let mut k: i64 = 0;
    let thirty_second = crate::parse::parse_decimal(ctx, Sign::Pos, "0.03125");
    while x.e > -2 {
        x = mul(ctx, &x, &thirty_second);
        k += 5;
    }

    // Guard digits: 2·log₁₀(2ᵏ) + 5, computed in floating point exactly as the
    // original computes it.
    let guard = ((2f64).powi(k as i32).ln() / core::f64::consts::LN_10 * 2.0 + 5.0) as i64;
    wpr += guard;

    let mut denominator = Decimal::from_i32(1);
    let mut pow = Decimal::from_i32(1);
    let mut sum = Decimal::from_i32(1);
    let mut i: i64 = 0;
    let mut rep = 0;
    ctx.cfg.precision = wpr;

    let result = loop {
        pow = mul(ctx, &pow, &x);
        finalise(ctx, &mut pow, Some(wpr), rounding::DOWN, false);

        i += 1;
        denominator = mul(ctx, &denominator, &Decimal::from_integer(i));

        let term = divide(ctx, &pow, &denominator, Some(wpr), rounding::DOWN, false, None);
        let t = add(ctx, &sum, &term);

        if agree(&t, &sum, wpr) {
            // Undo the argument reduction by squaring back k times.
            for _ in 0..k {
                sum = mul(ctx, &sum, &sum);
                finalise(ctx, &mut sum, Some(wpr), rounding::DOWN, false);
            }

            if sd.is_none() {
                if rep < 3 && check_rounding_digits(sum.digits(), wpr - guard, rm, Some(rep)) {
                    // Too close to a rounding boundary to decide. Start the
                    // whole summation again with ten more digits.
                    wpr += 10;
                    ctx.cfg.precision = wpr;
                    denominator = Decimal::from_i32(1);
                    pow = Decimal::from_i32(1);
                    sum = Decimal::from_i32(1);
                    i = 0;
                    rep += 1;
                    continue;
                }
                ctx.cfg.precision = pr;
                ctx.external = true;
                finalise(ctx, &mut sum, Some(pr), rm, TRUNCATED_BY_ASSIGNMENT);
                break sum;
            }

            ctx.cfg.precision = pr;
            break sum;
        }

        sum = t;
    };

    ctx.cfg.precision = pr;
    if sd.is_some() {
        ctx.external = external_before;
    }
    result
}

/// `ln(y)`, rounded to `sd` digits, or to the configured precision when `sd`
/// is `None`.
pub fn natural_logarithm(ctx: &mut Ctx, y: &Decimal, sd: Option<i64>) -> Result<Decimal> {
    let pr = ctx.cfg.precision;
    let rm = ctx.cfg.rounding;
    let guard: i64 = 10;

    // Negative, non-finite, zero, or exactly one.
    let is_one = y.is_finite() && y.e == 0 && y.digits() == [1];
    if y.s.is_negative() || y.d.is_none() || y.digits()[0] == 0 || is_one {
        return Ok(if y.is_finite() && y.digits()[0] == 0 {
            Decimal::infinity(Sign::Neg)
        } else if y.s != Sign::Pos {
            Decimal::nan()
        } else if y.is_finite() {
            Decimal::zero(Sign::Pos)
        } else {
            y.clone()
        });
    }

    let external_before = ctx.external;
    if sd.is_none() {
        ctx.external = false;
    }
    let mut wpr = sd.unwrap_or(pr) + guard;
    ctx.cfg.precision = wpr;

    let mut c = digits_to_string(y.digits());
    let mut c0 = c.as_bytes()[0] - b'0';
    let mut e = y.e;
    let mut n: i64 = 1;

    let x: Decimal;

    if e.abs() < 1_500_000_000_000_000 {
        // Argument reduction: square up until the leading digits are 7…13,
        // which is where the series converges fastest, recording how many
        // times so the sum can be divided by it afterwards.
        let mut work = y.clone();
        while (c0 < 7 && c0 != 1) || (c0 == 1 && c.as_bytes().get(1).is_some_and(|&b| b > b'3')) {
            work = mul(ctx, &work, y);
            c = digits_to_string(work.digits());
            c0 = c.as_bytes()[0] - b'0';
            n += 1;
        }

        e = work.e;

        // Separate the power of ten: ln(a·10ᵇ) = ln(a) + b·ln(10).
        if c0 > 1 {
            x = crate::parse::parse_decimal(ctx, Sign::Pos, &format!("0.{c}"));
            e += 1;
        } else {
            x = crate::parse::parse_decimal(ctx, Sign::Pos, &format!("{}.{}", c0, &c[1..]));
        }
    } else {
        // The reduction above would overflow for an argument this large, so
        // apply the identity first instead and recurse on the mantissa.
        let ln10 = get_ln10(ctx, wpr + 2)?;
        let scale = crate::parse::parse_decimal(ctx, Sign::Pos, &e.abs().to_string());
        let mut t = mul(ctx, &ln10, &scale);
        if e < 0 {
            t.s = t.s.negated();
        }
        let mantissa =
            crate::parse::parse_decimal(ctx, Sign::Pos, &format!("{}.{}", c0, &c[1..]));
        let inner = natural_logarithm(ctx, &mantissa, Some(wpr - guard))?;
        let mut result = add(ctx, &inner, &t);
        ctx.cfg.precision = pr;
        if sd.is_none() {
            ctx.external = true;
            finalise(ctx, &mut result, Some(pr), rm, TRUNCATED_BY_ASSIGNMENT);
        } else {
            ctx.external = external_before;
        }
        return Ok(result);
    }

    // x is now near 1. Sum the series in t = (x−1)/(x+1).
    let x1 = x.clone();
    let one = Decimal::from_i32(1);

    let mut t = {
        let numerator = sub(ctx, &x, &one);
        let denominator = add(ctx, &x, &one);
        divide(ctx, &numerator, &denominator, Some(wpr), rounding::DOWN, false, None)
    };
    let mut sum = t.clone();
    let mut numerator = t.clone();
    let mut x2 = mul(ctx, &t, &t);
    finalise(ctx, &mut x2, Some(wpr), rounding::DOWN, false);
    let mut denominator: i64 = 3;
    let mut rep: Option<i64> = None;

    let result = loop {
        numerator = mul(ctx, &numerator, &x2);
        finalise(ctx, &mut numerator, Some(wpr), rounding::DOWN, false);

        let divisor = Decimal::from_integer(denominator);
        let term = divide(ctx, &numerator, &divisor, Some(wpr), rounding::DOWN, false, None);
        let next = add(ctx, &sum, &term);

        if agree(&next, &sum, wpr) {
            sum = mul(ctx, &sum, &Decimal::from_i32(2));

            // Reverse the argument reduction. The `e != 0` guard is not an
            // optimisation: −0 + 0 is +0, and a negative zero has to stay
            // negative for the rounding to come out right.
            if e != 0 {
                let ln10 = get_ln10(ctx, wpr + 2)?;
                let scale = crate::parse::parse_decimal(ctx, Sign::Pos, &e.abs().to_string());
                let mut contribution = mul(ctx, &ln10, &scale);
                if e < 0 {
                    contribution.s = contribution.s.negated();
                }
                sum = add(ctx, &sum, &contribution);
            }
            let divisor = Decimal::from_integer(n);
            sum = divide(ctx, &sum, &divisor, Some(wpr), rounding::DOWN, false, None);

            if sd.is_none() {
                if check_rounding_digits(sum.digits(), wpr - guard, rm, rep) {
                    wpr += guard;
                    ctx.cfg.precision = wpr;
                    let numerator_seed = {
                        let a = sub(ctx, &x1, &one);
                        let b = add(ctx, &x1, &one);
                        divide(ctx, &a, &b, Some(wpr), rounding::DOWN, false, None)
                    };
                    t = numerator_seed.clone();
                    numerator = numerator_seed.clone();
                    sum = numerator_seed;
                    x2 = mul(ctx, &t, &t);
                    finalise(ctx, &mut x2, Some(wpr), rounding::DOWN, false);
                    denominator = 1;
                    rep = Some(1);
                    denominator += 2;
                    continue;
                }
                ctx.cfg.precision = pr;
                ctx.external = true;
                finalise(ctx, &mut sum, Some(pr), rm, TRUNCATED_BY_ASSIGNMENT);
                break sum;
            }

            ctx.cfg.precision = pr;
            break sum;
        }

        sum = next;
        denominator += 2;
    };

    ctx.cfg.precision = pr;
    if sd.is_some() {
        ctx.external = external_before;
    }
    Ok(result)
}

/// `ln(x)` at the configured precision.
pub fn ln(ctx: &mut Ctx, x: &Decimal) -> Result<Decimal> {
    natural_logarithm(ctx, x, None)
}

/// `exp(x)` at the configured precision.
pub fn exp(ctx: &mut Ctx, x: &Decimal) -> Decimal {
    natural_exponential(ctx, x, None)
}

/// The number of digits of `x` — used by `log` to spot exact powers of ten.
pub(crate) fn leading_digit_count(x: &Decimal) -> i64 {
    digit_count(x.digits()[0])
}

/// Whether `x` is exactly a power of ten, and if so which.
pub(crate) fn power_of_ten(x: &Decimal) -> Option<i64> {
    (x.digits() == [1]).then_some(x.e)
}

/// A comparison helper shared with `log`.
pub(crate) fn equals(a: &Decimal, b: &Decimal) -> bool {
    compare(a, b) == Some(core::cmp::Ordering::Equal)
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

    fn show_ln(ctx: &mut Ctx, text: &str) -> String {
        let value = ln(ctx, &d(text)).expect("within the constant's precision");
        to_string(&value, &ctx.cfg)
    }

    fn show_exp(ctx: &mut Ctx, text: &str) -> String {
        let value = exp(ctx, &d(text));
        to_string(&value, &ctx.cfg)
    }

    /// Every expectation below was read off upstream decimal.js in Node.
    #[test]
    fn natural_logarithms_of_familiar_values() {
        let mut ctx = Ctx::default();
        assert_eq!(show_ln(&mut ctx, "1"), "0");
        assert_eq!(show_ln(&mut ctx, "2"), "0.69314718055994530942");
        assert_eq!(show_ln(&mut ctx, "10"), "2.302585092994045684");
        assert_eq!(show_ln(&mut ctx, "0.5"), "-0.69314718055994530942");
    }

    #[test]
    fn natural_exponentials_of_familiar_values() {
        let mut ctx = Ctx::default();
        assert_eq!(show_exp(&mut ctx, "0"), "1");
        assert_eq!(show_exp(&mut ctx, "1"), "2.7182818284590452354");
        assert_eq!(show_exp(&mut ctx, "-1"), "0.3678794411714423216");
        assert_eq!(show_exp(&mut ctx, "2"), "7.3890560989306502272");
    }

    #[test]
    fn logarithm_edge_cases() {
        let mut ctx = Ctx::default();
        assert!(ln(&mut ctx, &d("-1")).unwrap().is_nan(), "ln of a negative");
        assert!(ln(&mut ctx, &Decimal::nan()).unwrap().is_nan());

        let zero = ln(&mut ctx, &Decimal::zero(Sign::Pos)).unwrap();
        assert!(zero.is_infinite() && zero.is_negative(), "ln(0) is -Infinity");

        assert!(ln(&mut ctx, &Decimal::infinity(Sign::Pos)).unwrap().is_infinite());
        assert!(ln(&mut ctx, &Decimal::infinity(Sign::Neg)).unwrap().is_nan());
    }

    #[test]
    fn exponential_edge_cases() {
        let mut ctx = Ctx::default();
        assert!(exp(&mut ctx, &Decimal::nan()).is_nan());
        assert!(exp(&mut ctx, &Decimal::infinity(Sign::Pos)).is_infinite());
        assert!(exp(&mut ctx, &Decimal::infinity(Sign::Neg)).is_zero());
        // Above 10^17 the result saturates rather than being computed.
        assert!(exp(&mut ctx, &d("1e18")).is_infinite());
        assert!(exp(&mut ctx, &d("-1e18")).is_zero());
    }

    #[test]
    fn the_two_are_inverse_to_the_working_precision() {
        let mut ctx = Ctx::default();
        for text in ["2", "3", "10", "0.5", "1234.5678"] {
            let value = d(text);
            let logged = ln(&mut ctx, &value).unwrap();
            let back = exp(&mut ctx, &logged);
            // The round trip loses a digit or two to the two roundings, so
            // compare at a slightly reduced precision.
            let difference = crate::arith::sub(&mut ctx, &back, &value);
            let relative =
                divide(&mut ctx, &difference, &value, Some(20), rounding::HALF_UP, false, None);
            assert!(
                relative.is_zero() || relative.e < -17,
                "exp(ln({text})) round-trips, got relative error {}",
                to_string(&relative, &ctx.cfg)
            );
        }
    }

    #[test]
    fn the_documented_restart_counterexample_comes_out_right() {
        // The original records this case in a comment: without restarting the
        // summation at higher precision it produces 98372560.1229999999.
        let mut ctx = Ctx::default();
        ctx.cfg.precision = 18;
        ctx.cfg.rounding = rounding::DOWN;
        assert_eq!(
            show_exp(&mut ctx, "18.404272462595034083567793919843761"),
            "98372560.123"
        );
    }

    #[test]
    fn the_documented_logarithm_counterexample_comes_out_right() {
        // Likewise: without the restart this is 18.7246299999.
        let mut ctx = Ctx::default();
        ctx.cfg.precision = 12;
        ctx.cfg.rounding = rounding::DOWN;
        assert_eq!(show_ln(&mut ctx, "135520028.6126091714265381533"), "18.72463");
    }

    #[test]
    fn asking_for_more_digits_than_ln10_carries_is_an_error() {
        let mut ctx = Ctx::default();
        assert_eq!(
            get_ln10(&mut ctx, LN10_PRECISION + 1).unwrap_err(),
            Error::PrecisionLimitExceeded
        );
        assert!(get_ln10(&mut ctx, LN10_PRECISION).is_ok());
    }

    /// The series sum is a truncation, and `finalise` has to be told so.
    ///
    /// Both cases round *up* at a precision where every digit of the computed
    /// tail came out zero, so only [`TRUNCATED_BY_ASSIGNMENT`] can distinguish
    /// "the tail is zero" from "the tail was never reached". The true tails
    /// here sit near the 79th and 140th digits, far past any working precision
    /// these routines use. Both expectations are the original suite's own, and
    /// both lose their last digits without the flag.
    #[test]
    fn a_sum_that_stops_short_still_rounds_as_if_it_had_not() {
        let mut ctx = Ctx::default();
        ctx.cfg.rounding = rounding::UP;
        ctx.cfg.to_exp_neg = -9_000_000_000_000_000;

        ctx.cfg.precision = 46;
        assert_eq!(
            show_ln(&mut ctx, "1.000000000000000000000000000000000000001"),
            concat!(
                "0.0000000000000000000000000000000000000009999999999999999999999999999999999999",
                "995000001"
            )
        );

        ctx.cfg.precision = 85;
        assert_eq!(
            show_ln(
                &mut ctx,
                "1.0000000000000000000000000000000000000000000000000001230000000000756"
            ),
            concat!(
                "0.0000000000000000000000000000000000000000000000000001230000000000755999999999",
                "999999999999999999999999999924354999999907011999999971423201"
            )
        );
    }
}
