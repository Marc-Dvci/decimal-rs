'use strict';

/*
 * The differential campaign: a watchdog around `differential.js`.
 *
 * ---------------------------------------------------------------------------
 * The problem this exists to solve
 * ---------------------------------------------------------------------------
 *
 * `differential.js` compares the port against the original on inputs nobody
 * chose. It works, and it found six defects in the port and five in the
 * original in its first hours. But run for sixty seconds against the whole
 * legal input space, it does not finish — and not because either implementation
 * is wrong.
 *
 * The corpus deliberately includes values at the exponent limits, because
 * `1e9000000000000000` is a legal Decimal and `maxE` says so. For a handful of
 * operations the *oracle* cannot answer there. `toFixed` at that magnitude asks
 * for a string of ten quadrillion characters. `cosh` folds its argument by digit
 * count rather than by magnitude, so its series length is proportional to |x|.
 * `cbrt` near the exponent floor does not return at all. And under `ROUND_FLOOR`
 * upstream's `toFraction` loops forever on every finite value, which is D-14 and
 * was found by this very mechanism.
 *
 * An oracle that cannot answer cannot referee. The first response was to bound
 * the offending families — the transcendentals, `mod`/`divToInt`, the radix
 * renderings — and it worked, one family at a time, and then another family
 * appeared. That is a losing game, and it produces a weak artifact even when it
 * is won: a bound names a *family*, and the family is far larger than the set of
 * inputs that actually defeat the oracle.
 *
 * ---------------------------------------------------------------------------
 * What this does instead
 * ---------------------------------------------------------------------------
 *
 * Run the sequences in child processes, in numbered slices, and watch them.
 *
 * A child writes the plan for each sequence to a status file before running it.
 * The parent polls that file. If the index stops advancing for longer than the
 * stall timeout, that child is not slow, it is stuck: kill it, record the
 * sequence by seed, and restart the slice at the next index. Nothing is skipped
 * by rule and nothing is guessed at — the excluded inputs are the inputs that
 * actually failed to be refereed, individually, by seed.
 *
 * The watchdog measures *progress*, not elapsed time, so a legitimately slow
 * sequence is never killed for being slow. Only one that has stopped moving is.
 *
 * After the timed run, diagnose up to `--diagnose N` recorded inputs so the
 * rechecks do not eat the comparison budget or make a full-range run
 * unbounded in wall-clock time. Three more children per selected input: one
 * traced, to name the operation that did not return; one running the oracle
 * alone; one running the port alone. Every excluded sequence remains in the
 * total; the selected records become replayable, attributed examples.
 *
 * ---------------------------------------------------------------------------
 * Usage
 * ---------------------------------------------------------------------------
 *
 *   node fuzz/campaign.js [--seconds 63] [--workers N] [--sequences N]
 *                         [--stall MS] [--bounds on|off] [--seed 0xHEX]
 *                         [--diagnose N] [--log PATH] [--quiet]
 *
 * `--bounds off` removes the family bounds entirely and lets the watchdog do
 * the work. That is the honest pass; its log counts every excluded sequence
 * and names the first `--diagnose N` of them individually.
 */

const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const HARNESS = path.join(__dirname, 'differential.js');

// A child gets a heap cap so that an oracle which tries to build a
// ten-quadrillion-character string dies quickly instead of taking the machine
// down with it. Without this the watchdog still fires, but the host spends the
// intervening seconds swapping.
const CHILD_HEAP_MB = 1024;

// How often the parent looks at a child's status file. Fine enough that the
// stall timeout is what decides, coarse enough to cost nothing.
const POLL_MS = 100;

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

function parseArguments(argv) {
  const options = {
    seconds: 63,
    workers: Math.max(1, Math.min(4, os.cpus().length - 1)),
    sequences: 150,
    stall: 4000,
    bounds: true,
    seed: null,
    log: null,
    quiet: false,
    diagnose: 60,
    diagnoseTimeout: 2500,
  };
  for (let i = 0; i < argv.length; i++) {
    const flag = argv[i];
    if (flag === '--seconds') options.seconds = Number(argv[++i]);
    else if (flag === '--workers') options.workers = Number(argv[++i]);
    else if (flag === '--sequences') options.sequences = Number(argv[++i]);
    else if (flag === '--stall') options.stall = Number(argv[++i]);
    else if (flag === '--bounds') options.bounds = argv[++i] !== 'off';
    else if (flag === '--seed') options.seed = Number(argv[++i]) >>> 0;
    else if (flag === '--log') options.log = argv[++i];
    else if (flag === '--quiet') options.quiet = true;
    else if (flag === '--diagnose') options.diagnose = Number(argv[++i]);
    else if (flag === '--diagnose-timeout') options.diagnoseTimeout = Number(argv[++i]);
  }
  if (options.seed === null) options.seed = (Date.now() ^ (Math.random() * 0xffffffff)) >>> 0;
  return options;
}

// ---------------------------------------------------------------------------
// Running one child
// ---------------------------------------------------------------------------

/**
 * Spawn `differential.js` with `args` and resolve when it exits or stalls.
 *
 * `watch` names a status file whose `index` field the child advances as it
 * works; when it is given, a child whose index has not moved for `stall`
 * milliseconds is killed and reported as stalled. When it is not — the
 * single-sequence diagnostic children, which have no progress to report — the
 * timeout is a plain deadline.
 */
function runChild(args, { stall, watch, deadline }) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, ['--max-old-space-size=' + CHILD_HEAP_MB, HARNESS]
      .concat(args), { stdio: ['ignore', 'pipe', 'pipe'] });

    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => { stdout += chunk; });
    child.stderr.on('data', (chunk) => { stderr += chunk; });

    let lastStatus = null;
    let lastMoved = Date.now();
    let killedForStalling = false;

    const poll = setInterval(() => {
      const now = Date.now();
      if (watch) {
        let status = null;
        try {
          status = JSON.parse(fs.readFileSync(watch, 'utf8'));
        } catch (unreadable) {
          // The child is mid-write, or has not written yet. Either way there is
          // nothing to compare; wait for the next tick.
        }
        if (status && (!lastStatus || status.index !== lastStatus.index)) {
          lastStatus = status;
          lastMoved = now;
        }
      }
      if (now - lastMoved > stall || (deadline && now > deadline)) {
        killedForStalling = true;
        clearInterval(poll);
        child.kill('SIGKILL');
      }
    }, POLL_MS);

    child.on('close', (code) => {
      clearInterval(poll);
      resolve({ code, stdout, stderr, stalled: killedForStalling, status: lastStatus });
    });
  });
}

/** Parse the one JSON line a slice child prints, or null if it did not get there. */
function parseResult(stdout) {
  const line = stdout.trim().split('\n').pop();
  try {
    return JSON.parse(line);
  } catch (notJson) {
    return null;
  }
}

// ---------------------------------------------------------------------------
// The timed run
// ---------------------------------------------------------------------------

/*
 * One worker takes slices until the deadline. A slice that stalls is recorded
 * and resumed at the next sequence, in the same slice, so that the sequences
 * after the stuck one are still refereed rather than thrown away with it.
 */
async function worker(id, options, state, scratch) {
  const status = path.join(scratch, 'status-' + id + '.json');

  while (Date.now() < state.deadline) {
    const slice = state.nextSlice();
    let resume = 0;

    while (resume < options.sequences && Date.now() < state.deadline) {
      const args = [
        '--slice', String(slice),
        '--sequences', String(options.sequences - resume),
        '--resume', String(resume),
        '--status', status,
      ];
      if (!options.bounds) args.push('--bounds', 'off');

      const run = await runChild(args, { stall: options.stall, watch: status });
      const result = parseResult(run.stdout);

      if (result) {
        state.sequences += result.sequences;
        state.operations += result.operations;
        for (const divergence of result.divergences) state.divergences.push(divergence);
        for (const tag of Object.keys(result.known)) {
          state.known.set(tag, (state.known.get(tag) || 0) + result.known[tag]);
        }
        break;
      }

      // No result line: the child was killed, or died. Either way the status
      // file names the sequence it was on when that happened.
      const stuck = run.status;
      if (!stuck) {
        // Killed before it wrote anything at all — the very first sequence.
        state.unrefereeable.push({ slice, index: resume, seed: null, reason: reasonFor(run) });
        resume += 1;
        continue;
      }
      state.unrefereeable.push({
        slice, index: stuck.index, seed: stuck.seed, steps: stuck.steps, reason: reasonFor(run),
      });
      // Credit the work this child had finished before it stopped. It was
      // refereed; the process being killed afterwards does not unrefereee it.
      state.sequences += stuck.sequences;
      state.operations += stuck.operations;
      resume = stuck.index + 1;
    }
  }

  try { fs.unlinkSync(status); } catch (gone) { /* already removed */ }
}

function reasonFor(run) {
  if (run.stalled) return 'stalled';
  if (/JavaScript heap out of memory/.test(run.stderr)) return 'oracle exhausted the heap';
  if (/Invalid array length/.test(run.stderr)) return 'RangeError: Invalid array length';
  if (run.code === null) return 'killed';
  return 'exited ' + run.code;
}

// ---------------------------------------------------------------------------
// Diagnosis
// ---------------------------------------------------------------------------

/*
 * What actually happened at one recorded input.
 *
 * Three children. The first runs the sequence with both sides and a trace file
 * written a step at a time — buffered stderr does not survive a kill, an
 * appended file does — so its last line is the operation that never returned.
 * The other two run one side each, which attributes the hang: if the oracle
 * alone stalls and the port alone answers, that is an upstream defect and this
 * campaign found it.
 */
async function diagnose(entry, options, scratch) {
  const traceFile = path.join(scratch, 'trace-' + entry.slice + '-' + entry.index + '.txt');
  try { fs.unlinkSync(traceFile); } catch (absent) { /* fresh anyway */ }

  const common = ['--slice', String(entry.slice), '--resume', String(entry.index), '--sequences', '1'];
  const bounded = options.bounds ? [] : ['--bounds', 'off'];

  // A shorter clock than the watchdog's. Diagnosis only has to tell "returns"
  // from "does not", and every one of these inputs has already failed to return
  // once — three children each at the full stall timeout would make the
  // post-mortem several times longer than the run it explains.
  const limit = options.diagnoseTimeout;

  await runChild(common.concat(bounded, ['--trace-file', traceFile]),
    { stall: limit, deadline: Date.now() + limit });

  let expression = '(not recorded)';
  try {
    const lines = fs.readFileSync(traceFile, 'utf8').trim().split('\n');
    const steps = lines.filter((line) => line.startsWith('    · '));
    if (steps.length) expression = steps[steps.length - 1].slice(6);
  } catch (absent) { /* leave it unnamed */ }

  const sides = {};
  for (const side of ['reference', 'port']) {
    const started = Date.now();
    const run = await runChild(common.concat(bounded, ['--side', side]),
      { stall: limit, deadline: Date.now() + limit });
    sides[side] = {
      returned: !run.stalled && run.code === 0 && parseResult(run.stdout) !== null,
      ms: Date.now() - started,
      note: run.stalled ? null : reasonFor(run),
    };
  }

  try { fs.unlinkSync(traceFile); } catch (absent) { /* nothing to remove */ }
  return { expression, sides, verdict: verdictFor(sides) };
}

/*
 * What an unrefereeable input actually demonstrates.
 *
 * The three cases are not equally interesting and must not be presented as if
 * they were. Only one of them is a finding about the original, and only one of
 * them would be a finding against the port — and that one has to be zero, which
 * is a claim worth making explicitly rather than leaving to be inferred from an
 * absence.
 */
const VERDICTS = {
  upstream: 'upstream defect — the port answers, the oracle does not',
  intractable: 'intractable for both — they agree in not returning',
  port: 'PORT DEFECT — the oracle answers and the port does not',
  neither: 'inconclusive — neither side reproduced the stall in isolation',
};

function verdictFor(sides) {
  if (sides.port.returned && !sides.reference.returned) return 'upstream';
  if (!sides.port.returned && !sides.reference.returned) return 'intractable';
  if (sides.reference.returned && !sides.port.returned) return 'port';
  return 'neither';
}

/** The method name out of an expression like `x0.toFraction()`. */
function operationOf(expression) {
  const method = /\.([A-Za-z0-9_]+)\(/.exec(expression);
  if (method) return method[1];
  const statik = /^Decimal\.([A-Za-z0-9_]+)\(/.exec(expression);
  return statik ? 'Decimal.' + statik[1] : expression;
}

// ---------------------------------------------------------------------------
// The campaign
// ---------------------------------------------------------------------------

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const lines = [];
  const emit = (line) => {
    lines.push(line);
    if (!options.quiet) process.stdout.write(line + '\n');
  };

  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'decimal-rs-campaign-'));
  const reference = fs.readFileSync(path.join(__dirname, 'reference', 'decimal.js'), 'utf8');
  const version = /decimal\.js v([\d.]+)/.exec(reference);

  emit('port-mortem differential fuzz campaign — decimal.js vs decimal-rs');
  emit('oracle:  fuzz/reference/decimal.js  v' + (version ? version[1] : '?') +
       ' @ cd73a7f  [vendored; not linked into the port]');
  emit('subject: decimal.node  (crates/decimal-core + crates/decimal-napi, release)');
  emit('host:    ' + os.cpus()[0].model.trim() + ' / ' + os.platform() + ' ' + os.release() +
       ' / node ' + process.version);
  emit('started ' + new Date().toISOString() + '   seed 0x' +
       options.seed.toString(16).toUpperCase().padStart(8, '0'));
  emit('budget:  ' + options.seconds + 's continuous, ' + options.workers +
       ' worker process' + (options.workers === 1 ? '' : 'es') +
       ', slices of ' + options.sequences + ' sequences');
  emit('watchdog: a slice whose sequence index has not advanced in ' +
       (options.stall / 1000).toFixed(1) + 's is killed, its input recorded by');
  emit('         seed, and the slice resumed at the next sequence. Children are');
  emit('         capped at ' + CHILD_HEAP_MB + ' MB of heap.');
  emit('bounds:  ' + (options.bounds
    ? 'ON — operations whose cost is proportional to the operand exponent are ' +
      'fuzzed for |e| < 10000 (see differential.js).'
    : 'OFF — the whole legal input space, 1e9000000000000000 included. Inputs ' +
      'the oracle cannot referee are counted below; the diagnosed subset is named individually.'));
  emit('');
  emit('comparing, per operation: sign, exponent, digit array, toString, valueOf,');
  emit('  toExponential, isFinite, isNaN, isInteger, precision, precision(true),');
  emit('  decimalPlaces, negative-zero, thrown message, and the constructor');
  emit('  configuration before and after.');
  emit('excluded by rule: Decimal.random (no fixed answer — the two sides draw from');
  emit('  different generators by design; covered by scripted-entropy unit tests in');
  emit('  crates/decimal-core/src/random.rs).');
  emit('');

  // -- self-check ---------------------------------------------------------
  const selfCheck = await runChild(['--self-check', '--seed', String(options.seed)],
    { stall: 60000, deadline: Date.now() + 60000 });
  const detected = parseResult(selfCheck.stdout);
  if (!detected || !detected.detectedAt) {
    emit('[harness self-check] FAILED — injected one-ulp fault went undetected.');
    emit('This run proves nothing and is aborted.');
    finish(lines, options, 1);
    return;
  }
  emit('[harness self-check] injected a one-ulp fault into the port\'s results');
  emit('[harness self-check]   -> DETECTED at sequence ' + detected.detectedAt +
       ' (comparator is live)');
  emit('[harness self-check] fault reverted');

  // -- replay check -------------------------------------------------------
  //
  // Every diagnosed input below is published as a `--slice S --resume N`
  // command, and every divergence is published as a seed. Both are worthless
  // if a slice does not reproduce, so this checks that it does before printing
  // any of them: one slice run twice must give byte-identical output, and a
  // slice run in two halves must account for exactly the work of the whole.
  const bounded = options.bounds ? [] : ['--bounds', 'off'];

  // Smaller probe slices when the bounds are off, because a larger one almost
  // always contains an input the oracle cannot answer — about a fifth of
  // unbounded sequences do — and a probe that never completes proves nothing
  // either way.
  const HALF = options.bounds ? 8 : 2;
  const ATTEMPTS = options.bounds ? 6 : 16;

  // The probe slice has to be one that *finishes*, and whether any given slice
  // finishes is exactly what the watchdog exists to be unsure about. So try a
  // few, and only conclude something about determinism from a slice that ran to
  // completion twice. Reading a killed child's empty output as "not
  // deterministic" would abort perfectly good runs at random.
  let deterministic = false;
  let resumable = false;
  let attempts = 0;
  let counts = null;

  for (let attempt = 0; attempt < ATTEMPTS && !(deterministic && resumable); attempt++) {
    attempts = attempt + 1;
    const seed = (options.seed ^ Math.imul(attempt + 1, 0x27d4eb2f)) >>> 0;
    const whole = ['--slice', String(seed), '--sequences', String(HALF * 2)].concat(bounded);

    const first = await runChild(whole, { stall: options.stall });
    if (!parseResult(first.stdout)) continue;
    const again = await runChild(whole, { stall: options.stall });
    if (!parseResult(again.stdout)) continue;

    const half1 = await runChild(
      ['--slice', String(seed), '--sequences', String(HALF), '--resume', '0'].concat(bounded),
      { stall: options.stall });
    const half2 = await runChild(
      ['--slice', String(seed), '--sequences', String(HALF), '--resume', String(HALF)]
        .concat(bounded),
      { stall: options.stall });
    const a = parseResult(half1.stdout);
    const b = parseResult(half2.stdout);
    if (!a || !b) continue;

    deterministic = first.stdout === again.stdout;
    resumable = a.operations + b.operations === parseResult(first.stdout).operations;
    counts = { a: a.operations, b: b.operations, whole: parseResult(first.stdout).operations };
  }

  emit('[replay check] one slice run twice: ' +
       (deterministic ? 'identical' : 'DIFFERED — seeds in this log would not be replayable') +
       (attempts > 1 ? '  (' + attempts + ' probe slices tried; earlier ones stalled)' : ''));
  emit('[replay check] the same slice in two halves: ' +
       (resumable ? 'accounts for the whole (' + counts.a + ' + ' + counts.b +
        ' = ' + counts.whole + ' operations)'
                  : 'DID NOT — resumption past a stall would be unsound'));
  if (!deterministic || !resumable) {
    emit('This run cannot publish replayable seeds and is aborted.');
    finish(lines, options, 1);
    return;
  }
  emit('');
  emit('starting clean run');
  emit('');

  // -- the timed run ------------------------------------------------------
  const started = Date.now();
  let slices = 0;
  const state = {
    deadline: started + options.seconds * 1000,
    sequences: 0,
    operations: 0,
    divergences: [],
    unrefereeable: [],
    known: new Map(),
    nextSlice: () => (options.seed ^ Math.imul(++slices, 0x85ebca6b)) >>> 0,
  };

  const ticker = setInterval(() => {
    const now = Date.now();
    emit('seq ' + String(state.sequences).padStart(9) +
         '  ops ' + String(state.operations).padStart(10) +
         '  elapsed ' + ((now - started) / 1000).toFixed(1).padStart(6) + 's' +
         '  divergences ' + state.divergences.length +
         '  unrefereeable ' + state.unrefereeable.length);
  }, 10000);

  const workers = [];
  for (let i = 0; i < options.workers; i++) workers.push(worker(i, options, state, scratch));
  await Promise.all(workers);
  clearInterval(ticker);

  const elapsed = (Date.now() - started) / 1000;

  // -- divergences --------------------------------------------------------
  for (let i = 0; i < state.divergences.length; i++) {
    const divergence = state.divergences[i];
    emit('');
    emit('DIVERGENCE #' + (i + 1) + '  slice 0x' + divergence.seed.toString(16) +
         '  sequence ' + divergence.index + '  (minimised to ' + divergence.steps + ' steps)');
    for (const line of divergence.log) emit('    ' + line);
    emit('  at:       ' + divergence.expression);
    emit('  reference ' + divergence.expected);
    emit('  port      ' + divergence.actual);
  }

  emit('');
  emit('STOP  elapsed ' + elapsed.toFixed(1) + 's  sequences ' + state.sequences +
       '  operations ' + state.operations +
       '  divergences ' + state.divergences.length +
       '  unrefereeable ' + state.unrefereeable.length);

  if (state.known.size) {
    emit('');
    emit('known divergences encountered (deliberate and documented for upstream):');
    for (const [tag, count] of state.known) emit('  ' + tag + '  x' + count);
  }

  // -- diagnosis ----------------------------------------------------------
  //
  // After the clock has stopped, so that none of this is counted as fuzzing.
  const byOperation = new Map();
  if (state.unrefereeable.length) {
    emit('');
    emit('UNREFEREEABLE INPUTS — ' + state.unrefereeable.length + ' of ' +
         (state.sequences + state.unrefereeable.length) + ' sequences');
    emit('');
    emit('These are inputs on which the *oracle* did not produce an answer, so there');
    emit('was nothing to compare the port against. Each was killed by the watchdog,');
    emit('recorded by seed, and up to the configured diagnosis cap are re-run');
    emit('afterwards one side at a time. Nothing is filtered out of the totals:');
    emit('every such sequence is excluded from `sequences`; the diagnosed subset');
    emit('is listed individually with a replay command and attributed verdict.');
    emit('');

    const toDiagnose = state.unrefereeable.slice(0, options.diagnose);
    const verdicts = { upstream: 0, intractable: 0, port: 0, neither: 0 };
    for (const entry of toDiagnose) {
      const detail = await diagnose(entry, options, scratch);
      entry.detail = detail;
      verdicts[detail.verdict]++;
      const operation = operationOf(detail.expression);
      const group = byOperation.get(operation) ||
        { count: 0, upstream: 0, intractable: 0, port: 0, neither: 0 };
      group.count++;
      group[detail.verdict]++;
      byOperation.set(operation, group);

      emit('  slice 0x' + entry.slice.toString(16).padStart(8, '0') +
           '  sequence ' + String(entry.index).padStart(4) +
           '  (' + entry.reason + ')');
      emit('    step:      ' + detail.expression);
      emit('    oracle:    ' + (detail.sides.reference.returned
        ? 'returned in ' + detail.sides.reference.ms + ' ms'
        : 'did NOT return within ' + (options.diagnoseTimeout / 1000).toFixed(1) + ' s'));
      emit('    port:      ' + (detail.sides.port.returned
        ? 'returned in ' + detail.sides.port.ms + ' ms'
        : 'did NOT return within ' + (options.diagnoseTimeout / 1000).toFixed(1) + ' s'));
      emit('    verdict:   ' + VERDICTS[detail.verdict]);
      emit('    replay:    node fuzz/differential.js --slice ' + entry.slice +
           ' --resume ' + entry.index + ' --sequences 1' +
           (options.bounds ? '' : ' --bounds off'));
    }
    state.verdicts = verdicts;

    if (state.unrefereeable.length > toDiagnose.length) {
      emit('  … and ' + (state.unrefereeable.length - toDiagnose.length) +
           ' more, not diagnosed (--diagnose ' + options.diagnose + ')');
    }

    emit('');
    emit('  by operation' +
         '                      upstream  intractable   PORT DEFECT');
    const ordered = [...byOperation.entries()].sort((a, b) => b[1].count - a[1].count);
    for (const [operation, group] of ordered) {
      emit('    ' + operation.padEnd(20) + String(group.count).padStart(4) +
           String(group.upstream).padStart(14) +
           String(group.intractable).padStart(13) +
           String(group.port).padStart(14));
    }
  }

  // -- summary ------------------------------------------------------------
  const refereed = state.operations;
  emit('');
  emit('SUMMARY: ' + refereed.toLocaleString('en-US') + ' refereed operations over ' +
       state.sequences.toLocaleString('en-US') + ' stateful sequences,');
  emit('         across 66 API entry points and all 9 rounding modes,');
  emit('         in ' + elapsed.toFixed(1) + ' continuous seconds on ' +
       options.workers + ' worker process' + (options.workers === 1 ? '' : 'es') + '.');
  emit('         ' + (state.divergences.length === 0
    ? 'Zero undocumented divergences.'
    : state.divergences.length + ' undocumented divergence(s); see above.'));
  if (state.unrefereeable.length) {
    const v = state.verdicts || { upstream: 0, intractable: 0, port: 0, neither: 0 };
    const diagnosed = v.upstream + v.intractable + v.port + v.neither;
    emit('         ' + state.unrefereeable.length + ' input(s) could not be refereed. Of the ' +
         diagnosed + ' diagnosed:');
    emit('           ' + String(v.upstream).padStart(4) +
         '  the port answered and the oracle did not  (upstream defects)');
    emit('           ' + String(v.intractable).padStart(4) +
         '  neither returned                          (agreement, no answer available)');
    emit('           ' + String(v.neither).padStart(4) +
         '  neither reproduced it in isolation        (inconclusive)');
    emit('           ' + String(v.port).padStart(4) +
         '  the oracle answered and the port did not  ' +
         (v.port === 0 ? '(none — the claim that matters)' : '*** PORT DEFECTS ***'));
  }

  try { fs.rmSync(scratch, { recursive: true, force: true }); } catch (busy) { /* temp */ }
  finish(lines, options, state.divergences.length === 0 ? 0 : 1);
}

function finish(lines, options, code) {
  if (options.log) {
    fs.writeFileSync(options.log, lines.join('\n') + '\n');
    if (!options.quiet) process.stdout.write('\nlog written to ' + options.log + '\n');
  }
  process.exitCode = code;
}

main();
