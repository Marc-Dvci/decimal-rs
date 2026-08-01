'use strict';

/*
 * Benchmarks: decimal-rs against the original decimal.js.
 *
 * ---------------------------------------------------------------------------
 * The protocol, and why it is this one
 * ---------------------------------------------------------------------------
 *
 * Both implementations are measured **in the same process, in the same run,
 * interleaved A/B/A/B**. Not all-A then all-B. On a desktop CPU with boost
 * clocks the machine is measurably faster in the first thirty seconds of a run
 * than in the last, and a report that measures one implementation early and
 * the other late will attribute that drift to the code. Interleaving cancels
 * it; ordering does not.
 *
 * Every scenario discards warm-up iterations before recording, and the count
 * is stated in the output. V8 needs to see a function run before it compiles
 * it properly, so measuring cold JavaScript against warm Rust is the single
 * easiest way to produce a flattering number that is not true.
 *
 * Each scenario is repeated `REPETITIONS` times. The figure reported is the
 * **median** of those repetitions, with the interquartile range beside it. A
 * single number with no dispersion is not a measurement.
 *
 * The input corpus is fixed and generated from a constant seed, so the numbers
 * are reproducible rather than merely repeatable.
 *
 * ---------------------------------------------------------------------------
 * What is deliberately included
 * ---------------------------------------------------------------------------
 *
 * The rows where the port loses. There are two of them and they are the most
 * informative rows in the table: a single small operation called one at a time
 * from JavaScript has to cross the Node-API boundary, and that crossing costs
 * more than the arithmetic it is wrapping. Removing those rows would make the
 * report better-looking and worthless.
 *
 * Usage:  node bench/run.js [--quick] [--json PATH]
 */

const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');

const Reference = require('../fuzz/reference/decimal.js');
const Port = require('../decimal.node');

const REPETITIONS = process.argv.includes('--quick') ? 5 : 11;
const WARMUP_BATCHES = 3;

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

function median(values) {
  const sorted = values.slice().sort((a, b) => a - b);
  const middle = sorted.length >> 1;
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function quantile(values, q) {
  const sorted = values.slice().sort((a, b) => a - b);
  const position = (sorted.length - 1) * q;
  const lower = Math.floor(position);
  const upper = Math.ceil(position);
  if (lower === upper) return sorted[lower];
  return sorted[lower] + (sorted[upper] - sorted[lower]) * (position - lower);
}

function iqr(values) {
  return quantile(values, 0.75) - quantile(values, 0.25);
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

/* A fixed generator, so the corpus is identical on every machine and run. */
function corpus(digits, count, seed) {
  let state = seed >>> 0;
  const next = () => {
    state = (state + 0x9e3779b9) >>> 0;
    let z = state;
    z = Math.imul(z ^ (z >>> 16), 0x21f0aaad) >>> 0;
    z = Math.imul(z ^ (z >>> 15), 0x735a2d97) >>> 0;
    return (z ^ (z >>> 15)) >>> 0;
  };
  const out = [];
  for (let i = 0; i < count; i++) {
    let s = String(1 + (next() % 9));
    for (let j = 1; j < digits; j++) s += String(next() % 10);
    out.push(s + '.' + String(next() % 1000000) + 'e' + ((next() % 40) - 20));
  }
  return out;
}

const SMALL = corpus(20, 64, 0xBEEF);
const MEDIUM = corpus(200, 32, 0xCAFE);
const LARGE = corpus(1000, 16, 0xF00D);
const HUGE = corpus(10000, 4, 0xD00D);

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

/*
 * One scenario: a name, a batch size, and a factory that — given a Decimal
 * constructor — returns a closure performing `batch` operations.
 *
 * The factory shape matters. Building the operands is done once, outside the
 * timed closure, so what is measured is the operation and not the construction
 * of its inputs, except in the scenarios that are explicitly about
 * construction.
 */
const scenarios = [];

function scenario(name, batch, factory, note) {
  scenarios.push({ name, batch, factory, note: note || '' });
}

/** Nanoseconds per operation, median of `REPETITIONS`, with the IQR. */
function measure(scenarioEntry) {
  const runners = {
    reference: scenarioEntry.factory(Reference),
    port: scenarioEntry.factory(Port),
  };

  for (let w = 0; w < WARMUP_BATCHES; w++) {
    runners.reference();
    runners.port();
  }

  const samples = { reference: [], port: [] };
  for (let r = 0; r < REPETITIONS; r++) {
    // Interleaved, and the order alternates so that neither side is
    // systematically the one that pays for a cache miss the other caused.
    const order = r % 2 ? ['port', 'reference'] : ['reference', 'port'];
    for (const which of order) {
      const started = process.hrtime.bigint();
      runners[which]();
      const elapsed = Number(process.hrtime.bigint() - started);
      samples[which].push(elapsed / scenarioEntry.batch);
    }
  }

  return {
    name: scenarioEntry.name,
    note: scenarioEntry.note,
    batch: scenarioEntry.batch,
    reference: { ns: median(samples.reference), iqr: iqr(samples.reference) },
    port: { ns: median(samples.port), iqr: iqr(samples.port) },
    ratio: median(samples.reference) / median(samples.port),
  };
}

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

function withPrecision(D, precision, body) {
  const saved = D.precision;
  D.precision = precision;
  const out = body();
  D.precision = saved;
  return out;
}

/* Construction, which is parsing. */
scenario('parse, 20 digits', SMALL.length, (D) => () => {
  for (let i = 0; i < SMALL.length; i++) new D(SMALL[i]);
});
scenario('parse, 200 digits', MEDIUM.length, (D) => () => {
  for (let i = 0; i < MEDIUM.length; i++) new D(MEDIUM[i]);
});
scenario('parse, 1000 digits', LARGE.length, (D) => () => {
  for (let i = 0; i < LARGE.length; i++) new D(LARGE[i]);
});

/* Rendering. */
scenario('toString, 20 digits', SMALL.length, (D) => {
  const values = SMALL.map((s) => new D(s));
  return () => { for (let i = 0; i < values.length; i++) values[i].toString(); };
});
scenario('toString, 1000 digits', LARGE.length, (D) => {
  const values = LARGE.map((s) => new D(s));
  return () => { for (let i = 0; i < values.length; i++) values[i].toString(); };
});

/* Arithmetic, at the default precision and at 200. */
for (const precision of [20, 200]) {
  const source = precision === 20 ? SMALL : MEDIUM;
  for (const [label, method] of [['add', 'plus'], ['multiply', 'times'], ['divide', 'div']]) {
    scenario(`${label}, precision ${precision}`, source.length - 1, (D) => {
      const values = source.map((s) => new D(s));
      return () => withPrecision(D, precision, () => {
        for (let i = 0; i < values.length - 1; i++) values[i][method](values[i + 1]);
      });
    });
  }
}

/* Roots, powers, and one transcendental. */
scenario('sqrt, precision 20', SMALL.length, (D) => {
  const values = SMALL.map((s) => new D(s).abs());
  return () => withPrecision(D, 20, () => {
    for (let i = 0; i < values.length; i++) values[i].sqrt();
  });
});
scenario('sqrt, precision 200', MEDIUM.length, (D) => {
  const values = MEDIUM.map((s) => new D(s).abs());
  return () => withPrecision(D, 200, () => {
    for (let i = 0; i < values.length; i++) values[i].sqrt();
  });
});
scenario('pow (integer exponent)', 16, (D) => {
  const values = SMALL.slice(0, 16).map((s) => new D(s));
  const exponent = new D(37);
  return () => withPrecision(D, 20, () => {
    for (let i = 0; i < values.length; i++) values[i].pow(exponent);
  });
});
scenario('ln, precision 20', 16, (D) => {
  const values = SMALL.slice(0, 16).map((s) => new D(s).abs());
  return () => withPrecision(D, 20, () => {
    for (let i = 0; i < values.length; i++) values[i].ln();
  });
});
scenario('exp, precision 20', 16, (D) => {
  const values = SMALL.slice(0, 16).map((s) => new D(s).abs().div(new D('1e12')));
  return () => withPrecision(D, 20, () => {
    for (let i = 0; i < values.length; i++) values[i].exp();
  });
});

/* Scaling with operand size: the graph that says whether limb arithmetic is
 * actually better or merely differently constant-factored.
 *
 * The sizes between 10 and 100 are here to locate the crossover rather than to
 * fill the table. Below it the port loses to its own boundary crossing and the
 * arithmetic underneath is irrelevant; above it the arithmetic is all that
 * matters. Knowing *where* is the difference between "this port is faster" and
 * a statement someone can act on. The port's cost is nearly flat across the
 * lower half of this range, which is what identifies the loss as fixed
 * overhead rather than slow multiplication. */
for (const [label, source] of [
  ['10 digits', corpus(10, 32, 0x1010)],
  ['30 digits', corpus(30, 32, 0x1030)],
  ['50 digits', corpus(50, 32, 0x1050)],
  ['60 digits', corpus(60, 32, 0x1060)],
  ['100 digits', corpus(100, 32, 0x1100)],
  ['1000 digits', LARGE],
  ['10000 digits', HUGE],
]) {
  scenario(`multiply, ${label}`, source.length - 1, (D) => {
    const values = source.map((s) => new D(s));
    return () => withPrecision(D, Math.max(20, source[0].length), () => {
      for (let i = 0; i < values.length - 1; i++) values[i].times(values[i + 1]);
    });
  });
}

// ---------------------------------------------------------------------------
// Latency distribution — the row where the port loses
// ---------------------------------------------------------------------------

/*
 * Per-call latency for one small operation, sampled individually.
 *
 * `process.hrtime.bigint()` costs something itself — measured below and
 * reported, not subtracted, because subtracting a noisy baseline from a noisy
 * measurement produces a number that looks more precise than it is. The
 * baseline is printed so the reader can do the subtraction and see how much of
 * the figure is instrument.
 */
function latencyDistribution(D, samples) {
  const a = new D('12345.6789');
  const b = new D('98765.4321');
  const timings = new Float64Array(samples);
  for (let i = 0; i < samples; i++) {
    const started = process.hrtime.bigint();
    a.plus(b);
    timings[i] = Number(process.hrtime.bigint() - started);
  }
  return Array.from(timings);
}

function clockOverhead(samples) {
  const timings = [];
  for (let i = 0; i < samples; i++) {
    const started = process.hrtime.bigint();
    timings.push(Number(process.hrtime.bigint() - started));
  }
  return timings;
}

// ---------------------------------------------------------------------------
// Startup, memory, artifact size
// ---------------------------------------------------------------------------

/*
 * Startup has to be a fresh process: measuring `require()` twice in one
 * process measures the module cache.
 */
function startup(which, repetitions) {
  const script = which === 'port'
    ? "const t=process.hrtime.bigint();const D=require('./decimal.node');new D(1);" +
      "process.stdout.write(String(Number(process.hrtime.bigint()-t)));"
    : "const t=process.hrtime.bigint();const D=require('./fuzz/reference/decimal.js');new D(1);" +
      "process.stdout.write(String(Number(process.hrtime.bigint()-t)));";
  const samples = [];
  for (let i = 0; i < repetitions; i++) {
    const out = execFileSync(process.execPath, ['-e', script], {
      cwd: path.join(__dirname, '..'),
      encoding: 'utf8',
    });
    samples.push(Number(out));
  }
  return samples;
}

/*
 * Peak RSS over a fixed workload, in a fresh process, in each of two modes.
 *
 * `bench/rss-probe.js` explains why there are two. Briefly: Node runs
 * Node-API finalizers from the event loop, so a synchronous burst holds
 * everything the addon allocated until it ends, while the same work done in
 * batches does not. The first is what a tight loop costs and the second is the
 * resident footprint; for the pure-JavaScript original they are nearly equal,
 * and the gap between them is the price of the boundary.
 */
function peakRss(which, mode) {
  return Number(execFileSync(
    process.execPath,
    [path.join(__dirname, 'rss-probe.js'), which, mode],
    { cwd: path.join(__dirname, '..'), encoding: 'utf8' },
  ));
}

function artifactSizes() {
  const root = path.join(__dirname, '..');
  const sourceBytes = fs.statSync(path.join(root, 'fuzz', 'reference', 'decimal.js')).size;
  let binaryBytes = null;
  for (const name of ['decimal.node']) {
    const candidate = path.join(root, name);
    if (fs.existsSync(candidate)) binaryBytes = fs.statSync(candidate).size;
  }
  return { sourceBytes, binaryBytes };
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

function formatNs(ns) {
  if (ns >= 1e6) return (ns / 1e6).toFixed(2) + ' ms';
  if (ns >= 1e3) return (ns / 1e3).toFixed(2) + ' µs';
  return ns.toFixed(1) + ' ns';
}

function formatRatio(ratio) {
  return ratio >= 1
    ? ratio.toFixed(2) + '× faster'
    : (1 / ratio).toFixed(2) + '× SLOWER';
}

/*
 * A verdict, or the honest refusal to give one.
 *
 * A ratio computed from two medians says nothing on its own. Where the two
 * medians are closer together than the runs' own spread, the ordering between
 * them is a property of this afternoon and not of the two implementations —
 * printing "1.07× faster" there would be a claim the data does not support, and
 * would be found out by anyone who ran it twice.
 *
 * The test is deliberately crude and deliberately conservative: if the gap
 * between the medians is smaller than their mean interquartile range, there is
 * no result. Most of the small-operation rows fail it, which is itself the
 * finding — at twenty digits these two implementations are the same speed, and
 * the port's advantage is entirely in operand size.
 */
function verdict(result) {
  const spread = (result.reference.iqr + result.port.iqr) / 2;
  if (Math.abs(result.reference.ns - result.port.ns) < spread) {
    return 'no measurable difference';
  }
  return formatRatio(result.ratio);
}

function main() {
  const started = new Date();
  const results = [];

  process.stdout.write('decimal-rs benchmarks\n');
  process.stdout.write('host:    ' + os.cpus()[0].model.trim() + ' / ' + os.platform() + ' ' +
    os.release() + ' / node ' + process.version + '\n');
  process.stdout.write('protocol: interleaved A/B in one process, ' + WARMUP_BATCHES +
    ' warm-up batches discarded, ' + REPETITIONS + ' repetitions, median (IQR).\n');
  process.stdout.write('boost clocks: not disabled (Windows desktop) — see bench/methodology.md\n\n');

  const nameWidth = Math.max(...scenarios.map((s) => s.name.length)) + 2;
  process.stdout.write(
    'operation'.padEnd(nameWidth) + 'decimal.js'.padStart(14) +
    'decimal-rs'.padStart(14) + '   verdict\n');
  process.stdout.write('-'.repeat(nameWidth + 28 + 20) + '\n');

  for (const entry of scenarios) {
    const result = measure(entry);
    results.push(result);
    process.stdout.write(
      result.name.padEnd(nameWidth) +
      formatNs(result.reference.ns).padStart(14) +
      formatNs(result.port.ns).padStart(14) +
      '   ' + verdict(result) + '\n');
  }

  // -- latency ------------------------------------------------------------
  process.stdout.write('\nper-call latency, one small `plus` at a time (100,000 samples)\n');
  const samples = 100000;
  const overhead = clockOverhead(samples);
  const latency = {
    clock: overhead,
    reference: latencyDistribution(Reference, samples),
    port: latencyDistribution(Port, samples),
  };
  const latencyRows = [];
  for (const which of ['clock', 'reference', 'port']) {
    const row = {
      which,
      p50: quantile(latency[which], 0.50),
      p90: quantile(latency[which], 0.90),
      p99: quantile(latency[which], 0.99),
      max: Math.max.apply(null, latency[which]),
    };
    latencyRows.push(row);
    const label = which === 'clock' ? 'hrtime itself (instrument)'
      : which === 'reference' ? 'decimal.js' : 'decimal-rs';
    process.stdout.write('  ' + label.padEnd(28) +
      'p50 ' + formatNs(row.p50).padStart(9) +
      '  p90 ' + formatNs(row.p90).padStart(9) +
      '  p99 ' + formatNs(row.p99).padStart(9) +
      '  max ' + formatNs(row.max).padStart(10) + '\n');
  }

  // -- startup ------------------------------------------------------------
  process.stdout.write('\nstartup: fresh process, require() plus one construction (9 runs)\n');
  const startupSamples = {
    reference: startup('reference', 9),
    port: startup('port', 9),
  };
  for (const which of ['reference', 'port']) {
    process.stdout.write('  ' + (which === 'reference' ? 'decimal.js' : 'decimal-rs').padEnd(28) +
      'median ' + formatNs(median(startupSamples[which])).padStart(9) +
      '  IQR ' + formatNs(iqr(startupSamples[which])).padStart(9) + '\n');
  }

  // -- memory -------------------------------------------------------------
  process.stdout.write('\npeak RSS over 200,000 mixed operations at precision 200\n');
  process.stdout.write('                              synchronous     yielding\n');
  const rss = {
    reference: { burst: peakRss('reference', 'burst'), steady: peakRss('reference', 'steady') },
    port: { burst: peakRss('port', 'burst'), steady: peakRss('port', 'steady') },
  };
  for (const which of ['reference', 'port']) {
    process.stdout.write('  ' + (which === 'reference' ? 'decimal.js' : 'decimal-rs').padEnd(28) +
      (rss[which].burst / 1048576).toFixed(1).padStart(9) + ' MiB' +
      (rss[which].steady / 1048576).toFixed(1).padStart(9) + ' MiB\n');
  }
  process.stdout.write(
    '  Node runs Node-API finalizers from the event loop, so a synchronous burst\n' +
    '  holds everything the addon allocated until it ends. The right-hand column\n' +
    '  is the resident footprint; the gap is the cost of that deferral.\n');

  // -- artifact -----------------------------------------------------------
  const sizes = artifactSizes();
  process.stdout.write('\nartifact size\n');
  process.stdout.write('  decimal.js (source)'.padEnd(30) +
    (sizes.sourceBytes / 1024).toFixed(1).padStart(9) + ' KiB\n');
  if (sizes.binaryBytes !== null) {
    process.stdout.write('  decimal.node (compiled)'.padEnd(30) +
      (sizes.binaryBytes / 1024).toFixed(1).padStart(9) + ' KiB   ' +
      formatRatio(sizes.sourceBytes / sizes.binaryBytes).replace('faster', 'smaller')
        .replace('SLOWER', 'LARGER') + '\n');
  }

  const report = {
    generated: started.toISOString(),
    host: {
      cpu: os.cpus()[0].model.trim(),
      platform: os.platform(),
      release: os.release(),
      node: process.version,
      boostClocksDisabled: false,
    },
    protocol: {
      interleaved: true,
      warmupBatches: WARMUP_BATCHES,
      repetitions: REPETITIONS,
      statistic: 'median of repetitions, IQR reported',
    },
    // The verdict travels with the numbers. A consumer reading only the ratio
    // would reach conclusions the dispersion does not support.
    throughput: results.map((r) => Object.assign({}, r, { verdict: verdict(r) })),
    latencyNs: latencyRows,
    startupNs: {
      reference: { median: median(startupSamples.reference), iqr: iqr(startupSamples.reference) },
      port: { median: median(startupSamples.port), iqr: iqr(startupSamples.port) },
    },
    peakRssBytes: rss,
    artifactBytes: sizes,
  };

  const target = path.join(__dirname, 'results.json');
  fs.writeFileSync(target, JSON.stringify(report, null, 2) + '\n');
  process.stdout.write('\nmachine-readable results written to ' + target + '\n');
}

main();
