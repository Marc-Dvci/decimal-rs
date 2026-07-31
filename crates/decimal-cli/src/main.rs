//! Standalone CLI over `decimal-core`.
//!
//! Exists so that the port stands on its own as a Rust library: it is
//! demonstrably not a wrapper around anything, and it runs with no Node
//! present. See DECISIONS.md D-01.

fn main() {
    println!("decimal-cli {} (scaffold)", env!("CARGO_PKG_VERSION"));
}
