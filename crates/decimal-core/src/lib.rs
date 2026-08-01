//! # decimal-core
//!
//! An arbitrary-precision decimal type: a Rust port of
//! [decimal.js](https://github.com/MikeMcl/decimal.js) v10.6.0, upstream
//! commit `cd73a7f`.
//!
//! ## What this crate is trying to be
//!
//! It is trying to be *the same program*. Not a better one.
//!
//! That distinction governs every line here. decimal.js does not promise
//! correctly-rounded transcendental functions; it promises its own particular
//! results, produced by its own particular series expansions with its own
//! particular guard digits. A `ln` that is more accurate than the original's
//! `ln` is, for this crate's purposes, a bug — it would fail the original test
//! suite, which is the thing being preserved. So where the original loses
//! digits to cancellation, this port loses the same digits, in the same place,
//! by the same arithmetic. Where the original's behaviour looked like a defect
//! worth reporting, it was reported upstream rather than quietly corrected
//! here (see `DECISIONS.md`).
//!
//! ## Representation
//!
//! A value is a sign, a base-10 exponent, and a digit array:
//!
//! ```text
//!     value  =  s  ×  0.d₀d₁d₂…  ×  10^(e+1)
//! ```
//!
//! where the digit array `d` is stored in base 10⁷, most-significant limb
//! first. Base 10⁷ is chosen — as in the original — because the product of two
//! limbs stays below 2⁵³, so the original can multiply limbs in IEEE doubles
//! without loss. This port has 64-bit integers available and does not need the
//! restriction, but keeping the same base keeps the same digit boundaries, and
//! digit boundaries are observable: rounding to a precision, the placement of
//! guard digits, and the exact point at which a series expansion is truncated
//! all depend on them. A "better" base would produce different answers.
//!
//! ## The three shapes a value can take
//!
//! The original encodes finite, infinite, and NaN values in two nullable
//! fields: `d === null` marks a non-finite value, and `s` being `NaN` rather
//! than `±1` distinguishes NaN from ±Infinity. That is three states carried by
//! two fields, with the pairing left to the programmer to remember.
//!
//! Here the same three states are carried by [`Sign`] and [`Decimal::d`], with
//! the pairing stated as an invariant the constructors maintain:
//!
//! > **Invariant.** `s == Sign::Nan` if and only if the value is NaN, and NaN
//! > implies `d == None`. A value with `d == None` and `s != Sign::Nan` is
//! > ±Infinity. A value with `d == Some(_)` is finite, and its digit array is
//! > non-empty, has no leading zero limb unless the value is zero, and has no
//! > trailing zero limb.
//!
//! The invariant is not decoration: `finalise` restores it as its last act,
//! and every arithmetic routine assumes it on entry.
//!
//! ## Safety
//!
//! There is no `unsafe` in this crate, and there cannot be: `Cargo.toml`
//! carries `unsafe_code = "forbid"`, which the compiler enforces. There are
//! also no dependencies. The limb arithmetic is written here rather than
//! delegated to a bignum crate, because delegating it would have made this a
//! wrapper rather than a port, and because no existing crate reproduces
//! decimal.js's rounding.

#![deny(missing_docs)]

pub mod arith;
pub mod config;
pub mod constants;
pub mod decimal;
pub mod elementary;
pub mod error;
pub(crate) mod exact;
pub mod format;
pub mod fraction;
pub mod inverse;
pub mod ops;
pub mod parse;
pub mod power;
pub mod radix;
pub mod random;
pub mod roots;
pub mod round;
pub mod trig;

pub use config::{rounding, Config, Ctx};
pub use decimal::{Decimal, Sign};
pub use error::{Error, Result};

/// The base in which digit limbs are stored: each limb holds seven decimal
/// digits, so limb values run from 0 to 9 999 999.
pub const BASE: u32 = 10_000_000;

/// Decimal digits carried by one limb; `10^LOG_BASE == BASE`.
pub const LOG_BASE: i64 = 7;

/// The largest permitted `precision`, and the largest permitted first argument
/// to `toDecimalPlaces`, `toExponential`, `toFixed`, `toPrecision` and
/// `toSignificantDigits`.
pub const MAX_DIGITS: i64 = 1_000_000_000;

/// The bound on `|e|`, and on the magnitude of `toExpNeg`, `toExpPos`, `minE`
/// and `maxE`.
pub const EXP_LIMIT: i64 = 9_000_000_000_000_000;

/// `Number.MAX_SAFE_INTEGER`, i.e. 2⁵³ − 1.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// The most elements a JavaScript array can hold **when it is grown one index
/// at a time**, which is every place the original grows a digit array: 2²⁷.
///
/// This is a limit of the *original's* host rather than of Rust, and it is
/// reproduced on purpose. A `Vec` has no such bound, which sounds like an
/// improvement right up until an operation the original refuses in a catchable
/// way instead asks the allocator for ten petabytes and takes the process down
/// with it. See [`Ctx::array_limit_exceeded`].
///
/// # Why 2²⁷ and not the specification's 2³² − 1
///
/// 2³² − 1 is the largest value an array's `length` may *hold*; it is not the
/// largest array that can be *built*. A 64-bit V8 stores a dense array's
/// elements in a `FixedArray`, whose backing store is capped at one gigabyte of
/// eight-byte slots, so growth by assignment stops at 2²⁷ elements and throws
/// `RangeError: Invalid array length` there — four billion elements below where
/// the specification would. `node scripts/host-limits.js` measures it and
/// checks it against this constant rather than trusting either number.
///
/// The distinction is not academic. `divide`'s quotient loop is bounded only by
/// the working precision, and `Decimal.set({ precision: 1e9 })` — the largest
/// precision the library documents — asks it for 1e9/7 + 2 ≈ 1.43 × 10⁸ limbs.
/// That is above 2²⁷ and far below 2³² − 1, so `new Decimal(1).div(3)` throws
/// upstream; with the specification's constant here the port would have
/// answered, and disagreed with the original on a three-line program in a
/// documented configuration. See DECISIONS.md D-19.
pub const MAX_ARRAY_LENGTH: i64 = 134_217_728;

/// The claim in the paragraph above, checked by the compiler: the largest
/// precision the library accepts really does ask `divide` for more limbs than
/// the host will hold. If a future edit reconciles these two constants, the
/// case that motivated this one has stopped existing and the reasoning around
/// it needs re-reading rather than the assertion needs deleting.
const _: () = assert!(MAX_DIGITS / LOG_BASE + 2 > MAX_ARRAY_LENGTH);

/// ECMAScript's `ToInt32`, i.e. what the original's pervasive `… | 0` does.
///
/// Rust's `as i32` is *saturating* on floats and is not this: `1e16 as i32` is
/// `i32::MAX`, while `1e16 | 0` in JavaScript is 1_874_919_424. The difference
/// is invisible until an intermediate leaves the 32-bit range, at which point
/// the original silently wraps — sometimes to a negative number — and any port
/// that saturated, or that kept the wide value, has stopped computing the same
/// function. See D-19, where the wide value cost a process.
pub fn to_int32(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    // `trunc` and `rem_euclid` are both exact on integral doubles of any
    // magnitude, so the residue below is the specification's `modulo 2³²`
    // rather than an approximation of it.
    let residue = value.trunc().rem_euclid(4_294_967_296.0);
    (residue as u32) as i32
}

/// Powers of ten up to the base, for limb-splitting arithmetic.
///
/// Indexing this table rather than calling a general `pow` matters: the
/// original performs these divisions in floating point, and the results agree
/// with integer arithmetic only because every power involved is exactly
/// representable. Keeping them exact keeps the agreement exact.
pub(crate) const POW10: [u32; 9] = [
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
];

/// The number of decimal digits in `w`, counting zero as having one digit.
///
/// This is the `for (digits = 1, k = xd[0]; k >= 10; k /= 10) digits++;` idiom
/// that appears half a dozen times in the original, given a name.
#[inline]
pub(crate) fn digit_count(w: u32) -> i64 {
    let mut digits = 1;
    let mut k = w;
    while k >= 10 {
        k /= 10;
        digits += 1;
    }
    digits
}

/// `10^k` as a `u32`, for `0 <= k <= 8`.
///
/// Every caller has already established that `k` is in range by construction —
/// the exponents involved are differences of digit positions within a single
/// limb — so a lookup outside the table is a bug in this crate, not bad input.
#[inline]
pub(crate) fn pow10(k: i64) -> u32 {
    POW10[k as usize]
}

/// `w / 10^k`, where `k` may be far larger than any power of ten this table
/// holds.
///
/// The original computes these divisors as `Math.pow(10, k)` in a double. When
/// `k` is enormous — which happens for a value at the extreme of the exponent
/// range, where the significant-digit target passed to `finalise` is around
/// −9 × 10¹⁵ — that is `Infinity`, and `w / Infinity` is `0`. Reproducing that
/// is not a nicety: without it the table lookup is an out-of-bounds index and
/// the process aborts, which is exactly what `ceil` on a value near `minE`
/// did.
#[inline]
pub(crate) fn div_pow10(w: u64, k: i64) -> u64 {
    if k >= POW10.len() as i64 {
        0
    } else {
        w / u64::from(pow10(k))
    }
}

/// `w % 10^k`, with the same caveat: `w % Infinity` is `w`.
#[inline]
pub(crate) fn mod_pow10(w: u32, k: i64) -> u32 {
    if k >= POW10.len() as i64 {
        w
    } else {
        w % pow10(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_count_counts_decimal_digits() {
        assert_eq!(digit_count(0), 1, "zero has one digit, not none");
        assert_eq!(digit_count(9), 1);
        assert_eq!(digit_count(10), 2);
        assert_eq!(digit_count(999_999), 6);
        assert_eq!(digit_count(1_000_000), 7);
        assert_eq!(digit_count(BASE - 1), 7, "the widest a limb can be");
    }

    #[test]
    fn pow10_agrees_with_repeated_multiplication() {
        let mut expected: u64 = 1;
        for k in 0..=8 {
            assert_eq!(u64::from(pow10(k)), expected);
            expected *= 10;
        }
    }

    #[test]
    fn to_int32_wraps_where_rust_would_saturate() {
        // Inside the 32-bit range the two agree, which is why the difference
        // hides for so long.
        assert_eq!(to_int32(0.0), 0);
        assert_eq!(to_int32(-1.5), -1, "truncation is toward zero, not down");
        assert_eq!(to_int32(2_147_483_647.0), i32::MAX);

        // Outside it they do not. `1e16 as i32` is `i32::MAX`; `1e16 | 0` is
        // this. Every value below was read out of Node, not derived here.
        assert_eq!(to_int32(1e16), 1_874_919_424);
        assert_eq!(to_int32(2_147_483_648.0), i32::MIN);
        assert_eq!(to_int32(4_294_967_296.0), 0);
        assert_eq!(to_int32(-4_294_967_297.0), -1);

        // The value that cost a process: the limb target `divide` computes for
        // `sinh` one exponent below the ceiling. See D-19.
        assert_eq!(
            to_int32(8_999_999_999_999_967f64 / 7.0 + 2.0),
            -1_354_212_501
        );

        // The specification maps every non-finite input to zero.
        assert_eq!(to_int32(f64::NAN), 0);
        assert_eq!(to_int32(f64::INFINITY), 0);
        assert_eq!(to_int32(f64::NEG_INFINITY), 0);
    }

    #[test]
    fn the_array_ceiling_is_the_one_the_host_enforces() {
        // Not the specification's 2³² − 1. `scripts/host-limits.js` measures
        // the live value and fails if it has moved; this only pins the constant
        // against a careless edit, and the relation that makes the distinction
        // matter is asserted at the definition itself, at compile time.
        assert_eq!(MAX_ARRAY_LENGTH, 1 << 27);
    }
}
