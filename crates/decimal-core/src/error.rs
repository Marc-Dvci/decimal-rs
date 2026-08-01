//! The errors this library raises, and the exact text it raises them with.
//!
//! The messages are part of the observable behaviour, not diagnostics. The
//! original's test helper accepts any thrown `Error` whose message matches
//! `/DecimalError/`, and the constructor interpolates the offending value into
//! the message — so `new Decimal('foo')` reports
//! `[DecimalError] Invalid argument: foo`. Reproducing the prefix and the
//! interpolation is therefore part of the port.

use core::fmt;

/// The prefix on every message.
pub const DECIMAL_ERROR: &str = "[DecimalError] ";

/// A failure that the original signals by throwing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A value or configuration setting outside its permitted range, or of a
    /// type the constructor does not accept. Carries the offending value,
    /// already converted to text the way JavaScript would convert it.
    InvalidArgument(String),
    /// More digits were requested of `PI` or `LN10` than the built-in
    /// constants hold.
    PrecisionLimitExceeded,
    /// `crypto: true` was configured but no cryptographic source is available.
    CryptoUnavailable,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(DECIMAL_ERROR)?;
        match self {
            Error::InvalidArgument(value) => write!(f, "Invalid argument: {value}"),
            Error::PrecisionLimitExceeded => f.write_str("Precision limit exceeded"),
            Error::CryptoUnavailable => f.write_str("crypto unavailable"),
        }
    }
}

impl std::error::Error for Error {}

/// The result of any operation that the original can throw from.
pub type Result<T> = core::result::Result<T, Error>;

/// The original's `checkInt32(i, min, max)`: reject anything that is not an
/// integer within the given inclusive range.
///
/// The original's test is `i !== ~~i`, which is a 32-bit truncation, so it
/// also rejects values outside `i32` — including ones inside `[min, max]` when
/// the range itself is wider, as it is for `MAX_DIGITS`. That is reproduced
/// rather than corrected.
pub fn check_int32(value: f64, min: i64, max: i64) -> Result<i64> {
    let truncated = value as i32;
    if value != f64::from(truncated) || (value as i64) < min || (value as i64) > max {
        return Err(Error::InvalidArgument(crate::format::number_to_string(
            value,
        )));
    }
    Ok(value as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_match_the_originals_text_exactly() {
        assert_eq!(
            Error::InvalidArgument("foo".into()).to_string(),
            "[DecimalError] Invalid argument: foo"
        );
        assert_eq!(
            Error::PrecisionLimitExceeded.to_string(),
            "[DecimalError] Precision limit exceeded"
        );
        assert_eq!(
            Error::CryptoUnavailable.to_string(),
            "[DecimalError] crypto unavailable"
        );
    }

    #[test]
    fn check_int32_accepts_integers_in_range() {
        assert_eq!(check_int32(5.0, 0, 8), Ok(5));
        assert_eq!(check_int32(0.0, 0, 8), Ok(0));
        assert_eq!(check_int32(8.0, 0, 8), Ok(8));
    }

    #[test]
    fn check_int32_rejects_non_integers_and_out_of_range() {
        assert!(check_int32(1.5, 0, 8).is_err());
        assert!(check_int32(-1.0, 0, 8).is_err());
        assert!(check_int32(9.0, 0, 8).is_err());
        assert!(check_int32(f64::NAN, 0, 8).is_err());
        assert!(check_int32(f64::INFINITY, 0, 8).is_err());
    }

    #[test]
    fn the_offending_value_is_interpolated_as_javascript_would() {
        // Not "1.5" via Rust's Display, but String(1.5) -- which agrees here,
        // and would not for 1e21.
        assert_eq!(
            check_int32(1e21, 0, crate::MAX_DIGITS).unwrap_err(),
            Error::InvalidArgument("1e+21".into())
        );
    }
}
