//! `number_to_string` against Node's own output.
//!
//! The unit tests beside the implementation check the cases that were reasoned
//! about. This one checks the cases that were not: 5,829 doubles, keyed by
//! their bit patterns, each with the string Node actually printed for it.
//!
//! The corpus is deliberately lopsided towards the places where the notation
//! rules change — every power of ten from 10⁻³³⁰ to 10³³⁰ with its immediate
//! floating-point neighbours, the 10²¹ and 10⁻⁷ switch-over points, denormals,
//! and the edges of exact integer representation — because those are where a
//! plausible-looking implementation diverges, and where the divergence would
//! otherwise surface as unrelated failures scattered across the original
//! suite.
//!
//! Regenerate with:
//!
//! ```text
//! node scripts/dump-number-fixture.js > crates/decimal-core/tests/fixtures/number-to-string.txt
//! ```

use decimal_core::format::number_to_string;

const FIXTURE: &str = include_str!("fixtures/number-to-string.txt");

#[test]
fn matches_node_on_every_value_in_the_corpus() {
    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for line in FIXTURE.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }

        let (bits, expected) = line
            .split_once(' ')
            .unwrap_or_else(|| panic!("malformed fixture line: {line:?}"));
        let value = f64::from_bits(
            u64::from_str_radix(bits, 16).unwrap_or_else(|_| panic!("bad bit pattern: {bits:?}")),
        );

        let actual = number_to_string(value);
        if actual != expected {
            // Report the first few in full; a long list of the same mistake is
            // less useful than the mistake plus a count.
            if failures.len() < 12 {
                failures.push(format!(
                    "  {bits}  expected {expected:?}, got {actual:?}"
                ));
            }
        }
        checked += 1;
    }

    assert!(checked > 5_000, "the fixture should be substantial, saw {checked}");

    if !failures.is_empty() {
        panic!(
            "number_to_string diverged from Node on some of {checked} values:\n{}",
            failures.join("\n")
        );
    }
}

/// The corpus is only as good as its coverage of the interesting region, so
/// check that it actually contains the boundary cases it is supposed to.
#[test]
fn the_corpus_covers_both_notation_thresholds() {
    let strings: Vec<&str> = FIXTURE
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once(' ').map(|(_, s)| s))
        .collect();

    assert!(
        strings.iter().any(|s| s.contains("e+21")),
        "the corpus must reach the upper notation threshold"
    );
    assert!(
        strings.iter().any(|s| *s == "1e-7"),
        "the corpus must reach the lower notation threshold"
    );
    assert!(
        strings.iter().any(|s| s.len() > 18 && !s.contains('e')),
        "the corpus must include long fixed-notation values"
    );
}
