//! `decimal-core` — arbitrary-precision decimal arithmetic.
//!
//! A faithful Rust port of [decimal.js](https://github.com/MikeMcl/decimal.js)
//! v10.6.0 (`cd73a7f`). The goal is *behavioural equivalence*, not improvement:
//! where the original loses precision, this port loses the same precision, so
//! that the original test suite passes byte-for-byte unmodified.
//!
//! This crate has no dependencies and contains no `unsafe` code — the
//! `unsafe_code = "forbid"` lint in `Cargo.toml` is enforced by the compiler.

/// Digits are stored in base 10^7, most-significant limb first.
pub const BASE: u32 = 10_000_000;

/// Decimal digits carried by one limb.
pub const LOG_BASE: u32 = 7;

/// Upper bound on `precision`.
pub const MAX_DIGITS: u32 = 1_000_000_000;

/// Bound on `|exponent|`, and on `toExpNeg` / `toExpPos` / `minE` / `maxE`.
pub const EXP_LIMIT: i64 = 9_000_000_000_000_000;

/// `Number.MAX_SAFE_INTEGER`.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Placeholder — replaced by the real representation in the next commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decimal {
    /// Sign: `1` or `-1`. Carried independently of `d`, so `-0` is
    /// representable exactly as the original represents it.
    pub s: i8,
    /// Base-10 exponent. `i64` because the original allows `|e| <= 9e15`,
    /// which does not fit in `i32`.
    pub e: i64,
    /// Digit limbs, base 10^7, most-significant first. `None` mirrors the
    /// original's `d === null`, i.e. the value is Infinity or NaN.
    pub d: Option<Vec<u32>>,
}
