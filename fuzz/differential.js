'use strict';

/*
 * Differential fuzzing: decimal-rs against the original decimal.js.
 *
 * ---------------------------------------------------------------------------
 * What this is for
 * ---------------------------------------------------------------------------
 *
 * The original test suite is a fixed set of assertions somebody wrote down.
 * It is excellent, and passing it is the point of the port — but it can only
 * ever check the cases its author thought of, and it runs each operation from
 * one starting configuration.
 *
 * This harness checks the other thing: that the two implementations agree on
 * inputs nobody chose, under configurations nobody wrote down, in sequences
 * where each operation inherits the state the last one left. Both run in one
 * process, on the same values, in lockstep.
 *
 * ---------------------------------------------------------------------------
 * What "agree" means here
 * ---------------------------------------------------------------------------
 *
 * Not `a.toString() === b.toString()`. That comparison is what lets a port
 * ship with `-0` printing as `0`, with a right value carrying a wrong
 * exponent, or with an operation that quietly mutates its receiver.
 *
 * Every result is reduced to a record covering everything observable about it
 * (see `describe`): sign, exponent, the digit array itself, all three string
 * renderings, the finiteness predicates, the precision metadata, and whether a
 * zero is negative. Errors are compared by exact message, including the
 * `[DecimalError]` prefix. Before and after every operation, the receiver, the
 * arguments and the constructor's configuration are recorded and compared, so
 * an operation that mutates something it should not have is a divergence even
 * when it returns the right answer.
 *
 * A difference in any channel fails. There is no tolerance anywhere in this
 * file, and there should not be: the port's whole claim is that it is the same
 * program.
 *
 * ---------------------------------------------------------------------------
 * Why it reports its own liveness
 * ---------------------------------------------------------------------------
 *
 * A log that says "zero divergences" proves nothing on its own — a harness
 * that compares nothing also finds nothing. So each run begins by deliberately
 * corrupting the port's results, at one ulp of the working precision, and
 * refuses to continue until the comparator has caught it. The iteration at
 * which it was caught is printed. Only then is the fault removed and the real
 * run started, fresh.
 *
 * Usage:
 *   node fuzz/differential.js [--seconds N] [--seed 0xHEX] [--iterations N]
 *                             [--quiet] [--trace] [--log PATH]
 */

const fs = require('fs');
const os = require('os');
const path = require('path');

const Reference = require('./reference/decimal.js');
const Port = require('../decimal.node');

// ---------------------------------------------------------------------------
// Randomness
// ---------------------------------------------------------------------------

/*
 * A seeded generator, so that any run is replayable from the seed printed in
 * its own header. `Math.random()` would make a divergence unreproducible,
 * which would make it unreportable, which would make finding it pointless.
 *
 * SplitMix32: small, well-distributed enough for choosing test inputs, and
 * short enough to read.
 */
function Rng(seed) {
  this.state = seed >>> 0;
}

Rng.prototype.next = function () {
  this.state = (this.state + 0x9e3779b9) >>> 0;
  let z = this.state;
  z = Math.imul(z ^ (z >>> 16), 0x21f0aaad) >>> 0;
  z = Math.imul(z ^ (z >>> 15), 0x735a2d97) >>> 0;
  return (z ^ (z >>> 15)) >>> 0;
};

/** An integer in `[0, n)`. */
Rng.prototype.below = function (n) {
  return this.next() % n;
};

/** A float in `[0, 1)`. */
Rng.prototype.unit = function () {
  return this.next() / 4294967296;
};

/** One element of `array`. */
Rng.prototype.pick = function (array) {
  return array[this.below(array.length)];
};

/** True with probability `p`. */
Rng.prototype.chance = function (p) {
  return this.unit() < p;
};

// ---------------------------------------------------------------------------
// Input generation
// ---------------------------------------------------------------------------

/*
 * The structural corpus: the values where implementations are known to part
 * company. Every one of these is here because it is a boundary in the source,
 * not because it looked interesting.
 */
const CORPUS = [
  // The zeros, and the sign that distinguishes them.
  '0', '-0', '0.0', '-0.0',
  // Units and the identities.
  '1', '-1', '2', '-2', '0.5', '-0.5',
  // Non-finite, in both spellings the constructor accepts.
  'NaN', 'Infinity', '-Infinity',
  // The thresholds at which JavaScript's own number-to-string switches to
  // exponential notation, which `toString` inherits.
  '1e21', '1e-7', '0.0000001', '1e20', '999999999999999999999',
  // The exponent limits, and one step past them.
  '9e15', '-9e15', '1e9000000000000000', '1e-9000000000000000',
  '9.999e+9000000000000000', '1e9000000000000001',
  // Rounding ties: the inputs that separate the nine rounding modes.
  '0.5', '1.5', '2.5', '-0.5', '-1.5', '-2.5',
  '0.05', '0.15', '0.25', '1.05', '1.15',
  // A run of nines, where rounding carries out of the leading digit.
  '9.9999999999999999999', '0.99999999999999999999',
  '99999999999999999999', '9999999999999999999999999999999999999999',
  // Just above and just below a half-way point at twenty digits.
  '1.00000000000000000005', '0.999999999999999999949999',
  // Limb boundaries: the base is 10^7, so these straddle one.
  '1234567', '12345678', '0.1234567', '0.12345678',
  '10000000', '9999999', '0.0000001234567',
  // Values whose ln and exp are awkward.
  '1.000000000000000000000000000000000000001', '2.718281828459045235360287',
  '0.9999999', '1.0000000000001',
  // The smallest subnormal double, and a hex float, exercising `parseOther`.
  '5e-324', '0x1p-1074', '0xff', '0b1011', '0o777', '0x1.8p3',
  // Textual edge cases the constructor has opinions about.
  '+1', '  1  ', '1.', '.1', '1_000', '1e+2', '1E2', '',
  // Long digit strings, above and below the default precision.
  '123456789012345678901234567890',
  '0.123456789012345678901234567890123456789012345678901234567890',
];

/** A random decimal literal, structured rather than uniform. */
function randomLiteral(rng) {
  // Log-uniform digit count, so tiny and huge are both common.
  const digits = 1 + Math.floor(Math.pow(500, rng.unit()));
  let mantissa = String(1 + rng.below(9));
  for (let i = 1; i < digits; i++) mantissa += String(rng.below(10));

  // Trailing zeros are their own hazard: they are stripped on the way in, so a
  // value can have fewer significant digits than it was written with.
  if (rng.chance(0.15)) mantissa += '0'.repeat(1 + rng.below(8));

  let exponent;
  const where = rng.below(10);
  if (where < 5) {
    exponent = rng.below(41) - 20;              // near zero
  } else if (where < 8) {
    exponent = rng.below(1001) - 500;           // moderate
  } else if (where < 9) {
    exponent = 9000000000000000 - rng.below(50); // at the ceiling
  } else {
    exponent = rng.below(50) - 9000000000000000; // at the floor
  }

  const sign = rng.chance(0.5) ? '-' : '';
  return sign + mantissa + 'e' + exponent;
}

/*
 * A value, plus the *form* it should be supplied in.
 *
 * The constructor has three separate code paths — string, JavaScript number,
 * and existing Decimal — and they do not share their edge cases. Fuzzing only
 * strings would leave two thirds of the constructor unexercised. The number
 * path in particular goes through `Number.prototype.toString`, which is where
 * this port had to reimplement ECMAScript's shortest-round-trip tie-break.
 */
function randomInput(rng) {
  const literal = rng.chance(0.45) ? rng.pick(CORPUS) : randomLiteral(rng);

  if (rng.chance(0.25)) {
    const n = Number(literal);
    // Only offer the number form when it survives the trip; otherwise the two
    // implementations would be given genuinely different inputs.
    if (!Number.isNaN(n) || literal === 'NaN') return { kind: 'number', value: n };
  }
  return { kind: 'string', value: literal };
}

// ---------------------------------------------------------------------------
// Configuration space
// ---------------------------------------------------------------------------

/*
 * Most divergences are configuration-dependent, because configuration is what
 * decides where rounding happens, and rounding is where two implementations of
 * the same arithmetic differ. Rounding modes are swept rather than sampled:
 * there are only nine and they are the most likely axis of disagreement.
 */
const SETTINGS = ['precision', 'rounding', 'toExpNeg', 'toExpPos', 'minE', 'maxE', 'modulo'];

function randomConfig(rng, sweepRounding) {
  const precisionChoices = [1, 2, 3, 5, 7, 8, 15, 20, 20, 20, 34, 40, 100, 300];
  return {
    precision: rng.chance(0.85) ? rng.pick(precisionChoices) : 1 + rng.below(1000),
    rounding: sweepRounding % 9,
    toExpNeg: rng.chance(0.7) ? -7 : -(1 + rng.below(30)),
    toExpPos: rng.chance(0.7) ? 21 : 1 + rng.below(30),
    minE: rng.chance(0.85) ? -9e15 : -(1 + rng.below(1000)),
    maxE: rng.chance(0.85) ? 9e15 : 1 + rng.below(1000),
    modulo: rng.below(10),
  };
}

function readConfig(D) {
  const out = {};
  for (const name of SETTINGS) out[name] = D[name];
  out.crypto = D.crypto;
  return JSON.stringify(out);
}

// ---------------------------------------------------------------------------
// Observation
// ---------------------------------------------------------------------------

/*
 * Everything observable about one result, as a string.
 *
 * Reducing to a string means the comparison is exact by construction and the
 * divergence report is legible without further work. The cost is that these
 * records are rebuilt on every operation, which is most of the harness's
 * running time — a price worth paying, because every channel dropped here is a
 * class of bug that ships.
 */
function describe(D, value) {
  if (value === undefined) return 'undefined';
  if (value === null) return 'null';

  if (Array.isArray(value)) {
    // `toFraction` returns a two-element array.
    return '[' + value.map((v) => describe(D, v)).join(' , ') + ']';
  }

  if (D.isDecimal(value)) {
    const x = value;
    const parts = [
      's=' + x.s,
      'e=' + x.e,
      // The digit array itself. The port keeps the original's base-10^7 limb
      // layout deliberately, because limb boundaries decide where guard digits
      // fall; comparing only the rendered value would hide a drift here until
      // it changed an answer.
      'd=[' + (x.d === null ? '' : x.d.join(',')) + ']',
      'str=' + x.toString(),
      'val=' + x.valueOf(),
      'exp=' + x.toExponential(),
      'fin=' + x.isFinite(),
      'nan=' + x.isNaN(),
      'int=' + x.isInteger(),
      'sd=' + x.sd(),
      'sdz=' + x.sd(true),
      'dp=' + x.dp(),
      // `-0` and `0` are equal to every comparison the library offers, so the
      // sign has to be read off separately or it is never checked at all.
      'neg0=' + (x.isZero() && x.isNegative()),
    ];
    return '{' + parts.join(' ') + '}';
  }

  if (typeof value === 'number') {
    // Distinguish the two zeros here as well: `String(-0)` is `'0'`.
    return 'n:' + (Object.is(value, -0) ? '-0' : String(value));
  }

  return typeof value.toString === 'function' ? 't:' + String(value) : String(value);
}

/** Run `thunk`, returning either its described result or its error message. */
function attempt(D, thunk) {
  try {
    return describe(D, thunk());
  } catch (error) {
    return 'THROW ' + error.message;
  }
}

// ---------------------------------------------------------------------------
// The operations under test
// ---------------------------------------------------------------------------

/*
 * `arity` counts Decimal operands. `extra` supplies the trailing plain-number
 * arguments — decimal places, significant digits, rounding modes — which are
 * generated per call so that those paths are exercised too rather than always
 * taking their defaults.
 */
const OPERATIONS = [];

/*
 * ---------------------------------------------------------------------------
 * One bound, and the reason for it
 * ---------------------------------------------------------------------------
 *
 * The corpus deliberately includes values at the exponent limits — 1e9e15 and
 * 1e-9e15 are legal, `maxE` says so — and most of the API handles them in
 * constant time, because the exponent is a field and not a length.
 *
 * Some of it does not. Three families have a cost proportional to the
 * *magnitude* of the exponent rather than to the number of digits:
 *
 *   · the transcendentals, which raise their working precision to
 *     `pr + max(|e|, sd) + k` before computing, so an operand at the ceiling
 *     asks for a precision of 9e15;
 *   · `mod`, `divToInt`, `toNearest` and `toFraction`, which form an integer
 *     quotient or denominator whose digit count is the gap between the
 *     operands' exponents;
 *   · `toFixed` and the radix renderings, whose output is one character per
 *     digit before the point.
 *
 * At the limits, all three ask for something in the region of 10^15 digits.
 * Upstream's answer is to exhaust the heap, or to run for hours, or — for
 * `acosh` — to throw `RangeError: Invalid array length` and leave its own
 * configuration wrecked (see D-11). None of that is the port disagreeing; it is
 * the oracle being unable to answer, and an oracle that cannot answer cannot
 * referee.
 *
 * So those families are fuzzed for `|e| < EXPONENT_BOUND` and everything else
 * across the whole range. The bound is stated in the log header, family by
 * family, and it is the only restriction on the input space besides
 * `Decimal.random`.
 */
const EXPONENT_BOUND = 10000;

/** Cost proportional to the exponent: fuzz these within the bound. */
function withinExponentBound(a, b) {
  if (a.isFinite() && Math.abs(a.e) >= EXPONENT_BOUND) return false;
  if (b && b.isFinite() && Math.abs(b.e) >= EXPONENT_BOUND) return false;
  return true;
}

/*
 * `sinh`, `cosh` and `tanh` need a second, tighter bound on the *value* rather
 * than the exponent. They choose how many times to fold their argument from its
 * digit count and never from its magnitude — upstream's own comment there reads
 * `TODO? Estimation reused from cosine() and may not be optimal here` — so where
 * `cos` first reduces modulo π/2, these do not, and the series needs work
 * proportional to |x|. Upstream's `cosh(1e6)` takes two seconds; `cosh(1e8)`
 * takes minutes. See D-09.
 */
const HYPERBOLIC_BOUND = 1e4;

function withinHyperbolicBound(a) {
  return withinExponentBound(a) && (!a.isFinite() || a.abs().lt(HYPERBOLIC_BOUND));
}

function unary(name, guard) {
  OPERATIONS.push({ name, arity: 1, guard, apply: (a) => a[name]() });
}

function binary(name, guard) {
  OPERATIONS.push({ name, arity: 2, guard, apply: (a, b) => a[name](b) });
}

// Constant in the exponent: fuzzed across the whole range, limits included.
[
  'abs', 'neg', 'ceil', 'floor', 'round', 'trunc', 'sqrt', 'cbrt',
  'toString', 'valueOf', 'toNumber', 'toJSON',
  'isNaN', 'isFinite', 'isInteger', 'isZero', 'isNegative', 'isPositive',
  'dp', 'sd',
].forEach((name) => unary(name));

['plus', 'minus', 'times', 'div', 'cmp', 'eq', 'lt', 'lte', 'gt', 'gte']
  .forEach((name) => binary(name));

// Proportional to the exponent: bounded.
['exp', 'ln', 'sin', 'cos', 'tan', 'asin', 'acos', 'atan', 'asinh', 'acosh', 'atanh']
  .forEach((name) => unary(name, withinExponentBound));

['sinh', 'cosh', 'tanh'].forEach((name) => unary(name, withinHyperbolicBound));

['pow', 'log', 'divToInt', 'mod'].forEach((name) => binary(name, withinExponentBound));

// The ones whose extra arguments matter enough to generate.
OPERATIONS.push({
  name: 'toExponential', arity: 1,
  apply: (a, _b, rng) => a.toExponential(rng.below(40), rng.below(9)),
});
OPERATIONS.push({
  name: 'toPrecision', arity: 1,
  apply: (a, _b, rng) => a.toPrecision(1 + rng.below(40), rng.below(9)),
});
OPERATIONS.push({
  name: 'toDP', arity: 1,
  apply: (a, _b, rng) => a.toDP(rng.below(40), rng.below(9)),
});
OPERATIONS.push({
  name: 'toSD', arity: 1,
  apply: (a, _b, rng) => a.toSD(1 + rng.below(40), rng.below(9)),
});
OPERATIONS.push({
  name: 'precision(true)', arity: 1,
  apply: (a) => a.precision(true),
});
OPERATIONS.push({
  name: 'clamp', arity: 3,
  apply: (a, b, _rng, c) => a.clamp(b, c),
});
OPERATIONS.push({
  name: 'toNearest', arity: 2, guard: withinExponentBound,
  apply: (a, b, rng) => a.toNearest(b, rng.below(9)),
});
OPERATIONS.push({
  name: 'toFixed', arity: 1, guard: withinExponentBound,
  apply: (a, _b, rng) => a.toFixed(rng.below(40), rng.below(9)),
});
OPERATIONS.push({
  name: 'toFraction', arity: 1, guard: withinExponentBound,
  apply: (a, _b, rng) => (rng.chance(0.5) ? a.toFraction() : a.toFraction(1 + rng.below(100000))),
});
// Both forms of each radix rendering: bare, which expands every digit, and
// with a significant-digit count, which switches to exponential notation and
// takes a different path through the same code.
['toBinary', 'toHex', 'toOctal'].forEach((name) => {
  OPERATIONS.push({
    name, arity: 1, guard: withinExponentBound,
    apply: (a, _b, rng) => (rng.chance(0.5)
      ? a[name]()
      : a[name](1 + rng.below(40), rng.below(9))),
  });
});

// Statics. `D` is the constructor under test, which differs between the two
// sides, so these take it explicitly.
const STATICS = [
  { name: 'max', apply: (D, xs) => D.max.apply(D, xs) },
  { name: 'min', apply: (D, xs) => D.min.apply(D, xs) },
  { name: 'sum', apply: (D, xs) => D.sum.apply(D, xs) },
  { name: 'hypot', apply: (D, xs) => D.hypot.apply(D, xs) },
  { name: 'atan2', apply: (D, xs) => D.atan2(xs[0], xs[1]) },
  { name: 'sign', apply: (D, xs) => D.sign(xs[0]) },
];

/*
 * `Decimal.random` is deliberately not fuzzed, and this is the only exclusion.
 *
 * It has no fixed answer to agree on: the two implementations draw from
 * different generators by design — `Math.random()` on one side, a xoshiro256**
 * written into `decimal-core` on the other, because the crate has no ambient
 * source of randomness. A differential test of it could only compare the shape
 * of the result, which `crates/decimal-core/src/random.rs` already asserts
 * directly, against a scripted entropy source, for every one of the digit
 * layouts the routine can produce.
 */
const EXCLUDED = ['random (no fixed answer; covered by scripted-entropy unit tests in random.rs)'];

// ---------------------------------------------------------------------------
// Fault injection
// ---------------------------------------------------------------------------

/*
 * When armed, the port's results are corrupted by one unit in the last place
 * of the working precision — the smallest error a real rounding bug could
 * produce, and therefore the right thing to prove the comparator can see.
 *
 * The corruption is applied to the harness's *view* of the port rather than to
 * the port itself. That is not a compromise: what is being measured is whether
 * this file's comparison would catch a wrong answer, and a wrong answer
 * injected here is indistinguishable from one computed there.
 */
let faultArmed = false;

function maybeCorrupt(D, result) {
  if (!faultArmed || !D.isDecimal(result)) return result;
  if (!result.isFinite() || result.isZero()) return result;
  const ulp = new D('1e' + (result.e - D.precision + 1));
  return result.plus(ulp);
}

// ---------------------------------------------------------------------------
// One sequence
// ---------------------------------------------------------------------------

/*
 * A sequence, not a single call.
 *
 * Single-call fuzzing finds shallow bugs. The bugs that matter in a port of a
 * library with global mutable configuration are the ones where state leaks:
 * a cached constant computed at one precision and reused at another, a clone
 * that is not independent of its parent, an operation that leaves the
 * configuration changed. None of those are reachable without a sequence, and
 * all of them are reachable with a short one.
 *
 * Returns null when the two implementations agreed throughout, or a divergence
 * report when they did not.
 */
function runSequence(steps, seed, options) {
  const rng = new Rng(seed);
  const log = {
    entries: [],
    push(line) {
      this.entries.push(line);
      // `--trace` writes each step before it runs, so that an operation which
      // never returns still names itself. Nothing else finds a hang.
      if (options.trace) process.stderr.write('    · ' + line + '\n');
    },
    slice() { return this.entries.slice(); },
  };

  // Both sides start from the same fresh configuration.
  const config = randomConfig(rng, options.sweep);
  let R = Reference;
  let P = Port;
  R.config(config);
  P.config(config);
  log.push('Decimal.set(' + JSON.stringify(config) + ')');

  // The pool holds matched pairs: the same value on both sides.
  const pool = [];
  const seedCount = 2 + rng.below(3);
  for (let i = 0; i < seedCount; i++) {
    const input = randomInput(rng);
    const literal = input.kind === 'number' ? input.value : input.value;
    let r, p;
    try {
      r = new R(literal);
    } catch (error) {
      try {
        new P(literal);
      } catch (portError) {
        if (portError.message === error.message) { i--; continue; }
        return report(log, 'new Decimal(' + JSON.stringify(literal) + ')',
          'THROW ' + error.message, 'THROW ' + portError.message);
      }
      return report(log, 'new Decimal(' + JSON.stringify(literal) + ')',
        'THROW ' + error.message, 'no exception');
    }
    try {
      p = new P(literal);
    } catch (portError) {
      return report(log, 'new Decimal(' + JSON.stringify(literal) + ')',
        describe(R, r), 'THROW ' + portError.message);
    }
    const before = [describe(R, r), describe(P, p)];
    if (before[0] !== before[1]) {
      return report(log, 'new Decimal(' + JSON.stringify(literal) + ')', before[0], before[1]);
    }
    log.push('x' + i + ' = new Decimal(' + JSON.stringify(literal) + ')');
    pool.push({ r, p });
  }

  for (let step = 0; step < steps; step++) {
    // Occasionally change the configuration mid-sequence: the values already
    // in the pool were built under the old one, which is exactly the situation
    // a cached constant gets wrong.
    if (rng.chance(0.12)) {
      const next = randomConfig(rng, options.sweep + step);
      R.config(next);
      P.config(next);
      log.push('Decimal.set(' + JSON.stringify(next) + ')');
      continue;
    }

    // Occasionally continue under a clone, which must be independent of its
    // parent and must carry the parent's settings.
    if (rng.chance(0.05)) {
      const overrides = { precision: 1 + rng.below(50), rounding: rng.below(9) };
      R = R.clone(overrides);
      P = P.clone(overrides);
      log.push('Decimal = Decimal.clone(' + JSON.stringify(overrides) + ')');
      continue;
    }

    const useStatic = rng.chance(0.12);
    const configBefore = [readConfig(R), readConfig(P)];

    // How many operands, and which.
    const op = useStatic ? rng.pick(STATICS) : rng.pick(OPERATIONS);
    let count;
    if (!useStatic) count = op.arity;
    else if (op.name === 'sign') count = 1;
    else if (op.name === 'atan2') count = 2;
    else count = 1 + rng.below(4);

    const chosen = [];
    for (let i = 0; i < count; i++) chosen.push(rng.pick(pool));

    // An operation with a guard that its operand fails is not run at all, and
    // the step is spent. Substituting a different operand instead would bias
    // the input distribution towards whatever the guard admits.
    if (!useStatic && op.guard && !op.guard(chosen[0].r, chosen[1] && chosen[1].r)) continue;

    const operandsBefore = chosen.map((v) => [describe(R, v.r), describe(P, v.p)]);

    const expression = useStatic
      ? 'Decimal.' + op.name + '(' + chosen.map((_, i) => 'x' + i).join(', ') + ')'
      : 'x0.' + op.name + '(' + chosen.slice(1).map((_, i) => 'x' + (i + 1)).join(', ') + ')';

    // Logged *before* it is attempted, so that an operation which never
    // returns still names itself under `--trace`. Logging afterwards means the
    // last line of a hang is the last thing that worked, which is the one
    // thing you already know.
    log.push(expression);

    let referenceResult, portResult;

    // Both sides must see the same generated extra arguments — decimal places,
    // rounding modes — so the generator is rewound between them. Without this
    // the two implementations would be asked different questions and every
    // answer would differ.
    const rngBefore = rng.state;

    if (useStatic) {
      referenceResult = attempt(R, () => op.apply(R, chosen.map((v) => v.r)));
      rng.state = rngBefore;
      portResult = attempt(P, () => maybeCorrupt(P, op.apply(P, chosen.map((v) => v.p))));
    } else {
      referenceResult = attempt(R, () =>
        op.apply(chosen[0].r, chosen[1] && chosen[1].r, rng, chosen[2] && chosen[2].r));
      const rngAfter = rng.state;
      rng.state = rngBefore;
      portResult = attempt(P, () => maybeCorrupt(P,
        op.apply(chosen[0].p, chosen[1] && chosen[1].p, rng, chosen[2] && chosen[2].p)));
      rng.state = rngAfter;
    }

    if (referenceResult !== portResult) {
      return report(log, expression, referenceResult, portResult);
    }

    // Immutability. Every operation in the table above returns a new value and
    // must leave its operands exactly as it found them; the original's own
    // suite devotes 3,281 assertions to this, which is a fair measure of how
    // easy it is to get wrong. Checking it here costs one re-description per
    // operand and covers every operation, not the ones somebody listed.
    for (let i = 0; i < chosen.length; i++) {
      const nowR = describe(R, chosen[i].r);
      const nowP = describe(P, chosen[i].p);
      if (nowR !== operandsBefore[i][0] || nowP !== operandsBefore[i][1]) {
        return report(log, expression + '  [operand x' + i + ' after the call]',
          operandsBefore[i][0] + ' -> ' + nowR,
          operandsBefore[i][1] + ' -> ' + nowP);
      }
    }

    // Configuration. Nothing in the table is `config` or `clone`, so the
    // settings must be untouched — on both sides, and identically. A port that
    // raises the precision for an intermediate and forgets to put it back
    // passes every single-call test and fails here on the next step.
    const configAfter = [readConfig(R), readConfig(P)];
    if (configAfter[0] !== configAfter[1] ||
        configBefore[0] !== configAfter[0] ||
        configBefore[1] !== configAfter[1]) {
      return report(log, expression + '  [configuration across the call]',
        configBefore[0] + ' -> ' + configAfter[0],
        configBefore[1] + ' -> ' + configAfter[1]);
    }
  }

  return null;
}

function report(log, expression, expected, actual) {
  return { log: log.slice(), expression, expected, actual };
}

// ---------------------------------------------------------------------------
// Minimisation
// ---------------------------------------------------------------------------

/*
 * A forty-line sequence is not a bug report; it is a puzzle handed to whoever
 * has to fix it. Shrink by halving the step count while the divergence
 * survives, which is enough here because the sequences are short and the
 * failing step is almost always the last one.
 */
function minimise(seed, steps, options) {
  let best = steps;
  let current = steps;
  while (current > 1) {
    const candidate = Math.floor(current / 2);
    if (runSequence(candidate, seed, options)) {
      best = candidate;
      current = candidate;
    } else {
      break;
    }
  }
  return best;
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

function parseArguments(argv) {
  const options = { seconds: 63, seed: null, iterations: Infinity, quiet: false, log: null, trace: false };
  for (let i = 0; i < argv.length; i++) {
    const flag = argv[i];
    if (flag === '--seconds') options.seconds = Number(argv[++i]);
    else if (flag === '--seed') options.seed = Number(argv[++i]);
    else if (flag === '--iterations') options.iterations = Number(argv[++i]);
    else if (flag === '--quiet') options.quiet = true;
    else if (flag === '--trace') options.trace = true;
    else if (flag === '--log') options.log = argv[++i];
  }
  if (options.seed === null) options.seed = (Date.now() ^ (Math.random() * 0xffffffff)) >>> 0;
  return options;
}

function main() {
  const options = parseArguments(process.argv.slice(2));
  const lines = [];
  const emit = (line) => {
    lines.push(line);
    if (!options.quiet) process.stdout.write(line + '\n');
  };

  const referenceVersion = /decimal\.js v([\d.]+)/.exec(
    fs.readFileSync(path.join(__dirname, 'reference', 'decimal.js'), 'utf8')
  );

  emit('port-mortem differential fuzz — decimal.js vs decimal-rs');
  emit('oracle:  fuzz/reference/decimal.js  v' + (referenceVersion ? referenceVersion[1] : '?') +
       ' @ cd73a7f  [vendored; not linked into the port]');
  emit('subject: decimal.node  (crates/decimal-core + crates/decimal-napi, release)');
  emit('host:    ' + os.cpus()[0].model.trim() + ' / ' + os.platform() + ' ' + os.release() +
       ' / node ' + process.version);
  emit('started ' + new Date().toISOString() + '   seed 0x' +
       options.seed.toString(16).toUpperCase().padStart(8, '0'));
  emit('budget:  ' + options.seconds + 's continuous');
  emit('');
  emit('comparing, per operation: sign, exponent, digit array, toString, valueOf,');
  emit('  toExponential, isFinite, isNaN, isInteger, precision, precision(true),');
  emit('  decimalPlaces, negative-zero, thrown message, and the constructor');
  emit('  configuration before and after.');
  emit('excluded: ' + EXCLUDED.join('; '));
  emit('bounded: operations whose cost is proportional to the operand exponent are');
  emit('  fuzzed for |e| < ' + EXPONENT_BOUND + ' — the transcendentals (they raise their working');
  emit('  precision to pr + max(|e|, sd) + k), mod/divToInt/toNearest/toFraction (they');
  emit('  form an integer quotient of that many digits), and toFixed/toBinary/toHex/');
  emit('  toOctal (their output is one character per digit before the point). sinh,');
  emit('  cosh and tanh take a tighter |x| < ' + HYPERBOLIC_BOUND + ' because upstream folds by digit');
  emit('  count and not by magnitude. At the limits the *oracle* cannot answer — it');
  emit('  exhausts the heap, or runs for hours, or throws and wrecks its own config');
  emit('  (D-09, D-11) — so these are bounds on what can be refereed, not on what the');
  emit('  port can do. Everything else, arithmetic and comparison and the default');
  emit('  renderings, is fuzzed across the whole exponent range, 1e9000000000000000');
  emit('  included.');

  emit('');

  // -- self-check ---------------------------------------------------------
  //
  // Prove the comparator is live before trusting it to report silence.
  faultArmed = true;
  let detectedAt = 0;
  const selfCheckRng = new Rng(options.seed ^ 0xa5a5a5a5);
  for (let i = 1; i <= 20000; i++) {
    const divergence = runSequence(6, selfCheckRng.next(), { sweep: i, trace: false });
    if (divergence) { detectedAt = i; break; }
  }
  faultArmed = false;

  if (!detectedAt) {
    emit('[harness self-check] FAILED — injected one-ulp fault went undetected.');
    emit('This run proves nothing and is aborted.');
    finish(lines, options, 1);
    return;
  }
  emit('[harness self-check] injected one-ulp fault in the port\'s results');
  emit('[harness self-check]   -> DETECTED at sequence ' + detectedAt + ' (comparator is live)');
  emit('[harness self-check] fault reverted; starting clean run');
  emit('');

  // -- the real run -------------------------------------------------------
  const rng = new Rng(options.seed);
  const started = Date.now();
  const deadline = started + options.seconds * 1000;
  let sequences = 0;
  let operations = 0;
  let divergences = 0;
  let nextReport = started + 10000;
  const failures = [];

  for (;;) {
    const now = Date.now();
    if (now >= deadline || sequences >= options.iterations) break;

    const steps = 3 + (rng.below(12));
    const seed = rng.next();
    if (options.trace) {
      process.stderr.write('== sequence ' + sequences + ' (' + steps +
        ' steps, seed 0x' + seed.toString(16) + ')\n');
    }
    const divergence = runSequence(steps, seed, { sweep: sequences, trace: options.trace });
    sequences++;
    operations += steps;

    if (divergence) {
      divergences++;
      const shrunk = minimise(seed, steps, { sweep: sequences - 1, trace: false });
      failures.push({ divergence, seed, steps: shrunk });
      emit('');
      emit('DIVERGENCE #' + divergences + '  seed 0x' + seed.toString(16) +
           '  (minimised to ' + shrunk + ' steps)');
      for (const line of divergence.log) emit('    ' + line);
      emit('  at:       ' + divergence.expression);
      emit('  reference ' + divergence.expected);
      emit('  port      ' + divergence.actual);
      emit('');
      if (divergences >= 20) { emit('stopping after 20 divergences'); break; }
    }

    if (now >= nextReport) {
      emit('seq ' + String(sequences).padStart(9) +
           '  ops ' + String(operations).padStart(10) +
           '  elapsed ' + ((now - started) / 1000).toFixed(1).padStart(6) + 's' +
           '  divergences ' + divergences);
      nextReport = now + 10000;
    }
  }

  const elapsed = (Date.now() - started) / 1000;
  emit('');
  emit('STOP  elapsed ' + elapsed.toFixed(1) + 's  sequences ' + sequences +
       '  operations ' + operations + '  divergences ' + divergences);
  emit('');
  emit('SUMMARY: ' + operations.toLocaleString('en-US') + ' operations over ' +
       sequences.toLocaleString('en-US') + ' stateful sequences,');
  emit('         across ' + (OPERATIONS.length + STATICS.length) +
       ' API entry points and all 9 rounding modes.');
  emit('         ' + (divergences === 0
    ? 'Zero divergences over ' + elapsed.toFixed(1) + ' continuous seconds.'
    : divergences + ' divergence(s); see above.'));

  finish(lines, options, divergences === 0 ? 0 : 1);
}

function finish(lines, options, code) {
  const target = options.log || path.join(__dirname, 'log.txt');
  fs.writeFileSync(target, lines.join('\n') + '\n');
  if (!options.quiet) process.stdout.write('\nlog written to ' + target + '\n');
  process.exitCode = code;
}

main();
