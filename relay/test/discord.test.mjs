// SPDX-License-Identifier: MPL-2.0
import test from 'node:test';
import assert from 'node:assert/strict';
import { DiscordRest, DiscordBridge } from '../discord.mjs';
import { EventStore } from '../store.mjs';
import { config, CHANNEL, OTHER, message, until } from './helpers.mjs';

test('webhook sends use wait=true, no bot credentials, no allowed mentions, and server display name', async () => {
  const calls = [];
  const rest = new DiscordRest(config(), { fetchImpl: async (url, options) => {
    calls.push({ url, options });
    return Response.json(message());
  } });
  const result = await rest.send('Alice · Overlay', '@everyone hello');
  assert.equal(result.id, '323456789012345678');
  assert.equal(new URL(calls[0].url).searchParams.get('wait'), 'true');
  assert.equal(calls[0].options.headers.Authorization, undefined);
  assert.equal(calls[0].options.redirect, 'error');
  const body = JSON.parse(calls[0].options.body);
  assert.equal(body.username, 'Alice · Overlay');
  assert.deepEqual(body.allowed_mentions, { parse: [], users: [], roles: [], replied_user: false });
});

test('webhook channel preflight rejects misconfiguration', async () => {
  const rest = new DiscordRest(config(), { fetchImpl: async () => Response.json({ type: 1, channel_id: OTHER }) });
  await assert.rejects(rest.verifyWebhook(), /allowed channel/);
});

test('webhook retries explicit 429 but never an ambiguous POST timeout or server error', async () => {
  let calls = 0;
  const waits = [];
  const rest = new DiscordRest(config(), { fetchImpl: async () => {
    calls += 1;
    return calls === 1 ? Response.json({ retry_after: 0.25 }, { status: 429 }) : Response.json(message());
  }, delay: async (delay) => { waits.push(delay); } });
  assert.equal((await rest.send('Alice', 'hi')).id, '323456789012345678');
  assert.deepEqual(waits, [250]);
  for (const mode of ['network', 'server']) {
    let count = 0;
    const failing = new DiscordRest(config(), { fetchImpl: async () => {
      count += 1;
      if (mode === 'network') throw new Error('network');
      return Response.json({ message: 'failure' }, { status: 500 });
    }, delay: async () => {} });
    await assert.rejects(failing.send('Alice', 'hi'), (error) => error.uncertain);
    assert.equal(count, 1);
  }
});

test('history fetches at most two pages and uses a bot Authorization header', async () => {
  const calls = [];
  const first = Array.from({ length: 100 }, (_, index) => message(String(523456789012345678n - BigInt(index))));
  const rest = new DiscordRest(config(), { fetchImpl: async (url, options) => {
    calls.push({ url, options });
    return Response.json(calls.length === 1 ? first : [message()]);
  } });
  assert.equal((await rest.history()).length, 101);
  assert.equal(calls.length, 2);
  assert.equal(calls[0].options.headers.Authorization, 'Bot test-bot-token-not-real');
  assert.equal(new URL(calls[1].url).searchParams.get('before'), first.at(-1).id);
});

class MockSocket extends EventTarget {
  static sockets = [];
  constructor(url) { super(); this.url = url; this.readyState = 1; this.sent = []; MockSocket.sockets.push(this); }
  send(text) { this.sent.push(JSON.parse(text)); }
  close(code = 1000) {
    this.readyState = 3;
    const event = new Event('close');
    Object.defineProperty(event, 'code', { value: code });
    this.dispatchEvent(event);
  }
  receive(packet) { this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(packet) })); }
}

test('Gateway buffers history races, maps replies, deduplicates echoes, edits, deletes, and resumes', async (t) => {
  MockSocket.sockets = [];
  const store = new EventStore();
  let historyResolve;
  const delayedHistory = new Promise((resolve) => { historyResolve = resolve; });
  let histories = 0;
  const rest = { history: async () => { histories += 1; return histories === 1 ? delayedHistory : [message()]; } };
  const bridge = new DiscordBridge(config(), store, rest, { WebSocketImpl: MockSocket, random: () => 0.5, log: () => {} });
  t.after(() => bridge.stop());
  bridge.start();
  const socket = MockSocket.sockets[0];
  socket.receive({ op: 10, d: { heartbeat_interval: 10000 } });
  assert.equal(socket.sent[0].op, 2);
  assert.equal(socket.sent[0].d.intents, 33280);
  socket.receive({ op: 0, t: 'READY', s: 1, d: { session_id: 'session', resume_gateway_url: 'wss://gateway-us-east1-b.discord.gg' } });
  const translation = message('323456789012345679', {
    content: 'Hello', message_reference: { message_id: '323456789012345678' }, author: { username: 'Translator', bot: true },
  });
  socket.receive({ op: 0, t: 'MESSAGE_CREATE', s: 2, d: translation });
  socket.receive({ op: 0, t: 'MESSAGE_CREATE', s: 3, d: message('323456789012345680', { channel_id: OTHER }) });
  historyResolve([message()]);
  await until(() => store.connected);
  assert.equal(store.messages().length, 2);
  assert.equal(store.get(translation.id).parent_id, '323456789012345678');
  const beforeEcho = store.cursor;
  bridge.ingestWebhook(translation);
  assert.equal(store.cursor, beforeEcho);
  socket.receive({ op: 0, t: 'MESSAGE_UPDATE', s: 4, d: { id: translation.id, channel_id: CHANNEL, content: 'Hello edited' } });
  assert.equal(store.get(translation.id).author, 'Translator');
  assert.equal(store.get(translation.id).parent_id, '323456789012345678');
  assert.equal(store.get(translation.id).content, 'Hello edited');
  socket.receive({ op: 0, t: 'MESSAGE_DELETE_BULK', s: 5, d: { channel_id: CHANNEL, ids: [translation.id] } });
  assert.equal(store.has(translation.id), false);
  socket.receive({ op: 7 });
  assert.equal(store.connected, false);
  await until(() => MockSocket.sockets.length === 2, 1500);
  const resumed = MockSocket.sockets[1];
  assert.match(resumed.url, /gateway-us-east1-b\.discord\.gg/);
  resumed.receive({ op: 10, d: { heartbeat_interval: 10000 } });
  assert.equal(resumed.sent[0].op, 6);
  assert.equal(resumed.sent[0].d.seq, 5);
  resumed.receive({ op: 0, t: 'RESUMED', s: 6, d: {} });
  await until(() => store.connected);
});

test('Gateway answers requested heartbeats and stops on invalid intents', async (t) => {
  MockSocket.sockets = [];
  const store = new EventStore();
  const logs = [];
  const bridge = new DiscordBridge(config(), store, { history: async () => [] }, {
    WebSocketImpl: MockSocket, random: () => 0.5, log: (value) => logs.push(value),
  });
  t.after(() => bridge.stop());
  bridge.start();
  const socket = MockSocket.sockets[0];
  socket.receive({ op: 10, d: { heartbeat_interval: 10000 } });
  socket.receive({ op: 1 });
  assert.equal(socket.sent.at(-1).op, 1);
  socket.receive({ op: 11 });
  socket.close(4014);
  assert.equal(store.connected, false);
  assert.match(logs.at(-1), /Message Content Intent/);
});
