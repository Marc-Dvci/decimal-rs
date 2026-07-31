# decimal-rs — a Rust port of decimal.js

**Track F (JavaScript → Rust)** · Port Mortem 2026 · solo entry
Upstream: [MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js) @ [`cd73a7f`](https://github.com/MikeMcl/decimal.js/commit/cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f) (v10.6.0, MIT)

| | |
|---|---|
| **Original test suite** | *in progress* — baseline upstream: 22,628 / 22,628 |
| **Test files modified** | **0 of 69** — SHA-256 manifest enforced by the build |
| **JavaScript in this project** | **none** — Node's own resolver loads the Rust binary |
| **Unsafe in `decimal-core`** | **0**, compiler-enforced (`unsafe_code = "forbid"`) |
| **Dependencies of `decimal-core`** | **0** |

## Build and verify — one command

```
docker build -t decimal-rs . && docker run --rm decimal-rs
```

That builds the Rust port from source, verifies that all 69 files of the
original test suite are byte-identical to upstream, and runs that **unmodified**
suite against the compiled Rust artifact.

Without Docker: `make` (requires Rust 1.97.1 — pinned in `rust-toolchain.toml` —
and Node 24).

## How the original tests reach the Rust code

`test/setup.js`, untouched, contains one line:

```js
Decimal = require('../decimal');
```

Node's resolver tries `../decimal.js`, then `../decimal.json`, then
`../decimal.node`. The original `decimal.js` is not present — it is the thing
being replaced — and `decimal.node` is the compiled Rust library.

So the module object the tests receive *is* the addon, and the addon's
`module.exports` *is* the `Decimal` constructor. There is no adapter, no shim,
and no JavaScript file anywhere between the test suite and the Rust code. This
required writing the N-API entry point by hand; the usual derive macros cannot
return a constructor function as the module itself.

## Layout

```
crates/decimal-core   the port: parsing, limb arithmetic, rounding, formatting.
                      Zero dependencies, zero unsafe, no Node concepts.
crates/decimal-napi   the only place that knows Node exists: object protocol,
                      config state, error mapping. All unsafe lives here.
crates/decimal-cli    standalone binary — the port runs with no Node present.
test/                 the original suite, unmodified and hash-pinned.
tests/                ORIGINAL_HASHES.txt and verification artifacts.
```

## Documents

- [DECISIONS.md](DECISIONS.md) — every non-trivial divergence, and why
