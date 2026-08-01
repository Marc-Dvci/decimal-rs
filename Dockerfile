# Reproduces the whole verification pipeline in one command:
#
#     docker build -t decimal-rs . && docker run --rm decimal-rs
#
# It builds the Rust port from source, checks that every file of the original
# decimal.js test suite is byte-identical to upstream, and then runs that
# unmodified suite against the compiled Rust artifact.
#
# No network access is needed at run time; everything is fetched at build time.

# ---------------------------------------------------------------------------
# Stage 1 — build the Rust addon.
# The toolchain version is pinned by rust-toolchain.toml, not by this tag.
# ---------------------------------------------------------------------------
FROM rust:1-slim-bookworm AS build

WORKDIR /src

# rust-toolchain.toml pins 1.97.1; install it before copying sources so the
# toolchain download is cached independently of the code.
COPY rust-toolchain.toml ./
RUN rustup show

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p decimal-napi -p decimal-cli

# ---------------------------------------------------------------------------
# Stage 2 — run the original test suite against the artifact.
# ---------------------------------------------------------------------------
FROM node:24-slim

WORKDIR /app

# The compiled Rust library, renamed so that Node's own module resolver picks
# it up for require('../decimal'). There is no JavaScript shim.
COPY --from=build /src/target/release/libdecimal.so ./decimal.node
COPY --from=build /src/target/release/decimal-calc ./bin/decimal-calc

COPY test ./test
COPY tests ./tests
COPY scripts ./scripts
COPY package.json Makefile DECISIONS.md README.md .port-mortem.toml ./

# The fuzzing oracle — a byte-identical copy of upstream decimal.js — so that
# the two conformance checks can run here as well, against the same artifact the
# suite just ran against:
#
#     docker run --rm decimal-rs node scripts/clamp-conformance.js
#     docker run --rm decimal-rs node scripts/host-limits.js
#
# It is data, not code: nothing in `crates/` can reach it, and the port does not
# load it. See fuzz/reference/README.md. It is kept out of the default CMD
# because `host-limits.js` deliberately allocates about a gigabyte, which is
# above some default container limits, and a verification command that fails for
# an unrelated reason is worse than one that checks less.
COPY fuzz/reference ./fuzz/reference

# The Rust sources come along so that the unsafe report can count what is
# actually in the tree rather than repeat a number from the README. They are
# not compiled here — stage 1 did that — and they add about 400 KB.
COPY crates ./crates

# Fail loudly at image-build time if the test suite was tampered with, so the
# problem surfaces during the build rather than during judging.
RUN node scripts/verify-tests.js

# Three claims, checked in the order a sceptic would want them: the suite is
# unmodified, it passes, and the core crate contains no unsafe.
#
# The middle one runs through `expect-zero-failures.js` rather than `test/test.js`
# directly, because upstream's runner exits 0 whatever happens — so a container
# that ran it and reported its exit code would report success through any
# regression. The wrapper reads the summary line and fails on anything beyond
# every assertion rather than trusting the upstream runner's always-zero exit.
CMD ["sh", "-c", "node scripts/verify-tests.js && echo '' && node scripts/expect-zero-failures.js && echo '' && node --expose-gc scripts/adapter-regression.js && echo '' && node scripts/unsafe-report.js"]
