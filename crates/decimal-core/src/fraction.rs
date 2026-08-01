//! The nearest fraction with a bounded denominator.
//!
//! # What is being computed
//!
//! Given `x` and a bound `N`, [`to_fraction`] returns the fraction `p/q` with
//! `0 < q ≤ N` closest to `x`. Every value this library holds is already
//! rational — it is a finite string of decimal digits — so `0.1` could always
//! be answered with `1000…/10000…`. The point is to find the *simplest* such
//! fraction, and `1/10` is the answer wanted.
//!
//! # How
//!
//! By the continued-fraction expansion of `x`, which is the classical answer
//! and the right one. Writing
//!
//! ```text
//!     x = a₀ + 1/(a₁ + 1/(a₂ + …))
//! ```
//!
//! the *convergents* `p_k/q_k` obtained by truncating after `a_k` obey the
//! recurrences
//!
//! ```text
//!     p_k = a_k·p_{k−1} + p_{k−2}        q_k = a_k·q_{k−1} + q_{k−2}
//! ```
//!
//! and each is a best rational approximation to `x` among all fractions with
//! denominator no larger than its own. So the loop below runs the recurrence
//! until the denominator would exceed `N`, and the last convergent that fits is
//! one of the two candidates.
//!
//! It is only one of two, and this is the part a naive implementation gets
//! wrong. The best fraction with denominator `≤ N` need not be a convergent: it
//! may be a *semiconvergent*, formed by backing the last partial quotient off
//! from `a_k` to the largest multiple that still fits under the bound,
//!
//! ```text
//!     j = ⌊(N − q_{k−2}) / q_{k−1}⌋      p = j·p_{k−1} + p_{k−2}
//!                                        q = j·q_{k−1} + q_{k−2}
//! ```
//!
//! which is the `d2 = divide(maxD.minus(d0), d1, 0, 1, 1)` at the end. Both
//! candidates are then formed and the closer to `x` is returned — with ties
//! going to the convergent, since the comparison is `< 1` rather than `< 0`.
//!
//! # The default bound
//!
//! With no argument, `N` is `10^e` where `e` is the number of decimal places
//! `x` has: the smallest denominator that can represent `x` exactly. So
//! `toFraction()` on a value that *is* a simple fraction returns it, and on one
//! that is not returns the value itself over a power of ten. When `e ≤ 0` — an
//! integer — the bound is 1, and the answer is `x/1`.
//!
//! An explicit bound larger than that default is silently reduced to it. There
//! is no point searching past the point where an exact answer exists.
//!
//! # Precision
//!
//! The whole search runs unrounded, at a working precision of twice the digit
//! count of `x`. That is not a safety margin picked by feel: the numerators and
//! denominators of the convergents grow to roughly the size of `x`'s own
//! numerator and denominator, whose product is what the final comparison
//! divides. Rounding any of the recurrence would put the search on a different
//! sequence of fractions altogether, not merely a less accurate one.

use crate::arith::{add, compare, divide, mul, sub};
use crate::config::rounding;
use crate::error::Error;
use crate::format::digits_to_string;
use crate::roots::magnitude;
use crate::{pow10, Ctx, Decimal, Result, Sign, LOG_BASE};

/// A fraction: numerator and denominator, in that order.
///
/// The original returns a two-element JavaScript array, whose `toString` is
/// `"n,d"` — which is what its test suite compares against. A named pair says
/// the same thing without the caller having to remember which end is which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fraction {
    /// The numerator, carrying the sign of the original value.
    pub numerator: Decimal,
    /// The denominator, always positive.
    pub denominator: Decimal,
}

/// What `toFraction` produces: a fraction, or — for a non-finite value — that
/// value unchanged.
///
/// The original's `if (!xd) return new Ctor(x);` returns a *Decimal* rather
/// than an array from this method, so `new Decimal(NaN).toFraction()` is `NaN`
/// and not `[NaN, NaN]`. That change of return type is invisible in JavaScript
/// and would be easy to lose; here it is in the type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fractional {
    /// A finite value's nearest fraction.
    Ratio(Fraction),
    /// NaN or ±Infinity, returned as itself.
    NonFinite(Decimal),
}

/// `10^e`, in the aligned limb layout.
///
/// The leading limb is `10^(e mod 7)` and the exponent is `e`, so that the
/// decimal point still falls on a limb boundary. The original builds this by
/// mutating a fresh zero in place; stating it as a constructor makes the
/// invariant — one non-zero limb, no others — visible.
fn power_of_ten(e: i64) -> Decimal {
    Decimal::finite(Sign::Pos, e, vec![pow10(e.rem_euclid(LOG_BASE))])
}

/// `x` as the fraction `p/q` with `q` no larger than `max_denominator`, or with
/// `q` the smallest denominator representing `x` exactly when no bound is
/// given.
///
/// A bound that is not a positive integer is an error, and the message carries
/// the offending value as the original renders it.
pub fn to_fraction(
    ctx: &mut Ctx,
    x: &Decimal,
    max_denominator: Option<&Decimal>,
) -> Result<Fractional> {
    if !x.is_finite() {
        return Ok(Fractional::NonFinite(x.clone()));
    }

    let one = Decimal::from_i32(1);

    // The number of decimal places of `x`, and with it the smallest exact
    // denominator, `10^e`.
    let e = x.significant_digits() - x.e - 1;
    let exact_denominator = power_of_ten(e);

    // The largest bound worth searching under: `10^e` if `x` has a fractional
    // part, and 1 if it does not.
    let ceiling = if e > 0 { exact_denominator.clone() } else { one.clone() };

    let max_denominator = match max_denominator {
        None => ceiling,
        Some(n) => {
            if !n.is_integer() || compare(n, &one) == Some(core::cmp::Ordering::Less) {
                return Err(Error::InvalidArgument(crate::format::interpolated(n, &ctx.cfg)));
            }
            // A bound above `10^e` cannot buy a better answer, so it is capped
            // rather than honoured.
            if compare(n, &exact_denominator) == Some(core::cmp::Ordering::Greater) {
                ceiling
            } else {
                n.clone()
            }
        }
    };

    let saved_precision = ctx.cfg.precision;
    let working = x.digits().len() as i64 * LOG_BASE * 2;

    let (numerator, denominator) = ctx.without_clamping(|ctx| {
        ctx.cfg.precision = working;

        // The recurrence runs on |x| — the sign is reattached at the end — and
        // starts from x's digits read as an integer over `10^e`.
        let mut n = crate::parse::parse_decimal(
            ctx,
            Sign::Pos,
            &digits_to_string(x.digits()),
        );
        let mut den = exact_denominator.clone();

        // The two previous convergents, p_{k−1}/q_{k−1} and p_{k−2}/q_{k−2},
        // seeded with 1/0 and 0/1 so that the first pass produces a₀/1.
        let (mut p1, mut q1) = (one.clone(), Decimal::zero(Sign::Pos));
        let (mut p0, mut q0) = (Decimal::zero(Sign::Pos), one.clone());

        loop {
            // The next partial quotient: ⌊numerator / denominator⌋.
            let a = divide(ctx, &n, &den, Some(0), rounding::DOWN, true, None);

            let q_next = {
                let scaled = mul(ctx, &a, &q1);
                add(ctx, &q0, &scaled)
            };

            // The expansion has terminated: `den` reached zero, so `a` is
            // infinite and so is this convergent. The original has no test for
            // this — it relies on the comparison below, since `+Infinity` is
            // greater than any bound.
            //
            // That reliance is what makes upstream's `toFraction` hang under
            // `ROUND_FLOOR`, and this line is the whole of the difference
            // (D-14 / BUG-004). Under that one mode a subtraction which
            // cancels exactly returns *negative* zero, so `a` is `-Infinity`,
            // so this convergent is `-Infinity`, which is not greater than the
            // bound; the loop goes round, `-Infinity × -0` makes it NaN, and
            // every comparison from then on is false. It never terminates —
            // for every finite value, `0` and `1` included.
            //
            // Testing finiteness rather than the sign is not a repair of the
            // arithmetic but of the *termination test*, and it leaves the
            // answer alone: in the eight modes where upstream returns, it
            // breaks at exactly the same iteration with exactly the same
            // convergents, because `+Infinity` already failed the comparison
            // below. In the ninth it returns what the other eight do, which is
            // the only defensible answer — the fraction is a property of the
            // value, and none of this recurrence is supposed to be rounded.
            if !q_next.is_finite() {
                break;
            }

            if compare(&q_next, &max_denominator) == Some(core::cmp::Ordering::Greater) {
                break;
            }

            q0 = q1;
            q1 = q_next;

            let p_next = {
                let scaled = mul(ctx, &a, &p1);
                add(ctx, &p0, &scaled)
            };
            p0 = p1;
            p1 = p_next;

            // And the remainder becomes the next denominator: the expansion of
            // the reciprocal of the fractional part.
            let remainder = {
                let scaled = mul(ctx, &a, &den);
                sub(ctx, &n, &scaled)
            };
            n = den;
            den = remainder;
        }

        // The best semiconvergent under the bound: back the last partial
        // quotient off to j = ⌊(N − q_{k−2}) / q_{k−1}⌋.
        let j = {
            let headroom = sub(ctx, &max_denominator, &q0);
            divide(ctx, &headroom, &q1, Some(0), rounding::DOWN, true, None)
        };
        let mut p_semi = {
            let scaled = mul(ctx, &j, &p1);
            add(ctx, &p0, &scaled)
        };
        let q_semi = {
            let scaled = mul(ctx, &j, &q1);
            add(ctx, &q0, &scaled)
        };

        p_semi.s = x.s;
        p1.s = x.s;

        // Which of the two is closer? Ties go to the convergent, because the
        // original's comparison is `< 1` and not `< 0`.
        let error_of = |ctx: &mut Ctx, p: &Decimal, q: &Decimal| {
            let value = divide(ctx, p, q, Some(working), rounding::DOWN, false, None);
            let difference = sub(ctx, &value, x);
            magnitude(&difference)
        };

        let convergent_error = error_of(ctx, &p1, &q1);
        let semi_error = error_of(ctx, &p_semi, &q_semi);

        if compare(&convergent_error, &semi_error) != Some(core::cmp::Ordering::Greater) {
            (p1, q1)
        } else {
            (p_semi, q_semi)
        }
    });

    ctx.cfg.precision = saved_precision;

    Ok(Fractional::Ratio(Fraction {
        numerator,
        denominator,
    }))
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

    /// Render as the original's array does: `"n,d"`.
    fn run(ctx: &mut Ctx, text: &str, bound: Option<&str>) -> String {
        let bound = bound.map(d);
        let result = to_fraction(ctx, &d(text), bound.as_ref()).expect("a valid bound");
        match result {
            Fractional::Ratio(f) => format!(
                "{},{}",
                to_string(&f.numerator, &ctx.cfg),
                to_string(&f.denominator, &ctx.cfg)
            ),
            Fractional::NonFinite(v) => to_string(&v, &ctx.cfg),
        }
    }

    /// Every expectation is one of the original test suite's own.
    #[test]
    fn the_default_bound_gives_the_exact_fraction() {
        let mut ctx = Ctx::default();
        assert_eq!(run(&mut ctx, "0.1", None), "1,10");
        assert_eq!(run(&mut ctx, "-0.1", None), "-1,10");
        assert_eq!(run(&mut ctx, "0.0", None), "0,1");
        assert_eq!(run(&mut ctx, "-0.625", None), "-5,8");
        assert_eq!(run(&mut ctx, "543.017930", None), "54301793,100000");
        assert_eq!(run(&mut ctx, "123.45", None), "2469,20");
    }

    /// The bounded cases, where the answer is an approximation and the choice
    /// between convergent and semiconvergent actually bites.
    #[test]
    fn a_bounded_denominator_gives_the_nearest_approximation() {
        let mut ctx = Ctx::default();
        assert_eq!(run(&mut ctx, "5.1582612935891", Some("3")), "5,1");
        assert_eq!(run(&mut ctx, "8.14969395596340", Some("4682")), "14645,1797");
        assert_eq!(run(&mut ctx, "4.28004634702", Some("82418")), "350921,81990");
        assert_eq!(run(&mut ctx, "9.610016056348", Some("8529")), "65819,6849");
    }

    /// π to twenty places, under the bounds that make the classical
    /// approximations appear. 22/7 is Archimedes'; 355/113 is Zu Chongzhi's,
    /// and is correct to six decimal places.
    ///
    /// The interesting entry is the last pair. 355/113 is a convergent, and the
    /// convergent after it is 103993/33102 — so under a bound of 30000 there is
    /// *no* convergent between them to return, and the right answer, 94053/29938,
    /// is a semiconvergent. An implementation that stopped at the last
    /// convergent that fits would answer 355/113 here, and would be wrong by a
    /// factor of thirty in its error. This is the case that justifies the
    /// second candidate.
    #[test]
    fn pi_yields_its_classical_approximations() {
        let mut ctx = Ctx::default();
        let pi = "3.14159265358979323846";
        assert_eq!(run(&mut ctx, pi, Some("10")), "22,7");
        assert_eq!(run(&mut ctx, pi, Some("100")), "311,99");
        assert_eq!(run(&mut ctx, pi, Some("113")), "355,113");
        assert_eq!(run(&mut ctx, pi, Some("33102")), "103993,33102");
        assert_eq!(run(&mut ctx, pi, Some("30000")), "94053,29938");
    }

    /// A bound above the exact denominator buys nothing, and is capped.
    #[test]
    fn an_oversized_bound_is_capped_at_the_exact_denominator() {
        let mut ctx = Ctx::default();
        assert_eq!(run(&mut ctx, "123.45", Some("123e399")), "2469,20");
        assert_eq!(run(&mut ctx, "123.45", Some("21")), "2469,20");
        assert_eq!(run(&mut ctx, "123.45", Some("10")), "1111,9");
    }

    #[test]
    fn a_bound_that_is_not_a_positive_integer_is_rejected() {
        let mut ctx = Ctx::default();
        for bad in ["7.5", "0", "0.99", "-1", "-23"] {
            assert!(
                to_fraction(&mut ctx, &d("123.45"), Some(&d(bad))).is_err(),
                "{bad} is not a usable maximum denominator"
            );
        }
        assert!(to_fraction(&mut ctx, &d("123.45"), Some(&Decimal::nan())).is_err());
    }

    /// The answer must not depend on the rounding mode, and the search must
    /// terminate under all nine of them.
    ///
    /// This is the regression test for D-14. Upstream fails it in the strongest
    /// sense available: under `ROUND_FLOOR` it does not return at all, for any
    /// finite input. Nothing in the original suite catches it because every one
    /// of its two hundred `toFraction` assertions runs at the default rounding.
    ///
    /// `0` and `1` are in the list deliberately. It would be reasonable to
    /// expect a defect this shape to need an awkward operand; it needs none.
    #[test]
    fn the_search_terminates_under_every_rounding_mode() {
        let expected = [
            ("0", "0,1"),
            ("1", "1,1"),
            ("7", "7,1"),
            ("-4", "-4,1"),
            ("0.5", "1,2"),
            ("2.5", "5,2"),
            ("0.1", "1,10"),
            ("3.14159", "314159,100000"),
            ("123456789012345678901234567890", "1.2345678901234567890123456789e+29,1"),
        ];

        for mode in 0..=8u8 {
            let mut ctx = Ctx::default();
            ctx.cfg.rounding = mode;
            for (input, answer) in expected {
                assert_eq!(
                    run(&mut ctx, input, None),
                    answer,
                    "toFraction({input}) under rounding mode {mode}"
                );
            }
        }
    }

    #[test]
    fn a_non_finite_value_is_returned_as_itself() {
        let mut ctx = Ctx::default();
        let nan = to_fraction(&mut ctx, &Decimal::nan(), None).unwrap();
        assert!(matches!(nan, Fractional::NonFinite(v) if v.is_nan()));

        let infinite = to_fraction(&mut ctx, &Decimal::infinity(Sign::Pos), None).unwrap();
        assert!(matches!(infinite, Fractional::NonFinite(v) if v.is_infinite()));
    }
}
