# Where the original test suite is, and why it is not in this directory

**The original suite lives at [`../test/`](../test/), unmodified.** All 69
files, at the paths upstream gives them, byte-identical to
`MikeMcl/decimal.js @ cd73a7f`. This directory holds the two manifests that
prove it, and nothing else.

| file | what it pins |
|---|---|
| [`ORIGINAL_HASHES.txt`](ORIGINAL_HASHES.txt) | SHA-256 of every file under `test/`, taken from the pristine upstream clone at kickoff, before any port code existed |
| [`ORACLE_SHA256.txt`](ORACLE_SHA256.txt) | SHA-256 of `fuzz/reference/decimal.js`, the vendored fuzzing oracle |

```
node scripts/verify-tests.js      # both manifests; the build runs it first
```

## Why `test/` and not `tests/original/`

The suggested layout in the brief puts the untouched originals under
`tests/original/`. This port keeps them where upstream put them, because
moving them would change what they mean.

`test/setup.js` is one line:

```js
Decimal = require('../decimal');
```

That relative path is the whole reason this port needs no adapter. Node
resolves it to `../decimal.node` — the compiled Rust library — so the module
object the tests receive *is* the port, with no shim, no wrapper and no
JavaScript in between. Relocating the suite to `tests/original/` would make
`../decimal` resolve to `tests/decimal.node`, and the honest ways to fix that
are to edit `setup.js`, which is exactly what must not happen, or to place a
second copy of the artifact where the suite would look for it, which is a
shim by another name.

So the originals stay at their upstream paths, the hashes are pinned from the
upstream clone rather than from this repository, and `scripts/verify-tests.js`
fails the build on any mismatch — including a file added or removed. The
negative control is exercised: flip a byte in any file under `test/` and the
build stops.

**Files modified: 0 of 69, and every assertion passes.** The adapter satisfies
the suite's prototype-identity requirement structurally, by defining the
instance methods on one shared plain prototype; the design and the Node-API
constraint that shapes it are D-08 in [`../DECISIONS.md`](../DECISIONS.md).

## Tests this port added

The port's own tests are Rust unit tests, next to the code they test, and run
by `cargo test`. They are deliberately *not* under `test/`, so that everything
in that directory remains upstream's and the hash manifest covers the whole of
it.

```
cargo test --release          # 168 core + 3 cli + 2 fixture
```
