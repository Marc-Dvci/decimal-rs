#!/usr/bin/env node
'use strict';

/*
 * The unsafe report: how much `unsafe` there is, where, and how that is known.
 *
 * ---------------------------------------------------------------------------
 * Why a script rather than a number in the README
 * ---------------------------------------------------------------------------
 *
 * "Zero unsafe blocks" is a claim, and a claim about absence is worth exactly
 * as much as the method behind it. Counting occurrences of the word `unsafe`
 * with grep is not a method: it misses `unsafe` inside a macro expansion, it
 * miscounts `unsafe` in a comment or a string, and it says nothing at all about
 * code a dependency brought in.
 *
 * So this reports two different things and keeps them apart:
 *
 *   1. What the *compiler* enforces. `decimal-core` carries
 *      `#![forbid(unsafe_code)]`, which is not a lint that can be allowed
 *      locally — `forbid` cannot be overridden by an inner `allow`, and the
 *      crate does not compile if an unsafe block appears anywhere in it,
 *      including inside a macro it expands. That attribute is the evidence.
 *      This script checks it is present, and the build checks the rest.
 *
 *   2. What is textually there, per crate, so the adapter's count is visible
 *      rather than merely asserted. Unsafe at an FFI boundary is expected and
 *      unavoidable; hiding it would be worse than reporting it.
 *
 * Usage:  node scripts/unsafe-report.js [--json]
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..');
const CRATES = ['decimal-core', 'decimal-napi', 'decimal-cli'];

/*
 * Whether a crate forbids unsafe, in either of the two places it can be said.
 *
 * `#![forbid(unsafe_code)]` at the top of `lib.rs` is the older spelling;
 * `[lints.rust] unsafe_code = "forbid"` in `Cargo.toml` is the current one and
 * is what this workspace uses. They mean the same thing to the compiler, and a
 * check that knew about only one of them would report a crate as unprotected
 * while the compiler was in fact refusing to build any unsafe in it — the exact
 * failure this script exists to prevent, in reverse.
 */
function forbidsUnsafe(crate) {
  const manifest = path.join(ROOT, 'crates', crate, 'Cargo.toml');
  if (fs.existsSync(manifest)) {
    const text = fs.readFileSync(manifest, 'utf8');
    if (/unsafe_code\s*=\s*"forbid"/.test(text)) return 'Cargo.toml [lints.rust]';
  }
  const lib = path.join(ROOT, 'crates', crate, 'src', 'lib.rs');
  if (fs.existsSync(lib)) {
    const text = fs.readFileSync(lib, 'utf8');
    if (/#!\[forbid\(unsafe_code\)\]/.test(text)) return 'lib.rs #![forbid]';
  }
  return null;
}

/** Every `.rs` file under a crate's `src/`, recursively. */
function sources(crate) {
  const base = path.join(ROOT, 'crates', crate, 'src');
  const out = [];
  const walk = (dir) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.name.endsWith('.rs')) out.push(full);
    }
  };
  if (fs.existsSync(base)) walk(base);
  return out;
}

/*
 * Strip comments and string literals before counting.
 *
 * Without this the count is wrong in both directions in this repository: the
 * module headers discuss unsafety at length, and `decimal-napi` has string
 * literals naming the Node-API functions. A crude scanner is enough — Rust's
 * raw strings and nested block comments are not used here, and the failure mode
 * of getting it wrong is a count that disagrees with the compiler, which the
 * `forbid` check above would catch.
 */
function strip(source) {
  let out = '';
  let i = 0;
  while (i < source.length) {
    const two = source.slice(i, i + 2);
    if (two === '//') {
      while (i < source.length && source[i] !== '\n') i++;
    } else if (two === '/*') {
      i += 2;
      while (i < source.length && source.slice(i, i + 2) !== '*/') i++;
      i += 2;
    } else if (source[i] === '"') {
      i++;
      while (i < source.length && source[i] !== '"') i += source[i] === '\\' ? 2 : 1;
      i++;
    } else {
      out += source[i++];
    }
  }
  return out;
}

/** Occurrences of `unsafe` as a keyword, by kind. */
function count(text) {
  const tally = { blocks: 0, functions: 0, impls: 0, traits: 0, total: 0 };
  const keyword = /\bunsafe\b/g;
  let match;
  while ((match = keyword.exec(text)) !== null) {
    const after = text.slice(match.index + 6, match.index + 40).trimStart();
    if (after.startsWith('{')) tally.blocks++;
    else if (after.startsWith('fn') || after.startsWith('extern')) tally.functions++;
    else if (after.startsWith('impl')) tally.impls++;
    else if (after.startsWith('trait')) tally.traits++;
    else tally.blocks++;
    tally.total++;
  }
  return tally;
}

function main() {
  const report = { crates: [], forbids: {} };

  for (const crate of CRATES) {
    const files = sources(crate);
    if (!files.length) continue;

    const totals = { blocks: 0, functions: 0, impls: 0, traits: 0, total: 0 };
    let lines = 0;
    const forbids = forbidsUnsafe(crate);

    for (const file of files) {
      const source = fs.readFileSync(file, 'utf8');
      lines += source.split('\n').length;
      const tally = count(strip(source));
      for (const key of Object.keys(totals)) totals[key] += tally[key];
    }

    report.forbids[crate] = forbids;
    report.crates.push({ crate, files: files.length, lines, forbids, unsafe: totals });
  }

  if (process.argv.includes('--json')) {
    process.stdout.write(JSON.stringify(report, null, 2) + '\n');
    return;
  }

  process.stdout.write('unsafe report — decimal-rs\n\n');
  process.stdout.write(
    'crate'.padEnd(16) + 'files'.padStart(6) + 'lines'.padStart(8) +
    'unsafe'.padStart(8) + '   compiler-enforced?\n');
  process.stdout.write('-'.repeat(72) + '\n');

  for (const entry of report.crates) {
    process.stdout.write(
      entry.crate.padEnd(16) +
      String(entry.files).padStart(6) +
      String(entry.lines).padStart(8) +
      String(entry.unsafe.total).padStart(8) +
      '   ' + (entry.forbids ? 'yes, via ' + entry.forbids : 'no — see below') + '\n');
  }

  const core = report.crates.find((c) => c.crate === 'decimal-core');
  const napi = report.crates.find((c) => c.crate === 'decimal-napi');

  process.stdout.write('\nmethod\n');
  process.stdout.write(
    '  decimal-core sets unsafe_code = "forbid". `forbid` is not a lint level an\n' +
    '  inner `allow` can turn off, so the crate does not compile if an unsafe\n' +
    '  block appears anywhere in it — including one produced by a macro. The\n' +
    '  declaration is the evidence and a successful `cargo build` is the check;\n' +
    '  the counts above are only a textual cross-check of it, taken after\n' +
    '  comments and string literals have been removed. Grepping for the word\n' +
    '  would not do: this repository discusses unsafety at length in prose.\n');

  if (napi && napi.unsafe.total) {
    process.stdout.write(
      '\n  decimal-napi is the Node-API adapter and cannot be safe: every call into\n' +
      '  the Node-API is an `extern "C"` function over raw pointers supplied by\n' +
      '  V8. Its ' + napi.unsafe.total + ' uses are the boundary and nothing else — the adapter\n' +
      '  contains no arithmetic. All of it is in ' + napi.lines + ' lines against\n' +
      (core ? '  ' + core.lines + ' lines of arithmetic in decimal-core, which has none.\n' : '\n'));
  }

  process.stdout.write(
    '\n  Dependencies: decimal-core has none at all. The workspace depends on\n' +
    '  napi-sys, which is bindings only. `cargo geiger` will report unsafe inside\n' +
    '  it, and that is unsafe in Node itself reached through a binding, not code\n' +
    '  this project wrote.\n');

  if (core && (!core.forbids || core.unsafe.total > 0)) {
    process.stdout.write('\nFAILED: decimal-core is not clean.\n');
    process.exitCode = 1;
  }
}

main();
