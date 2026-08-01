'use strict';

/*
 * One upstream finding, run against one implementation, in its own process.
 *
 * Three of the seven do not terminate and two of the remainder leave the
 * library in a state that would corrupt anything measured after them, so each
 * case has to be isolated. The parent — `repro-upstream.js` — spawns this with
 * a timeout and reads the single JSON line it prints.
 *
 *   node fuzz/repro-case.js <case> <reference|port>
 *
 * Each case returns `{ outcome, ms, note }`. `outcome` is the observable thing:
 * a value, or `THROW <message>`. A case that hangs prints nothing at all, and
 * the parent's timeout is what records it.
 */

const path = require('path');

const which = process.argv[3] || 'reference';
const Decimal = which === 'port'
  ? require('../decimal.node')
  : require(path.join(__dirname, 'reference', 'decimal.js'));

/** Run `thunk`, reducing whatever happens to a string. */
function outcome(thunk) {
  try {
    const value = thunk();
    return String(value).slice(0, 48);
  } catch (error) {
    return 'THROW ' + error.constructor.name + ': ' + error.message;
  }
}

/*
 * Whether the library is still enforcing its own exponent limits.
 *
 * `external` is a module-level flag with no accessor, so it is measured by its
 * effect: the constructor clamps to `maxE` only while it is set. Several of
 * these findings leave it cleared, which is the part that outlives the call.
 */
function clampsStillApplied() {
  try {
    return !new Decimal('1e' + (Decimal.maxE + 1)).isFinite();
  } catch (error) {
    return 'threw';
  }
}

const cases = {
  /*
   * BUG-004. The continued fraction in `toFraction` terminates by cancelling
   * exactly, and under ROUND_FLOOR that cancellation returns -0. The quotient
   * is then -Infinity, the convergent is -Infinity, and the loop's test — "has
   * the denominator grown past the bound" — is false. One iteration later
   * everything is NaN and every comparison is false for ever.
   */
  'tofraction-round-floor': () => {
    Decimal.set({ precision: 20, rounding: Decimal.ROUND_FLOOR });
    return { outcome: outcome(() => new Decimal(1).toFraction()) };
  },

  /*
   * BUG-005. Build a value while `maxE` is wide, narrow `maxE` below its
   * exponent, take a hyperbolic. The first argument reduction overflows, so
   * `taylorSeries` is summing an infinity, and its `if (t.d[k] !== void 0)`
   * dereferences null. It throws from inside its own `external = false`, and
   * nothing restores the flag.
   */
  'sinh-overflowing-series': () => {
    Decimal.set({ precision: 20, maxE: 9e15 });
    const x = new Decimal('5.879302975574934568e100');
    Decimal.set({ precision: 100, maxE: 73 });
    const result = outcome(() => x.sinh());
    return { outcome: result, note: 'clamps still applied afterwards: ' + clampsStillApplied() };
  },

  /*
   * BUG-006. `toLessThanHalfPi` forms the multiple of π to subtract with the
   * clamps in force, so above `maxE` the multiple is Infinity. `isOdd(t)` then
   * reads `t.d.length`, and `t.d` is null. `cosine` and `sine` read the same
   * field one line into their own bodies.
   */
  'cos-overflowing-reduction': () => {
    Decimal.set({ precision: 34, maxE: 9e15 });
    const x = new Decimal('-4.9481810070120303e809');
    Decimal.set({ precision: 20, rounding: 7, maxE: 104 });
    return { outcome: outcome(() => x.cos()) };
  },

  /*
   * BUG-003. `toPower` opens with `x = new Ctor(x)`, which is a clamping copy
   * and can turn a finite receiver into Infinity. Everything after that line
   * assumes a digit array.
   */
  'pow-clamped-base': () => {
    Decimal.set({ precision: 20, maxE: 9e15 });
    const x = new Decimal('1e10');
    Decimal.set({ maxE: 5 });
    return { outcome: outcome(() => x.pow(3)) };
  },

  /*
   * BUG-002. `acosh` raises the working precision, computes, and lowers it
   * again — with no `try`/`finally`. Near the exponent limit the raised
   * precision is around 9e15 and the alignment inside `minus` asks for an array
   * longer than JavaScript allows. The restoring assignments are skipped, and
   * the constructor is left at that precision permanently.
   */
  'acosh-configuration-leak': () => {
    Decimal.set({ precision: 20, rounding: 4, maxE: 9e15 });
    const x = new Decimal('9.87e8999999999999000');
    const result = outcome(() => x.acosh());
    const started = Date.now();
    const follow = outcome(() => new Decimal(1).div(3));
    return {
      outcome: result,
      note: 'precision left at ' + Decimal.precision + '; the next 1/3 took ' +
        (Date.now() - started) + ' ms and gave ' + follow,
    };
  },

  /*
   * A non-termination rather than a crash: `cbrt` of an operand near the
   * exponent floor. Halley's iteration is run at a working precision derived
   * from the exponent, which here is nine quadrillion.
   */
  'cbrt-exponent-floor': () => {
    Decimal.set({ precision: 20, minE: -9e15 });
    const x = new Decimal('-602e-8999999999999975');
    return { outcome: outcome(() => x.cbrt()) };
  },

  /*
   * Not a defect, a cost: `cosh`/`sinh`/`tanh` choose how many times to fold
   * their argument from its *digit count*, never from its magnitude, so the
   * series length is proportional to |x|. The maintainer's own `TODO?` sits on
   * that line. Included because it is the difference between two seconds and
   * half a second, and because the port had to fix an i32 overflow to get there.
   */
  'cosh-argument-fold-cost': () => {
    Decimal.set({ precision: 20, maxE: 9e15, minE: -9e15 });
    const x = new Decimal('1e6');
    return { outcome: outcome(() => x.cosh()) };
  },
};

const name = process.argv[2];
const chosen = cases[name];
if (!chosen) {
  process.stderr.write('unknown case: ' + name + '\n');
  process.exit(2);
}

const started = Date.now();
const result = chosen();
result.ms = Date.now() - started;
process.stdout.write(JSON.stringify(result) + '\n');
