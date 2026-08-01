//! Comparison, addition, subtraction, multiplication and division.
//!
//! # The shape of these routines
//!
//! All five share a preamble and an epilogue, and almost all of the subtlety
//! is in them rather than in the arithmetic proper.
//!
//! The **preamble** disposes of the non-finite and zero cases. There are more
//! of these than one expects — `0 − 0`, `∞ − ∞`, `0 × ∞`, `∞ / ∞`, `0 / 0`
//! each have their own answer, and the sign of the result is not always the
//! sign one would guess. The original writes these as dense nested
//! conditionals; they are written out here as separate cases, because the
//! whole point of the exercise is that they come out the same.
//!
//! The **epilogue** is `finalise`, which rounds to the working precision and
//! applies the exponent limits — but only when `ctx.external` is set. An
//! intermediate value computed inside `pow` or `ln` deliberately skips it.
//!
//! In between, the arithmetic operates on base-10⁷ limb arrays that have first
//! been *aligned*: since a value is `0.d × 10^(e+1)` and limbs carry seven
//! digits each, two values can be added limb-wise only once their base-10⁷
//! exponents `⌊e/7⌋` agree. Aligning them means prepending zero limbs to
//! whichever has the smaller exponent.
//!
//! ## Why alignment is capped
//!
//! Adding `1e1000000` to `1` would, done naively, prepend about 140,000 zero
//! limbs to the smaller value, nearly all of which cannot affect a result
//! carrying twenty significant digits. Both `plus` and `minus` therefore cap
//! the number of prepended zeros at roughly `⌈precision/7⌉`, and truncate the
//! smaller operand to a single limb when the cap bites.
//!
//! This is not merely an optimisation, and it is the reason the cap is
//! transcribed rather than reinvented: the truncated operand still has to
//! influence the rounding of the last digit — it is what makes the result
//! *inexact* — so where exactly the cap falls, and the `+ 2` on it in `minus`
//! against the `+ 1` in `plus`, are observable in the final digit. They are
//! copied exactly.

use crate::{digit_count, format, round::finalise, Ctx, Decimal, Sign, BASE, LOG_BASE};
use core::cmp::Ordering;

/// The base-10 exponent implied by a base-10⁷ exponent and a leading limb.
///
/// The original's `getBase10Exponent`: seven decimal digits per limb, plus
/// however many digits the leading limb actually has.
pub(crate) fn base10_exponent(digits: &[u32], e: i64) -> i64 {
    e * LOG_BASE + digit_count(digits[0]) - 1
}

/// `⌈a / b⌉` for non-negative `a` and positive `b`.
fn ceil_div(a: i64, b: i64) -> i64 {
    if a <= 0 {
        0
    } else {
        (a + b - 1) / b
    }
}

/// Prepend `count` zero limbs, shifting the value down by that many limbs.
fn prepend_zeros(d: &mut Vec<u32>, count: i64) {
    if count <= 0 {
        return;
    }
    let mut zeros = vec![0u32; count as usize];
    zeros.append(d);
    *d = zeros;
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// Compare two values, or report that they are unordered because one is NaN.
///
/// The original returns `1`, `-1`, `0` or `NaN` from a single dense
/// expression; `Option<Ordering>` says the same thing with the NaN case
/// visible in the type.
pub fn compare(x: &Decimal, y: &Decimal) -> Option<Ordering> {
    // Either NaN or ±Infinity?
    if x.d.is_none() || y.d.is_none() {
        if x.is_nan() || y.is_nan() {
            return None;
        }
        if x.s != y.s {
            return Some(order_of(x.s));
        }
        // Same sign, and at least one is infinite.
        return Some(match (x.d.is_none(), y.d.is_none()) {
            (true, true) => Ordering::Equal, // both ±Infinity, same sign
            // The infinite one is the larger in magnitude; the sign then says
            // whether larger in magnitude means greater or less.
            (infinite_x, _) => {
                if infinite_x ^ x.s.is_negative() {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
        });
    }

    let xd = x.digits();
    let yd = y.digits();

    // Either zero? A zero compares by the *other* value's sign, and two zeros
    // are equal regardless of their signs — so -0 == 0, as in the original.
    if xd[0] == 0 || yd[0] == 0 {
        return Some(if xd[0] != 0 {
            order_of(x.s)
        } else if yd[0] != 0 {
            order_of(y.s.negated())
        } else {
            Ordering::Equal
        });
    }

    if x.s != y.s {
        return Some(order_of(x.s));
    }

    if x.e != y.e {
        return Some(orient((x.e > y.e) ^ x.s.is_negative()));
    }

    for (a, b) in xd.iter().zip(yd.iter()) {
        if a != b {
            return Some(orient((a > b) ^ x.s.is_negative()));
        }
    }

    Some(match xd.len().cmp(&yd.len()) {
        Ordering::Equal => Ordering::Equal,
        longer => orient((longer == Ordering::Greater) ^ x.s.is_negative()),
    })
}

fn order_of(s: Sign) -> Ordering {
    if s.is_negative() {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

fn orient(greater: bool) -> Ordering {
    if greater {
        Ordering::Greater
    } else {
        Ordering::Less
    }
}

// ---------------------------------------------------------------------------
// Addition and subtraction
// ---------------------------------------------------------------------------

/// `x + y`.
///
/// # The argument is re-judged first
///
/// `P.plus` opens with `y = new Ctor(y)`, and so do `P.minus` and `P.times` —
/// a clamping copy, not a clone (D-12). There is no function form of these in
/// the original; every internal use is a method call, so the copy happens every
/// time. Hence [`clamped_copy`] here rather than at the call sites.
///
/// It is a no-op while the clamps are suppressed, which is most of the time
/// inside this crate, and it bites exactly where the original's does:
/// `asinh(-1.5e300)` with `maxE` at 100 is NaN, because `sqrt` turns the clamps
/// back on (see `Ctx::without_clamping`) and the `.plus(x)` that follows it
/// then re-judges `x` into −Infinity, against a +Infinity root.
///
/// `divide` is deliberately not in this list: it *is* a function in the
/// original, called directly by a dozen routines, and it does not re-judge. The
/// method that does is `P.dividedBy`, which the adapter reaches through
/// `coerce`.
pub fn add(ctx: &mut Ctx, x: &Decimal, y: &Decimal) -> Decimal {
    let y = &crate::ops::clamped_copy(ctx, y);

    // Non-finite operands.
    if x.d.is_none() || y.d.is_none() {
        if x.is_nan() || y.is_nan() {
            return Decimal::nan();
        }
        if x.d.is_none() {
            // x is infinite: y finite, or both infinite with the same sign,
            // gives x; opposite infinities give NaN.
            return if y.d.is_some() || x.s == y.s {
                x.clone()
            } else {
                Decimal::nan()
            };
        }
        // x finite, y infinite.
        return y.clone();
    }

    if x.s != y.s {
        return sub(ctx, x, &negated(y));
    }

    let pr = ctx.cfg.precision;
    let rm = ctx.cfg.rounding;

    if x.digits()[0] == 0 || y.digits()[0] == 0 {
        // Adding zero: the result is whichever operand is non-zero, and if
        // both are zero it is the second operand — which carries the sign the
        // original gives to `0 + 0`.
        let mut result = if y.digits()[0] == 0 {
            x.clone()
        } else {
            y.clone()
        };
        if ctx.external {
            finalise(ctx, &mut result, Some(pr), rm, false);
        }
        return result;
    }

    let mut xd = x.digits().to_vec();
    let mut yd = y.digits().to_vec();

    // Align the two limb arrays on a common base-10⁷ exponent.
    let mut e = y.e.div_euclid(LOG_BASE);
    let k = x.e.div_euclid(LOG_BASE);
    let mut shift = k - e;

    if shift != 0 {
        let x_is_smaller = shift < 0;
        if x_is_smaller {
            shift = -shift;
        } else {
            e = k;
        }
        let len = if x_is_smaller { yd.len() } else { xd.len() } as i64;

        // Cap the padding, and drop the smaller operand to a single limb when
        // the cap applies. `+ 1` here; `minus` uses `+ 2`.
        let cap = ceil_div(pr, LOG_BASE);
        let limit = if cap > len { cap + 1 } else { len + 1 };
        let target = if x_is_smaller { &mut xd } else { &mut yd };
        if shift > limit {
            shift = limit;
            target.truncate(1);
        }
        // The cap bounds the padding by the working precision — but the
        // working precision is itself unbounded, and `asinh` raises it to
        // twice the operand's exponent. See `Ctx::array_limit_exceeded`.
        if shift + target.len() as i64 > crate::MAX_ARRAY_LENGTH {
            ctx.array_limit_exceeded = true;
            return Decimal::nan();
        }
        prepend_zeros(target, shift);
    }

    // Add in place into the longer array, which is guaranteed to hold the
    // result: the two arrays now start at the same exponent, so the longer
    // one's extra limbs are the less significant ones and are already correct.
    if xd.len() < yd.len() {
        core::mem::swap(&mut xd, &mut yd);
    }
    let mut carry: u32 = 0;
    let mut i = yd.len();
    while i > 0 {
        i -= 1;
        let total = xd[i] + yd[i] + carry;
        xd[i] = total % BASE;
        carry = total / BASE;
    }

    if carry != 0 {
        xd.insert(0, carry);
        e += 1;
    }

    // No zero check is needed: two non-zero values of the same sign cannot
    // sum to zero.
    while xd.last() == Some(&0) {
        xd.pop();
    }

    let mut result = Decimal::finite(x.s, base10_exponent(&xd, e), xd);
    if ctx.external {
        finalise(ctx, &mut result, Some(pr), rm, false);
    }
    result
}

/// `x − y`. The argument is re-judged against the exponent limits first; see
/// [`add`].
pub fn sub(ctx: &mut Ctx, x: &Decimal, y: &Decimal) -> Decimal {
    let y = &crate::ops::clamped_copy(ctx, y);

    // Non-finite operands.
    if x.d.is_none() || y.d.is_none() {
        if x.is_nan() || y.is_nan() {
            return Decimal::nan();
        }
        if x.d.is_some() {
            // x finite, y infinite: the answer is −y.
            return Decimal::infinity(y.s.negated());
        }
        // x is infinite: y finite, or opposite infinities, gives x; two
        // infinities of the same sign give NaN.
        return if y.d.is_some() || x.s != y.s {
            x.clone()
        } else {
            Decimal::nan()
        };
    }

    if x.s != y.s {
        return add(ctx, x, &negated(y));
    }

    let pr = ctx.cfg.precision;
    let rm = ctx.cfg.rounding;

    if x.digits()[0] == 0 || y.digits()[0] == 0 {
        let mut result = if y.digits()[0] != 0 {
            negated(y)
        } else if x.digits()[0] != 0 {
            x.clone()
        } else {
            // Both zero. IEEE 754 (2008) §6.3: 0 − 0 is −0 when rounding
            // towards −Infinity, and +0 otherwise.
            return Decimal::zero(if rm == 3 { Sign::Neg } else { Sign::Pos });
        };
        if ctx.external {
            finalise(ctx, &mut result, Some(pr), rm, false);
        }
        return result;
    }

    let mut xd = x.digits().to_vec();
    let mut yd = y.digits().to_vec();

    let mut e = y.e.div_euclid(LOG_BASE);
    let xe = x.e.div_euclid(LOG_BASE);
    let mut shift = xe - e;

    // `x_is_smaller` records which operand has the smaller magnitude, so that
    // the subtraction can always be done larger-minus-smaller and the sign
    // applied afterwards.
    let x_is_smaller;
    let skip;

    if shift != 0 {
        x_is_smaller = shift < 0;
        if x_is_smaller {
            shift = -shift;
        } else {
            e = xe;
        }
        let len = if x_is_smaller { yd.len() } else { xd.len() } as i64;
        let limit = ceil_div(pr, LOG_BASE).max(len) + 2;
        let target = if x_is_smaller { &mut xd } else { &mut yd };
        if shift > limit {
            shift = limit;
            target.truncate(1);
        }
        // As in `add`: the cap is proportional to a precision that callers may
        // have raised past anything allocatable.
        if shift + target.len() as i64 > crate::MAX_ARRAY_LENGTH {
            ctx.array_limit_exceeded = true;
            return Decimal::nan();
        }
        prepend_zeros(target, shift);
        skip = shift;
    } else {
        // Same base-10⁷ exponent: compare limb by limb to find the larger.
        let mut smaller = xd.len() < yd.len();
        let common = xd.len().min(yd.len());
        for i in 0..common {
            if xd[i] != yd[i] {
                smaller = xd[i] < yd[i];
                break;
            }
        }
        x_is_smaller = smaller;
        skip = 0;
    }

    let mut sign = x.s;
    if x_is_smaller {
        core::mem::swap(&mut xd, &mut yd);
        sign = sign.negated();
    }

    // Pad the minuend so that it is at least as long as the subtrahend. The
    // subtrahend is deliberately *not* padded: the subtraction only has to
    // start where it ends.
    while xd.len() < yd.len() {
        xd.push(0);
    }

    let mut i = yd.len();
    while i > skip as usize {
        i -= 1;
        if xd[i] < yd[i] {
            // Borrow from the nearest non-zero limb to the left, turning every
            // zero limb passed on the way into BASE − 1.
            let mut j = i;
            loop {
                if j == 0 {
                    break;
                }
                j -= 1;
                if xd[j] == 0 {
                    xd[j] = BASE - 1;
                } else {
                    break;
                }
            }
            xd[j] -= 1;
            xd[i] += BASE;
        }
        xd[i] -= yd[i];
    }

    while xd.last() == Some(&0) {
        xd.pop();
    }
    while xd.first() == Some(&0) {
        xd.remove(0);
        e -= 1;
    }

    if xd.is_empty() {
        return Decimal::zero(if rm == 3 { Sign::Neg } else { Sign::Pos });
    }

    let mut result = Decimal::finite(sign, base10_exponent(&xd, e), xd);
    if ctx.external {
        finalise(ctx, &mut result, Some(pr), rm, false);
    }
    result
}

/// A copy with the sign flipped. NaN negates to NaN.
pub fn negated(x: &Decimal) -> Decimal {
    let mut out = x.clone();
    out.s = out.s.negated();
    out
}

// ---------------------------------------------------------------------------
// Multiplication
// ---------------------------------------------------------------------------

/// `x × y`, by long multiplication. The argument is re-judged against the
/// exponent limits first; see [`add`].
pub fn mul(ctx: &mut Ctx, x: &Decimal, y: &Decimal) -> Decimal {
    let y = &crate::ops::clamped_copy(ctx, y);
    let sign = x.s.product(y.s);

    let x_zero = x.d.as_deref().map(|d| d[0] == 0).unwrap_or(false);
    let y_zero = y.d.as_deref().map(|d| d[0] == 0).unwrap_or(false);

    if x.d.is_none() || x_zero || y.d.is_none() || y_zero {
        // NaN if either operand is NaN, or if a zero meets an infinity.
        if sign == Sign::Nan || (x_zero && y.d.is_none()) || (y_zero && x.d.is_none()) {
            return Decimal::nan();
        }
        // Otherwise an infinity dominates, and failing that the result is a
        // signed zero.
        return if x.d.is_none() || y.d.is_none() {
            Decimal::infinity(sign)
        } else {
            Decimal::zero(sign)
        };
    }

    let mut e = x.e.div_euclid(LOG_BASE) + y.e.div_euclid(LOG_BASE);

    let (long, short) = if x.digits().len() < y.digits().len() {
        (y.digits(), x.digits())
    } else {
        (x.digits(), y.digits())
    };

    // The product of two limbs is below 10¹⁴, so a `u64` accumulator holds a
    // limb product plus a limb plus a carry without any risk. The original
    // relies on the same bound holding in an IEEE double, which is why the
    // base is 10⁷ and not something larger.
    let mut r = vec![0u32; long.len() + short.len()];
    let mut carry: u64 = 0;

    for i in (0..short.len()).rev() {
        carry = 0;
        let mut k = long.len() + i;
        while k > i {
            let t = u64::from(r[k]) + u64::from(short[i]) * u64::from(long[k - i - 1]) + carry;
            r[k] = (t % u64::from(BASE)) as u32;
            carry = t / u64::from(BASE);
            k -= 1;
        }
        r[k] = ((u64::from(r[k]) + carry) % u64::from(BASE)) as u32;
    }

    while r.last() == Some(&0) {
        r.pop();
    }

    // The leading slot of the result array is only occupied if the final carry
    // filled it; otherwise it is a spurious zero and the exponent stands.
    if carry != 0 {
        e += 1;
    } else {
        r.remove(0);
    }

    let mut result = Decimal::finite(sign, base10_exponent(&r, e), r);
    if ctx.external {
        let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
        finalise(ctx, &mut result, Some(pr), rm, false);
    }
    result
}

// ---------------------------------------------------------------------------
// Division
// ---------------------------------------------------------------------------

/// Multiply a limb array by a single limb, returning a fresh array.
fn multiply_integer(x: &[u32], k: u32, base: u32) -> Vec<u32> {
    let mut out = x.to_vec();
    let mut carry: u64 = 0;
    for i in (0..out.len()).rev() {
        let temp = u64::from(out[i]) * u64::from(k) + carry;
        out[i] = (temp % u64::from(base)) as u32;
        carry = temp / u64::from(base);
    }
    if carry != 0 {
        out.insert(0, carry as u32);
    }
    out
}

/// Compare the first `a_len` limbs of `a` with the first `b_len` of `b`.
fn compare_limbs(a: &[u32], b: &[u32], a_len: usize, b_len: usize) -> i32 {
    if a_len != b_len {
        return if a_len > b_len { 1 } else { -1 };
    }
    for i in 0..a_len {
        if a[i] != b[i] {
            return if a[i] > b[i] { 1 } else { -1 };
        }
    }
    0
}

/// Subtract `b` from `a` in place over `a_len` limbs, then drop leading zeros.
///
/// The intermediate `a[i] - borrow` is allowed to go negative before the
/// borrow from the next limb is added back, and the sign of that intermediate
/// is what decides whether a borrow propagates. In JavaScript that happens for
/// free; here it needs a signed accumulator. Doing this in `u32` gives
/// `0 - 1 == u32::MAX`, which then compares as *larger* than the subtrahend,
/// suppresses the borrow, and silently produces a limb far above the base —
/// a defect that surfaces nowhere near the subtraction that caused it.
fn subtract_limbs(a: &mut Vec<u32>, b: &[u32], a_len: usize, base: u32) {
    let mut borrow: i64 = 0;
    for i in (0..a_len).rev() {
        let bi = i64::from(b.get(i).copied().unwrap_or(0));
        let reduced = i64::from(a[i]) - borrow;
        borrow = i64::from(reduced < bi);
        a[i] = (borrow * i64::from(base) + reduced - bi) as u32;
    }
    while a.len() > 1 && a[0] == 0 {
        a.remove(0);
    }
}

/// `x / y`, by long division.
///
/// `pr` is the significant-digit target; `None` means "use the configured
/// precision", which also selects the configured rounding mode. `dp` switches
/// the target from significant digits to decimal places. `base`, when given,
/// runs the routine over a digit array in that base with one digit per limb —
/// which is how base conversion reuses this code.
pub fn divide(
    ctx: &mut Ctx,
    x: &Decimal,
    y: &Decimal,
    pr: Option<i64>,
    rm: u8,
    dp: bool,
    base: Option<u32>,
) -> Decimal {
    let sign = if x.s == y.s { Sign::Pos } else { Sign::Neg };

    let x_zero = x.d.as_deref().map(|d| d[0] == 0).unwrap_or(false);
    let y_zero = y.d.as_deref().map(|d| d[0] == 0).unwrap_or(false);

    if x.d.is_none() || x_zero || y.d.is_none() || y_zero {
        // NaN if either is NaN, if both are zero, or if both are infinite.
        let both_zero = x.d.is_some() && y.d.is_some() && x_zero && y_zero;
        let both_infinite = x.d.is_none() && y.d.is_none();
        if x.is_nan() || y.is_nan() || both_zero || both_infinite {
            return Decimal::nan();
        }
        // Zero numerator or infinite denominator gives a signed zero;
        // otherwise the denominator is zero and the result is infinite.
        return if x_zero || y.d.is_none() {
            Decimal::zero(sign)
        } else {
            Decimal::infinity(sign)
        };
    }

    let (base, log_base, mut e) = match base {
        Some(b) => (b, 1i64, x.e - y.e),
        None => (
            BASE,
            LOG_BASE,
            x.e.div_euclid(LOG_BASE) - y.e.div_euclid(LOG_BASE),
        ),
    };

    let mut xd = x.digits().to_vec();
    let mut yd = y.digits().to_vec();
    let mut y_len = yd.len();
    let mut x_len = xd.len();

    let mut qd: Vec<u32> = Vec::new();

    // The result exponent may be one less than `e`, when the divisor's leading
    // digits exceed the dividend's. The comparison stops at the end of the
    // divisor: in the original the walk runs off the end of `yd` and compares
    // `undefined`, which is never equal and never greater, so both tests below
    // are guarded by `i < yd.len()`.
    //
    // A digit array arriving here from `toStringBinary` may carry trailing
    // zeros, hence the bounds-tolerant read of `xd`.
    {
        let mut i = 0;
        while i < yd.len() && yd[i] == xd.get(i).copied().unwrap_or(0) {
            i += 1;
        }
        if i < yd.len() && yd[i] > xd.get(i).copied().unwrap_or(0) {
            e -= 1;
        }
    }

    let (sd, rm) = match pr {
        None => (ctx.cfg.precision, ctx.cfg.rounding),
        Some(p) if dp => (p + (x.e - y.e) + 1, rm),
        Some(p) => (p, rm),
    };
    let pr = pr.unwrap_or(ctx.cfg.precision);

    let more;

    if sd < 0 {
        qd.push(1);
        more = true;
    } else {
        // Convert the target from decimal digits to limbs, with two limbs of
        // headroom so that rounding has something to look at.
        let mut sd = sd / log_base + 2;
        let mut i = 0usize;

        if y_len == 1 {
            // A single-limb divisor divides in one pass, with the remainder
            // carried in a scalar.
            let divisor = u64::from(yd[0]);
            let mut k: u64 = 0;
            sd += 1;

            // The original's `for (; (i < xL || k) && sd--; i++)`. The
            // decrement of `sd` happens only when the first test passes, so it
            // belongs in the body and not in the condition.
            while (i < x_len || k != 0) && sd != 0 {
                sd -= 1;
                let t = k * u64::from(base) + u64::from(xd.get(i).copied().unwrap_or(0));
                qd.push((t / divisor) as u32);
                k = t % divisor;
                i += 1;
            }

            more = k != 0 || i < x_len;
        } else {
            // Knuth's algorithm D, as the original writes it: normalise so the
            // divisor's leading limb is at least half the base, then estimate
            // one quotient limb at a time and correct the estimate.
            let mut k = base / (yd[0] + 1);
            if k > 1 {
                yd = multiply_integer(&yd, k, base);
                xd = multiply_integer(&xd, k, base);
                y_len = yd.len();
                x_len = xd.len();
            }

            let mut xi = y_len;
            let mut rem: Vec<u32> = xd[..y_len.min(xd.len())].to_vec();
            let mut rem_len = rem.len();
            while rem_len < y_len {
                rem.push(0);
                rem_len += 1;
            }

            let mut yz = yd.clone();
            yz.insert(0, 0);

            let mut yd0 = yd[0];
            if yd[1] >= base / 2 {
                yd0 += 1;
            }

            // Tracks whether `rem[0]` would be `undefined` in the original.
            let mut rem_is_defined = true;

            loop {
                k = 0;
                let mut cmp = compare_limbs(&yd, &rem, y_len, rem_len);

                if cmp < 0 {
                    // Estimate how many times the divisor goes into the
                    // remainder, using only the leading limbs.
                    let mut rem0 = u64::from(rem[0]);
                    if y_len != rem_len {
                        rem0 = rem0 * u64::from(base) + u64::from(rem.get(1).copied().unwrap_or(0));
                    }
                    k = (rem0 / u64::from(yd0)) as u32;

                    let prod;
                    if k > 1 {
                        if k >= base {
                            k = base - 1;
                        }
                        let mut p = multiply_integer(&yd, k, base);
                        let prod_len = p.len();
                        rem_len = rem.len();
                        cmp = compare_limbs(&p, &rem, prod_len, rem_len);

                        // The estimate can be one too high; correct it by
                        // subtracting the divisor back off the product.
                        if cmp == 1 {
                            k -= 1;
                            let d: &[u32] = if y_len < prod_len { &yz } else { &yd };
                            subtract_limbs(&mut p, d, prod_len, base);
                        }
                        prod = p;
                    } else {
                        // `cmp` is −1 here. A zero estimate needs no second
                        // comparison below, so it is forced to 1 to skip it.
                        if k == 0 {
                            cmp = 1;
                            k = 1;
                        }
                        prod = yd.clone();
                    }

                    let mut prod = prod;
                    let prod_len = prod.len();
                    if prod_len < rem_len {
                        prod.insert(0, 0);
                    }

                    subtract_limbs(&mut rem, &prod, rem_len, base);

                    // If the product was below the remainder, the estimate may
                    // have been one too low.
                    if cmp == -1 {
                        rem_len = rem.len();
                        cmp = compare_limbs(&yd, &rem, y_len, rem_len);
                        if cmp < 1 {
                            k += 1;
                            let d: &[u32] = if y_len < rem_len { &yz } else { &yd };
                            subtract_limbs(&mut rem, d, rem_len, base);
                        }
                    }

                    rem_len = rem.len();
                } else if cmp == 0 {
                    k += 1;
                    rem = vec![0];
                    rem_len = 1;
                }
                // When cmp == 1 the divisor exceeds the remainder and k stays 0.

                qd.push(k);
                i += 1;

                // Bring down the next digit of the dividend. When the dividend
                // is exhausted the original stores `undefined` here, which is
                // what its loop condition and its `more` flag both test; that
                // is tracked explicitly rather than by a sentinel value,
                // because 0 is a perfectly good digit.
                if cmp != 0 && rem[0] != 0 {
                    rem.push(xd.get(xi).copied().unwrap_or(0));
                    rem_len += 1;
                } else {
                    match xd.get(xi).copied() {
                        Some(digit) => {
                            rem = vec![digit];
                            rem_is_defined = true;
                        }
                        None => {
                            rem = vec![0];
                            rem_is_defined = false;
                        }
                    }
                    rem_len = 1;
                }

                // The original's `while ((xi++ < xL || rem[0] !== void 0) && sd--)`.
                // Both `xi++` and `sd--` are post-increments inside the
                // condition, and `sd--` is only reached when the left disjunct
                // holds, so the order here is load-bearing.
                let dividend_remains = xi < x_len;
                xi += 1;
                let keep_going = (dividend_remains || rem_is_defined) && sd != 0;
                sd -= 1;
                if !keep_going {
                    break;
                }
            }

            more = rem_is_defined;
        }

        // A leading zero limb is an artefact of the estimate, not a digit.
        if qd.first() == Some(&0) {
            qd.remove(0);
        }
        let _ = i;
    }

    if qd.is_empty() {
        qd.push(0);
    }

    let mut q = Decimal::finite(sign, 0, qd);

    if log_base == 1 {
        // Base conversion: the caller wants the raw digits and the exactness
        // flag, not a rounded decimal.
        q.e = e;
        ctx.inexact = more;
    } else {
        let digits = q.digits();
        q.e = digit_count(digits[0]) + e * log_base - 1;
        let target = if dp { pr + q.e + 1 } else { pr };
        finalise(ctx, &mut q, Some(target), rm, more);
    }

    q
}

/// Truncate a digit array to `len` limbs, reporting whether anything was lost.
///
/// The original's `truncate`, whose return value is `undefined` rather than
/// `false` when nothing was dropped — used only for its truthiness.
fn truncate_limbs(d: &mut Vec<u32>, len: usize) -> bool {
    if d.len() > len {
        d.truncate(len);
        true
    } else {
        false
    }
}

/// `x^n` for an integer `n`, by exponentiation by squaring.
///
/// The working arrays are truncated to `⌈pr/7⌉ + 4` limbs at every step rather
/// than being allowed to grow, which is what keeps a large exponent from
/// producing a multi-megabyte intermediate. The final `++r.d[n]` looks like a
/// mistake and is not: when the result was truncated but happens to end in a
/// zero limb, that zero would make the value look exact to `finalise`, so the
/// original bumps it to keep the inexactness visible. It is transcribed
/// exactly, including the fact that it perturbs the last limb.
pub fn int_pow(ctx: &mut Ctx, x: &Decimal, n: i64, pr: i64) -> Decimal {
    let mut is_truncated = false;
    let mut r = Decimal::from_i32(1);
    let mut x = x.clone();
    let mut n = n;
    let k = ceil_div(pr, LOG_BASE) + 4;

    // Deliberately *not* `without_clamping`. The original ends this function
    // with a bare
    //
    //     external = true;
    //
    // which sets the flag rather than restoring what it was — and callers exist
    // that had turned clamping off and do not expect it back. `parseOther` is
    // one: it clears the flag around the whole radix conversion, then reaches
    // `Decimal.pow(2, p)`, which reaches here, which hands clamping back on.
    // `pow`'s very next line is its reciprocal branch, `new Ctor(1).div(r)`,
    // and `div` re-judges its argument against `maxE` — so it clamps after all.
    //
    // That is observable, and not obscurely. With `maxE` at 41,
    // `new Decimal('0x1p-1074')` is 0 upstream: `2^1074` has exponent 323, the
    // restored flag lets the division's constructor see it, and it becomes
    // Infinity on the way in. A correct save-and-restore here answers 5e-324 —
    // the better value, and the wrong one for a port. Found by the differential
    // campaign; no assertion in the suite reaches it, because every radix test
    // runs at the default `maxE`.
    ctx.external = false;
    loop {
        if n % 2 != 0 {
            r = mul(ctx, &r, &x);
            if let Some(d) = r.d.as_mut() {
                if truncate_limbs(d, k as usize) {
                    is_truncated = true;
                }
            }
        }
        n /= 2;
        if n == 0 {
            if let Some(d) = r.d.as_mut() {
                let last = d.len() - 1;
                if is_truncated && d[last] == 0 {
                    d[last] += 1;
                }
            }
            break;
        }
        x = mul(ctx, &x, &x);
        if let Some(d) = x.d.as_mut() {
            truncate_limbs(d, k as usize);
        }
    }
    ctx.external = true;

    r
}

/// `x₀ + x₁ + …`, rounded **once**.
///
/// # Why this is not a fold of `plus`
///
/// It very nearly is — but the clamps are suppressed for the whole
/// accumulation and `finalise` is applied only at the end, so the sum is
/// carried at full working width and rounded a single time. A plain fold would
/// round at every step and accumulate the error of every one of them; that is
/// the difference between a sum and a `reduce`, and it is why the original
/// gives this its own function instead of leaving it to the caller.
///
/// # The early exit
///
/// The loop stops at the first NaN, because nothing can un-poison a sum, and
/// because the original's `for (; x.s && ++i < args.length;)` tests the
/// accumulator's sign *before* advancing. Note the consequence: an argument
/// after the first NaN is never even constructed, so `Decimal.sum(NaN, {})`
/// returns NaN where `Decimal.sum({}, NaN)` throws. Reproduced, because a
/// conversion that never happens cannot fail.
///
/// An empty list is not the identity; the original constructs `new this(args[0])`
/// unconditionally and so raises on `undefined`. Callers therefore reject the
/// empty case before reaching here, and this returns NaN if they do not.
pub fn sum(ctx: &mut Ctx, values: &[Decimal]) -> Decimal {
    let Some((first, rest)) = values.split_first() else {
        return Decimal::nan();
    };

    let mut x = ctx.without_clamping(|ctx| {
        let mut x = first.clone();
        for y in rest {
            if x.is_nan() {
                break;
            }
            x = add(ctx, &x, y);
        }
        x
    });

    let (pr, rm) = (ctx.cfg.precision, ctx.cfg.rounding);
    finalise(ctx, &mut x, Some(pr), rm, false);
    x
}

/// Format helper used by the tests below and by the CLI: the value as
/// `toString` would render it under `ctx`.
pub fn render(ctx: &Ctx, x: &Decimal) -> String {
    format::to_string(x, &ctx.cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_decimal;

    fn d(text: &str) -> Decimal {
        let ctx = Ctx::default();
        if let Some(rest) = text.strip_prefix('-') {
            parse_decimal(&ctx, Sign::Neg, rest)
        } else {
            parse_decimal(&ctx, Sign::Pos, text)
        }
    }

    fn show(x: &Decimal) -> String {
        format::to_string(x, &crate::Config::default())
    }

    // -- comparison ------------------------------------------------------

    #[test]
    fn comparison_orders_finite_values() {
        assert_eq!(compare(&d("1"), &d("2")), Some(Ordering::Less));
        assert_eq!(compare(&d("2"), &d("1")), Some(Ordering::Greater));
        assert_eq!(compare(&d("1"), &d("1")), Some(Ordering::Equal));
        assert_eq!(compare(&d("-2"), &d("-1")), Some(Ordering::Less));
        assert_eq!(compare(&d("-1"), &d("1")), Some(Ordering::Less));
        assert_eq!(compare(&d("1.5"), &d("1.25")), Some(Ordering::Greater));
        assert_eq!(compare(&d("100"), &d("99.9")), Some(Ordering::Greater));
    }

    #[test]
    fn nan_is_unordered_against_everything_including_itself() {
        assert_eq!(compare(&Decimal::nan(), &d("1")), None);
        assert_eq!(compare(&d("1"), &Decimal::nan()), None);
        assert_eq!(compare(&Decimal::nan(), &Decimal::nan()), None);
    }

    #[test]
    fn infinities_compare_by_sign_and_magnitude() {
        let pos = Decimal::infinity(Sign::Pos);
        let neg = Decimal::infinity(Sign::Neg);
        assert_eq!(compare(&pos, &neg), Some(Ordering::Greater));
        assert_eq!(compare(&pos, &pos), Some(Ordering::Equal));
        assert_eq!(compare(&neg, &neg), Some(Ordering::Equal));
        assert_eq!(compare(&pos, &d("1e300")), Some(Ordering::Greater));
        assert_eq!(compare(&d("1e300"), &pos), Some(Ordering::Less));
        assert_eq!(compare(&neg, &d("-1e300")), Some(Ordering::Less));
    }

    #[test]
    fn the_two_zeros_are_equal() {
        // -0 == 0 compares equal even though the values are distinguishable.
        let neg_zero = Decimal::zero(Sign::Neg);
        let pos_zero = Decimal::zero(Sign::Pos);
        assert_eq!(compare(&neg_zero, &pos_zero), Some(Ordering::Equal));
        assert_eq!(compare(&pos_zero, &d("1")), Some(Ordering::Less));
        assert_eq!(compare(&neg_zero, &d("-1")), Some(Ordering::Greater));
    }

    // -- addition and subtraction ----------------------------------------

    #[test]
    fn addition_of_small_values() {
        let mut ctx = Ctx::default();
        assert_eq!(show(&add(&mut ctx, &d("1"), &d("2"))), "3");
        assert_eq!(show(&add(&mut ctx, &d("0.1"), &d("0.2"))), "0.3");
        assert_eq!(show(&add(&mut ctx, &d("-1"), &d("1"))), "0");
        // 1e20 + 1 needs 21 significant digits, so at the default precision of
        // 20 the one is rounded away entirely.
        assert_eq!(
            show(&add(&mut ctx, &d("1e20"), &d("1"))),
            "100000000000000000000"
        );
    }

    #[test]
    fn the_classic_floating_point_embarrassment_is_exact_here() {
        // 0.1 + 0.2 is 0.3 exactly. That is the entire reason this library
        // exists, so it had better hold.
        let mut ctx = Ctx::default();
        assert_eq!(show(&add(&mut ctx, &d("0.1"), &d("0.2"))), "0.3");
    }

    #[test]
    fn addition_carries_across_limb_boundaries() {
        let mut ctx = Ctx::default();
        assert_eq!(
            show(&add(&mut ctx, &d("9999999"), &d("1"))),
            "10000000",
            "a carry out of a full limb"
        );
        assert_eq!(
            show(&add(&mut ctx, &d("9999999999999999"), &d("1"))),
            "10000000000000000"
        );
    }

    #[test]
    fn subtraction_borrows_across_limb_boundaries() {
        let mut ctx = Ctx::default();
        assert_eq!(show(&sub(&mut ctx, &d("10000000"), &d("1"))), "9999999");
        assert_eq!(
            show(&sub(&mut ctx, &d("10000000000000000"), &d("1"))),
            "9999999999999999"
        );
        assert_eq!(show(&sub(&mut ctx, &d("1"), &d("0.9"))), "0.1");
    }

    #[test]
    fn subtraction_picks_the_sign_of_the_larger_operand() {
        let mut ctx = Ctx::default();
        assert_eq!(show(&sub(&mut ctx, &d("1"), &d("2"))), "-1");
        assert_eq!(show(&sub(&mut ctx, &d("2"), &d("1"))), "1");
        assert_eq!(show(&sub(&mut ctx, &d("-1"), &d("-2"))), "1");
    }

    #[test]
    fn subtracting_equal_values_gives_a_signed_zero() {
        let mut ctx = Ctx::default();
        let zero = sub(&mut ctx, &d("1"), &d("1"));
        assert!(zero.is_zero() && !zero.is_negative());

        // IEEE 754 6.3: the difference of equals is -0 when rounding towards
        // -Infinity, and +0 in every other mode.
        ctx.cfg.rounding = crate::config::rounding::FLOOR;
        let zero = sub(&mut ctx, &d("1"), &d("1"));
        assert!(zero.is_zero() && zero.is_negative(), "FLOOR gives -0");
    }

    #[test]
    fn non_finite_addition_and_subtraction() {
        let mut ctx = Ctx::default();
        let inf = Decimal::infinity(Sign::Pos);
        let ninf = Decimal::infinity(Sign::Neg);

        assert!(add(&mut ctx, &inf, &d("1")).is_infinite());
        assert!(add(&mut ctx, &inf, &inf).is_infinite());
        assert!(add(&mut ctx, &inf, &ninf).is_nan(), "inf + -inf is NaN");
        assert!(sub(&mut ctx, &inf, &inf).is_nan(), "inf - inf is NaN");
        assert!(sub(&mut ctx, &inf, &ninf).is_infinite());
        assert!(add(&mut ctx, &Decimal::nan(), &d("1")).is_nan());

        // A finite minus an infinity is the negated infinity.
        let r = sub(&mut ctx, &d("1"), &inf);
        assert!(r.is_infinite() && r.is_negative());
    }

    #[test]
    fn addition_respects_the_working_precision() {
        let mut ctx = Ctx::default(); // precision 20
                                      // The exact sum needs 21 significant digits, so the one is rounded
                                      // away and the result is indistinguishable from the larger operand.
        assert_eq!(
            show(&add(&mut ctx, &d("1e20"), &d("1"))),
            "100000000000000000000"
        );
        // One digit of room, and it survives.
        ctx.cfg.precision = 21;
        assert_eq!(
            show(&add(&mut ctx, &d("1e20"), &d("1"))),
            "100000000000000000001"
        );
        ctx.cfg.precision = 5;
        assert_eq!(show(&add(&mut ctx, &d("100000"), &d("1"))), "100000");
    }

    // -- multiplication --------------------------------------------------

    #[test]
    fn multiplication_of_small_values() {
        let mut ctx = Ctx::default();
        assert_eq!(show(&mul(&mut ctx, &d("2"), &d("3"))), "6");
        assert_eq!(show(&mul(&mut ctx, &d("1.5"), &d("2"))), "3");
        assert_eq!(show(&mul(&mut ctx, &d("0.1"), &d("0.2"))), "0.02");
        assert_eq!(show(&mul(&mut ctx, &d("-2"), &d("3"))), "-6");
        assert_eq!(show(&mul(&mut ctx, &d("-2"), &d("-3"))), "6");
    }

    #[test]
    fn multiplication_spans_many_limbs() {
        let mut ctx = Ctx::default();
        ctx.cfg.precision = 40;
        // Exponential notation, because the exponent 39 is past the default
        // `toExpPos` of 21 — the same string upstream produces.
        assert_eq!(
            show(&mul(
                &mut ctx,
                &d("12345678901234567890"),
                &d("98765432109876543210")
            )),
            "1.2193263113702179522374638011112635269e+39"
        );
    }

    #[test]
    fn multiplication_by_zero_and_infinity() {
        let mut ctx = Ctx::default();
        let inf = Decimal::infinity(Sign::Pos);
        let zero = Decimal::zero(Sign::Pos);

        assert!(mul(&mut ctx, &zero, &inf).is_nan(), "0 x inf is NaN");
        assert!(mul(&mut ctx, &inf, &zero).is_nan());
        assert!(mul(&mut ctx, &inf, &d("2")).is_infinite());
        assert!(mul(&mut ctx, &d("2"), &zero).is_zero());

        // The sign of a zero product follows the signs of the operands.
        let r = mul(&mut ctx, &d("-2"), &zero);
        assert!(r.is_zero() && r.is_negative(), "-2 x 0 is -0");
    }

    // -- division --------------------------------------------------------

    fn div(ctx: &mut Ctx, a: &str, b: &str) -> Decimal {
        let rm = ctx.cfg.rounding;
        divide(ctx, &d(a), &d(b), None, rm, false, None)
    }

    #[test]
    fn division_of_small_values() {
        let mut ctx = Ctx::default();
        assert_eq!(show(&div(&mut ctx, "6", "3")), "2");
        assert_eq!(show(&div(&mut ctx, "1", "2")), "0.5");
        assert_eq!(show(&div(&mut ctx, "1", "4")), "0.25");
        assert_eq!(show(&div(&mut ctx, "-6", "3")), "-2");
        assert_eq!(show(&div(&mut ctx, "6", "-3")), "-2");
    }

    #[test]
    fn division_rounds_a_repeating_expansion_to_the_precision() {
        let mut ctx = Ctx::default(); // precision 20, ROUND_HALF_UP
        assert_eq!(show(&div(&mut ctx, "1", "3")), "0.33333333333333333333");
        assert_eq!(show(&div(&mut ctx, "2", "3")), "0.66666666666666666667");
        assert_eq!(show(&div(&mut ctx, "1", "7")), "0.14285714285714285714");
    }

    #[test]
    fn division_by_a_multi_limb_divisor() {
        let mut ctx = Ctx::default();
        assert_eq!(
            show(&div(&mut ctx, "1", "12345678901234567890")),
            "8.1000000729000006635e-20"
        );
        assert_eq!(
            show(&div(
                &mut ctx,
                "1219326311370217952237463801111263526900",
                "12345678901234567890"
            )),
            "98765432109876543210",
            "the multi-limb quotient comes out exact"
        );
    }

    #[test]
    fn division_by_zero_and_of_zero() {
        let mut ctx = Ctx::default();
        assert!(div(&mut ctx, "1", "0").is_infinite());
        assert!(div(&mut ctx, "-1", "0").is_negative());
        assert!(div(&mut ctx, "0", "0").is_nan());
        assert!(div(&mut ctx, "0", "1").is_zero());

        let inf = Decimal::infinity(Sign::Pos);
        let rm = ctx.cfg.rounding;
        assert!(divide(&mut ctx, &inf, &inf, None, rm, false, None).is_nan());
        assert!(divide(&mut ctx, &d("1"), &inf, None, rm, false, None).is_zero());
        assert!(divide(&mut ctx, &inf, &d("1"), None, rm, false, None).is_infinite());
    }

    #[test]
    fn division_reverses_multiplication() {
        let mut ctx = Ctx::default();
        ctx.cfg.precision = 40;
        for (a, b) in [("1", "3"), ("22", "7"), ("355", "113"), ("1e30", "7")] {
            let q = div(&mut ctx, a, b);
            let back = mul(&mut ctx, &q, &d(b));
            // Not exactly `a`, since the quotient was rounded — but it must
            // agree to nearly the full working precision.
            let difference = sub(&mut ctx, &back, &d(a));
            let rm = ctx.cfg.rounding;
            let relative = divide(&mut ctx, &difference, &d(a), None, rm, false, None);
            assert!(
                relative.is_zero() || relative.e < -35,
                "{a}/{b} round-trips to within the working precision, got {}",
                show(&relative)
            );
        }
    }

    // -- sum -------------------------------------------------------------

    /// The reason `sum` exists as its own function rather than as a fold.
    ///
    /// `1e20 + 1` rounds back to `1e20` at precision 20, so folding `plus`
    /// over these three loses the `1` at the first step and answers zero. `sum`
    /// keeps the accumulation unrounded and answers one. Both are shown here,
    /// because the test is the contrast and not either value alone.
    #[test]
    fn a_sum_rounds_once_where_a_fold_rounds_every_step() {
        let mut ctx = Ctx::default();
        let terms = [d("1e20"), d("1"), d("-1e20")];

        assert_eq!(show(&sum(&mut ctx, &terms)), "1");

        let mut folded = terms[0].clone();
        for term in &terms[1..] {
            folded = add(&mut ctx, &folded, term);
        }
        assert_eq!(show(&folded), "0");
    }

    #[test]
    fn a_sum_stops_at_the_first_nan() {
        let mut ctx = Ctx::default();
        assert!(sum(&mut ctx, &[d("1"), Decimal::nan(), d("2")]).is_nan());
        assert!(sum(&mut ctx, &[]).is_nan(), "no terms is not the identity");
        assert_eq!(show(&sum(&mut ctx, &[d("100"), d("200"), d("300")])), "600");
    }
}
