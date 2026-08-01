//! Turning text into a value.
//!
//! The original dispatches on four regular expressions — one for ordinary
//! decimal literals and one each for the hexadecimal, binary and octal forms —
//! and sends anything the decimal pattern matches to `parseDecimal`, anything
//! else to `parseOther`.
//!
//! Those patterns are reproduced here as hand-written recognisers rather than
//! by taking a regular-expression dependency. That is partly to keep the
//! crate's dependency count at zero, and partly because the patterns are
//! *anchored, tiny, and load-bearing*: `isDecimal` is what decides whether
//! `'1_000'` is a number or an error, and writing it out makes the answer
//! legible instead of hiding it in a character class. Each recogniser below
//! states the pattern it implements, so the two can be compared directly.

/// `/^(\d+(\.\d*)?|\.\d+)(e[+-]?\d+)?$/i`
///
/// An unsigned decimal literal: digits with an optional fractional part, or a
/// bare fractional part, optionally followed by a decimal exponent. The sign
/// has already been stripped by the caller.
///
/// Note what this deliberately does *not* accept: an empty string, a lone
/// `'.'`, a bare exponent such as `'e5'`, whitespace, and underscore
/// separators. Underscores are handled further along, in the "other" path,
/// which strips them and re-tests.
pub fn is_decimal_literal(s: &[u8]) -> bool {
    let mut at = 0;

    // `\d+(\.\d*)?`  or  `\.\d+`
    let integer_digits = count_digits(s, &mut at);
    if integer_digits == 0 {
        // No integer part, so a point followed by at least one digit is the
        // only remaining possibility.
        if !eat(s, &mut at, b'.') {
            return false;
        }
        if count_digits(s, &mut at) == 0 {
            return false;
        }
    } else if eat(s, &mut at, b'.') {
        // A trailing point with no digits after it is allowed: `1.` is valid.
        count_digits(s, &mut at);
    }

    // `(e[+-]?\d+)?`
    if at < s.len() && (s[at] | 0x20) == b'e' {
        at += 1;
        if at < s.len() && (s[at] == b'+' || s[at] == b'-') {
            at += 1;
        }
        if count_digits(s, &mut at) == 0 {
            return false;
        }
    }

    at == s.len()
}

/// `/^0x([0-9a-f]+(\.[0-9a-f]*)?|\.[0-9a-f]+)(p[+-]?\d+)?$/i`
pub fn is_hex_literal(s: &[u8]) -> bool {
    is_radix_literal(s, b'x', 16)
}

/// `/^0b([01]+(\.[01]*)?|\.[01]+)(p[+-]?\d+)?$/i`
pub fn is_binary_literal(s: &[u8]) -> bool {
    is_radix_literal(s, b'b', 2)
}

/// `/^0o([0-7]+(\.[0-7]*)?|\.[0-7]+)(p[+-]?\d+)?$/i`
pub fn is_octal_literal(s: &[u8]) -> bool {
    is_radix_literal(s, b'o', 8)
}

/// The shared shape of the three non-decimal literals: a two-character prefix,
/// digits of the given radix with an optional fractional part, and an optional
/// *binary* exponent introduced by `p`.
///
/// The `p` suffix is the easily-missed part of this grammar. `0x1.8p3` is
/// twelve: the mantissa is read in the stated radix, but the exponent that
/// follows `p` is a power of **two**, in decimal. It is the C99 hex-float
/// syntax, and the constructor tests exercise it.
fn is_radix_literal(s: &[u8], marker: u8, radix: u32) -> bool {
    if s.len() < 2 || s[0] != b'0' || (s[1] | 0x20) != marker {
        return false;
    }
    let mut at = 2;

    let integer_digits = count_radix_digits(s, &mut at, radix);
    if integer_digits == 0 {
        if !eat(s, &mut at, b'.') {
            return false;
        }
        if count_radix_digits(s, &mut at, radix) == 0 {
            return false;
        }
    } else if eat(s, &mut at, b'.') {
        count_radix_digits(s, &mut at, radix);
    }

    if at < s.len() && (s[at] | 0x20) == b'p' {
        at += 1;
        if at < s.len() && (s[at] == b'+' || s[at] == b'-') {
            at += 1;
        }
        if count_digits(s, &mut at) == 0 {
            return false;
        }
    }

    at == s.len()
}

/// Advance past a run of decimal digits, returning how many there were.
fn count_digits(s: &[u8], at: &mut usize) -> usize {
    let start = *at;
    while *at < s.len() && s[*at].is_ascii_digit() {
        *at += 1;
    }
    *at - start
}

/// Advance past a run of digits valid in `radix`, returning how many.
fn count_radix_digits(s: &[u8], at: &mut usize, radix: u32) -> usize {
    let start = *at;
    while *at < s.len() && (s[*at] as char).is_digit(radix) {
        *at += 1;
    }
    *at - start
}

/// Consume `c` if it is next.
fn eat(s: &[u8], at: &mut usize, c: u8) -> bool {
    if *at < s.len() && s[*at] == c {
        *at += 1;
        true
    } else {
        false
    }
}

use crate::{Ctx, Decimal, Sign, LOG_BASE};

/// Parse a decimal literal — one that [`is_decimal_literal`] accepts — into a
/// value with the given sign.
///
/// # The shape of the algorithm
///
/// The original does this in three movements, and so does this:
///
///   1. **Flatten.** Remove the decimal point and fold the explicit exponent
///      into a single base-10 exponent `e`, so that the input becomes a bare
///      digit string scaled by a power of ten.
///   2. **Trim.** Strip leading and trailing zeros. Leading zeros move `e`;
///      trailing zeros do not affect the value at all. If nothing survives,
///      the value is zero.
///   3. **Pack.** Cut the digit string into base-10⁷ limbs. The cut is *not*
///      simply every seven characters from the left: it is aligned so that the
///      decimal point falls on a limb boundary, which is what makes the
///      exponent arithmetic in every other routine work. The first limb is
///      therefore usually short, and the last is padded with zeros on the
///      right.
pub fn parse_decimal(ctx: &Ctx, s: Sign, text: &str) -> Decimal {
    let mut digits: Vec<u8> = Vec::with_capacity(text.len());
    let mut point_at: Option<usize> = None;
    let mut exponent_part: i64 = 0;

    // -- 1. Flatten ------------------------------------------------------
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'.' => point_at = Some(digits.len()),
            b'e' | b'E' => {
                exponent_part = parse_exponent(&text.as_bytes()[index + 1..]);
                break;
            }
            b => digits.push(b - b'0'),
        }
    }

    // `e` counts the digits before the point. With no point, that is all of
    // them; the explicit exponent then shifts it.
    //
    // Saturating, not wrapping. The original carries this in a double, where
    // an absurd exponent becomes a large finite value (or Infinity) and is
    // then caught by the overflow check at the end. An `i64` that wrapped
    // would instead turn `1e999999999999999999999` into a small, plausible,
    // and completely wrong exponent. Saturating into a range far outside
    // `EXP_LIMIT` reproduces the original's outcome — the value overflows —
    // while keeping the limb arithmetic below in range.
    let mut e =
        saturate_exponent((point_at.unwrap_or(digits.len()) as i64).saturating_add(exponent_part));

    // -- 2. Trim ---------------------------------------------------------
    let first_significant = digits.iter().position(|&d| d != 0);
    let Some(first) = first_significant else {
        // Every digit was a zero.
        return Decimal::zero(s);
    };
    let last = digits
        .iter()
        .rposition(|&d| d != 0)
        .expect("a non-zero digit exists, found just above");
    let digits = &digits[first..=last];

    // Leading zeros were part of the count above; remove their contribution.
    // The exponent convention is `value = 0.d × 10^(e+1)`, hence the −1.
    e = saturate_exponent(e.saturating_sub(first as i64 + 1));

    // -- 3. Pack ---------------------------------------------------------
    //
    // The first limb holds however many digits are needed to align the rest on
    // a boundary: `(e + 1) mod 7`, taken as a non-negative residue. When that
    // is zero the first limb is full-width like the others.
    let mut head = (e + 1) % LOG_BASE;
    if e < 0 {
        head += LOG_BASE;
    }

    let mut limbs: Vec<u32> = Vec::new();
    let len = digits.len() as i64;
    let mut at: i64;

    if head < len {
        if head > 0 {
            limbs.push(digits_to_limb(&digits[..head as usize]));
        }
        at = head;
        while at + LOG_BASE <= len {
            limbs.push(digits_to_limb(
                &digits[at as usize..(at + LOG_BASE) as usize],
            ));
            at += LOG_BASE;
        }
        // Whatever remains becomes the final limb, padded on the right so that
        // it occupies its full seven digit positions.
        let tail = &digits[at as usize..];
        limbs.push(pad_right_to_limb(tail));
    } else {
        // Every digit fits in the first, short limb, which still has to be
        // padded out to the alignment the exponent implies.
        let mut limb: u32 = 0;
        for k in 0..head {
            limb = limb * 10 + u32::from(if k < len { digits[k as usize] } else { 0 });
        }
        // `head - len` trailing zeros were folded in by the loop above, which
        // is exactly the padding the original appends before its final push.
        limbs.push(limb);
    }

    let mut x = Decimal::finite(s, e, limbs);
    x.strip_trailing_zero_limbs();

    // The exponent limits apply to a freshly parsed value too, unless the
    // caller is assembling an intermediate — `parseOther` builds a hex float
    // out of two values either of which may legitimately overflow.
    if ctx.external {
        if x.e > ctx.cfg.max_e {
            x.d = None;
            x.e = 0;
        } else if x.e < ctx.cfg.min_e {
            x.e = 0;
            x.d = Some(vec![0]);
        }
    }

    x
}

/// Convert a digit string in `base_in` to an array of limbs in `base_out`.
///
/// Schoolbook: repeatedly multiply the accumulator by the input base and add
/// the next digit, normalising carries as they appear. Quadratic in the input
/// length, which is fine — the inputs are numeric literals, not files.
pub fn convert_base(digits: &[u8], base_in: u32, base_out: u32) -> Vec<u32> {
    // Least-significant-first while accumulating; reversed on the way out.
    let mut arr: Vec<u64> = vec![0];

    for &digit in digits {
        for limb in arr.iter_mut() {
            *limb *= u64::from(base_in);
        }
        arr[0] += u64::from(digit);

        let mut j = 0;
        while j < arr.len() {
            if arr[j] > u64::from(base_out) - 1 {
                if j + 1 == arr.len() {
                    arr.push(0);
                }
                arr[j + 1] += arr[j] / u64::from(base_out);
                arr[j] %= u64::from(base_out);
            }
            j += 1;
        }
    }

    arr.reverse();
    arr.into_iter().map(|limb| limb as u32).collect()
}

/// Parse a literal that is not an ordinary decimal: an underscore-separated
/// decimal, `Infinity`, `NaN`, or a hexadecimal, binary or octal literal with
/// an optional binary exponent.
///
/// The radix forms are converted by reading the mantissa as an integer in its
/// own base and then *dividing* by `base^fractionDigits` — so hexadecimal
/// float parsing goes through the arithmetic core, and cannot be finished
/// before division works. The exponent clamps are suppressed throughout,
/// because either the mantissa or the divisor may legitimately stray outside
/// the representable range before the quotient brings it back.
pub fn parse_other(ctx: &mut Ctx, s: Sign, text: &str) -> crate::Result<Decimal> {
    // Digit separators: `1_000_000`, but only between digits.
    if text.contains('_') {
        let mut stripped = String::with_capacity(text.len());
        let bytes = text.as_bytes();
        for (index, &b) in bytes.iter().enumerate() {
            if b == b'_'
                && index > 0
                && bytes[index - 1].is_ascii_digit()
                && bytes.get(index + 1).is_some_and(u8::is_ascii_digit)
            {
                continue;
            }
            stripped.push(b as char);
        }
        if is_decimal_literal(stripped.as_bytes()) {
            return Ok(parse_decimal(ctx, s, &stripped));
        }
        return Err(crate::Error::InvalidArgument(text.to_string()));
    }

    if text == "Infinity" {
        return Ok(Decimal::infinity(s));
    }
    if text == "NaN" {
        return Ok(Decimal::nan());
    }

    let bytes = text.as_bytes();
    let base: u32 = if is_hex_literal(bytes) {
        16
    } else if is_binary_literal(bytes) {
        2
    } else if is_octal_literal(bytes) {
        8
    } else {
        return Err(crate::Error::InvalidArgument(text.to_string()));
    };

    // Split off a binary exponent, if present.
    let lowered = text.to_ascii_lowercase();
    let (mantissa, binary_exponent) = match lowered.find('p') {
        Some(at) => (
            &lowered[2..at],
            parse_exponent(&lowered.as_bytes()[at + 1..]),
        ),
        None => (&lowered[2..], 0),
    };

    // Separate the fractional part; its length says what power of the base to
    // divide by afterwards.
    let (digits_text, fraction_length) = match mantissa.find('.') {
        Some(at) => (
            format!("{}{}", &mantissa[..at], &mantissa[at + 1..]),
            (mantissa.len() - 1 - at) as i64,
        ),
        None => (mantissa.to_string(), 0),
    };

    let digit_values: Vec<u8> = digits_text
        .chars()
        .map(|c| c.to_digit(base).expect("validated by the recogniser") as u8)
        .collect();

    let mut limbs = convert_base(&digit_values, base, crate::BASE);
    let limb_exponent = limbs.len() as i64 - 1;

    // Trailing zero limbs are not part of the value.
    while limbs.last() == Some(&0) {
        limbs.pop();
    }
    if limbs.is_empty() {
        return Ok(Decimal::zero(s));
    }

    let mut x = Decimal::finite(
        s,
        crate::arith::base10_exponent(&limbs, limb_exponent),
        limbs,
    );

    let result = ctx.without_clamping(|ctx| {
        if fraction_length > 0 {
            // log10(16) < 1.21, so four decimal digits per input digit is
            // always enough to make the division exact.
            let total_digits = digits_text.len() as i64;
            let divisor = crate::arith::int_pow(
                ctx,
                &Decimal::from_i32(base as i32),
                fraction_length,
                fraction_length * 2,
            );
            let rm = ctx.cfg.rounding;
            x = crate::arith::divide(ctx, &x, &divisor, Some(total_digits * 4), rm, false, None);
        }

        if binary_exponent != 0 {
            // The original:
            //
            //     if (p) x = x.times(Math.abs(p) < 54 ? mathpow(2, p) : Decimal.pow(2, p));
            //
            // Two scales, not one, and which of them is used is observable.
            //
            // Below 54 the scale is a *double*. Every power of two in that
            // range is exact as a double and converts to a Decimal exactly, so
            // the only rounding is the multiplication's.
            //
            // At 54 and above it is the library's own `pow` — and `pow` is not
            // merely `int_pow` with a reciprocal. It rounds the power to the
            // working precision, and before doing any of that it estimates the
            // result's exponent and returns 0 or Infinity outright if the
            // estimate falls outside `minE`/`maxE`. Computing the scale
            // directly, as this did, bypasses that estimate: with `maxE` at 41
            // and precision 1, `new Decimal('0x1p-1074')` is 0 upstream —
            // because `pow` gives up on `2^-1074` — and was 5e-324 here.
            //
            // Found by the differential campaign, not by the suite; the suite's
            // radix modules all run at the default `maxE`.
            let scale = if binary_exponent.abs() < 54 {
                let doubled = (2f64).powi(binary_exponent as i32);
                parse_decimal(ctx, Sign::Pos, &crate::format::number_to_string(doubled))
            } else {
                let sign = if binary_exponent < 0 {
                    Sign::Neg
                } else {
                    Sign::Pos
                };
                let exponent =
                    parse_decimal(ctx, sign, &binary_exponent.unsigned_abs().to_string());
                crate::power::to_power(ctx, &Decimal::from_i32(2), &exponent)?
            };
            x = crate::arith::mul(ctx, &x, &scale);
        }
        Ok(x.clone())
    })?;

    let mut result = result;
    crate::round::finalise(ctx, &mut result, None, ctx.cfg.rounding, false);
    Ok(result)
}

/// Clamp an exponent into a range wide enough to contain every representable
/// value and narrow enough that the limb arithmetic cannot overflow.
///
/// `EXP_LIMIT` is 9 × 10¹⁵, so a bound a billion beyond it is still four
/// hundred times smaller than `i64::MAX`. Anything reaching this clamp is
/// already destined to overflow to Infinity or underflow to zero, so where
/// exactly it saturates is not observable.
fn saturate_exponent(e: i64) -> i64 {
    const SAFE_BOUND: i64 = crate::EXP_LIMIT + 1_000_000_000;
    e.clamp(-SAFE_BOUND, SAFE_BOUND)
}

/// Read the signed decimal integer following an `e`.
///
/// It cannot overflow into nonsense: the caller has already established that
/// at least one digit is present, and a run of digits long enough to overflow
/// an `i64` describes an exponent that is saturated to the same place either
/// way, since `EXP_LIMIT` is only 9 × 10¹⁵.
fn parse_exponent(bytes: &[u8]) -> i64 {
    let (negative, digits) = match bytes.first() {
        Some(b'+') => (false, &bytes[1..]),
        Some(b'-') => (true, &bytes[1..]),
        _ => (false, bytes),
    };

    let mut magnitude: i64 = 0;
    for &b in digits {
        magnitude = magnitude
            .saturating_mul(10)
            .saturating_add(i64::from(b - b'0'));
    }

    if negative {
        -magnitude
    } else {
        magnitude
    }
}

/// Pack up to seven decimal digits into a limb, most significant first.
fn digits_to_limb(digits: &[u8]) -> u32 {
    digits
        .iter()
        .fold(0u32, |limb, &d| limb * 10 + u32::from(d))
}

/// Pack digits into a limb, padding on the right to the full seven positions.
fn pad_right_to_limb(digits: &[u8]) -> u32 {
    let mut limb = digits_to_limb(digits);
    for _ in digits.len()..LOG_BASE as usize {
        limb *= 10;
    }
    limb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepts(s: &str) -> bool {
        is_decimal_literal(s.as_bytes())
    }

    #[test]
    fn decimal_literals_match_the_original_pattern() {
        for good in [
            "0", "1", "12", "1.", "1.0", "1.5", ".5", "0.5", "1e5", "1E5", "1e+5", "1e-5",
            "1.5e-5", ".5e5", "007", "1.",
        ] {
            assert!(accepts(good), "should accept {good:?}");
        }
        for bad in [
            "", ".", "-1", "+1", "e5", "1e", "1e+", "1.2.3", "1_000", " 1", "1 ", "0x10",
            "Infinity", "NaN", "1e5.5",
        ] {
            assert!(!accepts(bad), "should reject {bad:?}");
        }
    }

    #[test]
    fn radix_literals_match_their_patterns() {
        assert!(is_hex_literal(b"0xff"));
        assert!(is_hex_literal(b"0XFF"));
        assert!(is_hex_literal(b"0x1.8"));
        assert!(is_hex_literal(b"0x.8"));
        assert!(is_hex_literal(b"0x1.8p3"), "the C99 hex-float form");
        assert!(is_hex_literal(b"0x1p-3"));
        assert!(!is_hex_literal(b"0x"));
        assert!(!is_hex_literal(b"0xg"));
        assert!(!is_hex_literal(b"0x1.8p"), "p must be followed by digits");

        assert!(is_binary_literal(b"0b1011"));
        assert!(is_binary_literal(b"0b1.1p4"));
        assert!(!is_binary_literal(b"0b2"));

        assert!(is_octal_literal(b"0o777"));
        assert!(!is_octal_literal(b"0o8"));
    }

    /// A hex float with a large negative binary exponent is measured against
    /// `maxE`, because the scale it is divided by is not.
    ///
    /// `0x1p-1074` is 4.94e-324 and comfortably inside the default limits, so
    /// nothing about the *answer* suggests `maxE` should have any say. But the
    /// original reaches it through `Decimal.pow(2, -1074)`, whose reciprocal
    /// branch divides by `2^1074` — exponent 323 — and `div` re-judges its
    /// argument against `maxE` on the way in. Below 323 the divisor becomes
    /// Infinity and the answer is 0.
    ///
    /// Two separate transcription errors had to be fixed for this to hold, and
    /// they cancel in the default configuration, which is why the suite is
    /// silent about both: `parse_other` computed the scale itself instead of
    /// going through `pow`, and `int_pow` restored the clamping flag where the
    /// original merely sets it. D-15.
    #[test]
    fn a_hex_float_is_clamped_through_the_scale_it_divides_by() {
        let mut ctx = Ctx::default();
        for (max_e, expected) in [(41i64, "0"), (322, "0"), (323, "5e-324"), (400, "5e-324")] {
            ctx.cfg.precision = 1;
            ctx.cfg.max_e = max_e;
            let x = parse_other(&mut ctx, Sign::Pos, "0x1p-1074").expect("a valid literal");
            assert_eq!(
                crate::format::to_string(&x, &ctx.cfg),
                expected,
                "new Decimal('0x1p-1074') with maxE = {max_e}"
            );
        }
    }

    /// Reconstruct the decimal digit string a value represents, so that
    /// parsing can be checked without depending on the formatter.
    fn digit_string(x: &Decimal) -> String {
        let limbs = x.digits();
        let mut s = limbs[0].to_string();
        for &w in &limbs[1..] {
            s.push_str(&format!("{w:07}"));
        }
        s.trim_end_matches('0').to_string()
    }

    fn parse(text: &str) -> Decimal {
        parse_decimal(&Ctx::default(), Sign::Pos, text)
    }

    #[test]
    fn integers_parse_to_the_expected_exponent_and_digits() {
        let x = parse("1");
        assert_eq!(x.e, 0);
        assert_eq!(digit_string(&x), "1");

        let x = parse("12345678901234567890");
        assert_eq!(x.e, 19, "twenty digits, so 0.d x 10^20");
        assert_eq!(digit_string(&x), "1234567890123456789");
    }

    #[test]
    fn fractions_and_exponents_agree_on_the_same_value() {
        // Five spellings of one hundred and twenty-three thousandths.
        let expected = parse("0.123");
        for spelling in ["0.123", ".123", "123e-3", "1.23e-1", "0.0123e1"] {
            let got = parse(spelling);
            assert_eq!(got.e, expected.e, "exponent of {spelling:?}");
            assert_eq!(got.digits(), expected.digits(), "digits of {spelling:?}");
        }
        assert_eq!(expected.e, -1);
    }

    #[test]
    fn leading_and_trailing_zeros_are_discarded() {
        let x = parse("000123000");
        assert_eq!(x.e, 5, "nine characters, six of them significant leading");
        assert_eq!(digit_string(&x), "123");

        let x = parse("1.2300000");
        assert_eq!(digit_string(&x), "123");
        assert_eq!(x.e, 0);
    }

    #[test]
    fn every_spelling_of_zero_is_zero() {
        for spelling in ["0", "0.0", ".0", "0e10", "0.000e-10", "00000"] {
            let x = parse(spelling);
            assert!(x.is_zero(), "{spelling:?} should be zero");
            assert_eq!(x.e, 0);
            assert!(!x.is_negative());
        }
        assert!(parse_decimal(&Ctx::default(), Sign::Neg, "0").is_negative());
    }

    /// The limb layout is not free: it is the alignment every other routine
    /// assumes, and the exponent arithmetic in `finalise`, `divide` and the
    /// series expansions all depend on it. Each expectation below was read
    /// off the original by evaluating `new Decimal(v).d` in Node, not derived
    /// from this implementation.
    #[test]
    fn limbs_are_aligned_so_the_point_falls_on_a_boundary() {
        // The first limb holds `(e + 1) mod 7` digits — three here — and the
        // rest hold seven each.
        let x = parse("1234567890");
        assert_eq!(x.e, 9);
        assert_eq!(x.digits(), &[123, 4_567_890]);

        // e = 0, so the first limb holds a single digit and the remainder
        // spills into a second limb padded on the right.
        let x = parse("1.5");
        assert_eq!(x.e, 0);
        assert_eq!(x.digits(), &[1, 5_000_000]);

        // e = -1, so `(e + 1) mod 7` is 0: the first limb is full width.
        let x = parse("0.123");
        assert_eq!(x.e, -1);
        assert_eq!(x.digits(), &[1_230_000]);

        // e = 20 -> (e+1) mod 7 = 0, again full width.
        let x = parse("1e20");
        assert_eq!(x.e, 20);
        assert_eq!(x.digits(), &[1_000_000]);
    }

    #[test]
    fn small_negative_exponents_shift_into_the_first_limb() {
        // e = -7, so `(e+1) mod 7` is -6, which becomes 1: a single leading
        // digit, and no padding at all.
        let x = parse("1e-7");
        assert_eq!(x.e, -7);
        assert_eq!(x.digits(), &[1]);
    }

    #[test]
    fn the_exponent_limits_apply_to_parsing() {
        let ctx = Ctx::new(crate::Config {
            max_e: 10,
            min_e: -10,
            ..crate::Config::default()
        });
        assert!(parse_decimal(&ctx, Sign::Pos, "1e11").is_infinite());
        assert!(parse_decimal(&ctx, Sign::Pos, "1e-11").is_zero());
        assert!(parse_decimal(&ctx, Sign::Pos, "1e10").is_finite());
    }

    #[test]
    fn a_huge_exponent_saturates_rather_than_wrapping() {
        // The original computes this in a double, where it becomes Infinity
        // and then overflows to Infinity. Wrapping an i64 here would instead
        // produce a small finite exponent, which is the bug this guards.
        let x = parse("1e999999999999999999999");
        assert!(x.is_infinite(), "an absurd exponent overflows, not wraps");
    }
}
