// SPDX-License-Identifier: MPL-2.0
import test from 'node:test';
import assert from 'node:assert/strict';
import { createRelayServer } from '../server.mjs';
import { EventStore, normalizeMessage } from '../store.mjs';
import { NonceLedger } from '../nonce.mjs';
import { config, CHANNEL, OTHER, KEY, temporary, message } from './helpers.mjs';

async function fixture(t, relayConfig = config()) {
  const store = new EventStore();
  const ledger = new NonceLedger(await temporary(t));
  await ledger.open();
  const calls = [];
  const server = await createRelayServer({ config: relayConfig, store, ledger, clock: () => 10000,
    send: async (name, content) => { calls.push({ name, content }); return { id: String(323456789012345678n + BigInt(calls.length)) }; },
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); return new Promise((resolve) => server.close(resolve)); });
  const base = `http://127.0.0.1:${server.address().port}`;
  const headers = { Authorization: `Bearer ${KEY}`, 'X-Discord-Channel': CHANNEL };
  return { store, calls, base, headers };
}

test('HTTP authenticates both directions, restricts channel, and exposes only minimal health', async (t) => {
  const { base, headers } = await fixture(t);
  assert.deepEqual(await (await fetch(`${base}/health`)).json(), { ok: true });
  assert.equal((await fetch(`${base}/v1/events?after=0`)).status, 401);
  assert.equal((await fetch(`${base}/v1/events?after=0`, { headers: { ...headers, 'X-Discord-Channel': OTHER } })).status, 403);
  assert.equal((await fetch(`${base}/v1/events?after=0`, { headers: { ...headers, Origin: 'https://example.invalid' } })).status, 403);
  assert.equal((await fetch(`${base}/v1/events?after=NaN`, { headers })).status, 400);
  const response = await fetch(`${base}/v1/events?after=0`, { headers });
  assert.equal(response.headers.get('cache-control'), 'no-store');
  assert.equal(response.headers.get('access-control-allow-origin'), null);
  assert.deepEqual((await response.json()).events, []);
});

test('HTTP send uses server display name, deduplicates nonce, and rate-limits only new sends', async (t) => {
  const { base, headers, calls } = await fixture(t);
  const send = (nonce, content = '안녕하세요') => fetch(`${base}/v1/messages`, {
    method: 'POST', headers: { ...headers, 'Content-Type': 'application/json' },
    body: JSON.stringify({ nonce, content, displayName: 'forged name', channelId: OTHER }),
  });
  const first = await send('one');
  assert.equal(first.status, 201);
  const id = (await first.json()).id;
  const retry = await send('one');
  assert.equal(retry.status, 200);
  assert.equal((await retry.json()).id, id);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].name, '앨리스 · Overlay');
  assert.equal((await send('one', 'changed')).status, 409);
  for (let index = 2; index <= 5; index += 1) assert.equal((await send(`message-${index}`)).status, 201);
  assert.equal((await send('six')).status, 429);
  assert.equal((await send('one')).status, 200);
  assert.equal(calls.length, 5);
});

test('long poll wakes for a new message and later reports deletion', async (t) => {
  const { base, headers, store } = await fixture(t);
  const snapshot = await (await fetch(`${base}/v1/events?after=0`, { headers })).json();
  const pending = fetch(`${base}/v1/events?after=${snapshot.cursor}&wait=25000`, { headers }).then((response) => response.json());
  store.upsert(normalizeMessage(message()));
  const live = await pending;
  assert.equal(live.reset, false);
  assert.equal(live.events[0].message.content, '원문');
  store.delete('323456789012345678');
  const deleted = await (await fetch(`${base}/v1/events?after=${live.cursor}&wait=0`, { headers })).json();
  assert.deepEqual(deleted.events, [{ op: 'delete', id: '323456789012345678' }]);
});

test('personal romaji policy preserves the server name and text and is nonce-bound', async (t) => {
  const { base, headers, calls } = await fixture(t);
  const send = (nonce, enabled) => fetch(`${base}/v1/messages`, {
    method: 'POST', headers: { ...headers, 'Content-Type': 'application/json' },
    body: JSON.stringify({ nonce, content: 'arigatou', romaji_japanese_input: enabled, displayName: 'forged name' }),
  });
  assert.equal((await send('romaji', true)).status, 201);
  assert.deepEqual(calls[0], { name: 'Sideglow [romaji] · 앨리스 · Overlay', content: 'arigatou' });
  assert.equal((await send('romaji', true)).status, 200);
  assert.equal((await send('romaji', false)).status, 409);
  assert.equal((await send('plain', false)).status, 201);
  assert.deepEqual(calls[1], { name: '앨리스 · Overlay', content: 'arigatou' });
  for (const invalid of ['true', 1, null, {}]) {
    assert.equal((await send('invalid', invalid)).status, 400);
  }
  assert.equal(calls.length, 2);
});

test('an unchecked sender nickname cannot act as the reserved romaji label', async (t) => {
  const relayConfig = config();
  relayConfig.users[0].displayName = 'Sideglow [romaji] · ☆';
  const { base, headers, calls } = await fixture(t, relayConfig);
  const response = await fetch(`${base}/v1/messages`, {
    method: 'POST', headers: { ...headers, 'Content-Type': 'application/json' },
    body: JSON.stringify({ nonce: 'reserved-name', content: 'hello', romaji_japanese_input: false }),
  });
  assert.equal(response.status, 201);
  assert.deepEqual(calls, [{ name: 'Sideglow · Sideglow [romaji] · ☆', content: 'hello' }]);
});
