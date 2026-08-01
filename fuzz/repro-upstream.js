'use strict';

/*
 * Every upstream defect this port's differential campaign found, run against
 * both implementations, side by side.
 *
 * ---------------------------------------------------------------------------
 *
 * Six of the seven cases are things the original does that a caller cannot
 * recover from: three null dereferences, one non-terminating loop, one
 * non-terminating iteration, and one configuration leak that leaves the library
 * permanently unusable. The seventh is a cost, not a defect.
 *
 * Each runs in its own process with a timeout, because three of them do not
 * return and two of the rest wreck the state of anything measured afterwards.
 * The individual cases are in `repro-case.js` and can be run one at a time:
 *
 *     node fuzz/repro-case.js tofraction-round-floor reference
 *
 * The write-ups, one per finding, are in `docs/upstream/`.
 *
 * Usage:  node fuzz/repro-upstream.js [--timeout MS]
 */

const { spawn } = require('child_process');
const path = require('path');

const CASES = [
  ['tofraction-round-floor', 'BUG-004', 'toFraction never returns under ROUND_FLOOR'],
  ['sinh-overflowing-series', 'BUG-005', 'taylorSeries dereferences null and leaves the clamps off'],
  ['cos-overflowing-reduction', 'BUG-006', 'the argument reduction of sin/cos/tan dereferences null'],
  ['pow-clamped-base', 'BUG-003', 'toPower dereferences null when the clamp made the base infinite'],
  ['acosh-configuration-leak', 'BUG-002', 'acosh leaks precision and rounding when it throws'],
  ['cbrt-exponent-floor', '—', 'cbrt does not return near the exponent floor'],
  ['cosh-argument-fold-cost', '—', 'the hyperbolic fold is chosen by digit count, not magnitude'],
];

function parseArguments(argv) {
  const options = { timeout: 20000 };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--timeout') options.timeout = Number(argv[++i]);
  }
  return options;
}

function run(name, side, timeout) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath,
      [path.join(__dirname, 'repro-case.js'), name, side],
      { stdio: ['ignore', 'pipe', 'pipe'] });

    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (c) => { stdout += c; });
    child.stderr.on('data', (c) => { stderr += c; });

    const started = Date.now();
    const timer = setTimeout(() => child.kill('SIGKILL'), timeout);

    child.on('close', () => {
      clearTimeout(timer);
      const elapsed = Date.now() - started;
      try {
        resolve(Object.assign({ elapsed }, JSON.parse(stdout.trim().split('\n').pop())));
      } catch (notJson) {
        resolve({
          elapsed,
          outcome: /panicked/.test(stderr)
            ? 'PANICKED: ' + (/panicked at [^\n]*\n([^\n]*)/.exec(stderr) || [, '?'])[1]
            : 'DID NOT RETURN within ' + (timeout / 1000).toFixed(0) + ' s',
        });
      }
    });
  });
}

async function main() {
  const options = parseArguments(process.argv.slice(2));

  process.stdout.write(
    'upstream defects found by the decimal-rs differential campaign\n' +
    '  decimal.js v10.6.0 @ cd73a7f  ·  node ' + process.version +
    '  ·  timeout ' + (options.timeout / 1000).toFixed(0) + ' s per case\n' +
    '  write-ups in docs/upstream/\n');

  for (const [name, tag, description] of CASES) {
    process.stdout.write('\n' + tag.padEnd(9) + description + '\n');
    process.stdout.write('  ' + '-'.repeat(72) + '\n');
    for (const side of ['reference', 'port']) {
      const result = await run(name, side, options.timeout);
      const label = side === 'reference' ? 'decimal.js' : 'decimal-rs';
      process.stdout.write('  ' + label.padEnd(12) +
        String(result.outcome).padEnd(50) + ' ' +
        String(result.ms === undefined ? result.elapsed : result.ms).padStart(6) + ' ms\n');
      if (result.note) process.stdout.write('  ' + ' '.repeat(12) + result.note + '\n');
    }
  }

  process.stdout.write(
    '\nEvery case above is an input on which the original produces no usable\n' +
    'answer. Where this port differs, the difference is recorded in DECISIONS.md\n' +
    '(D-11, D-13, D-14, D-16, D-17) and is always the same judgement: reproduce\n' +
    'what the original computes, decline what merely breaks it.\n');
}

main();
