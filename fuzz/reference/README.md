# The oracle

`decimal.js` in this directory is the **unmodified original**, upstream commit
[`cd73a7f`](https://github.com/MikeMcl/decimal.js/commit/cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f)
(v10.6.0, MIT, © Michael Mclaughlin), vendored byte-for-byte.

It is here for one purpose: to be the reference side of the differential fuzz
harness in `../differential.js`, which runs both implementations on the same
inputs in the same process and compares everything observable about the
results.

**It is not part of the port.** Concretely:

- nothing under `crates/` references it, directly or indirectly;
- it is not in the Cargo build graph, and could not be — it is JavaScript;
- the test suite does not reach it. `test/setup.js` loads `../decimal`, which
  Node resolves to `decimal.node`, the compiled Rust library at the repository
  root. This file is two directories away and named differently;
- it is not shipped, packaged, or loaded at runtime by anything the port
  installs.

Deleting this directory breaks `make fuzz` and `make conformance` — everything
that compares the two implementations, and nothing that builds or runs the port.
That is the whole of its coupling to the project, and it is the reason it lives
under `fuzz/` rather than anywhere a build could find it by accident.

Its byte-identity to upstream is checked by `scripts/verify-tests.js` along
with the test suite, so it cannot quietly drift into being a modified oracle —
which would be a much subtler way to fake a passing fuzz run than modifying the
tests.
