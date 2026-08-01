#!/usr/bin/env node
// Verifies that the two things this project must not have edited are
// byte-identical to upstream: every file under test/, and the fuzzing oracle.
//
// This is the mechanical backing for the claim "0 test files modified". It
// runs as part of the default build and as the first step of the Docker
// image's entrypoint, so a modified test file breaks the build rather than
// silently inflating the pass rate.
//
// The oracle is checked for the same reason and is the subtler of the two.
// `fuzz/reference/decimal.js` is what every differential claim is a claim
// *about*; editing it would fake a clean campaign while leaving the test suite
// passing, so it would not show up anywhere else.
//
// Deliberately dependency-free and platform-independent: it accepts both the
// GNU coreutils text form ("<hash>  path") and the binary form
// ("<hash> *path"), and it reads every file as raw bytes so that a line-ending
// rewrite is detected rather than normalised away.

'use strict';

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const root = path.resolve(__dirname, '..');

/** Read one `<hash>  path` manifest into a map, rejecting anything malformed. */
function readManifest(name) {
  const text = fs.readFileSync(path.join(root, 'tests', name), 'utf8');
  const entries = new Map();
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (line === '' || line.startsWith('#')) continue;
    const m = /^([0-9a-f]{64})\s+\*?(.+)$/.exec(line);
    if (!m) {
      console.error(`verify-tests: unparseable line in ${name}: ${rawLine}`);
      process.exit(2);
    }
    entries.set(m[2], m[1]);
  }
  if (entries.size === 0) {
    console.error(`verify-tests: ${name} lists no files, so it checks nothing`);
    process.exit(2);
  }
  return entries;
}

const expected = readManifest('ORIGINAL_HASHES.txt');
const oracle = readManifest('ORACLE_SHA256.txt');

function walk(dir) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else out.push(path.relative(root, full).split(path.sep).join('/'));
  }
  return out;
}

const present = walk(path.join(root, 'test')).sort();
const problems = [];

for (const file of present) {
  if (!expected.has(file)) {
    problems.push(`ADDED     ${file}  (not present upstream)`);
  }
}

/** Hash every listed file and record whatever does not match. */
function check(entries) {
  for (const [file, want] of entries) {
    const full = path.join(root, file);
    if (!fs.existsSync(full)) {
      problems.push(`MISSING   ${file}`);
      continue;
    }
    const got = crypto.createHash('sha256').update(fs.readFileSync(full)).digest('hex');
    if (got !== want) {
      problems.push(`MODIFIED  ${file}\n            expected ${want}\n            actual   ${got}`);
    }
  }
}

check(expected);
check(oracle);

if (problems.length > 0) {
  console.error('\n  UPSTREAM INTEGRITY CHECK FAILED\n');
  for (const p of problems) console.error(`  ${p}`);
  console.error(
    `\n  ${problems.length} problem(s). Neither the original test files nor the ` +
      `fuzzing oracle may be modified.\n`
  );
  process.exit(1);
}

console.log(
  `  Original test suite verified: ${expected.size} files byte-identical to ` +
    `upstream decimal.js @ cd73a7f (0 modified, 0 added, 0 removed).`
);
console.log(
  `  Fuzzing oracle verified:      ${oracle.size} file byte-identical to the same commit.`
);
