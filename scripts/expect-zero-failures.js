#!/usr/bin/env node
'use strict';

/*
 * Upstream's runner prints failures but exits zero. Stream its output, parse
 * its final count, and make any failed assertion fail automation.
 */

const { spawn } = require('child_process');
const path = require('path');

const suite = spawn(process.execPath, [path.join(__dirname, '..', 'test', 'test.js')], {
  cwd: path.join(__dirname, '..'),
});
let output = '';

for (const stream of [suite.stdout, suite.stderr]) {
  stream.on('data', (chunk) => {
    output += chunk;
    (stream === suite.stdout ? process.stdout : process.stderr).write(chunk);
  });
}

suite.on('close', (code) => {
  if (code !== 0) {
    console.error(`\nThe upstream runner exited ${code}.`);
    process.exitCode = 1;
    return;
  }

  const summary = /In total, ([\d,]+) of ([\d,]+) tests passed/.exec(output);
  if (!summary) {
    console.error('\nThe suite printed no final assertion count.');
    process.exitCode = 1;
    return;
  }

  const passed = Number(summary[1].replace(/,/g, ''));
  const asserted = Number(summary[2].replace(/,/g, ''));
  if (passed !== asserted) {
    console.error(`\n${asserted - passed} of ${asserted} upstream assertions failed.`);
    process.exitCode = 1;
    return;
  }

  console.log(`\n  strict gate: all ${asserted.toLocaleString('en-GB')} upstream assertions passed.`);
});
