// SPDX-License-Identifier: MPL-2.0
import test from 'node:test';
import assert from 'node:assert/strict';
import { validateConfig } from '../config.mjs';
import { EventStore, normalizeMessage, MAX_MESSAGE_BYTES, MAX_BATCH_BYTES } from '../store.mjs';
import { validateMessage } from '../server.mjs';
import { CHANNEL, KEY, message } from './helpers.mjs';

test('configuration confines plaintext transport, resolves secrets, and rejects duplicate user keys', () => {
  const raw = { channelId: CHANNEL, users: [{ id: 'alice', displayName: 'Alice', accessKeyEnv: 'ALICE' }] };
  const env = { DISCORD_BOT_TOKEN: 'fake', DISCORD_WEBHOOK_URL: `https://discord.com/api/webhooks/${CHANNEL}/FAKE`, ALICE: KEY };
  const valid = validateConfig(raw, '/tmp', env);
  assert.equal(valid.listen.host, '127.0.0.1');
  assert.equal(valid.users[0].keyHash.length, 32);
  assert.equal(valid.users[0].accessKey, undefined);
  assert.throws(() => validateConfig({ ...raw, listen: { host: '0.0.0.0' } }, '/tmp', env), /Plain HTTP/);
  assert.throws(() => validateConfig(raw, '/tmp', { ...env, DISCORD_WEBHOOK_URL: `https://evil.example/api/webhooks/${CHANNEL}/FAKE` }), /HTTPS discord.com/);
  assert.throws(() => validateConfig({ ...raw, users: [...raw.users, { id: 'bob', displayName: 'Bob', accessKeyEnv: 'ALICE' }] }, '/tmp', env), /different access key/);
  assert.equal(validateConfig({ ...raw, listen: { host: '0.0.0.0', tls: { keyFile: 'key.pem', certFile: 'cert.pem' } } }, '/tmp', env).listen.tls.keyFile, '/tmp/key.pem');
});

test('reply mapping retains source id without requiring referenced_message', () => {
  const normalized = normalizeMessage(message('323456789012345679', {
    content: '', message_reference: { message_id: '323456789012345678' }, referenced_message: null,
    author: { username: 'Translator', bot: true }, embeds: [{ description: '번역문', fields: [{ name: '언어', value: '한국어' }] }],
    attachments: [{ filename: 'image.png', url: 'https://not-fetched.invalid/image.png' }],
  }));
  assert.equal(normalized.parent_id, '323456789012345678');
  assert.equal(normalized.bot, true);
  assert.match(normalized.content, /번역문\n언어: 한국어\n\[첨부 파일: image.png\]/);
  assert.doesNotMatch(normalized.content, /not-fetched/);
});

test('empty initial history has a nonzero cursor and subsequent polling waits', async () => {
  const store = new EventStore();
  const initial = store.batch(0);
  assert.equal(initial.reset, true);
  assert.ok(initial.cursor > 0);
  assert.equal(store.batch(initial.cursor).reset, false);
  let resolved = false;
  const pending = store.wait(initial.cursor, 1000).then((value) => { resolved = true; return value; });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(resolved, false);
  store.setConnected(true);
  const result = await pending;
  assert.equal(result.connected, true);
  assert.deepEqual(result.events, []);
});

test('live edits, Gateway echoes, deletions, stale cursors, and history resets remain bounded', () => {
  const store = new EventStore({ maxMessages: 2, maxEvents: 3 });
  store.replaceHistory([normalizeMessage(message())]);
  const initial = store.batch(0);
  assert.equal(store.upsert(normalizeMessage(message())), false);
  assert.equal(store.cursor, initial.cursor);
  store.upsert(normalizeMessage(message(undefined, { content: 'edited' })));
  assert.equal(store.batch(initial.cursor).events[0].message.content, 'edited');
  store.upsert(normalizeMessage(message('323456789012345679')));
  store.upsert(normalizeMessage(message('323456789012345680')));
  assert.equal(store.messages().length, 2);
  assert.equal(store.has('323456789012345678'), false);
  assert.equal(store.batch(initial.cursor).reset, true);
  const beforeDelete = store.cursor;
  store.delete('323456789012345679');
  assert.deepEqual(store.batch(beforeDelete).events, [{ op: 'delete', id: '323456789012345679' }]);
  store.replaceHistory([]);
  assert.equal(store.batch(beforeDelete).reset, true);
});

test('aborted long poll removes its waiter and resolves cleanly', async () => {
  const store = new EventStore();
  const abort = new AbortController();
  const result = store.wait(store.cursor, 25000, abort.signal);
  abort.abort();
  assert.equal((await result).cursor, store.cursor);
});

test('message validation counts UTF-16 code units and preserves literal text', () => {
  assert.equal(validateMessage({ content: '한글\n<script>literal</script>', nonce: 'native-1' }).content, '한글\n<script>literal</script>');
  assert.equal(validateMessage({ content: '😀'.repeat(1000), nonce: 'n' }).content.length, 2000);
  assert.throws(() => validateMessage({ content: '😀'.repeat(1001), nonce: 'n' }), /2,000/);
  assert.throws(() => validateMessage({ content: ' ', nonce: 'n' }), /2,000/);
  assert.throws(() => validateMessage({ content: 'hi\u0000', nonce: 'n' }), /2,000/);
  assert.throws(() => validateMessage({ content: 'hi', nonce: '../path' }), /전송 번호/);
});

test('event pagination respects byte budget and advances only through delivered changes', () => {
  const store = new EventStore({ maxMessages: 2, maxBatchBytes: 80 * 1024, initialCursor: 1000 });
  const after = store.cursor;
  for (let index = 0; index < 5; index += 1) {
    store.upsert(normalizeMessage(message(undefined, { content: `${index}` + '한'.repeat(11900) })));
  }
  assert.ok(Buffer.byteLength(JSON.stringify(store.messages()[0])) <= MAX_MESSAGE_BYTES);
  assert.match(store.messages()[0].content, /긴 메시지 일부 생략/);
  const first = store.batch(after);
  assert.ok(Buffer.byteLength(JSON.stringify(first)) <= 80 * 1024);
  assert.equal(first.events.length, 2);
  assert.equal(first.cursor, after + 2);
  const second = store.batch(first.cursor);
  const third = store.batch(second.cursor);
  assert.equal(first.events.length + second.events.length + third.events.length, 5);
  assert.equal(third.cursor, store.cursor);
  const full = new EventStore();
  full.replaceHistory(Array.from({ length: 200 }, (_, index) => normalizeMessage(message(String(323456789012345678n + BigInt(index)), { content: '\u0001'.repeat(12000) }))));
  assert.ok(Buffer.byteLength(JSON.stringify(full.batch(0))) < MAX_BATCH_BYTES);
});

test('a cursor from before a server restart forces a snapshot rather than silently losing history', () => {
  const before = new EventStore({ initialCursor: 1000 });
  before.replaceHistory([normalizeMessage(message())]);
  const cursor = before.cursor;
  const restarted = new EventStore({ initialCursor: 1000000 });
  restarted.replaceHistory([normalizeMessage(message(undefined, { content: 'after restart' }))]);
  const snapshot = restarted.batch(cursor);
  assert.equal(snapshot.reset, true);
  assert.equal(snapshot.events[0].message.content, 'after restart');
});

test('normalization never splits emoji at content, author, attachment, sticker, or timestamp cutoffs', () => {
  const content = normalizeMessage(message(undefined, { content: 'a'.repeat(11999) + '😀' }));
  assert.equal(content.content, 'a'.repeat(11999));
  const author = normalizeMessage(message(undefined, { author: { username: 'a'.repeat(99) + '😀' } }));
  assert.equal(author.author, 'a'.repeat(99));
  const attachment = normalizeMessage(message(undefined, { content: '', attachments: [{ filename: 'a'.repeat(199) + '😀' }] }));
  assert.equal(attachment.content, `[첨부 파일: ${'a'.repeat(199)}]`);
  const sticker = normalizeMessage(message(undefined, { content: '', sticker_items: [{ name: 'a'.repeat(99) + '😀' }] }));
  assert.equal(sticker.content, `[스티커: ${'a'.repeat(99)}]`);
  const timestamp = normalizeMessage(message(undefined, { timestamp: 'a'.repeat(63) + '😀' }));
  assert.equal(timestamp.timestamp, 'a'.repeat(63));
  const malformed = normalizeMessage(message(undefined, { content: 'A\uD800B\uDC00C', author: { username: '\uD800' } }));
  assert.equal(malformed.content, 'A\uFFFDB\uFFFDC');
  assert.equal(malformed.author, '\uFFFD');
  for (const normalized of [content, author, attachment, sticker, timestamp, malformed]) {
    for (const value of Object.values(normalized)) if (typeof value === 'string') assert.equal(value.isWellFormed(), true);
  }
});

test('history bounds and the multibyte byte-budget preserve valid Unicode at every truncation boundary', () => {
  const store = new EventStore();
  store.upsert({
    id: 'a'.repeat(21) + '😀', parent_id: 'p'.repeat(21) + '😀',
    author: 'n'.repeat(99) + '😀', content: 'x'.repeat(11999) + '😀',
    timestamp: 't'.repeat(63) + '😀', bot: false,
  });
  const simple = store.messages()[0];
  assert.equal(simple.id, 'a'.repeat(21));
  assert.equal(simple.parent_id, 'p'.repeat(21));
  assert.equal(simple.author, 'n'.repeat(99));
  assert.equal(simple.content, 'x'.repeat(11999));
  assert.equal(simple.timestamp, 't'.repeat(63));
  store.replaceHistory([normalizeMessage(message(undefined, { content: '한'.repeat(9000) + '😀'.repeat(1500) }))]);
  const bounded = store.messages()[0];
  assert.match(bounded.content, /긴 메시지 일부 생략/);
  assert.ok(Buffer.byteLength(JSON.stringify(bounded)) <= MAX_MESSAGE_BYTES);
  const prefix = bounded.content.slice(0, bounded.content.indexOf('\n[긴 메시지 일부 생략]'));
  assert.ok(prefix.endsWith('😀'));
  for (const value of Object.values(bounded)) if (typeof value === 'string') assert.equal(value.isWellFormed(), true);
  assert.doesNotMatch(JSON.stringify(store.batch(0)), /\\u[dD][89a-fA-F][0-9a-fA-F]{2}/);
});
