//! The value type: its representation, its invariant, and the predicates that
//! read it without changing it.

use crate::{digit_count, LOG_BASE};

/// The sign of a value, including the case where there is no sign because the
/// value is NaN.
///
/// The original stores this in `x.s` as `1`, `-1`, or `NaN`, and relies on
/// `NaN` being falsy and never equal to itself. That works, but it means every
/// site that reads `x.s` has to remember which of three things it might be.
/// Naming the three cases costs one `match` at the boundary and removes the
/// question everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sign {
    /// A positive value, or positive zero.
    Pos,
    /// A negative value, or negative zero.
    Neg,
    /// Not a number. Implies `d == None`.
    Nan,
}

impl Sign {
    /// The sign of a value with the opposite sign. NaN negates to NaN, as it
    /// does in the original, where `-NaN` is still `NaN`.
    #[inline]
    pub fn negated(self) -> Sign {
        match self {
            Sign::Pos => Sign::Neg,
            Sign::Neg => Sign::Pos,
            Sign::Nan => Sign::Nan,
        }
    }

    /// The sign of the product of two values: like signs give `Pos`, unlike
    /// give `Neg`, and NaN is contagious. This is the original's `x.s * y.s`.
    #[inline]
    pub fn product(self, other: Sign) -> Sign {
        match (self, other) {
            (Sign::Nan, _) | (_, Sign::Nan) => Sign::Nan,
            (a, b) if a == b => Sign::Pos,
            _ => Sign::Neg,
        }
    }

    /// `true` for `Neg`; `false` for `Pos` and for `Nan`.
    ///
    /// This is the original's `x.s < 0`, which is `false` for NaN because
    /// every comparison against NaN is false. The rounding modes CEIL/FLOOR
    /// and HALF_CEIL/HALF_FLOOR consult it, so the NaN case has to fall the
    /// same way here as it does there.
    #[inline]
    pub fn is_negative(self) -> bool {
        self == Sign::Neg
    }

    /// `1` for `Pos`, `-1` for `Neg`, `0` for `Nan` — the multiplier form used
    /// where a sign has to be applied to a magnitude.
    #[inline]
    pub fn as_i8(self) -> i8 {
        match self {
            Sign::Pos => 1,
            Sign::Neg => -1,
            Sign::Nan => 0,
        }
    }
}

/// An arbitrary-precision decimal.
///
/// See the crate documentation for the meaning of the fields and for the
/// invariant relating them. In brief:
///
/// ```text
///     finite    d = Some(limbs)   value = ±0.limbs × 10^(e+1)
///     infinite  d = None,  s = Pos | Neg
///     NaN       d = None,  s = Nan
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decimal {
    /// The sign, or `Nan`.
    pub s: Sign,
    /// The base-10 exponent, such that the value is `0.d × 10^(e+1)`.
    ///
    /// `i64` rather than `i32` because the original permits `|e| <= 9e15`,
    /// which does not fit in 32 bits. Arithmetic on it is done with explicit
    /// saturation at those limits: the original computes exponents in IEEE
    /// doubles, where going out of range yields ±Infinity rather than a trap,
    /// and Rust's default of panicking on overflow in debug builds would turn
    /// a value the original merely clamps into a crash.
    ///
    /// Meaningless, and conventionally zero, when `d` is `None`.
    pub e: i64,
    /// Digit limbs in base 10⁷, most significant first; `None` for a
    /// non-finite value.
    pub d: Option<Vec<u32>>,
}

impl Decimal {
    // -- Constructing the three shapes ------------------------------------

    /// NaN.
    #[inline]
    pub fn nan() -> Decimal {
        Decimal {
            s: Sign::Nan,
            e: 0,
            d: None,
        }
    }

    /// ±Infinity.
    #[inline]
    pub fn infinity(s: Sign) -> Decimal {
        debug_assert!(s != Sign::Nan, "infinity must have a definite sign");
        Decimal { s, e: 0, d: None }
    }

    /// ±0.
    ///
    /// The sign of a zero is carried independently of its digits, exactly as
    /// in the original, where the constructor sets `x.s = 1 / v < 0 ? -1 : 1`
    /// and the digit array is `[0]` either way. Negative zero is therefore
    /// representable and distinguishable, and the test suite checks that it
    /// is.
    #[inline]
    pub fn zero(s: Sign) -> Decimal {
        debug_assert!(s != Sign::Nan, "zero must have a definite sign");
        Decimal {
            s,
            e: 0,
            d: Some(vec![0]),
        }
    }

    /// A finite value from raw parts, without normalising them.
    ///
    /// The caller is asserting the invariant: `limbs` non-empty, no trailing
    /// zero limb, and every limb below [`crate::BASE`].
    #[inline]
    pub fn finite(s: Sign, e: i64, limbs: Vec<u32>) -> Decimal {
        debug_assert!(s != Sign::Nan, "a finite value must have a definite sign");
        debug_assert!(!limbs.is_empty(), "a finite value has at least one limb");
        debug_assert!(
            limbs.iter().all(|&w| w < crate::BASE),
            "every limb must be below the base"
        );
        Decimal {
            s,
            e,
            d: Some(limbs),
        }
    }

    /// A finite value of small magnitude, from an integer that fits in one or
    /// two limbs. Used for the many `new Ctor(1)`, `new Ctor(2)`, `new
    /// Ctor(0.5)`-style constants inside the algorithms.
    pub fn from_i32(n: i32) -> Decimal {
        if n == 0 {
            return Decimal::zero(Sign::Pos);
        }
        let s = if n < 0 { Sign::Neg } else { Sign::Pos };
        let mut magnitude = n.unsigned_abs();

        // Split into base-10⁷ limbs, least significant first, then reverse.
        let mut limbs = Vec::new();
        while magnitude > 0 {
            limbs.push(magnitude % crate::BASE);
            magnitude /= crate::BASE;
        }
        limbs.reverse();

        // The exponent counts digits before the point, less one; the leading
        // limb may be short, so it is measured rather than assumed.
        let e = (limbs.len() as i64 - 1) * LOG_BASE + digit_count(limbs[0]) - 1;

        let mut x = Decimal::finite(s, e, limbs);
        x.strip_trailing_zero_limbs();
        x
    }

    // -- Classification ---------------------------------------------------

    /// Whether the value is NaN.
    #[inline]
    pub fn is_nan(&self) -> bool {
        self.s == Sign::Nan
    }

    /// Whether the value is finite — neither infinite nor NaN.
    #[inline]
    pub fn is_finite(&self) -> bool {
        self.d.is_some()
    }

    /// Whether the value is ±Infinity.
    #[inline]
    pub fn is_infinite(&self) -> bool {
        self.d.is_none() && self.s != Sign::Nan
    }

    /// Whether the value is ±0.
    #[inline]
    pub fn is_zero(&self) -> bool {
        matches!(self.d.as_deref(), Some([0]))
    }

    /// Whether the value is negative. NaN is not negative; negative zero is.
    #[inline]
    pub fn is_negative(&self) -> bool {
        self.s.is_negative()
    }

    /// Whether the value is a **finite** integer.
    ///
    /// The original writes this as `!!this.d && mathfloor(this.e / LOG_BASE) >
    /// this.d.length - 2`: the digit array must exist, and must not extend past
    /// the units place.
    ///
    /// The leading `!!this.d` is the whole of the difference between this and
    /// the predicate one would write from the name. `Infinity` is not an
    /// integer here, and neither is `NaN` — which is the answer IEEE 754 gives
    /// and the answer `isFiniteEtc` checks for, but it is *not* the answer that
    /// falls out of "the digits do not reach past the point", since a
    /// non-finite value has no digits at all.
    ///
    /// It matters beyond that module: `toFraction` validates its argument with
    /// `!n.isInt()`, and so rejects a non-finite maximum denominator through
    /// this predicate rather than through a finiteness test of its own.
    #[inline]
    pub fn is_integer(&self) -> bool {
        match &self.d {
            None => false,
            Some(limbs) => self.e.div_euclid(LOG_BASE) + 1 >= limbs.len() as i64,
        }
    }

    // -- Reading the digits -----------------------------------------------

    /// The digit limbs of a finite value.
    ///
    /// # Panics
    ///
    /// Panics if the value is non-finite. Callers inside this crate check
    /// finiteness first, in the same order the original does; a panic here
    /// means a missing check, which is a bug rather than a bad input.
    #[inline]
    pub fn digits(&self) -> &[u32] {
        self.d
            .as_deref()
            .expect("digits() called on a non-finite value")
    }

    /// The number of significant decimal digits.
    ///
    /// This is the original's `getPrecision`: seven digits per limb, less the
    /// trailing zeros of the last limb, less the leading zeros of the first.
    /// A zero value has precision 1.
    pub fn significant_digits(&self) -> i64 {
        let limbs = self.digits();
        let last = limbs.len() - 1;
        let mut len = last as i64 * LOG_BASE + 1;

        let mut w = limbs[last];
        if w != 0 {
            while w % 10 == 0 {
                w /= 10;
                len -= 1;
            }
            let mut w = limbs[0];
            while w >= 10 {
                w /= 10;
                len += 1;
            }
        }
        len
    }

    /// The number of decimal places, i.e. digits after the point.
    ///
    /// The original's `decimalPlaces`, which returns NaN for a non-finite
    /// value; that case is the `None` here.
    pub fn decimal_places(&self) -> Option<i64> {
        let limbs = self.d.as_deref()?;

        let mut dp = (limbs.len() as i64 - 1 - self.e.div_euclid(LOG_BASE)) * LOG_BASE;

        // Discount the trailing zeros of the final limb.
        let mut w = *limbs.last().expect("non-empty by the invariant");
        if w != 0 {
            while w % 10 == 0 {
                w /= 10;
                dp -= 1;
            }
        }

        Some(dp.max(0))
    }

    // -- Maintaining the invariant ----------------------------------------

    /// Drop trailing zero limbs, which the invariant forbids, leaving at least
    /// one limb behind.
    ///
    /// This is the original's `for (i = xd.length; xd[--i] === 0;) xd.pop();`
    /// — the last act of `finalise`, and of several routines that build a
    /// digit array by hand.
    pub(crate) fn strip_trailing_zero_limbs(&mut self) {
        if let Some(limbs) = self.d.as_mut() {
            while limbs.len() > 1 && *limbs.last().expect("len > 1") == 0 {
                limbs.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_shapes_are_distinguishable() {
        let nan = Decimal::nan();
        assert!(nan.is_nan() && !nan.is_finite() && !nan.is_infinite());

        let inf = Decimal::infinity(Sign::Neg);
        assert!(inf.is_infinite() && !inf.is_finite() && !inf.is_nan());
        assert!(inf.is_negative());

        let zero = Decimal::zero(Sign::Pos);
        assert!(zero.is_finite() && zero.is_zero() && !zero.is_nan());
    }

    #[test]
    fn negative_zero_keeps_its_sign() {
        let neg = Decimal::zero(Sign::Neg);
        let pos = Decimal::zero(Sign::Pos);
        assert!(neg.is_zero() && pos.is_zero());
        assert!(neg.is_negative() && !pos.is_negative());
        assert_ne!(neg, pos, "-0 and 0 differ, as they do in the original");
    }

    #[test]
    fn nan_is_not_negative_even_though_it_has_no_sign() {
        // The original relies on `NaN < 0` being false; the rounding modes
        // depend on it falling this way.
        assert!(!Decimal::nan().is_negative());
    }

    #[test]
    fn sign_products_follow_the_originals_multiplication() {
        assert_eq!(Sign::Pos.product(Sign::Pos), Sign::Pos);
        assert_eq!(Sign::Neg.product(Sign::Neg), Sign::Pos);
        assert_eq!(Sign::Pos.product(Sign::Neg), Sign::Neg);
        assert_eq!(Sign::Nan.product(Sign::Pos), Sign::Nan);
        assert_eq!(Sign::Pos.product(Sign::Nan), Sign::Nan);
    }

    #[test]
    fn small_integers_round_trip_through_limbs() {
        for n in [1, 2, 5, 9, 10, 9_999_999, 10_000_000, 12_345_678, 2_000_000_000] {
            let x = Decimal::from_i32(n);
            assert!(x.is_finite() && !x.is_negative());
            // 0.d × 10^(e+1) == n, checked via the digit count.
            assert_eq!(
                x.significant_digits(),
                n.to_string().trim_end_matches('0').len() as i64,
                "significant digits of {n}"
            );
            assert_eq!(x.e, n.to_string().len() as i64 - 1, "exponent of {n}");
        }
        assert!(Decimal::from_i32(0).is_zero());
        assert!(Decimal::from_i32(-7).is_negative());
    }

    #[test]
    fn integrality_matches_the_digit_extent() {
        // 1 -> e = 0, one limb: integer.
        assert!(Decimal::from_i32(1).is_integer());
        // 0.5 -> e = -1, one limb: not an integer.
        let half = Decimal::finite(Sign::Pos, -1, vec![5_000_000]);
        assert!(!half.is_integer());

        // Neither infinity nor NaN is an integer — the original's leading
        // `!!this.d`. Read off `new Decimal(Infinity).isInt()` in Node, which
        // answers false; the predicate is about *finite* integrality.
        assert!(!Decimal::infinity(Sign::Pos).is_integer());
        assert!(!Decimal::infinity(Sign::Neg).is_integer());
        assert!(!Decimal::nan().is_integer());
    }
}
