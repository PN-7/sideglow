// SPDX-License-Identifier: MPL-2.0
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { NonceLedger, SendError } from '../nonce.mjs';
import { temporary } from './helpers.mjs';

test('same nonce shares an in-flight send and replays the ID after restart', async (t) => {
  const directory = await temporary(t);
  const ledger = new NonceLedger(directory);
  await ledger.open();
  let calls = 0;
  let release;
  const sent = new Promise((resolve) => { release = resolve; });
  const send = async () => { calls += 1; await sent; return { id: '323456789012345678' }; };
  const first = ledger.run('alice', 'nonce1', 'secret message content', send);
  const second = ledger.run('alice', 'nonce1', 'secret message content', send);
  release();
  assert.equal((await first).id, (await second).id);
  assert.equal(calls, 1);
  assert.doesNotMatch(await readFile(ledger.file, 'utf8'), /secret message content/);
  const restored = new NonceLedger(directory);
  await restored.open();
  assert.deepEqual(await restored.run('alice', 'nonce1', 'secret message content', send), { id: '323456789012345678', replayed: true });
  assert.equal(calls, 1);
  await assert.rejects(restored.run('alice', 'nonce1', 'different content', send), (error) => error.status === 409);
});

test('timeout and persisted pending state never automatically re-send', async (t) => {
  const directory = await temporary(t);
  const ledger = new NonceLedger(directory);
  await ledger.open();
  let calls = 0;
  const send = async () => { calls += 1; throw new SendError(502, 'timeout', { uncertain: true }); };
  await assert.rejects(ledger.run('alice', 'timeout', 'hello', send), (error) => error.uncertain);
  await assert.rejects(ledger.run('alice', 'timeout', 'hello', send), (error) => error.status === 409);
  assert.equal(calls, 1);
  await writeFile(ledger.file, JSON.stringify({ version: 1, entries: [{
    key: 'alice:crash', fingerprint: createHash('sha256').update('hello').digest('hex'),
    created: Date.now(), state: 'pending',
  }] }));
  const restored = new NonceLedger(directory);
  await restored.open();
  await assert.rejects(restored.run('alice', 'crash', 'hello', send), (error) => error.uncertain);
  assert.equal(calls, 1);
});

test('definitely rejected rate limits allow retry, and user nonces are isolated', async (t) => {
  const ledger = new NonceLedger(await temporary(t));
  await ledger.open();
  await assert.rejects(ledger.run('alice', 'same', 'hello', async () => { throw new SendError(429, 'later', { retryable: true }); }), (error) => error.status === 429);
  const alice = await ledger.run('alice', 'same', 'hello', async () => ({ id: '323456789012345678' }));
  const bob = await ledger.run('bob', 'same', 'hello', async () => ({ id: '323456789012345679' }));
  assert.notEqual(alice.id, bob.id);
});
