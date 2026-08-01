# decimal-rs — build and verification.
#
# The documented single command for reproducing everything is the Docker one:
#
#     docker build -t decimal-rs . && docker run --rm decimal-rs
#
# This Makefile is the same pipeline without Docker, for local development.

CARGO ?= cargo
NODE  ?= node

# The compiled cdylib has a different name and extension on every platform;
# all three are copied to ./decimal.node, which is what Node's resolver finds.
UNAME_S := $(shell uname -s 2>/dev/null || echo Windows_NT)
ifeq ($(OS),Windows_NT)
  ARTIFACT := target/release/decimal.dll
else ifeq ($(UNAME_S),Darwin)
  ARTIFACT := target/release/libdecimal.dylib
else
  ARTIFACT := target/release/libdecimal.so
endif

.PHONY: all build addon verify-tests test test-original test-adapter test-rust clean fmt lint \
        unsafe-report bench fuzz fuzz-strict fuzz-limits repro clamp-conformance \
        host-limits conformance soak

# Default target: nothing is considered built until the original, unmodified
# test suite has run against the Rust artifact.
all: verify-tests test

build:
	$(CARGO) build --release

addon: build
	cp $(ARTIFACT) ./decimal.node

## Fails the build if any file under test/ differs from the pinned upstream.
verify-tests:
	@$(NODE) scripts/verify-tests.js

## The original decimal.js suite, unmodified, against the Rust artifact.
##
## Through `expect-zero-failures.js`, because upstream's runner exits 0 whatever
## happens: gating on its exit code would gate on nothing. The wrapper reads the
## summary line and requires every assertion to pass.
test-original: addon verify-tests
	$(NODE) scripts/expect-zero-failures.js

test-adapter: addon
	$(NODE) --expose-gc scripts/adapter-regression.js

## The port's own Rust unit tests (kept out of test/ by design).
test-rust:
	$(CARGO) test --release

test: test-rust test-original test-adapter

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --all-targets -- -D warnings

## Counts unsafe blocks per crate and prints the method used.
unsafe-report:
	@$(NODE) scripts/unsafe-report.js

bench: addon
	$(NODE) bench/run.js

## The published differential campaign: 70 continuous seconds, slices watched by
## a parent that kills and records anything that stops making progress.
fuzz: addon
	$(NODE) fuzz/campaign.js --seconds 70 --log fuzz/log.txt

## Strict shared-API parity: no known waivers and no unrefereeable inputs.
fuzz-strict: addon
	$(NODE) fuzz/differential.js --strict --seconds 70 --seed 1592594996 --log fuzz/log-strict.txt

## The same, with the family bounds removed, so that every input the oracle
## cannot referee is named individually rather than by rule.
fuzz-limits: addon
	$(NODE) fuzz/campaign.js --seconds 70 --bounds off --stall 2000 --log fuzz/log-limits.txt

## Every method whose upstream body re-judges an operand against minE/maxE —
## receiver or argument, across all nine rounding modes where one is taken —
## checked against the oracle with the limits narrowed after the operand was
## built. The largest family of defect this port had; see DECISIONS.md D-12
## for the family and D-21 for why the check now varies four axes and not two.
clamp-conformance: addon
	$(NODE) scripts/clamp-conformance.js

## The limits the original inherits from its host rather than from its own code:
## the array ceiling V8 enforces, measured here and compared with the constant
## compiled into the port. See DECISIONS.md D-10 and D-19.
host-limits: addon
	$(NODE) scripts/host-limits.js

## Both targeted conformance checks. Neither is slow; both are exhaustive over
## an axis the original suite does not vary at all.
conformance: clamp-conformance host-limits

## Every defect found in the original, on both implementations, side by side.
repro: addon
	$(NODE) fuzz/repro-upstream.js

soak: addon
	$(NODE) scripts/soak.js --json bench/soak.json

clean:
	$(CARGO) clean
	rm -f ./decimal.node
