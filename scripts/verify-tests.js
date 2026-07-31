#!/usr/bin/env node
// Verifies that every file under test/ is byte-identical to the upstream
// decimal.js test suite as it stood at the pinned commit.
//
// This is the mechanical backing for the claim "0 test files modified". It
// runs as part of the default build and as the first step of the Docker
// image's entrypoint, so a modified test file breaks the build rather than
// silently inflating the pass rate.
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
const manifestPath = path.join(root, 'tests', 'ORIGINAL_HASHES.txt');

const manifest = fs.readFileSync(manifestPath, 'utf8');
const expected = new Map();

for (const rawLine of manifest.split(/\r?\n/)) {
  const line = rawLine.trim();
  if (line === '' || line.startsWith('#')) continue;
  const m = /^([0-9a-f]{64})\s+\*?(.+)$/.exec(line);
  if (!m) {
    console.error(`verify-tests: unparseable manifest line: ${rawLine}`);
    process.exit(2);
  }
  expected.set(m[2], m[1]);
}

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

for (const [file, want] of expected) {
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

if (problems.length > 0) {
  console.error('\n  ORIGINAL TEST SUITE INTEGRITY CHECK FAILED\n');
  for (const p of problems) console.error(`  ${p}`);
  console.error(`\n  ${problems.length} problem(s). The original test files must not be modified.\n`);
  process.exit(1);
}

console.log(
  `  Original test suite verified: ${expected.size} files byte-identical to ` +
    `upstream decimal.js @ cd73a7f (0 modified, 0 added, 0 removed).`
);
