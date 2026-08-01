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
 * The port deliberately does not reproduce that — D-11, documented as
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

/*
 * The sweep: every routine that can reach the ceiling, at a precision above it.
 *
 * ---------------------------------------------------------------------------
 * What this is for, and what it is *not* for
 * ---------------------------------------------------------------------------
 *
 * The cases above check that the port throws where the original throws. This
 * checks something weaker and, at these precisions, more important: that the
 * port **stops**.
 *
 * `divide` abandons a calculation by setting a flag and returning a placeholder
 * (see `arith::abandoned`). The routine that called it is still running, and it
 * will index digit arrays, divide by what it was handed, and iterate towards a
 * convergence test that can no longer fire. Getting that wrong does not produce
 * a wrong answer — it produces a dead process or a live one that never returns,
 * which is the failure mode this whole project exists to avoid. Three of these
 * hung and nine aborted before the protocol was written down.
 *
 * ---------------------------------------------------------------------------
 * Why the error is only required to *be* an error
 * ---------------------------------------------------------------------------
 *
 * Above 939,524,081 the original cannot compute anything (BUG-007), and which
 * error arrives first depends on the order in which two different limits are
 * met — the host's array ceiling, and the library's own 1025-digit constants
 * for π and ln 10. `ln`, `log` and `pow` reach the constants first here and the
 * array first upstream; both refuse, with different words.
 *
 * Chasing word-for-word parity there would mean reproducing the order in which
 * V8 runs out of backing store inside a series, at a configuration the original
 * cannot serve. So the sweep requires termination and an outcome, reports which
 * pairs agree exactly, and says so plainly rather than quietly relaxing the
 * comparison the cases above make.
 */
const SWEEP_PRECISION = 1e9;

const SWEEP = [
  ['div', (D) => new D(1).div(3)],
  ['sqrt', (D) => new D(2).sqrt()],
  ['cbrt', (D) => new D(2).cbrt()],
  ['ln', (D) => new D(2).ln()],
  ['exp', (D) => new D('0.5').exp()],
  ['sin', (D) => new D('0.5').sin()],
  ['atan', (D) => new D('0.5').atan()],
  ['asin', (D) => new D('0.5').asin()],
  ['sinh', (D) => new D('0.5').sinh()],
  ['pow', (D) => new D(2).pow(new D('0.5'))],
  ['log', (D) => new D(2).log(3)],
  ['mod', (D) => new D(10).mod(3)],
  ['toFraction', (D) => new D('0.5').toFraction()],
  ['toNearest', (D) => new D('1.5').toNearest(new D(1))],
  ['toBinary', (D) => new D('0.1').toBinary()],
];

/* ------------------------------------------------------------------------- *
 * The child: one case, one implementation.
 * ------------------------------------------------------------------------- */

function load(side) {
  return require(side === 'port'
    ? path.join(ROOT, 'decimal.node')
    : path.join(ROOT, 'fuzz', 'reference', 'decimal.js'));
}

function runCase(index, side) {
  const D = load(side);

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

/** One sweep entry, one implementation. Reports the outcome and the elapsed ms. */
function runSweep(index, side) {
  const D = load(side);
  const C = D.clone();
  C.set({ precision: SWEEP_PRECISION });

  const [, call] = SWEEP[index];
  const started = Date.now();
  let outcome;
  try {
    // Not every entry returns a Decimal: `toBinary` returns a string and
    // `toFraction` returns a pair. Fingerprinting on `sd()` alone turned a real
    // divergence — the port rendering a binary expansion the oracle refused to
    // build — into an identical TypeError from this harness, which is exactly
    // the way a check quietly stops checking.
    const value = call(C);
    outcome = typeof value === 'string' ? 'returned a ' + value.length + '-character string'
      : Array.isArray(value) ? 'returned ' + value.length + ' values'
        : 'returned sd=' + value.sd();
  } catch (error) {
    outcome = 'threw ' + error.constructor.name + ': ' + error.message;
  }
  process.stdout.write(JSON.stringify({ outcome, ms: Date.now() - started }) + '\n');
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

function attempt(index, side, flag = '--case') {
  const args = [__filename, flag, String(index), '--side', side];
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

  // -- the sweep ----------------------------------------------------------
  //
  // Above the threshold neither implementation can compute, so what is checked
  // here is that both *stop* and that neither takes its process with it.
  process.stdout.write('the sweep — every routine that can reach the ceiling, at precision ' +
    SWEEP_PRECISION + '\n');
  process.stdout.write('  both implementations must terminate with an outcome; the two\n' +
    '  errors need not be the same one, and where they are it is noted.\n\n');

  let identical = 0;
  let bothRefused = 0;
  SWEEP.forEach(([name], index) => {
    const reference = attempt(index, 'reference', '--sweep');
    const port = attempt(index, 'port', '--sweep');

    // Three verdicts, in decreasing order of what they promise.
    //   ok      the two outcomes are the same string
    //   ok*     both refused, naming different limits — allowed, and counted
    //   FAIL    anything else, including a child that did not survive
    const alive = (r) => !/DID NOT SURVIVE/.test(r.outcome);
    const refused = (r) => /^threw /.test(r.outcome);

    let verdict;
    if (!alive(reference) || !alive(port)) {
      verdict = 'FAIL  ';
    } else if (reference.outcome === port.outcome) {
      verdict = 'ok    ';
      identical++;
    } else if (refused(reference) && refused(port)) {
      verdict = 'ok*   ';
      bothRefused++;
    } else {
      verdict = 'FAIL  ';
    }
    if (verdict === 'FAIL  ') failures++;

    const timing = (r) => (r.ms === undefined ? '' : '  (' + r.ms + ' ms)');
    process.stdout.write('  ' + verdict + name.padEnd(12) +
      ' reference: ' + reference.outcome + timing(reference) + '\n');
    process.stdout.write('        ' + ''.padEnd(12) +
      ' port:      ' + port.outcome + timing(port) + '\n');
  });

  process.stdout.write('\n  ok* means both refused and named different limits — the host\'s\n' +
    '  array ceiling against the library\'s own 1025-digit constants. See D-19.\n\n');

  process.stdout.write('SUMMARY: ' + CASES.length + ' threshold cases and ' +
    SWEEP.length + ' swept routines, ' + failures + ' failing.\n');
  process.stdout.write('         ' + documented +
    ' documented divergence(s) in the configuration left behind (D-11).\n');
  process.stdout.write('         the sweep: every routine terminated; ' + identical +
    ' identical outcomes, ' + bothRefused + ' refused naming different limits.\n');
  process.exit(failures === 0 ? 0 : 1);
}

const argv = process.argv.slice(2);
const caseIndex = argv.indexOf('--case');
const sweepIndex = argv.indexOf('--sweep');
const side = argv[argv.indexOf('--side') + 1];

if (caseIndex !== -1) {
  runCase(Number(argv[caseIndex + 1]), side);
} else if (sweepIndex !== -1) {
  runSweep(Number(argv[sweepIndex + 1]), side === 'sweep-port' ? 'port' : side);
} else {
  main();
}
