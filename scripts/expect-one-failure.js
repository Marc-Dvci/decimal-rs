#!/usr/bin/env node
'use strict';

/*
 * Run the original test suite and hold it to the documented failure count.
 *
 * ---------------------------------------------------------------------------
 * Why this exists
 * ---------------------------------------------------------------------------
 *
 * `node test/test.js` **exits 0 whatever happens**. It prints its failures and
 * returns success, because upstream's runner was written to be read by a person
 * rather than gated on by a machine. That is upstream's file and it is
 * hash-pinned, so it is not going to change here.
 *
 * The consequence is that a continuous-integration job which runs the suite and
 * checks its exit code is not checking anything: the suite could start failing
 * five thousand assertions and the job would stay green. Since the whole claim
 * of this repository is "the original suite passes", a check that cannot see it
 * stop passing is worse than no check — it is a green badge that means nothing.
 *
 * So the count is read out of the suite's own summary line and compared with
 * the number this port documents:
 *
 *     In total, 22657 of 22658 tests passed in 0.268 secs.
 *
 * One failure is expected and explained — DECISIONS.md D-08, a Node-API
 * signature constraint on prototype identity. Two would be news.
 *
 * The denominator moves between runs, by design: about 6,000 of the assertions
 * are generated with `Math.random()`. That is why this reads the *difference*
 * and never the ratio.
 *
 *   node scripts/expect-one-failure.js
 */

const { spawn } = require('child_process');
const path = require('path');

/** The failure documented in D-08, and the only one tolerated. */
const ALLOWED = 1;

const suite = spawn(process.execPath, [path.join(__dirname, '..', 'test', 'test.js')], {
  cwd: path.join(__dirname, '..'),
});

let output = '';

suite.stdout.on('data', (chunk) => {
  output += chunk;
  process.stdout.write(chunk);
});
suite.stderr.on('data', (chunk) => {
  output += chunk;
  process.stderr.write(chunk);
});

suite.on('close', (code) => {
  if (code !== 0) {
    process.stderr.write('\nThe suite exited ' + code + ', which it is not supposed to do at all.\n');
    process.exit(1);
  }

  const summary = /In total, ([\d,]+) of ([\d,]+) tests passed/.exec(output);
  if (!summary) {
    // The suite printing no summary is itself a failure: it means the run did
    // not reach the end, which the exit code would not have told us either.
    process.stderr.write('\nThe suite printed no "In total" line. It did not finish.\n');
    process.exit(1);
  }

  const passed = Number(summary[1].replace(/,/g, ''));
  const asserted = Number(summary[2].replace(/,/g, ''));
  const failures = asserted - passed;

  if (failures > ALLOWED) {
    process.stderr.write(
      '\n' + failures + ' assertions failed; ' + ALLOWED + ' is documented (DECISIONS.md D-08).\n' +
      'The ' + (failures - ALLOWED) + ' beyond it are a regression.\n');
    process.exit(1);
  }

  if (failures < ALLOWED) {
    // Not an error, but not something to pass over in silence either.
    process.stdout.write(
      '\n  ' + failures + ' failures, fewer than the ' + ALLOWED + ' documented in D-08.\n' +
      '  If that is real rather than a flake, D-08 and the README now overstate the problem.\n');
    return;
  }

  process.stdout.write(
    '\n  ' + failures + ' failure of ' + asserted.toLocaleString('en-GB') + ' assertions, as documented (D-08).\n');
});
