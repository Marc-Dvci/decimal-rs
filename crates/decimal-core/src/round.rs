//! Rounding to a significant-digit count, and the overflow/underflow clamp.
//!
//! # `finalise`
//!
//! Almost every operation in the library ends by calling [`finalise`]. It does
//! three things, in this order:
//!
//!   1. rounds the digit array to `sd` significant digits under rounding mode
//!      `rm`, if `sd` is given;
//!   2. restores the representation invariant, by removing the trailing zero
//!      limbs that rounding may have exposed;
//!   3. clamps the exponent, turning anything above `maxE` into ±Infinity and
//!      anything below `minE` into ±0 — unless the caller has suppressed the
//!      clamp because it is still assembling an intermediate result.
//!
//! Getting this function exactly right is the whole game. Every other routine
//! inherits its rounding from here, so an error in it does not show up as one
//! failing module — it shows up as forty failing modules, each in a way that
//! looks unrelated to the others. It is therefore transcribed from the
//! original statement by statement, including the parts that look like they
//! could be simplified, and the transcription notes below record why the parts
//! that look redundant are not.
//!
//! ## What the local variables mean
//!
//! The original's names are terse and are kept, because the reader who wants
//! to check this against the source needs to be able to put them side by side.
//! Their meanings, which the original states only partially:
//!
//! | name  | meaning |
//! |-------|---------|
//! | `rd`  | the *rounding digit*: the first digit discarded, the one that decides the direction |
//! | `w`   | the limb of `xd` that contains `rd` |
//! | `xdi` | the index of `w` within `xd` |
//! | `digits` | the number of decimal digits of `w` (so `LOG_BASE - digits` is its leading-zero count) |
//! | `i`   | the index `rd` would have within `w` if every limb were written with its leading zeros |
//! | `j`   | the actual index of `rd` within `w`; negative means `rd` falls in a leading zero |
//!
//! ## On doing in integers what the original does in floating point
//!
//! The original divides limbs by powers of ten in IEEE doubles and truncates
//! with `| 0`. This port uses integer division. The two agree here, and only
//! here, because a limb is below 10⁷ and every divisor is an exact power of
//! ten below 10⁸, so each quotient is exactly representable and the
//! floating-point division is exact. Where the original relies on a float
//! result that is *not* exact — as `naturalLogarithm` does — this port keeps
//! the float. The rule followed throughout is to match the original's
//! arithmetic, not to improve on it.

use crate::{digit_count, div_pow10, mod_pow10, pow10, Ctx, Decimal, BASE, LOG_BASE};

/// Round `x` to `sd` significant digits with rounding mode `rm`, then apply
/// the exponent limits.
///
/// `sd == None` means "do not round", and the call is then only about the
/// exponent clamp — several callers use it that way.
///
/// `is_truncated` tells the routine that digits beyond the ones present were
/// already known to have been discarded, so the value is strictly greater in
/// magnitude than its digits suggest. `naturalExponential`,
/// `naturalLogarithm` and `squareRoot` all compute at raised precision and
/// then report back through this flag.
pub fn finalise(ctx: &mut Ctx, x: &mut Decimal, sd: Option<i64>, rm: u8, is_truncated: bool) {
    if round_to_significant_digits(x, sd, rm, is_truncated) == Then::ApplyLimits {
        apply_exponent_limits(ctx, x);
    }
}

/// What the rounding step leaves for [`finalise`] to do.
///
/// The distinction is not cosmetic. The original's rounding step has three
/// exits, and they differ in *which* of the remaining work they skip:
///
///   - `return x` — used for a non-finite value, and for a value all of whose
///     digits were rounded away — leaves the function altogether, so the
///     exponent clamp does **not** run;
///   - `break out` — used when fewer digits are present than were requested
///     and nothing was discarded — jumps past the trailing-zero removal but
///     *does* reach the clamp;
///   - falling off the end runs both.
///
/// Collapsing those three into two would be an easy mistake and a silent one.
/// In particular the `break out` path must not strip trailing zero limbs: a
/// digit array arriving from `toStringBinary` is allowed to carry them, and
/// the division routine relies on their still being there.
#[derive(PartialEq, Eq)]
enum Then {
    /// Apply the overflow and underflow limits.
    ApplyLimits,
    /// Return immediately; the limits do not apply.
    Stop,
}

fn round_to_significant_digits(
    x: &mut Decimal,
    sd: Option<i64>,
    rm: u8,
    mut is_truncated: bool,
) -> Then {
    let Some(mut sd) = sd else {
        // No rounding requested; the call is only about the exponent limits.
        return Then::ApplyLimits;
    };

    // Infinity and NaN have no digits to round, and leave without clamping.
    if x.d.is_none() {
        return Then::Stop;
    }

    let negative = x.s.is_negative();
    let xd = x.d.as_mut().expect("checked finite immediately above");

    // Locate the rounding digit. `i` is measured as though every limb were
    // written out to its full seven digits, which makes the arithmetic below
    // uniform; `j` then corrects for the leading zeros the first limb of a
    // value does not actually have.
    let mut digits = digit_count(xd[0]);
    let mut i = sd - digits;
    let xdi: i64;
    let j: i64;
    let w: u32;
    let rd: u32;

    if i < 0 {
        // The rounding digit lies within the first limb.
        i += LOG_BASE;
        j = sd;
        xdi = 0;
        w = xd[0];
        rd = (div_pow10(u64::from(w), digits - j - 1) % 10) as u32;
    } else {
        xdi = (i + LOG_BASE) / LOG_BASE; // ceil((i + 1) / LOG_BASE)

        if xdi >= xd.len() as i64 {
            if !is_truncated {
                // Fewer digits are present than were asked for and nothing was
                // discarded, so the value is already exact at this precision.
                // The original's `break out`: skip the rounding *and* the
                // trailing-zero removal, but still apply the exponent limits.
                return Then::ApplyLimits;
            }

            // The caller knows digits were lost beyond the array. Extend it
            // with zero limbs so that the rounding position exists, and let
            // `is_truncated` carry the information that they were not zero.
            while (xd.len() as i64) <= xdi {
                xd.push(0);
            }
            w = 0;
            rd = 0;
            digits = 1;
            i %= LOG_BASE;
            j = i - LOG_BASE + 1;
        } else {
            w = xd[xdi as usize];
            digits = digit_count(w);
            i %= LOG_BASE;
            j = i - LOG_BASE + digits;
            rd = if j < 0 {
                0
            } else {
                (div_pow10(u64::from(w), digits - j - 1) % 10) as u32
            };
        }
    }

    // Is anything non-zero discarded beyond the rounding digit? Three ways it
    // can be: the caller said so, `sd` was negative (so the whole value is
    // being discarded), a further limb exists, or the digits of `w` to the
    // right of `rd` are not all zero.
    is_truncated = is_truncated
        || sd < 0
        || (xdi + 1) < xd.len() as i64
        || if j < 0 {
            w != 0
        } else {
            mod_pow10(w, digits - j - 1) != 0
        };

    // The rounding decision itself. The four directed modes (0..=3) round away
    // from zero only when something was actually discarded and the mode points
    // that way; the five half-way modes (4..=8) turn on the rounding digit,
    // with ties broken by the mode.
    let round_up = if rm < 4 {
        (rd != 0 || is_truncated) && (rm == 0 || rm == if negative { 3 } else { 2 })
    } else {
        rd > 5
            || rd == 5
                && (rm == 4
                    || is_truncated
                    || rm == 6 && {
                        // HALF_EVEN: is the digit to the *left* of the
                        // rounding digit odd? When the rounding digit is the
                        // first of its limb, that neighbour is the last digit
                        // of the previous limb; when there is no previous
                        // limb, the original reads `undefined`, whose
                        // `% 10 & 1` is 0, so the tie falls as though the
                        // neighbour were even.
                        let left = if i > 0 {
                            if j > 0 {
                                div_pow10(u64::from(w), digits - j)
                            } else {
                                0
                            }
                        } else if xdi > 0 {
                            u64::from(xd[(xdi - 1) as usize])
                        } else {
                            0
                        };
                        left % 10 & 1 == 1
                    }
                    || rm == if negative { 8 } else { 7 })
    };

    // Rounding away every significant digit: the result is either zero or a
    // single one in the place just above the ones that were discarded.
    if sd < 1 || xd[0] == 0 {
        xd.clear();
        if round_up {
            // Re-express `sd` as a count of decimal places, then place a
            // single 1 at that position within its limb.
            sd -= x.e + 1;
            xd.push(pow10((LOG_BASE - sd % LOG_BASE) % LOG_BASE));
            x.e = if sd == 0 { 0 } else { -sd };
        } else {
            xd.push(0);
            x.e = 0;
        }
        // The original returns from `finalise` here, so the exponent limits
        // are deliberately not applied to this result.
        return Then::Stop;
    }

    // Discard the digits at and beyond the rounding position. `k` becomes the
    // increment that a round-up would add at that position.
    let mut xdi = xdi;
    let mut k: u32;
    if i == 0 {
        xd.truncate(xdi as usize);
        k = 1;
        xdi -= 1;
        // `xdi` cannot go below zero here: `i == 0` implies the rounding digit
        // begins a limb, and `xdi = ceil((i+1)/LOG_BASE)` is then at least 1.
        // The other branch that could reach `i == 0`, where the rounding digit
        // is in the first limb, requires `sd <= 0` and has already returned.
        debug_assert!(xdi >= 0, "the first limb is never the one truncated away");
    } else {
        xd.truncate(xdi as usize + 1);
        k = pow10(LOG_BASE - i);

        // Zero the digits of `w` from position `j` onwards, keeping the ones
        // to its left: 56700 becomes 56000 when 7 is the rounding digit.
        xd[xdi as usize] = if j > 0 {
            ((div_pow10(u64::from(w), digits - j) % u64::from(pow10(j))) as u32) * k
        } else {
            0
        };
    }

    if round_up {
        // Add `k` at the rounding position and propagate the carry leftwards.
        loop {
            if xdi == 0 {
                let before = digit_count(xd[0]);
                xd[0] += k;
                let after = digit_count(xd[0]);

                // A carry out of the leading limb lengthens the value, which
                // moves the decimal exponent. The limb can land exactly on
                // BASE — 9 999 999 + 1 — in which case it becomes a leading 1
                // and the extra order of magnitude is absorbed by `e`.
                if before != after {
                    x.e += 1;
                    if xd[0] == BASE {
                        xd[0] = 1;
                    }
                }
                break;
            }

            xd[xdi as usize] += k;
            if xd[xdi as usize] != BASE {
                break;
            }
            xd[xdi as usize] = 0;
            xdi -= 1;
            k = 1;
        }
    }

    x.strip_trailing_zero_limbs();
    Then::ApplyLimits
}

/// Step 3: overflow to ±Infinity above `maxE`, underflow to ±0 below `minE`.
///
/// Skipped entirely while `ctx.external` is false, which is how the original
/// lets an intermediate value hold an exponent that the final result may not.
fn apply_exponent_limits(ctx: &Ctx, x: &mut Decimal) {
    if !ctx.external {
        return;
    }

    if x.e > ctx.cfg.max_e {
        x.d = None;
        x.e = 0;
    } else if x.e < ctx.cfg.min_e {
        x.e = 0;
        x.d = Some(vec![0]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::rounding::*;
    use crate::{Config, Sign};

    /// Round a decimal literal and render the result.
    ///
    /// Every expectation in this module was produced by evaluating
    /// `new Decimal(v).toSignificantDigits(sd, rm).toString()` against
    /// upstream decimal.js in Node, and is quoted here as the oracle says it.
    /// Checking strings rather than limb arrays keeps the tests readable and,
    /// more importantly, keeps them honest: a limb array is this port's
    /// internal business, but the string is the observable behaviour the
    /// original promises.
    fn round_to(literal: &str, sd: i64, rm: u8) -> String {
        let mut ctx = Ctx::default();
        let sign = if let Some(stripped) = literal.strip_prefix('-') {
            let _ = stripped;
            Sign::Neg
        } else {
            Sign::Pos
        };
        let mut x = crate::parse::parse_decimal(&ctx, sign, literal.trim_start_matches('-'));
        finalise(&mut ctx, &mut x, Some(sd), rm, false);
        crate::format::to_string(&x, &ctx.cfg)
    }

    #[test]
    fn half_way_modes_break_ties_as_their_names_say() {
        assert_eq!(round_to("1.5", 1, HALF_UP), "2");
        assert_eq!(round_to("1.5", 1, HALF_DOWN), "1");
        assert_eq!(round_to("1.5", 1, HALF_EVEN), "2", "1 is odd, so move to 2");
        assert_eq!(round_to("2.5", 1, HALF_EVEN), "2", "2 is even, so stay");
    }

    #[test]
    fn half_even_reads_the_neighbour_across_a_limb_boundary() {
        // Eight digits, so the rounding digit begins the second limb and its
        // left neighbour is the last digit of the first — the case the
        // original reaches through `xd[xdi - 1]`, and the one most likely to
        // be got wrong.
        assert_eq!(round_to("11111125", 7, HALF_EVEN), "11111120", "2 is even");
        assert_eq!(round_to("11111135", 7, HALF_EVEN), "11111140", "3 is odd");
    }

    #[test]
    fn directed_modes_consult_the_sign() {
        assert_eq!(round_to("1.1", 1, CEIL), "2", "towards +Infinity");
        assert_eq!(round_to("-1.1", 1, CEIL), "-1", "also towards +Infinity");
        assert_eq!(round_to("-1.1", 1, FLOOR), "-2", "towards -Infinity");
    }

    #[test]
    fn down_never_rounds_up_and_up_always_does() {
        assert_eq!(round_to("1.9", 1, DOWN), "1");
        assert_eq!(round_to("1.1", 1, UP), "2");
        assert_eq!(round_to("1", 1, UP), "1", "nothing discarded, nothing to do");
    }

    #[test]
    fn a_carry_out_of_the_leading_limb_moves_the_exponent() {
        assert_eq!(round_to("9.99", 2, HALF_UP), "10");
    }

    #[test]
    fn a_carry_propagates_across_limbs() {
        // Fifteen nines: the carry travels the whole way and lengthens the
        // value, which is the path through `xd[0] == BASE`.
        assert_eq!(round_to("999999999999999", 14, HALF_UP), "1000000000000000");
    }

    #[test]
    fn rounding_away_every_digit_gives_zero_or_one_unit() {
        // `sd < 1` cannot be reached through the public `toSD`, which rejects
        // it, but `finalise` is called with it internally — `truncate` passes
        // `x.e + 1`, which is zero or negative for any value below one.
        let mut ctx = Ctx::default();

        let mut x = crate::parse::parse_decimal(&ctx, Sign::Pos, "0.04");
        finalise(&mut ctx, &mut x, Some(0), DOWN, false);
        assert!(x.is_zero(), "discarding everything, rounding down");

        let mut x = crate::parse::parse_decimal(&ctx, Sign::Pos, "0.04");
        finalise(&mut ctx, &mut x, Some(0), UP, false);
        assert_eq!(
            crate::format::to_string(&x, &ctx.cfg),
            "0.1",
            "one unit in the place just above the most significant digit discarded"
        );
    }

    #[test]
    fn non_finite_values_pass_through_untouched() {
        let mut ctx = Ctx::default();

        let mut inf = Decimal::infinity(Sign::Pos);
        finalise(&mut ctx, &mut inf, Some(5), HALF_UP, false);
        assert!(inf.is_infinite() && !inf.is_negative());

        let mut nan = Decimal::nan();
        finalise(&mut ctx, &mut nan, Some(5), HALF_UP, false);
        assert!(nan.is_nan());
    }

    #[test]
    fn the_exponent_limits_overflow_and_underflow() {
        let mut ctx = Ctx::new(Config {
            max_e: 10,
            min_e: -10,
            ..Config::default()
        });

        let mut big = Decimal::finite(Sign::Pos, 11, vec![1_000_000]);
        finalise(&mut ctx, &mut big, None, HALF_UP, false);
        assert!(big.is_infinite(), "above maxE becomes Infinity");

        let mut small = Decimal::finite(Sign::Neg, -11, vec![1_000_000]);
        finalise(&mut ctx, &mut small, None, HALF_UP, false);
        assert!(small.is_zero(), "below minE becomes zero");
        assert!(small.is_negative(), "and keeps its sign");
    }

    #[test]
    fn suppressing_the_clamp_lets_an_intermediate_exceed_the_limits() {
        let mut ctx = Ctx::new(Config {
            max_e: 10,
            ..Config::default()
        });

        let mut big = Decimal::finite(Sign::Pos, 11, vec![1_000_000]);
        ctx.without_clamping(|ctx| finalise(ctx, &mut big, None, HALF_UP, false));
        assert!(big.is_finite(), "the clamp was suppressed");
        assert_eq!(big.e, 11);

        // ...and the suppression is restored, not merely set.
        assert!(ctx.external);
    }

    #[test]
    fn truncation_flag_turns_an_exact_tie_into_a_round_up() {
        // 1.5 with HALF_DOWN stays at 1; but if the caller reports that
        // further non-zero digits were discarded, the value is above the tie
        // and must round up.
        let mut ctx = Ctx::default();
        let mut x = crate::parse::parse_decimal(&ctx, Sign::Pos, "1.5");
        finalise(&mut ctx, &mut x, Some(1), HALF_DOWN, true);
        assert_eq!(crate::format::to_string(&x, &ctx.cfg), "2");
    }

    #[test]
    fn rounding_restores_the_no_trailing_zero_limb_invariant() {
        // Ask for fewer digits than are present, so the rounding path runs to
        // completion and the strip at its end is reached.
        let mut x = Decimal::finite(Sign::Pos, 7, vec![1, 0, 0]);
        let mut ctx = Ctx::default();
        finalise(&mut ctx, &mut x, Some(8), HALF_UP, false);
        assert_eq!(x.digits(), &[1], "the invariant is restored on the way out");
    }

    #[test]
    fn asking_for_more_digits_than_exist_leaves_the_array_alone() {
        // This is the original's `break out`. There is nothing to round, and
        // trailing zero limbs must survive it: a digit array arriving from
        // base conversion is allowed to carry them, and division depends on
        // their still being present.
        let mut x = Decimal::finite(Sign::Pos, 7, vec![1, 0, 0]);
        let mut ctx = Ctx::default();
        finalise(&mut ctx, &mut x, Some(20), HALF_UP, false);
        assert_eq!(
            x.digits(),
            &[1, 0, 0],
            "break out skips the trailing-zero removal"
        );
    }
}
