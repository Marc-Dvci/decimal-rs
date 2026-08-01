#!/usr/bin/env node
'use strict';

/*
 * The soak: does long-running use leak across the FFI boundary?
 *
 * ---------------------------------------------------------------------------
 * What is actually at risk
 * ---------------------------------------------------------------------------
 *
 * `decimal-core` cannot leak. It is safe Rust with no dependencies and no
 * reference cycles; every value it produces is owned and dropped.
 *
 * The boundary can. Each Decimal the addon returns is a JavaScript object with
 * a Rust allocation attached, and the allocation is released by a finaliser
 * that V8 runs when it collects the wrapper. Three things can go wrong there
 * and none of them is visible in a short run: the finaliser might not be
 * registered on some path, a reference might be kept alive by the addon so the
 * wrapper is never collected, or V8 might simply not feel enough pressure to
 * collect — because the JavaScript object is tiny and the memory behind it is
 * not on V8's heap at all.
 *
 * The third is not a bug but it is the one that bites in production, and it is
 * why this samples RSS rather than `heapUsed`. A leak of native memory behind a
 * comfortable JavaScript heap looks like nothing at all from inside V8.
 *
 * ---------------------------------------------------------------------------
 * How it decides
 * ---------------------------------------------------------------------------
 *
 * Ten minutes of sustained mixed operations, RSS sampled every two seconds. The
 * first minute is discarded — startup, JIT warm-up and the first few collections
 * all inflate it, and including them would fit a rising line to a process that
 * is merely starting.
 *
 * The verdict is the slope of a least-squares fit through what remains,
 * expressed in MiB per hour, together with the difference between the last
 * quarter's mean and the first quarter's. A real leak shows in both. A single
 * ratio of last sample to first would be dominated by wherever the sawtooth of
 * garbage collection happened to be at the two ends.
 *
 * The same workload is run against the original as a control, so the number has
 * something to be compared with. A port that grows where the original does not
 * has a problem; both growing the same way is the workload, not the boundary.
 *
 * ---------------------------------------------------------------------------
 * Why this yields to the event loop, and why that is not cheating
 * ---------------------------------------------------------------------------
 *
 * Node runs Node-API finalizers from the event loop, not from the allocation
 * that triggered collection. So a *fully synchronous* burst of a million
 * operations defers every finalizer to the end of it, and RSS climbs for the
 * whole burst no matter how correct the addon is.
 *
 * The first version of this script did not yield, and reported the port
 * growing to 2.2 GiB in sixty seconds against the original's 92 MiB. That
 * looked exactly like a leak. It was two things at once: a real defect (see
 * `Env::wrap` — the addon was asking `napi_wrap` for a reference it never
 * released, which kept every Decimal alive for ever) and this artefact, which
 * remained after the defect was fixed. The distinguishing test is whether the
 * growth survives a turn of the loop, so this takes one every round.
 *
 * That is not a friendlier workload, it is the only one that measures the thing
 * being claimed. A program that never yields is not leaking; it is simply not
 * letting the runtime clean up yet, and it will get all of it back the moment
 * it does. A program that yields and *still* grows has a leak.
 *
 * Usage:  node scripts/soak.js [--minutes 10] [--interval 2] [--json PATH]
 */

const fs = require('fs');
const os = require('os');
const path = require('path');

const Reference = require(path.join(__dirname, '..', 'fuzz', 'reference', 'decimal.js'));
const Port = require(path.join(__dirname, '..', 'decimal.node'));

// ---------------------------------------------------------------------------
// The workload
// ---------------------------------------------------------------------------

/*
 * Deliberately mixed, and deliberately including the paths that allocate most:
 * long operands, the transcendentals, the string renderings, and the statics
 * that build intermediate values the caller never sees. A soak on `plus` alone
 * would exercise one allocation path out of a dozen.
 */
function workload(D, rounds) {
  const a = new D('123456789.123456789012345678901234567890');
  const b = new D('0.000000000987654321098765432109876543');
  const c = new D('7.3890560989306502272304274605750078');

  for (let i = 0; i < rounds; i++) {
    const x = a.plus(b).times(c).div(a.minus(b));
    x.sqrt().toString();
    x.toFixed(30);
    x.toExponential(20);
    a.ln();
    b.exp();
    c.pow(3);
    a.mod(c);
    D.hypot(a, b, c);
    D.sum(a, b, c, x);
    D.max(a, b, c);
    new D(x.toString()).toFraction(100000);
    a.sin();
    c.cosh();
    x.toBinary(40);
  }
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/** Least-squares slope of `ys` against `xs`, in units of y per unit of x. */
function slope(xs, ys) {
  const n = xs.length;
  const meanX = xs.reduce((s, v) => s + v, 0) / n;
  const meanY = ys.reduce((s, v) => s + v, 0) / n;
  let numerator = 0;
  let denominator = 0;
  for (let i = 0; i < n; i++) {
    numerator += (xs[i] - meanX) * (ys[i] - meanY);
    denominator += (xs[i] - meanX) ** 2;
  }
  return denominator === 0 ? 0 : numerator / denominator;
}

function mean(values) {
  return values.reduce((s, v) => s + v, 0) / values.length;
}

const MIB = 1024 * 1024;

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

function parseArguments(argv) {
  const options = { minutes: 10, interval: 2, json: null, warmup: 60 };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--minutes') options.minutes = Number(argv[++i]);
    else if (argv[i] === '--interval') options.interval = Number(argv[++i]);
    else if (argv[i] === '--json') options.json = argv[++i];
    else if (argv[i] === '--warmup') options.warmup = Number(argv[++i]);
  }
  return options;
}

/** Give the event loop a turn, so that pending finalizers can run. */
function tick() {
  return new Promise((resolve) => setTimeout(resolve, 1));
}

/*
 * Run one implementation for the configured duration, sampling RSS every
 * `interval` and yielding between batches.
 *
 * The batch is small — 25 rounds, a few milliseconds — so the loop gets a turn
 * often enough that finalizers keep pace with allocation, which is how a
 * server or a spreadsheet actually uses this library. A batch large enough to
 * outrun the finalizers would measure the deferral described above rather than
 * the footprint.
 */
async function soak(D, label, options, emit) {
  const started = Date.now();
  const deadline = started + options.minutes * 60 * 1000;
  const samples = [];
  let nextSample = started;
  let rounds = 0;

  emit('\n' + label);
  emit('  elapsed      RSS     rounds');

  while (Date.now() < deadline) {
    workload(D, 25);
    rounds += 25;
    await tick();

    const now = Date.now();
    if (now >= nextSample) {
      const rss = process.memoryUsage().rss;
      const at = (now - started) / 1000;
      samples.push({ at, rss, rounds });
      if (samples.length % 15 === 1) {
        emit('  ' + at.toFixed(0).padStart(6) + 's  ' +
             (rss / MIB).toFixed(1).padStart(7) + ' MiB  ' +
             String(rounds).padStart(9));
      }
      nextSample = now + options.interval * 1000;
    }
  }

  // Discard the warm-up window before fitting anything to the rest.
  const kept = samples.filter((s) => s.at >= options.warmup);
  const xs = kept.map((s) => s.at / 3600);
  const ys = kept.map((s) => s.rss / MIB);
  const quarter = Math.max(1, Math.floor(kept.length / 4));

  const result = {
    label,
    rounds,
    seconds: (Date.now() - started) / 1000,
    samples: samples.length,
    kept: kept.length,
    peakMiB: Math.max(...samples.map((s) => s.rss)) / MIB,
    firstQuarterMiB: mean(ys.slice(0, quarter)),
    lastQuarterMiB: mean(ys.slice(-quarter)),
    slopeMiBPerHour: slope(xs, ys),
  };
  result.driftMiB = result.lastQuarterMiB - result.firstQuarterMiB;
  return result;
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const lines = [];
  const emit = (line) => {
    lines.push(line);
    process.stdout.write(line + '\n');
  };

  emit('decimal-rs soak — RSS under sustained mixed operations');
  emit('host:    ' + os.cpus()[0].model.trim() + ' / ' + os.platform() + ' ' +
       os.release() + ' / node ' + process.version);
  emit('plan:    ' + options.minutes + ' minutes per implementation, RSS sampled every ' +
       options.interval + 's,');
  emit('         the first ' + options.warmup + 's discarded before fitting.');
  emit('         Both are run in this one process, the port second, yielding to');
  emit('         the event loop between batches so that finalizers can run.');

  const results = [
    await soak(Reference, 'decimal.js  (control)', options, emit),
    await soak(Port, 'decimal-rs  (subject)', options, emit),
  ];

  emit('\nverdict');
  emit('  implementation        rounds     peak RSS   first¼    last¼     drift    slope');
  emit('  ' + '-'.repeat(78));
  for (const r of results) {
    emit('  ' + r.label.padEnd(22) +
         String(r.rounds).padStart(8) +
         (r.peakMiB.toFixed(1) + ' MiB').padStart(13) +
         (r.firstQuarterMiB.toFixed(1)).padStart(9) +
         (r.lastQuarterMiB.toFixed(1)).padStart(9) +
         (r.driftMiB >= 0 ? '+' : '') + r.driftMiB.toFixed(1).padStart(8) +
         (r.slopeMiBPerHour >= 0 ? '  +' : '  ') + r.slopeMiBPerHour.toFixed(1) + ' MiB/h');
  }

  const port = results[1];
  const control = results[0];
  emit('');
  emit('  A leak would show as a positive slope *and* a positive drift, in the');
  emit('  subject and not in the control. Sawtooth from garbage collection moves');
  emit('  the two ends of the window around and shows in neither.');
  emit('');
  emit('  decimal-rs drifted ' + port.driftMiB.toFixed(1) + ' MiB with a slope of ' +
       port.slopeMiBPerHour.toFixed(1) + ' MiB/h; the control');
  emit('  drifted ' + control.driftMiB.toFixed(1) + ' MiB at ' +
       control.slopeMiBPerHour.toFixed(1) + ' MiB/h over the same workload.');

  if (options.json) {
    fs.writeFileSync(options.json, JSON.stringify({
      generated: new Date().toISOString(),
      host: os.cpus()[0].model.trim() + ' / ' + os.platform() + ' ' + os.release(),
      node: process.version,
      options,
      results,
    }, null, 2) + '\n');
    emit('\nmachine-readable results written to ' + options.json);
  }
}

main();
