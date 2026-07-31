# DECISIONS

Architectural decisions and deliberate divergences in porting
[MikeMcl/decimal.js](https://github.com/MikeMcl/decimal.js) to Rust.

Entries are written **at the moment the decision is made**, in order. Each one
records the context, the options that were actually considered, the decision,
and — the part that matters — the *consequence*.

---

## Baseline

Recorded before any port code existed, so that everything after it is verifiable.

| | |
|---|---|
| Upstream repository | `https://github.com/MikeMcl/decimal.js` |
| Upstream commit | `cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f` |
| Upstream commit date | 2026-07-14 — *Merge pull request #260 from apoorva-01/fix-asin-cancellation-near-1* |
| Upstream version | 10.6.0 |
| Licence | MIT (`LICENCE.md`, retained verbatim in this repo) |
| Implementation size | 4,952 lines, one file (`decimal.js`) |
| Test files | 69 files under `test/`, hash-pinned in [`tests/ORIGINAL_HASHES.txt`](tests/ORIGINAL_HASHES.txt) |
| Baseline test result | `In total, 22628 of 22628 tests passed in 0.595 secs.` |
| Manifest generated | 2026-07-31T18:04:51Z |
| Port code at this commit | **none** |

Baseline captured on the pristine upstream clone:

```
$ git rev-parse HEAD
cd73a7f830f07bc98e906d2ebe76e8c02cc20c8f
$ node test/test.js | tail -1
 In total, 22628 of 22628 tests passed in 0.595 secs.
```

The assertion total drifts slightly between runs (22,628 / 22,658 observed)
because five of the 61 test modules generate their inputs with `Math.random()`.
That is a property of the original suite, not of this port, and it is exploited
deliberately later — see the soak run in the README.

---
