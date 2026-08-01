#!/usr/bin/env node
'use strict';

/*
 * The host-limit conformance check.
 *
 * ---------------------------------------------------------------------------
 * What it is for
 * ---------------------------------------------------------------------------
 *
 * The original grows its digit arrays one index at a time — `qd[i] = …`,
 * `d.push(0)` — and JavaScript refuses, catchably, once such an array gets too
 * big. Several of the library's own routines can ask for one that big without
 * any unusual operand: `divide` sizes its quotient from the working precision,
 * and `sinh`, `asinh` and `acosh` all raise the working precision by the
 * operand's exponent. So `RangeError: Invalid array length` is part of the
 * original's observable behaviour, not an accident of a pathological input.
 *
 * Rust's `Vec` has no such ceiling. A port that simply lets it grow does not
 * throw where the original throws; it asks the allocator for tens of gigabytes
 * and the process dies. That is strictly worse than the exception it failed to
 * reproduce, and it is invisible to the test suite, which never leaves the
 * default precision of twenty. DECISIONS.md D-10 and D-19.
 *
 * This checks the reproduction two ways:
 *
 *   1. It measures the ceiling the host actually enforces, right now, and
 *      compares it with the constant compiled into the port. A number nobody
 *      re-measures is a number that has already drifted.
 *   2. It runs each case on both implementations, in separate processes, and
 *      compares the outcome — the value, or the error's *type* and message.
 *
 * ---------------------------------------------------------------------------
 * Two channels, because they answer to different rules
 * ---------------------------------------------------------------------------
 *
 * The outcome must agree, always. The configuration left behind afterwards need
 * not: the original raises the working precision before it throws and has no
 * `finally`, so a caught `RangeError` leaves it wedged at a precision of 9e15.
 * The port deliberately does not reproduce that — D-11, reported upstream as
 * BUG-002 — so the second channel is compared, reported, and required to
 * diverge only in the direction that decision predicts.
 *
 * Usage:
 *   node scripts/host-limits.js            # everything
 *   node scripts/host-limits.js --case 2 --side port   # one case, for a debugger
 */

const { execFileSync } = require('child_process');
const path = require('path');

const ROOT = path.join(__dirname, '..');
const CORE = path.join(ROOT, 'crates', 'decimal-core', 'src', 'lib.rs');

/* Long enough for the slowest case on the oracle, which spends about two
 * seconds filling an array before V8 stops it. */
const TIMEOUT_MS = 60000;

/*
 * Each case is a configuration and a call. `expectThrow` is what the *original*
 * does, stated here rather than derived from the run, so that a case which
 * quietly stopped reaching the ceiling — because a constant moved, or because
 * the ceiling did — is a failure and not a silent pass.
 */
const CASES = [
  {
    name: 'div at the largest documented precision',
    why: 'precision 1e9 asks divide for 1e9/7 + 2 limbs, which is above the ceiling',
    config: { precision: 1e9 },
    call: (D) => new D(1).div(3),
    expectThrow: true,
  },
  {
    name: 'sinh one exponent below the ceiling',
    why: 'sinh raises the precision by the operand exponent; the limb target then wraps negative',
    config: { precision: 5, rounding: 1, minE: -9e15, maxE: 9e15 },
    call: (D) => new D('859496e8999999999999953').sinh(),
    expectThrow: true,
  },
  {
    name: 'asinh near the exponent ceiling',
    why: 'the alignment inside plus wants 2.6e15 leading zeros — the case that found this class',
    config: { precision: 20, minE: -9e15, maxE: 9e15 },
    call: (D) => new D('1e8999999999999999').asinh(),
    expectThrow: true,
  },
  {
    name: 'acosh near the exponent ceiling',
    why: 'the same, through minus rather than plus',
    config: { precision: 20, minE: -9e15, maxE: 9e15 },
    call: (D) => new D('9.87e8999999999999000').acosh(),
    expectThrow: true,
  },
  {
    // The control. A ceiling set too low would break ordinary arithmetic, and
    // nothing above would notice: every case above expects a throw.
    name: 'div at a precision that fits — the control',
    why: 'must return; a ceiling set too low would fail here and only here',
    config: { precision: 1e6 },
    call: (D) => new D(1).div(3),
    expectThrow: false,
  },

  /*
   * The threshold itself, from both sides.
   *
   * `divide` asks for `⌊pr/7 + 2⌋ + 1` limbs, so it breaches 2²⁷ exactly when
   * `pr ≥ 7 × 134_217_726`. Both implementations were bisected independently and
   * both turn over between these two precisions — not near them, at them. That
   * is a stronger statement than "the port also throws eventually", and it is
   * the one worth pinning: it says the arithmetic that sizes the quotient is the
   * same arithmetic, including the 32-bit truncation in the middle of it.
   */
  {
    name: 'div at the largest precision that still fits, 939_524_081',
    why: 'one below the threshold — the last precision at which the original divides',
    config: { precision: 939524081 },
    call: (D) => new D(1).div(3),
    expectThrow: false,
  },
  {
    name: 'div at the smallest precision that does not, 939_524_082',
    why: 'one above the threshold — the first precision at which it does not',
    config: { precision: 939524082 },
    call: (D) => new D(1).div(3),
    expectThrow: true,
  },
];

/* ------------------------------------------------------------------------- *
 * The child: one case, one implementation.
 * ------------------------------------------------------------------------- */

function runCase(index, side) {
  const D = require(side === 'port'
    ? path.join(ROOT, 'decimal.node')
    : path.join(ROOT, 'fuzz', 'reference', 'decimal.js'));

  const testCase = CASES[index];
  const C = D.clone();
  C.set(testCase.config);

  let outcome;
  try {
    const value = testCase.call(C);
    // Fingerprinted by significant digits and exponent rather than by its
    // string. A quotient at these precisions runs to 939 million digits, and
    // rendering it is itself a way to run out of memory — which would report as
    // a failure of the port rather than of the harness.
    outcome = 'returned sd=' + value.sd() + ' e=' + value.e +
      ' first limb ' + value.d[0];
  } catch (error) {
    outcome = 'threw ' + error.constructor.name + ': ' + error.message;
  }

  process.stdout.write(JSON.stringify({
    outcome,
    precisionAfter: C.precision,
    roundingAfter: C.rounding,
  }) + '\n');
}

/* ------------------------------------------------------------------------- *
 * The parent.
 * ------------------------------------------------------------------------- */

/*
 * The largest array this host will build by assigning one index at a time.
 *
 * Not `2^32 - 1`. That is the largest number an array's `length` may *hold*; a
 * 64-bit V8 keeps a dense array's elements in a backing store capped at one
 * gigabyte of eight-byte slots and throws long before the specification would.
 * The difference is four billion elements, and the port's whole quotient loop
 * lives inside it.
 */
function measureCeiling() {
  const probe = 'const a = [];' +
    'try { for (let i = 0; ; i++) a[i] = 0; } catch (e) { console.log(a.length); }';
  return Number(execFileSync(process.execPath, ['-e', probe], {
    encoding: 'utf8',
    timeout: TIMEOUT_MS,
  }).trim());
}

/* What `MAX_ARRAY_LENGTH` is compiled as, read from the source rather than
 * restated here — the point of the comparison is that the two are independent. */
function declaredCeiling() {
  const source = require('fs').readFileSync(CORE, 'utf8');
  const match = /pub const MAX_ARRAY_LENGTH: i64 = ([0-9_]+);/.exec(source);
  return match ? Number(match[1].replace(/_/g, '')) : NaN;
}

function attempt(index, side) {
  const args = [__filename, '--case', String(index), '--side', side];
  try {
    const stdout = execFileSync(process.execPath, args, {
      encoding: 'utf8',
      timeout: TIMEOUT_MS,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    return JSON.parse(stdout.trim().split('\n').pop());
  } catch (error) {
    // A child that died rather than threw is the exact failure this check
    // exists to catch, so it is a result and not an error of the harness.
    const died = error.signal ? 'killed by ' + error.signal
      : error.status === null ? 'timed out'
        : 'exited ' + error.status;
    return { outcome: 'DID NOT SURVIVE — ' + died, precisionAfter: null, roundingAfter: null };
  }
}

function main() {
  let failures = 0;
  let documented = 0;

  const measured = measureCeiling();
  const declared = declaredCeiling();
  process.stdout.write('host array ceiling\n');
  process.stdout.write('  measured on this host   ' + measured + ' = 2^' + Math.log2(measured) + '\n');
  process.stdout.write('  compiled into the port  ' + declared + '\n');
  if (measured !== declared) {
    process.stdout.write('  *** these must agree — MAX_ARRAY_LENGTH in ' +
      path.relative(ROOT, CORE) + ' is stale ***\n');
    failures++;
  }
  process.stdout.write('\n');

  CASES.forEach((testCase, index) => {
    const reference = attempt(index, 'reference');
    const port = attempt(index, 'port');

    const agree = reference.outcome === port.outcome;
    const asExpected = /^threw /.test(reference.outcome) === testCase.expectThrow;

    process.stdout.write((agree && asExpected ? '  ok    ' : '  FAIL  ') + testCase.name + '\n');
    process.stdout.write('        ' + testCase.why + '\n');
    process.stdout.write('        reference  ' + reference.outcome + '\n');
    process.stdout.write('        port       ' + port.outcome + '\n');

    if (!agree) {
      failures++;
    }
    if (!asExpected) {
      process.stdout.write('        *** the reference no longer ' +
        (testCase.expectThrow ? 'reaches the ceiling here' : 'returns here') +
        ' — this case has stopped testing what it was written for ***\n');
      failures++;
    }

    if (reference.precisionAfter !== port.precisionAfter) {
      // D-11: the original raises the working precision and throws without a
      // `finally`, leaving the library wedged. The port restores it. Anything
      // other than "reference higher, port at its configured value" is news.
      const leak = reference.precisionAfter > port.precisionAfter &&
        port.precisionAfter === (testCase.config.precision || 20);
      process.stdout.write('        precision afterwards: reference ' +
        reference.precisionAfter + ', port ' + port.precisionAfter +
        (leak ? '  (D-11, the leak the port declines to reproduce)' : '  *** UNEXPECTED ***') + '\n');
      if (leak) {
        documented++;
      } else {
        failures++;
      }
    }
    process.stdout.write('\n');
  });

  process.stdout.write('SUMMARY: ' + CASES.length + ' cases, ' +
    (CASES.length - failures) + ' in agreement, ' + failures + ' failing.\n');
  process.stdout.write('         ' + documented +
    ' documented divergence(s) in the configuration left behind (D-11).\n');
  process.exit(failures === 0 ? 0 : 1);
}

const argv = process.argv.slice(2);
const caseIndex = argv.indexOf('--case');
if (caseIndex !== -1) {
  runCase(Number(argv[caseIndex + 1]), argv[argv.indexOf('--side') + 1]);
} else {
  main();
}
