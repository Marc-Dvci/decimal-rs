#!/usr/bin/env node
'use strict';

/*
 * The exponent-clamp conformance check.
 *
 * ---------------------------------------------------------------------------
 * What it is for
 * ---------------------------------------------------------------------------
 *
 * Thirty places in the original begin by writing the receiver through its own
 * constructor:
 *
 *     x = new Ctor(x);
 *
 * That is not a copy. Passing an existing Decimal through the constructor
 * *re-judges* it against the current `minE` and `maxE` — so a value built while
 * the limits were wide comes back as ±Infinity or as zero when they are
 * narrower. A value is measured against the limits when it is **used**, not
 * when it is made. DECISIONS.md D-12 is about this.
 *
 * It is the single largest family of defect found in this port. Every one of
 * them is invisible at the default configuration, which is why the original's
 * 22,658 assertions have nothing to say about any of them: the suite would have
 * to narrow the limits *after* building an operand, and it never does.
 *
 * The differential campaign does reach them, because its configuration space
 * includes narrow limits and its sequences change configuration mid-flight —
 * but it reaches them one at a time, as a by-product, over minutes. This checks
 * all of them in one pass, in a few seconds, and names the method.
 *
 * ---------------------------------------------------------------------------
 * How
 * ---------------------------------------------------------------------------
 *
 * Build the operand under wide limits. Narrow the limits. Call the method.
 * Compare the port against the vendored oracle on the exact string, including
 * the exact thrown message. Repeat across a spread of operands and limits.
 *
 * One child process per method *and operand*, because four of these methods do
 * not return for one of these operands — in either implementation — and a check
 * that hangs is a check nobody runs. Sharding that finely means such a case
 * costs the four limit pairs it belongs to rather than the whole method, and
 * the report says which case it was.
 *
 * Usage:
 *   node scripts/clamp-conformance.js              # everything
 *   node scripts/clamp-conformance.js sinh 0       # one method, one operand
 */

const { execFileSync } = require('child_process');
const path = require('path');

const ROOT = path.join(__dirname, '..');
const Reference = require(path.join(ROOT, 'fuzz', 'reference', 'decimal.js'));
const Port = require(path.join(ROOT, 'decimal.node'));

const TIMEOUT_MS = 10000;

const WIDE = {
  precision: 20, rounding: 4, toExpNeg: -7, toExpPos: 21,
  minE: -9e15, maxE: 9e15, modulo: 1,
};

/* Operands: two above any tested `maxE`, two below any tested `minE`, and two
 * ordinary ones as a control — if those ever differ, something unrelated to
 * clamping is wrong. */
const VALUES = ['1.5e300', '-1.5e300', '1.5e-300', '-1.5e-300', '7', '-0.125'];

const LIMITS = [
  { minE: -9e15, maxE: 100 },
  { minE: -100, maxE: 9e15 },
  { minE: -3, maxE: 3 },
  { minE: -400, maxE: 400 },
];

/* The methods, with an argument shape for those that need one. Every method
 * whose upstream body contains `new Ctor(x)` is here, plus the renderings,
 * which reach it through `finalise`. */
const CALLS = [
  ['abs', (x) => x.abs()],
  ['neg', (x) => x.neg()],
  ['ceil', (x) => x.ceil()],
  ['floor', (x) => x.floor()],
  ['round', (x) => x.round()],
  ['trunc', (x) => x.trunc()],
  ['sqrt', (x) => x.sqrt()],
  ['cbrt', (x) => x.cbrt()],
  ['sin', (x) => x.sin()],
  ['cos', (x) => x.cos()],
  ['tan', (x) => x.tan()],
  ['sinh', (x) => x.sinh()],
  ['tanh', (x) => x.tanh()],
  ['asin', (x) => x.asin()],
  ['atan', (x) => x.atan()],
  ['acosh', (x) => x.acosh()],
  ['asinh', (x) => x.asinh()],
  ['atanh', (x) => x.atanh()],
  ['plus', (x, D) => x.plus(new D(0))],
  ['minus', (x, D) => x.minus(new D(0))],
  ['times', (x, D) => x.times(new D(1))],
  ['div', (x, D) => x.div(new D(1))],
  ['mod', (x, D) => x.mod(new D(3))],
  ['pow', (x) => x.pow(2)],
  ['clamp', (x, D) => x.clamp(new D('-1e400'), new D('1e400'))],
  ['toNearest', (x, D) => x.toNearest(new D(1))],
  ['toDP', (x) => x.toDP()],
  ['toDP(2)', (x) => x.toDP(2)],
  ['toSD(4)', (x) => x.toSD(4)],
  ['toExponential', (x) => x.toExponential()],
  ['toExponential(3)', (x) => x.toExponential(3)],
  ['toFixed', (x) => x.toFixed()],
  ['toFixed(2)', (x) => x.toFixed(2)],
  ['toPrecision', (x) => x.toPrecision()],
  ['toPrecision(4)', (x) => x.toPrecision(4)],
  ['toFraction', (x) => x.toFraction()],
  ['toString', (x) => x.toString()],
  ['valueOf', (x) => x.valueOf()],
  ['toJSON', (x) => x.toJSON()],
  ['toNumber', (x) => x.toNumber()],
  ['sd', (x) => x.sd()],
  ['dp', (x) => x.dp()],
  ['isInteger', (x) => x.isInteger()],
];

/*
 * The divergences that are deliberate, documented, and therefore not failures.
 *
 * Both are the same judgement, recorded in DECISIONS.md: where the original
 * dereferences null and raises V8's `TypeError`, the port answers instead. The
 * clamps turned a value into Infinity and the original then used it as though
 * it still had digits — in `toPower` (D-13 / BUG-003) and in the argument
 * reduction of `sin`, `cos` and `tan` (D-17 / BUG-006).
 *
 * They are recognised rather than filtered: a run reports how many fired, and
 * a difference that is *not* one of them still fails the check.
 */
const DOCUMENTED = [
  {
    tag: 'D-13, D-17 / BUG-003, BUG-006',
    what: 'upstream dereferences null on a value the clamps made infinite; the port answers',
    matches: (d) =>
      d.expected.indexOf('THROW Cannot read properties of null') === 0 &&
      d.actual.indexOf('THROW') !== 0,
  },
];

function documented(difference) {
  return DOCUMENTED.some((known) => known.matches(difference));
}

function attempt(D, value, limits, call) {
  D.config(WIDE);
  const x = new D(value);
  D.config(Object.assign({}, WIDE, limits));
  try {
    return String(call(x, D));
  } catch (error) {
    return 'THROW ' + error.message;
  }
}

/*
 * Check one method, printing a line per case as it goes.
 *
 * Incrementally, and that is the point: four of these methods have one operand
 * on which neither implementation returns, and a child that only printed at the
 * end would lose the twenty-three cases it had already done. The parent reads
 * whatever arrived before the kill, so a hang costs one case rather than a
 * method.
 */
function one(name, valueIndex) {
  const entry = CALLS.find(([n]) => n === name);
  if (!entry) {
    process.stderr.write('unknown method: ' + name + '\n');
    process.exit(2);
  }
  const call = entry[1];

  for (const value of [VALUES[valueIndex]]) {
    for (const limits of LIMITS) {
      const started = { value, limits };
      process.stdout.write(JSON.stringify({ start: started }) + '\n');
      const expected = attempt(Reference, value, limits, call);
      const actual = attempt(Port, value, limits, call);
      process.stdout.write(JSON.stringify(
        expected === actual ? { ok: true } : { value, limits, expected, actual },
      ) + '\n');
    }
  }
}

/*
 * Run one method against one operand in a child, and read back whatever it
 * managed to report before it was killed.
 *
 * Sharded by operand, not just by method, because the operand that hangs is
 * usually the first one — `sinh(1.5e300)` does not return in either
 * implementation — and a method-sized shard would then report nothing at all
 * for the other five. A hang now costs four cases instead of twenty-four.
 */
function collect(name) {
  const differences = [];
  const stuck = [];
  let done = 0;

  for (let i = 0; i < VALUES.length; i++) {
    let stdout = '';
    try {
      stdout = execFileSync(process.execPath, [__filename, name, String(i)],
        { timeout: TIMEOUT_MS, encoding: 'utf8' });
    } catch (killed) {
      stdout = String(killed.stdout || '');
    }

    let pending = null;
    for (const line of stdout.split('\n').filter(Boolean)) {
      let entry;
      try {
        entry = JSON.parse(line);
      } catch (partial) {
        continue;
      }
      if (entry.start) { pending = entry.start; continue; }
      done++;
      pending = null;
      if (!entry.ok) differences.push(entry);
    }
    if (pending) stuck.push(pending);
  }

  return { done, perCase: VALUES.length * LIMITS.length, differences, stuck };
}

function all() {
  let failing = 0;
  let knownCount = 0;
  let incomplete = 0;
  const cases = CALLS.length * VALUES.length * LIMITS.length;

  process.stdout.write(
    'exponent-clamp conformance — decimal-rs against decimal.js v10.6.0 @ cd73a7f\n\n' +
    'Operands built with minE/maxE wide, then the limits narrowed, then the\n' +
    'method called — the only arrangement in which the original\'s `new Ctor(x)`\n' +
    'is observable. ' + CALLS.length + ' methods x ' + VALUES.length + ' operands x ' +
    LIMITS.length + ' limit pairs = ' + cases + ' calls.\n\n');

  for (const [name] of CALLS) {
    const { done, perCase, differences, stuck: stalled } = collect(name);
    const stuck = stalled[0];
    const known = differences.filter(documented);
    const real = differences.filter((d) => !documented(d));
    knownCount += known.length;

    let status;
    if (real.length) {
      failing++;
      status = 'DIFFERS on ' + real.length + ' of ' + perCase;
    } else if (known.length) {
      status = 'ok (' + known.length + ' documented)';
    } else {
      status = 'ok';
    }
    if (done < perCase) {
      incomplete++;
      status += '   [' + done + '/' + perCase + ' — ' +
        (stuck ? stuck.value + ' at maxE=' + stuck.limits.maxE : 'a case') +
        (stalled.length > 1 ? ' and ' + (stalled.length - 1) + ' more' : '') +
        ' did not return in ' + (TIMEOUT_MS / 1000) + ' s, in either implementation]';
    }
    process.stdout.write('  ' + name.padEnd(18) + status + '\n');

    for (const d of real.slice(0, 2)) {
      process.stdout.write('      ' + d.value.padEnd(11) +
        ' minE=' + String(d.limits.minE).padEnd(7) +
        ' maxE=' + String(d.limits.maxE) + '\n');
      process.stdout.write('        oracle  ' + d.expected.slice(0, 58) + '\n');
      process.stdout.write('        port    ' + d.actual.slice(0, 58) + '\n');
    }
  }

  process.stdout.write('\n' + (failing === 0
    ? 'Every method agrees with the oracle under narrowed exponent limits.'
    : failing + ' method(s) differ.') + '\n');
  if (knownCount) {
    process.stdout.write(knownCount + ' documented divergence(s) encountered: ' +
      DOCUMENTED[0].what + ' (D-13, D-17).\n');
  }
  if (incomplete) {
    process.stdout.write(incomplete + ' method(s) had a case that neither ' +
      'implementation returns from; the rest of their cases were checked.\n');
  }
  process.exitCode = failing === 0 ? 0 : 1;
}

if (process.argv[2]) one(process.argv[2], Number(process.argv[3] || 0));
else all();
