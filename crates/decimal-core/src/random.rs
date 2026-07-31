//! Pseudo-random values in `[0, 1)`.
//!
//! # Where the randomness comes from
//!
//! Nowhere in this crate — by construction. [`random`] takes its entropy as an
//! argument, through the [`Entropy`] trait, and has no ambient source of its
//! own.
//!
//! That is not fastidiousness. The original reaches for `Math.random()`, or for
//! `crypto` when the constructor is configured for it, and *which* it reaches
//! for is observable: `Decimal.config({crypto: true})` throws where no
//! cryptographic source exists. A port that hard-wired one source could not
//! reproduce that. Injecting the source also makes the routine testable in the
//! only way that matters here — against a known sequence, so that the digit
//! assembly below can be checked against an expected answer rather than against
//! a distributional hand-wave.
//!
//! [`Xoshiro256StarStar`] is supplied as the default, standing in for
//! `Math.random()`.
//!
//! # What is actually assembled
//!
//! `⌈sd/7⌉` limbs are drawn, each uniform on `[0, 10⁷)`, and laid down as the
//! digits of a value with exponent −1 — that is, `0.d₀d₁…`. Three corrections
//! then follow, in this order, and the order is not free:
//!
//! 1. **Trim the last limb** to the requested number of digits. `sd % 7` digits
//!    are wanted from it, so the rest are zeroed by an integer division and
//!    multiplication rather than by truncating the array — the limb has to keep
//!    its width or the digits after it would shift.
//! 2. **Drop trailing zero limbs**, restoring the invariant that a digit array
//!    has no trailing zero limb. This includes the limb just zeroed in step 1,
//!    and if every limb was zero, the value is zero and the exponent must be
//!    reset to 0 rather than left at −1.
//! 3. **Drop leading zero limbs**, decreasing the exponent by seven for each,
//!    and then by however many leading zeros remain inside the new first limb.
//!    A limb drawn as `42` contributes the digits `0000042`, so the value's
//!    exponent is five lower than the limb boundary suggests.
//!
//! Step 3 is why `random()` can return a value much smaller than `10⁻¹`: about
//! one draw in ten million produces a first limb of zero and an answer below
//! `10⁻⁷`.

use crate::error::check_int32;
use crate::{Ctx, Decimal, Result, Sign, LOG_BASE, MAX_DIGITS};

/// A source of uniform limb values.
///
/// One call yields one limb: an integer in `[0, 10⁷)`. Implementors owe
/// uniformity over that range; everything above assumes it and nothing checks
/// it.
pub trait Entropy {
    /// The next limb, uniform on `[0, 10⁷)`.
    fn next_limb(&mut self) -> u32;
}

/// xoshiro256\*\*, by Blackman and Vigna: the default stand-in for
/// `Math.random()`.
///
/// Chosen because it is four words of state, passes BigCrush, and can be
/// written out in a dozen lines — this crate has no dependencies, so a
/// generator that cannot be read in one sitting could not be justified. It is
/// emphatically **not** cryptographic, which is exactly the standing of
/// `Math.random()` that it replaces.
#[derive(Debug, Clone)]
pub struct Xoshiro256StarStar {
    s: [u64; 4],
}

impl Xoshiro256StarStar {
    /// A generator from a 64-bit seed, expanded through SplitMix64.
    ///
    /// The expansion is not decoration: seeding the four words with the same
    /// value, or with a small integer and three zeros, gives a state so close
    /// to all-zeros that the first several outputs are visibly poor. SplitMix64
    /// is the author's own prescribed remedy.
    pub fn seeded(seed: u64) -> Self {
        let mut z = seed;
        let mut next = || {
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            x ^ (x >> 31)
        };
        Xoshiro256StarStar {
            s: [next(), next(), next(), next()],
        }
    }

    /// A generator seeded from the clock and from the address of a local.
    ///
    /// Neither ingredient is strong, and together they are still not strong;
    /// this matches `Math.random()`, which promises nothing about
    /// unpredictability either. The address contributes whatever the platform's
    /// layout randomisation provides, so that two processes started in the same
    /// clock tick do not agree.
    pub fn from_environment() -> Self {
        let local = 0u8;
        let address = core::ptr::addr_of!(local) as u64;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Xoshiro256StarStar::seeded(nanos ^ address.rotate_left(32))
    }

    /// The next 64 bits of the stream.
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;

        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);

        result
    }
}

impl Default for Xoshiro256StarStar {
    fn default() -> Self {
        Xoshiro256StarStar::from_environment()
    }
}

impl Entropy for Xoshiro256StarStar {
    /// `Math.random() * 1e7 | 0`, spelled out.
    ///
    /// V8's `Math.random()` returns `bits / 2⁵³` for a 53-bit draw, so taking
    /// the top 53 bits of the stream and scaling reproduces both the value
    /// range and the truncation — including the fact that the product is formed
    /// in a double and then truncated towards zero, not rounded.
    fn next_limb(&mut self) -> u32 {
        let bits = self.next_u64() >> 11;
        let unit = bits as f64 / 9_007_199_254_740_992.0;
        (unit * 1e7) as u32
    }
}

/// A pseudo-random value in `[0, 1)` with at most `sd` significant digits.
///
/// `sd` defaults to the configured precision. It must be an integer in
/// `1 ..= MAX_DIGITS`; anything else is an error, with the original's message.
///
/// The result can have *fewer* than `sd` significant digits, and often does —
/// trailing zero limbs are dropped, and a leading zero limb costs seven digits
/// outright. The original's own test asserts `r.sd() <= sd`, not equality.
pub fn random(ctx: &Ctx, sd: Option<f64>, source: &mut impl Entropy) -> Result<Decimal> {
    let sd = match sd {
        None => ctx.cfg.precision,
        Some(value) => check_int32(value, 1, MAX_DIGITS)?,
    };

    let limbs = (sd + LOG_BASE - 1) / LOG_BASE;
    let mut d: Vec<u32> = (0..limbs).map(|_| source.next_limb()).collect();

    // 1. Trim the final limb to the `sd % 7` digits actually asked for. Zeroing
    //    the tail in place keeps the limb seven digits wide, which is what
    //    holds the earlier digits in their columns.
    let wanted = sd % LOG_BASE;
    let mut i = d.len() - 1;
    if d[i] != 0 && wanted != 0 {
        let scale = crate::pow10(LOG_BASE - wanted);
        d[i] = (d[i] / scale) * scale;
    }

    // 2. Drop trailing zero limbs. `i` walks down past them; reaching −1 means
    //    every limb was zero, and the value is zero.
    while d.get(i).copied() == Some(0) {
        d.pop();
        if i == 0 {
            break;
        }
        i -= 1;
    }

    if d.is_empty() {
        return Ok(Decimal::zero(Sign::Pos));
    }

    // 3. Drop leading zero limbs, seven digits of exponent apiece, then account
    //    for the leading zeros inside the limb that survives.
    let mut e: i64 = -1;
    while d[0] == 0 {
        d.remove(0);
        e -= LOG_BASE;
    }
    let width = crate::digit_count(d[0]);
    if width < LOG_BASE {
        e -= LOG_BASE - width;
    }

    Ok(Decimal::finite(Sign::Pos, e, d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::to_string;

    /// A source that hands out a fixed script of limbs, so that the digit
    /// assembly can be checked against an expected value rather than against a
    /// statistical property.
    struct Scripted {
        limbs: Vec<u32>,
        next: usize,
    }

    impl Scripted {
        fn new(limbs: &[u32]) -> Self {
            Scripted { limbs: limbs.to_vec(), next: 0 }
        }
    }

    impl Entropy for Scripted {
        fn next_limb(&mut self) -> u32 {
            let value = self.limbs[self.next.min(self.limbs.len() - 1)];
            self.next += 1;
            value
        }
    }

    fn run(sd: Option<f64>, limbs: &[u32]) -> String {
        let mut ctx = Ctx::default();
        ctx.cfg.to_exp_neg = -9_000_000_000_000_000;
        let value = random(&ctx, sd, &mut Scripted::new(limbs)).expect("a valid sd");
        to_string(&value, &ctx.cfg)
    }

    #[test]
    fn limbs_become_the_digits_after_the_point() {
        // Precision 20 asks for three limbs, of which the third contributes
        // 20 % 7 = 6 digits; its seventh is zeroed.
        assert_eq!(
            run(None, &[1_234_567, 7_654_321, 1_111_111]),
            "0.12345677654321111111"
        );
    }

    /// The trimming keeps the limb's width. Zeroing `1_111_111` to six digits
    /// gives `1_111_110`, not `111_111` — which would shift every later digit
    /// one place left.
    #[test]
    fn trimming_the_last_limb_does_not_shift_the_earlier_digits() {
        let mut ctx = Ctx::default();
        ctx.cfg.precision = 9;
        let value = random(&ctx, None, &mut Scripted::new(&[1_234_567, 8_900_000]))
            .expect("a valid sd");
        assert_eq!(to_string(&value, &ctx.cfg), "0.123456789");
    }

    /// A leading limb of zero costs seven digits of exponent outright, and the
    /// leading zeros inside the first surviving limb cost the rest: `[0, 42, …]`
    /// starts twelve places down, seven for the dropped limb and five for the
    /// width of `42`.
    #[test]
    fn leading_zeros_push_the_exponent_down() {
        assert_eq!(run(Some(20.0), &[0, 42, 5_000_000]), "0.000000000000425");
        assert_eq!(run(Some(7.0), &[42]), "0.0000042");
    }

    /// A zero limb in the *middle* is a run of seven zero digits and stays
    /// where it is. Only the ends are elastic — which is why the two trimming
    /// loops walk inwards from the ends rather than filtering the array.
    #[test]
    fn an_interior_zero_limb_is_seven_zero_digits() {
        assert_eq!(
            run(Some(20.0), &[1_111_111, 0, 2_222_222]),
            "0.11111110000000222222"
        );
    }

    #[test]
    fn an_all_zero_draw_is_zero() {
        let value = random(&Ctx::default(), Some(20.0), &mut Scripted::new(&[0]))
            .expect("a valid sd");
        assert!(value.is_zero() && !value.is_negative());
    }

    #[test]
    fn the_significant_digit_count_is_validated_the_way_the_original_validates_it() {
        let ctx = Ctx::default();
        for bad in [0.0, -1.0, 7.5, f64::NAN, f64::INFINITY] {
            assert!(
                random(&ctx, Some(bad), &mut Scripted::new(&[1])).is_err(),
                "{bad} is not a usable significant-digit count"
            );
        }
        assert!(random(&ctx, Some(1.0), &mut Scripted::new(&[1])).is_ok());
    }

    /// Every draw satisfies the four properties the original's test asserts:
    /// at most `sd` significant digits, and a value in `[0, 1)`.
    #[test]
    fn every_draw_lands_in_the_unit_interval_with_no_more_digits_than_asked() {
        let ctx = Ctx::default();
        let mut source = Xoshiro256StarStar::seeded(0x0DDB_A11D_EAD_BEEF);

        for sd in 1..=100 {
            let value = random(&ctx, Some(f64::from(sd)), &mut source).expect("a valid sd");
            assert!(
                value.significant_digits() <= i64::from(sd),
                "{sd} digits requested, {} produced",
                value.significant_digits()
            );
            assert!(!value.is_negative(), "never negative");
            // A value below 1 has a base-10 exponent below 0.
            assert!(value.is_zero() || value.e < 0, "strictly below one");
        }
    }

    /// The generator is a generator: reproducible from a seed, and not stuck.
    #[test]
    fn the_default_source_is_reproducible_and_not_degenerate() {
        let mut a = Xoshiro256StarStar::seeded(1);
        let mut b = Xoshiro256StarStar::seeded(1);
        let first: Vec<u32> = (0..64).map(|_| a.next_limb()).collect();
        let second: Vec<u32> = (0..64).map(|_| b.next_limb()).collect();
        assert_eq!(first, second, "the same seed gives the same stream");

        assert!(first.iter().all(|&limb| limb < 10_000_000), "in range");
        assert!(
            first.iter().collect::<std::collections::HashSet<_>>().len() > 60,
            "not stuck on a short cycle"
        );

        // A seed of 1 would leave three of xoshiro's four words zero without
        // the SplitMix64 expansion, and the first outputs would be tiny.
        assert!(
            first[..4].iter().any(|&limb| limb > 1_000_000),
            "the seed is properly diffused"
        );
    }
}
