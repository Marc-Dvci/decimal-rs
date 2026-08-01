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

.PHONY: all build addon verify-tests test test-original test-rust clean fmt lint \
        unsafe-report bench fuzz fuzz-limits repro soak

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
test-original: addon verify-tests
	$(NODE) test/test.js

## The port's own Rust unit tests (kept out of test/ by design).
test-rust:
	$(CARGO) test --release

test: test-rust test-original

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
	$(NODE) fuzz/campaign.js --seconds 70

## The same, with the family bounds removed, so that every input the oracle
## cannot referee is named individually rather than by rule.
fuzz-limits: addon
	$(NODE) fuzz/campaign.js --seconds 70 --bounds off --stall 2000

## Every defect found in the original, on both implementations, side by side.
repro: addon
	$(NODE) fuzz/repro-upstream.js

soak: addon
	$(NODE) scripts/soak.js --json bench/soak.json

clean:
	$(CARGO) clean
	rm -f ./decimal.node
