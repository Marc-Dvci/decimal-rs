#!/usr/bin/env node
'use strict';

/* Regression tests for behavior owned by the Node-API adapter, not decimal.js. */

const assert = require('assert/strict');
const Decimal = require('../decimal.node');

async function collectClones() {
  if (typeof global.gc !== 'function') {
    throw new Error('adapter-regression.js must run with node --expose-gc');
  }

  let finalized = 0;
  const registry = new FinalizationRegistry(() => { finalized += 1; });

  for (let i = 0; i < 1_000; i += 1) {
    const clone = Decimal.clone();
    registry.register(clone, i);
  }

  // Finalizers are deliberately asynchronous. The threshold distinguishes
  // collection from the old permanent napi_ref cycle without depending on an
  // allocator returning every page to the OS.
  for (let turn = 0; turn < 40 && finalized < 500; turn += 1) {
    global.gc();
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.ok(finalized >= 500, `only ${finalized}/1000 discarded clones were finalized`);
}

async function main() {
  const Clone = Decimal.clone({ precision: 7 });
  const direct = Decimal('1.25');
  const cloned = Clone('2.5');

  assert.ok(direct instanceof Decimal);
  assert.ok(cloned instanceof Clone);
  assert.equal(Object.hasOwn(direct, 'constructor'), true);
  assert.equal(direct.constructor, Decimal);
  assert.equal(cloned.constructor, Clone);
  assert.equal(Decimal.prototype, Clone.prototype);
  assert.equal(cloned.plus('0.125').toString(), '2.625');

  // A getter re-enters config while the outer config call is between fields.
  // This used to create overlapping &mut references in safe-looking Rust.
  Clone.config({
    get precision() {
      Clone.config({ rounding: Clone.ROUND_DOWN });
      return 11;
    },
    rounding: Clone.ROUND_HALF_EVEN,
  });
  assert.equal(Clone.precision, 11);
  assert.equal(Clone.rounding, Clone.ROUND_HALF_EVEN);

  // Rendering an invalid argument can re-enter too. decimal.js still rejects
  // the object after stringifying it; the important point is that no native
  // state reference spans the user hook.
  const reentrantOperand = {
    toString() {
      Clone.config({ precision: 9 });
      return '1.125';
    },
  };
  assert.throws(() => Clone(reentrantOperand), /Invalid argument: 1\.125/);
  assert.equal(Clone.precision, 9);

  // Rejected clones are also allowed to leave no permanent native root.
  for (let i = 0; i < 100; i += 1) {
    assert.throws(() => Decimal.clone(null), /Object expected/);
  }

  await collectClones();
  process.stdout.write('adapter regression: constructor, shared prototype, re-entry, lifecycle — passed\n');
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
