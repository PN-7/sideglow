// SPDX-License-Identifier: MPL-2.0
import { EventEmitter } from 'node:events';
import { randomInt } from 'node:crypto';

export const MAX_MESSAGE_BYTES = 32 * 1024;
export const MAX_BATCH_BYTES = 8 * 1024 * 1024;

/** Keep UTF-16 limits without producing lone surrogates that Rust JSON parsers reject. */
function safeText(value, maxUnits) {
  const text = String(value).toWellFormed();
  if (text.length <= maxUnits) return text;
  let end = maxUnits;
  const last = text.charCodeAt(end - 1);
  if (last >= 0xD800 && last <= 0xDBFF) end -= 1;
  return text.slice(0, end);
}

function boundedMessage(message) {
  const bounded = {
    id: safeText(message.id, 22),
    parent_id: message.parent_id ? safeText(message.parent_id, 22) : null,
    author: safeText(message.author, 100),
    content: safeText(message.content, 12000),
    timestamp: safeText(message.timestamp, 64),
    bot: Boolean(message.bot),
  };
  if (Buffer.byteLength(JSON.stringify(bounded)) <= MAX_MESSAGE_BYTES) return bounded;
  const original = bounded.content;
  const suffix = '\n[긴 메시지 일부 생략]';
  let low = 0;
  let high = original.length;
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    bounded.content = safeText(original, middle) + suffix;
    if (Buffer.byteLength(JSON.stringify(bounded)) <= MAX_MESSAGE_BYTES) low = middle;
    else high = middle - 1;
  }
  bounded.content = safeText(original, low) + suffix;
  return bounded;
}

function compareMessages(a, b) {
  const time = Date.parse(a.timestamp) - Date.parse(b.timestamp);
  if (Number.isFinite(time) && time !== 0) return time;
  return a.id.length - b.id.length || a.id.localeCompare(b.id);
}

/** Bounded current history plus a monotonic, bounded change log. */
export class EventStore {
  #messages = new Map();
  #events = [];
  #emitter = new EventEmitter();
  #cursor;
  #baseCursor;
  #lastReset = 0;
  #connected = false;
  constructor({ maxMessages = 200, maxEvents = 1000, maxBatchBytes = MAX_BATCH_BYTES,
    initialCursor = Date.now() * 1024 + randomInt(1024) } = {}) {
    if (!Number.isSafeInteger(initialCursor) || initialCursor < 1) throw new Error('Invalid initial event cursor');
    this.#cursor = initialCursor;
    this.#baseCursor = initialCursor;
    this.maxMessages = maxMessages;
    this.maxEvents = maxEvents;
    this.maxBatchBytes = maxBatchBytes;
    this.#emitter.setMaxListeners(500);
  }
  get connected() { return this.#connected; }
  get cursor() { return this.#cursor; }
  get(id) { return this.#messages.get(id); }
  has(id) { return this.#messages.has(id); }
  messages() { return [...this.#messages.values()].sort(compareMessages); }
  #append(event) {
    this.#cursor += 1;
    this.#events.push({ cursor: this.#cursor, event });
    if (this.#events.length > this.maxEvents) this.#events.shift();
    this.#emitter.emit('change');
  }
  setConnected(value) {
    if (value === this.#connected) return;
    this.#connected = value;
    this.#append(null);
  }
  replaceHistory(messages) {
    const sorted = [...new Map(messages.map(boundedMessage).map((message) => [message.id, message])).values()]
      .sort(compareMessages).slice(-this.maxMessages);
    this.#messages = new Map(sorted.map((message) => [message.id, message]));
    this.#lastReset = this.#cursor + 1;
    this.#append(null);
  }
  upsert(message) {
    message = boundedMessage(message);
    const old = this.#messages.get(message.id);
    if (old && JSON.stringify(old) === JSON.stringify(message)) return false;
    this.#messages.set(message.id, message);
    this.#append({ op: 'upsert', message });
    if (this.#messages.size > this.maxMessages) {
      for (const removed of this.messages().slice(0, this.#messages.size - this.maxMessages)) this.delete(removed.id);
    }
    return true;
  }
  delete(id) {
    if (!this.#messages.delete(id)) return false;
    this.#append({ op: 'delete', id });
    return true;
  }
  batch(after) {
    const earliest = this.#events[0]?.cursor ?? this.#cursor;
    const reset = after === 0 || after < this.#baseCursor || after > this.#cursor || after < this.#lastReset || after < earliest - 1;
    if (reset) return {
      cursor: this.#cursor, reset: true, connected: this.#connected,
      events: this.messages().map((message) => ({ op: 'upsert', message })),
    };
    let cursor = after;
    let bytes = 256; // Envelope, delimiters, and numeric cursor overhead.
    const events = [];
    for (const item of this.#events) {
      if (item.cursor <= after) continue;
      const size = item.event ? Buffer.byteLength(JSON.stringify(item.event)) + 1 : 0;
      if (bytes + size > this.maxBatchBytes) break;
      bytes += size;
      cursor = item.cursor;
      if (item.event) events.push(item.event);
    }
    return { cursor, reset: false, connected: this.#connected, events };
  }
  wait(after, milliseconds, signal) {
    const now = this.batch(after);
    if (now.reset || now.cursor !== after || milliseconds <= 0 || signal?.aborted) return Promise.resolve(now);
    return new Promise((resolve) => {
      let timer;
      const finish = () => {
        clearTimeout(timer);
        this.#emitter.off('change', finish);
        signal?.removeEventListener('abort', finish);
        resolve(this.batch(after));
      };
      this.#emitter.once('change', finish);
      signal?.addEventListener('abort', finish, { once: true });
      timer = setTimeout(finish, milliseconds);
    });
  }
}

export function normalizeMessage(raw) {
  const chunks = [];
  if (typeof raw.content === 'string' && raw.content) chunks.push(raw.content);
  for (const embed of Array.isArray(raw.embeds) ? raw.embeds : []) {
    if (typeof embed.title === 'string' && embed.title) chunks.push(embed.title);
    if (typeof embed.description === 'string' && embed.description) chunks.push(embed.description);
    for (const field of Array.isArray(embed.fields) ? embed.fields : []) {
      if (typeof field.name === 'string' && typeof field.value === 'string') chunks.push(`${field.name}: ${field.value}`);
    }
  }
  for (const attachment of Array.isArray(raw.attachments) ? raw.attachments : []) {
    chunks.push(`[첨부 파일: ${safeText(attachment.filename ?? 'file', 200)}]`);
  }
  for (const sticker of Array.isArray(raw.sticker_items) ? raw.sticker_items : []) {
    chunks.push(`[스티커: ${safeText(sticker.name ?? 'sticker', 100)}]`);
  }
  return {
    id: safeText(raw.id, 22),
    parent_id: raw.message_reference?.message_id ? safeText(raw.message_reference.message_id, 22) : null,
    author: safeText(raw.member?.nick ?? raw.author?.global_name ?? raw.author?.username ?? '알 수 없음', 100),
    content: safeText(chunks.join('\n').replace(/\u0000/g, ''), 12000),
    timestamp: safeText(typeof raw.timestamp === 'string' ? raw.timestamp : new Date().toISOString(), 64),
    bot: Boolean(raw.author?.bot || raw.webhook_id),
  };
}
