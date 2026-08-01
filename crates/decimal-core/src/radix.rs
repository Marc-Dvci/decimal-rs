//! Rendering a value in base 2, 8 or 16.
//!
//! # The strategy
//!
//! A decimal fraction rarely has a terminating expansion in another base, so
//! this cannot be done digit by digit. Instead the value is split: the digit
//! string is converted to the target base as an *integer*, and the fractional
//! part is restored by dividing by `10^(number of decimal places)`, itself
//! converted to the target base. The division runs through the same
//! [`divide`](crate::arith::divide) used everywhere else, in `base`-digit mode
//! — one digit per limb rather than seven — which is why that routine takes a
//! base parameter at all.
//!
//! The quotient's exactness comes back through `ctx.inexact`, and it feeds the
//! rounding decision: a quotient that was truncated has non-zero digits beyond
//! the ones present, which can turn a tie into a round-up.
//!
//! # The exponent suffix
//!
//! With an explicit significant-digit count the output carries a **binary**
//! exponent — `p+3` — regardless of the output base, which is the C99
//! hexadecimal-float convention and the inverse of the `0x1.8p3` form the
//! parser accepts. For base 16 and base 8 the mantissa is first re-grouped
//! into 4- or 3-bit units so that the leading digit is exactly 1, which is why
//! the significant-digit count is rescaled to `sd*4 - 3` or `sd*3 - 2` before
//! the conversion.

use crate::arith::divide;
use crate::error::{check_int32, Result};
use crate::format::{finite_to_string, non_finite_to_string};
use crate::parse::convert_base;
use crate::{Ctx, Decimal, MAX_DIGITS};

/// The digits used for bases up to 16.
const NUMERALS: &[u8] = b"0123456789abcdef";

/// Render `x` in `base_out`, which must be 2, 8 or 16.
///
/// `sd` present selects the exponent-suffixed form and is validated as the
/// original validates it.
pub fn to_string_binary(
    ctx: &mut Ctx,
    x: &Decimal,
    base_out: u32,
    sd: Option<f64>,
    rm: Option<f64>,
) -> Result<String> {
    let is_exp = sd.is_some();

    let (mut sd, rm) = match sd {
        Some(sd) => {
            let sd = check_int32(sd, 1, MAX_DIGITS)?;
            let rm = match rm {
                None => ctx.cfg.rounding,
                Some(rm) => check_int32(rm, 0, 8)? as u8,
            };
            (sd, rm)
        }
        None => (ctx.cfg.precision, ctx.cfg.rounding),
    };

    if !x.is_finite() {
        let str = non_finite_to_string(x).to_string();
        return Ok(if x.is_negative() {
            format!("-{str}")
        } else {
            str
        });
    }

    let mut str = finite_to_string(x, false, None);
    let point_at = str.find('.');

    // With an exponent suffix the mantissa is always built in base 2 and
    // re-grouped afterwards; without one it is built directly in the target
    // base.
    let base = if is_exp {
        if base_out == 16 {
            sd = sd * 4 - 3;
        } else if base_out == 8 {
            sd = sd * 3 - 2;
        }
        2
    } else {
        base_out
    };

    // The divisor that restores the fractional part: 10^(decimal places),
    // expressed in the working base.
    let divisor = point_at.map(|i| {
        // The point is removed *first*, and the place count is taken from the
        // shortened string — `str.length - i` after the replacement, not
        // before. Taking it before makes the divisor ten times too large, and
        // the result is then the wrong number rendered impeccably.
        str = str.replace('.', "");
        let places = str.len() as i64 - i as i64;
        // 1 followed by `places` zeros, converted digit by digit.
        let mut power = Decimal::from_i32(1);
        power.e = places;
        let decimal_text = finite_to_string(&power, false, None);
        let digits: Vec<u8> = decimal_text.bytes().map(|b| b - b'0').collect();
        let limbs = convert_base(&digits, 10, base);
        let e = limbs.len() as i64;
        Decimal::finite(crate::Sign::Pos, e, limbs)
    });

    let digits: Vec<u8> = str.bytes().map(|b| b - b'0').collect();
    let mut xd = convert_base(&digits, 10, base);
    let mut e = xd.len() as i64;

    while xd.last() == Some(&0) {
        xd.pop();
    }

    let mut str;
    if xd.is_empty() || xd[0] == 0 {
        str = if is_exp {
            "0p+0".to_string()
        } else {
            "0".to_string()
        };
    } else {
        let mut round_up = false;

        match divisor {
            None => e -= 1,
            Some(divisor) => {
                let mut numerator = x.clone();
                numerator.d = Some(xd.clone());
                numerator.e = e;
                let quotient = divide(ctx, &numerator, &divisor, Some(sd), rm, false, Some(base));
                xd = quotient.digits().to_vec();
                e = quotient.e;
                round_up = ctx.inexact;
            }
        }

        // The rounding digit is the one just past the requested count.
        let at = sd.max(0) as usize;
        let rounding_digit = xd.get(at).copied();
        let half = base / 2;
        round_up = round_up || xd.get(at + 1).is_some();

        round_up = if rm < 4 {
            (rounding_digit.is_some() || round_up)
                && (rm == 0 || rm == if x.is_negative() { 3 } else { 2 })
        } else {
            match rounding_digit {
                Some(d) if d > half => true,
                Some(d) if d == half => {
                    rm == 4
                        || round_up
                        || (rm == 6 && at > 0 && xd.get(at - 1).is_some_and(|w| w & 1 == 1))
                        || rm == if x.is_negative() { 8 } else { 7 }
                }
                _ => false,
            }
        };

        xd.truncate(at);
        // A quotient shorter than the requested count is padded so that the
        // carry below has somewhere to land; the trailing-zero scan that
        // follows removes any padding that was not needed.
        while xd.len() < at {
            xd.push(0);
        }

        if round_up {
            // The carry can cascade all the way out of the leading digit.
            let mut i = at;
            loop {
                if i == 0 {
                    e += 1;
                    xd.insert(0, 1);
                    break;
                }
                i -= 1;
                xd[i] += 1;
                if xd[i] < base {
                    break;
                }
                xd[i] = 0;
            }
        }

        // Trailing zeros are not part of the rendering.
        let mut len = xd.len();
        while len > 0 && xd[len - 1] == 0 {
            len -= 1;
        }

        str = xd[..len]
            .iter()
            .map(|&d| NUMERALS[d as usize] as char)
            .collect();

        if is_exp {
            if len > 1 {
                if base_out == 16 || base_out == 8 {
                    // Re-group the binary mantissa into 4- or 3-bit units, so
                    // that the leading digit is 1 as the C99 form requires.
                    let group = if base_out == 16 { 4 } else { 3 };
                    let mut padded_len = len - 1;
                    while !padded_len.is_multiple_of(group) {
                        str.push('0');
                        padded_len += 1;
                    }
                    let bits: Vec<u8> = str.bytes().map(|b| b - b'0').collect();
                    let regrouped = convert_base(&bits, base, base_out);
                    let mut end = regrouped.len();
                    while end > 0 && regrouped[end - 1] == 0 {
                        end -= 1;
                    }
                    str = String::from("1.");
                    for &d in &regrouped[1..end] {
                        str.push(NUMERALS[d as usize] as char);
                    }
                } else {
                    str = format!("{}.{}", &str[..1], &str[1..]);
                }
            }
            str.push_str(if e < 0 { "p" } else { "p+" });
            str.push_str(&e.to_string());
        } else if e < 0 {
            let mut leading = String::new();
            let mut remaining = e + 1;
            while remaining < 0 {
                leading.push('0');
                remaining += 1;
            }
            str = format!("0.{leading}{str}");
        } else {
            e += 1;
            if e > len as i64 {
                for _ in 0..(e - len as i64) {
                    str.push('0');
                }
            } else if e < len as i64 {
                str = format!("{}.{}", &str[..e as usize], &str[e as usize..]);
            }
        }
    }

    let prefix = match base_out {
        16 => "0x",
        2 => "0b",
        8 => "0o",
        _ => "",
    };
    let str = format!("{prefix}{str}");

    Ok(if x.is_negative() {
        format!("-{str}")
    } else {
        str
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_decimal;
    use crate::Sign;

    fn d(text: &str) -> Decimal {
        let ctx = Ctx::default();
        if let Some(rest) = text.strip_prefix('-') {
            parse_decimal(&ctx, Sign::Neg, rest)
        } else {
            parse_decimal(&ctx, Sign::Pos, text)
        }
    }

    fn render(ctx: &mut Ctx, text: &str, base: u32) -> String {
        to_string_binary(ctx, &d(text), base, None, None).expect("valid arguments")
    }

    /// All expectations read off upstream decimal.js in Node at precision 20.
    #[test]
    fn integers_render_in_every_base() {
        let mut ctx = Ctx::default();
        assert_eq!(render(&mut ctx, "255", 16), "0xff");
        assert_eq!(render(&mut ctx, "255", 8), "0o377");
        assert_eq!(render(&mut ctx, "255", 2), "0b11111111");
        assert_eq!(render(&mut ctx, "0", 16), "0x0");
        assert_eq!(render(&mut ctx, "1", 2), "0b1");
        assert_eq!(render(&mut ctx, "-255", 16), "-0xff");
    }

    #[test]
    fn fractions_go_through_the_division() {
        let mut ctx = Ctx::default();
        assert_eq!(render(&mut ctx, "0.5", 2), "0b0.1");
        assert_eq!(render(&mut ctx, "0.5", 16), "0x0.8");
        assert_eq!(render(&mut ctx, "1.5", 2), "0b1.1");
        assert_eq!(render(&mut ctx, "0.25", 2), "0b0.01");
    }

    #[test]
    fn non_finite_values_carry_no_prefix() {
        let mut ctx = Ctx::default();
        let inf = Decimal::infinity(Sign::Pos);
        assert_eq!(
            to_string_binary(&mut ctx, &inf, 16, None, None).unwrap(),
            "Infinity"
        );
        let nan = Decimal::nan();
        assert_eq!(
            to_string_binary(&mut ctx, &nan, 2, None, None).unwrap(),
            "NaN"
        );
        let neg = Decimal::infinity(Sign::Neg);
        assert_eq!(
            to_string_binary(&mut ctx, &neg, 8, None, None).unwrap(),
            "-Infinity"
        );
    }

    #[test]
    fn an_explicit_digit_count_adds_a_binary_exponent() {
        let mut ctx = Ctx::default();
        let text = to_string_binary(&mut ctx, &d("256"), 16, Some(1.0), None).unwrap();
        assert!(text.starts_with("0x1p+"), "got {text}");
    }

    #[test]
    fn out_of_range_arguments_are_rejected() {
        let mut ctx = Ctx::default();
        assert!(to_string_binary(&mut ctx, &d("1"), 16, Some(0.0), None).is_err());
        assert!(to_string_binary(&mut ctx, &d("1"), 16, Some(1.0), Some(9.0)).is_err());
    }

    #[test]
    fn round_trips_through_the_parser() {
        // Whatever this renders must parse back to the same value, which is
        // the property the two halves of the radix support have to share.
        let mut ctx = Ctx::default();
        for text in ["255", "1", "4096", "0.5", "0.25", "1.5"] {
            for base in [2u32, 8, 16] {
                let rendered = render(&mut ctx, text, base);
                let body = rendered.trim_start_matches('-');
                let parsed = crate::parse::parse_other(&mut ctx, Sign::Pos, body)
                    .expect("what we rendered must parse");
                let original = d(text);
                assert_eq!(
                    crate::arith::compare(&parsed, &original),
                    Some(core::cmp::Ordering::Equal),
                    "{text} in base {base} rendered as {rendered}"
                );
            }
        }
    }
}
