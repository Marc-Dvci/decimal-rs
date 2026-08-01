'use strict';

/*
 * Peak RSS over a fixed workload, in a fresh process.
 *
 *   node bench/rss-probe.js <reference|port> <burst|steady>
 *
 * Two modes, because for a native addon they measure genuinely different
 * things and reporting only one of them would mislead in whichever direction
 * was chosen.
 *
 * **burst** runs the whole workload without yielding. Node runs Node-API
 * finalizers from the event loop, so nothing the addon allocates is released
 * until the burst ends: this is the high-water mark of a tight synchronous
 * loop, and it is a real number that a caller doing exactly that will see.
 *
 * **steady** runs the same total work in batches with a turn of the loop
 * between them, which is how a server or a spreadsheet uses a library. This is
 * the resident footprint.
 *
 * For the pure-JavaScript original the two are nearly the same, because V8
 * collects its own objects during the loop and does not need a turn. The gap
 * between them *is* the cost of the boundary's deferred finalization, which
 * makes printing both more informative than printing either.
 *
 * Prints the peak in bytes.
 */

const path = require('path');

const ROOT = path.join(__dirname, '..');
const which = process.argv[2] === 'port' ? 'port' : 'reference';
const mode = process.argv[3] === 'steady' ? 'steady' : 'burst';

const Decimal = which === 'port'
  ? require(path.join(ROOT, 'decimal.node'))
  : require(path.join(ROOT, 'fuzz', 'reference', 'decimal.js'));

const TOTAL = 200000;
const BATCH = mode === 'steady' ? 2000 : TOTAL;

Decimal.precision = 200;

let peak = 0;
function sample() {
  const rss = process.memoryUsage().rss;
  if (rss > peak) peak = rss;
}

let a = new Decimal('1.2345678901234567890123456789');

function batch(count) {
  for (let i = 0; i < count; i++) {
    a = a.plus('0.000000000000000000001').times('1.0000001');
    if ((i & 1023) === 0) sample();
  }
  sample();
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 1));

(async () => {
  for (let done = 0; done < TOTAL; done += BATCH) {
    batch(Math.min(BATCH, TOTAL - done));
    if (mode === 'steady') await tick();
  }
  sample();
  process.stdout.write(String(peak));
})();
