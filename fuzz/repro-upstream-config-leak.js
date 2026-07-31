'use strict';

/*
 * Reproduction: decimal.js leaves `precision` and `rounding` raised when an
 * inverse hyperbolic function's internal computation throws.
 *
 * Found by fuzz/differential.js. Upstream v10.6.0, commit cd73a7f.
 *
 * ---------------------------------------------------------------------------
 * What happens
 * ---------------------------------------------------------------------------
 *
 * `acosh` and `asinh` raise the working precision before computing and lower
 * it afterwards:
 *
 *     pr = Ctor.precision;
 *     rm = Ctor.rounding;
 *     Ctor.precision = pr + Math.max(Math.abs(x.e), x.sd()) + 4;
 *     Ctor.rounding = 1;
 *     external = false;
 *     x = x.times(x).minus(1).sqrt().plus(x);   //  <-- can throw
 *     external = true;
 *     Ctor.precision = pr;                      //  <-- never reached
 *     Ctor.rounding = rm;
 *     return x.ln();
 *
 * There is no `try`/`finally`. For an argument near the exponent limit the
 * raised precision is around 9e15, and the alignment inside `minus` then asks
 * for an array longer than JavaScript allows, which throws `RangeError:
 * Invalid array length`. The two restoring assignments are skipped.
 *
 * The constructor is then left with `precision = 9000000000000304` and
 * `rounding = 1`, permanently. Every later operation — on any value, not just
 * the one that failed — is computed at that precision, so the next `1/3`
 * either exhausts the heap or takes minutes. `external` is left `false` too,
 * which disables the exponent clamps for the rest of the process.
 *
 * The library is aware of the hazard elsewhere: `getLn10` restores state
 * *before* it throws, with a comment saying that is deliberate. `acosh`,
 * `asinh` and `atanh` do not.
 *
 * ---------------------------------------------------------------------------
 * Why this port does not reproduce it
 * ---------------------------------------------------------------------------
 *
 * The standing rule for decimal-rs is fidelity: where the original is wrong,
 * the port is wrong in the same way, because the original's test suite is the
 * thing being preserved. This is the exception, and DECISIONS.md D-11 records
 * the reasoning. Briefly: no assertion in the suite covers it, the maintainer's
 * own code shows the opposite intent, and reproducing it would mean the port
 * could be put into a state where every subsequent call exhausts memory.
 *
 * Run with:  node fuzz/repro-upstream-config-leak.js
 */

const Reference = require('./reference/decimal.js');
const Port = require('../decimal.node');

function show(label, D) {
  process.stdout.write('\n' + label + '\n');

  D.config({ precision: 20, rounding: 4 });
  process.stdout.write('  before          precision=' + D.precision + ' rounding=' + D.rounding + '\n');

  // An ordinary finite value, near the exponent ceiling the library documents
  // as valid: maxE is 9e15 and this is well inside it.
  const x = new D('9.87e8999999999999000');

  let outcome;
  try {
    outcome = 'returned ' + x.acosh().toString().slice(0, 24);
  } catch (error) {
    outcome = 'threw ' + error.constructor.name + ': ' + error.message;
  }
  process.stdout.write('  acosh(9.87e+8999999999999000)  ' + outcome + '\n');
  process.stdout.write('  after           precision=' + D.precision + ' rounding=' + D.rounding + '\n');

  // The damage is not confined to the value that failed.
  const started = Date.now();
  let follow;
  try {
    follow = new D(1).div(3).toString().slice(0, 24);
  } catch (error) {
    follow = 'threw ' + error.constructor.name + ': ' + error.message;
  }
  process.stdout.write('  then 1/3        ' + follow +
    '   (' + (Date.now() - started) + ' ms)\n');
}

process.stdout.write(
  'decimal.js configuration leak on a thrown internal computation\n' +
  '  upstream cd73a7f (v10.6.0)  ·  node ' + process.version + '\n');

show('decimal.js  (reference)', Reference);
show('decimal-rs  (this port)', Port);

process.stdout.write('\nExpected: precision and rounding unchanged after the call, whether it\n');
process.stdout.write('returned or threw, and 1/3 still costing microseconds.\n');
