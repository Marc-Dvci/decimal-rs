//! Turning a value back into text.
//!
//! Two quite different jobs live here.
//!
//! The first is formatting a [`Decimal`], which is a matter of walking the
//! limb array and deciding where the point goes — [`digits_to_string`] and
//! [`finite_to_string`].
//!
//! The second is [`number_to_string`], which reproduces the ECMAScript
//! `Number::toString` algorithm for an IEEE double. That one is here because
//! the original reaches JavaScript's own number-to-string conversion in three
//! places that are all observable — constructing from a number, `toNumber`,
//! and interpolating an offending value into an error message — and Rust's
//! `{}` is *not* a drop-in replacement for it. See that function's own notes.

use crate::{Config, Decimal, Sign, LOG_BASE};

/// The decimal digits of a finite value, with no point, no sign, and no
/// exponent: just the significant digits, leading digit first.
///
/// The last limb needs care. Interior limbs are written with their leading
/// zeros, because a limb of `42` in the middle of a number means `0000042`.
/// The final limb needs its leading zeros too, but must then have its
/// *trailing* zeros removed, since those are padding rather than significance.
/// The original achieves both by emitting the padding first and the digits
/// last, which is why the loop below looks lopsided.
pub fn digits_to_string(d: &[u32]) -> String {
    let last = d.len() - 1;
    let mut out = String::with_capacity(d.len() * LOG_BASE as usize);

    let mut w = d[0];

    if last > 0 {
        out.push_str(&w.to_string());
        for &limb in &d[1..last] {
            let text = limb.to_string();
            for _ in text.len()..LOG_BASE as usize {
                out.push('0');
            }
            out.push_str(&text);
        }

        w = d[last];
        let width = w.to_string().len();
        for _ in width..LOG_BASE as usize {
            out.push('0');
        }
    } else if w == 0 {
        return "0".to_string();
    }

    // Trailing zeros of the final limb are padding, not digits. The invariant
    // guarantees `w != 0` here, so this terminates.
    debug_assert!(
        w != 0,
        "the final limb is non-zero unless the value is zero"
    );
    while w.is_multiple_of(10) {
        w /= 10;
    }
    out.push_str(&w.to_string());

    out
}

/// ±Infinity and NaN, unsigned.
///
/// The original writes this as `String(x.s * x.s / 0)`, which is `Infinity`
/// for either sign and `NaN` when the sign is itself NaN. The caller adds the
/// minus.
pub fn non_finite_to_string(x: &Decimal) -> &'static str {
    if x.s == Sign::Nan {
        "NaN"
    } else {
        "Infinity"
    }
}

/// The unsigned text of a value, in fixed or exponential notation.
///
/// `sd`, when given, is a minimum digit count: the result is padded with
/// zeros out to that many significant digits (in exponential form) or that
/// many total digits (in fixed form). `toExponential`, `toFixed` and
/// `toPrecision` all reach the formatter through this parameter.
pub fn finite_to_string(x: &Decimal, is_exp: bool, sd: Option<i64>) -> String {
    if !x.is_finite() {
        return non_finite_to_string(x).to_string();
    }

    let e = x.e;
    let mut str = digits_to_string(x.digits());
    let len = str.len() as i64;

    if is_exp {
        if let Some(sd) = sd {
            let k = sd - len;
            if k > 0 {
                str = with_point_after_first(&str) + &zeros(k);
            } else if len > 1 {
                str = with_point_after_first(&str);
            }
        } else if len > 1 {
            str = with_point_after_first(&str);
        }
        str.push_str(if e < 0 { "e" } else { "e+" });
        str.push_str(&e.to_string());
    } else if e < 0 {
        // Value below one: a zero, a point, then the digits pushed right.
        str = format!("0.{}{}", zeros(-e - 1), str);
        if let Some(sd) = sd {
            let k = sd - len;
            if k > 0 {
                str.push_str(&zeros(k));
            }
        }
    } else if e >= len {
        // Value is a whole number wider than its significant digits.
        str.push_str(&zeros(e + 1 - len));
        if let Some(sd) = sd {
            let k = sd - e - 1;
            if k > 0 {
                str.push('.');
                str.push_str(&zeros(k));
            }
        }
    } else {
        // The point falls inside the digits.
        let k = e + 1;
        if k < len {
            str = format!("{}.{}", &str[..k as usize], &str[k as usize..]);
        }
        if let Some(sd) = sd {
            let pad = sd - len;
            if pad > 0 {
                if e + 1 == len {
                    str.push('.');
                }
                str.push_str(&zeros(pad));
            }
        }
    }

    str
}

/// The value as `toString` renders it: exponential notation outside the
/// `[toExpNeg, toExpPos]` window, and a minus sign for every negative value
/// **except** negative zero.
///
/// That exception is not an oversight in the original — `valueOf` deliberately
/// differs from `toString` on exactly this point, and the test suite checks
/// both.
pub fn to_string(x: &Decimal, cfg: &Config) -> String {
    let str = finite_to_string(x, uses_exponential(x, cfg), None);
    if x.is_negative() && !x.is_zero() {
        format!("-{str}")
    } else {
        str
    }
}

/// The value as `valueOf` and `toJSON` render it: as [`to_string`], but
/// negative zero keeps its sign.
pub fn value_of(x: &Decimal, cfg: &Config) -> String {
    let str = finite_to_string(x, uses_exponential(x, cfg), None);
    if x.is_negative() {
        format!("-{str}")
    } else {
        str
    }
}

/// A value as it appears when interpolated into an error message.
///
/// The original writes `throw Error(invalidArgument + max)`, and `+` applied to
/// an object is not `toString()`. String concatenation calls `ToPrimitive` with
/// the **default** hint, which tries `valueOf` first and falls back to
/// `toString` only if that returns an object — and `valueOf` is the one
/// rendering in this library that shows the sign of a negative zero.
///
/// So `new Decimal(1).clamp(1, -0)` raises `[DecimalError] Invalid argument:
/// -0`, and not `: 0`. One character, in an error message, decided by an
/// implicit coercion rule three specifications deep.
///
/// Found by the differential fuzzer. The original's own suite has no assertion
/// for it, and the port said `0` here until it did.
pub fn interpolated(x: &Decimal, cfg: &Config) -> String {
    value_of(x, cfg)
}

/// Whether the configured thresholds put this value in exponential notation.
fn uses_exponential(x: &Decimal, cfg: &Config) -> bool {
    x.e <= cfg.to_exp_neg || x.e >= cfg.to_exp_pos
}

fn with_point_after_first(s: &str) -> String {
    format!("{}.{}", &s[..1], &s[1..])
}

fn zeros(k: i64) -> String {
    if k <= 0 {
        String::new()
    } else {
        "0".repeat(k as usize)
    }
}

/// The ECMAScript `Number::toString` of an IEEE double.
///
/// # Why this is written out rather than delegated to `{}`
///
/// Rust and JavaScript agree on the *digits*: both emit the shortest decimal
/// string that round-trips back to the same double. They disagree on the
/// *presentation*, in three ways, and each one is observable through the
/// original's test suite:
///
/// | value      | JavaScript | Rust `{}`     |
/// |------------|------------|---------------|
/// | `1e21`     | `1e+21`    | `1000000000000000000000` |
/// | `1e-7`     | `1e-7`     | `0.0000001`   |
/// | `1e21`     | `1e+21`    | — note the `+`, which Rust never emits |
///
/// JavaScript switches to exponential notation at 10²¹ and below 10⁻⁶; Rust
/// never switches at all for `{}`, and its `{:e}` always switches. And where
/// JavaScript does use exponential form it writes an explicit `+` on a
/// positive exponent.
///
/// So the digits are taken from Rust — `{:e}` gives the same shortest
/// round-trip digits the specification asks for — and the presentation rules
/// are applied here, following the algorithm in ECMA-262 §6.1.6.1.20 with its
/// variables `s`, `k` and `n`: `s` is the digit string, `k` its length, and
/// the value is `s × 10^(n-k)`.
pub fn number_to_string(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v == 0.0 {
        // Both zeros render as "0"; the sign of a negative zero is not shown,
        // matching `String(-0)`.
        return "0".to_string();
    }
    if v < 0.0 {
        return format!("-{}", number_to_string(-v));
    }
    if v.is_infinite() {
        return "Infinity".to_string();
    }

    // How many digits the shortest round-tripping form needs. Rust's
    // formatter is authoritative on the *count* — that is a property of the
    // double, not a matter of convention.
    let shortest = format!("{v:e}");
    let (mantissa, _) = shortest
        .split_once('e')
        .expect("Rust's LowerExp always emits an exponent");
    let k = mantissa.chars().filter(char::is_ascii_digit).count();

    // ...but not on *which* digits, when two candidates are equidistant. Round
    // the exact expansion to that many places, half-to-even, which is what
    // ECMAScript specifies. See the `exact` module for the case that forced
    // this.
    let (exact_digits, exact_n) = crate::exact::exact_decimal(v);
    let (digits, carried) = crate::exact::round_half_even(&exact_digits, k);
    let n = exact_n + i64::from(carried);
    let k = k as i64;

    if k <= n && n <= 21 {
        // Whole number, possibly with trailing zeros to add.
        format!("{digits}{}", zeros(n - k))
    } else if 0 < n && n <= 21 {
        // The point falls inside the digits.
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        // Small enough for a leading "0." but not for exponential form.
        format!("0.{}{digits}", zeros(-n))
    } else if k == 1 {
        format!("{digits}e{}{}", sign_of(n - 1), (n - 1).abs())
    } else {
        format!(
            "{}.{}e{}{}",
            &digits[..1],
            &digits[1..],
            sign_of(n - 1),
            (n - 1).abs()
        )
    }
}

fn sign_of(n: i64) -> char {
    if n < 0 {
        '-'
    } else {
        '+'
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_decimal;
    use crate::Ctx;

    fn parse(text: &str) -> Decimal {
        parse_decimal(&Ctx::default(), Sign::Pos, text)
    }

    #[test]
    fn digits_come_out_the_way_they_went_in() {
        for text in [
            "1",
            "12",
            "1234567",
            "12345678",
            "1234567890123456789012345",
            "100000000000001",
        ] {
            assert_eq!(digits_to_string(parse(text).digits()), text);
        }
    }

    #[test]
    fn interior_limbs_keep_their_leading_zeros() {
        // 1 followed by seven zeros then 1: the middle limb is 0000001 and
        // must not print as "1".
        let x = parse("100000001");
        assert_eq!(digits_to_string(x.digits()), "100000001");
    }

    #[test]
    fn zero_prints_as_a_single_digit() {
        assert_eq!(digits_to_string(&[0]), "0");
    }

    #[test]
    fn fixed_notation_places_the_point() {
        let cfg = Config::default();
        assert_eq!(to_string(&parse("1.5"), &cfg), "1.5");
        assert_eq!(to_string(&parse("0.001"), &cfg), "0.001");
        assert_eq!(to_string(&parse("1000"), &cfg), "1000");
        assert_eq!(to_string(&parse("123.456"), &cfg), "123.456");
    }

    #[test]
    fn the_thresholds_switch_to_exponential_notation() {
        let cfg = Config::default(); // toExpNeg -7, toExpPos 21
        assert_eq!(to_string(&parse("1e20"), &cfg), "100000000000000000000");
        assert_eq!(to_string(&parse("1e21"), &cfg), "1e+21");
        assert_eq!(to_string(&parse("1e-6"), &cfg), "0.000001");
        assert_eq!(to_string(&parse("1e-7"), &cfg), "1e-7");
    }

    #[test]
    fn to_string_hides_the_sign_of_negative_zero_but_value_of_shows_it() {
        let cfg = Config::default();
        let neg_zero = Decimal::zero(Sign::Neg);
        assert_eq!(to_string(&neg_zero, &cfg), "0");
        assert_eq!(value_of(&neg_zero, &cfg), "-0");

        let neg_one = parse_decimal(&Ctx::default(), Sign::Neg, "1");
        assert_eq!(to_string(&neg_one, &cfg), "-1");
        assert_eq!(value_of(&neg_one, &cfg), "-1");
    }

    #[test]
    fn non_finite_values_have_names_not_digits() {
        let cfg = Config::default();
        assert_eq!(to_string(&Decimal::nan(), &cfg), "NaN");
        assert_eq!(to_string(&Decimal::infinity(Sign::Pos), &cfg), "Infinity");
        assert_eq!(to_string(&Decimal::infinity(Sign::Neg), &cfg), "-Infinity");
    }

    /// The cases where Rust's own formatting would have been wrong. Each
    /// expectation is what `String(v)` produces in Node.
    #[test]
    fn number_to_string_follows_ecmascript_not_rust() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.0, "-1"),
            (0.1, "0.1"),
            (1.5, "1.5"),
            (100.0, "100"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (1.5e21, "1.5e+21"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (1e100, "1e+100"),
            (5e-324, "5e-324"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (f64::NAN, "NaN"),
            (0.000001, "0.000001"),
            (1234567890123456789.0, "1234567890123456800"),
        ];
        for &(value, expected) in cases {
            assert_eq!(number_to_string(value), expected, "String({value:?})");
        }
    }

    #[test]
    fn number_to_string_switches_exactly_at_the_thresholds() {
        // The boundary cases are the ones that break silently.
        assert_eq!(
            number_to_string(999999999999999900000.0),
            "999999999999999900000"
        );
        assert!(number_to_string(1e21).contains("e+"), "1e21 is exponential");
        assert!(!number_to_string(1e20).contains('e'), "1e20 is not");
        assert!(!number_to_string(1e-6).contains('e'), "1e-6 is not");
        assert!(number_to_string(1e-7).contains("e-"), "1e-7 is");
    }
}
