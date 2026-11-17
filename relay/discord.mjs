// SPDX-License-Identifier: MPL-2.0
import { SendError } from './nonce.mjs';
import { normalizeMessage } from './store.mjs';

const API = 'https://discord.com/api/v10';
const GATEWAY = 'wss://gateway.discord.gg/';
const INTENTS = (1 << 9) | (1 << 15); // GUILD_MESSAGES | MESSAGE_CONTENT
const SNOWFLAKE = /^[0-9]{16,20}$/;
const sleep = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

export class DiscordRest {
  constructor(config, { fetchImpl = fetch, delay = sleep, timeoutMs = 12000 } = {}) {
    this.config = config;
    this.fetch = fetchImpl;
    this.delay = delay;
    this.timeoutMs = timeoutMs;
  }
  async #request(url, { method = 'GET', body, bot = true, webhookSend = false } = {}) {
    let spentWaiting = 0;
    const deadline = Date.now() + (webhookSend ? 30000 : 60000);
    for (let attempt = 0; attempt < 4; attempt += 1) {
      let response;
      try {
        response = await this.fetch(url, {
          method, redirect: 'error', signal: AbortSignal.timeout(Math.max(1, Math.min(this.timeoutMs, deadline - Date.now()))),
          headers: {
            'User-Agent': 'SideglowRelay/0.1.0',
            ...(bot ? { Authorization: `Bot ${this.config.botToken}` } : {}),
            ...(body ? { 'Content-Type': 'application/json' } : {}),
          },
          ...(body ? { body: JSON.stringify(body) } : {}),
        });
      } catch {
        if (webhookSend) throw new SendError(502, 'Discord 전송 결과를 확인하지 못했습니다. 채널에서 확인하세요.', { uncertain: true });
        if (attempt < 3) { await this.delay(500 * (2 ** attempt)); continue; }
        throw new Error('Discord connection failed');
      }
      let payload;
      try {
        const text = await response.text();
        if (text.length > 4 * 1024 * 1024) throw new Error('Response too large');
        payload = text ? JSON.parse(text) : null;
      } catch {
        throw webhookSend ? new SendError(502, 'Discord 전송 응답을 확인하지 못했습니다.', { uncertain: true }) : new Error('Discord response was invalid');
      }
      if (response.status === 429) {
        const seconds = Number(payload?.retry_after ?? response.headers.get('retry-after') ?? 1);
        const retry = Number.isFinite(seconds) && seconds >= 0 ? Math.max(100, Math.ceil(seconds * 1000)) : 1000;
        // A 429 explicitly says this request was rejected. No retry after ambiguous transport/5xx failures.
        if (attempt < 3 && spentWaiting + retry <= 15000 && Date.now() + retry + 1000 < deadline) {
          spentWaiting += retry;
          await this.delay(retry);
          continue;
        }
        throw new SendError(429, 'Discord 전송 제한입니다. 잠시 후 다시 시도하세요.', { retryable: true, retryAfter: Math.ceil(retry / 1000) });
      }
      if (!response.ok) {
        if (webhookSend) {
          if (response.status >= 500) throw new SendError(502, 'Discord 전송 결과가 불확실합니다. 채널에서 확인하세요.', { uncertain: true });
          throw new SendError(502, `Discord가 전송을 거부했습니다 (HTTP ${response.status}). 서버 관리자에게 문의하세요.`);
        }
        if (response.status >= 500 && attempt < 3) { await this.delay(500 * (2 ** attempt)); continue; }
        const failure = new Error(`Discord request failed (HTTP ${response.status})`);
        failure.status = response.status;
        throw failure;
      }
      return payload;
    }
    throw new Error('Discord retry limit reached');
  }
  async verifyWebhook() {
    const data = await this.#request(this.config.webhookUrl, { bot: false });
    if (data?.type !== 1 || data.channel_id !== this.config.channelId) {
      throw new Error('Configured webhook is not an incoming webhook for the allowed channel');
    }
  }
  async history() {
    const first = await this.#request(`${API}/channels/${this.config.channelId}/messages?limit=100`);
    if (!Array.isArray(first)) throw new Error('Discord history response was invalid');
    if (first.length < 100) return first;
    const before = first.at(-1)?.id;
    if (!SNOWFLAKE.test(before ?? '')) throw new Error('Discord history cursor was invalid');
    const second = await this.#request(`${API}/channels/${this.config.channelId}/messages?limit=100&before=${before}`);
    if (!Array.isArray(second)) throw new Error('Discord history response was invalid');
    return [...first, ...second].slice(0, 200);
  }
  async send(displayName, content) {
    const url = new URL(this.config.webhookUrl);
    url.searchParams.set('wait', 'true');
    const message = await this.#request(url.href, {
      method: 'POST', bot: false, webhookSend: true,
      body: { content, username: displayName, allowed_mentions: { parse: [], users: [], roles: [], replied_user: false } },
    });
    if (!SNOWFLAKE.test(message?.id ?? '') || message.channel_id !== this.config.channelId) {
      throw new SendError(502, 'Discord 전송 응답이 예상과 달라 확인할 수 없습니다.', { uncertain: true });
    }
    return message;
  }
}

export class DiscordBridge {
  #socket;
  #heartbeatTimer;
  #reconnectTimer;
  #stopped = true;
  #sequence = null;
  #sessionId;
  #resumeUrl;
  #awaitingAck = false;
  #reconnectAttempt = 0;
  #syncGeneration = 0;
  #syncing = false;
  #buffer = [];
  #raw = new Map();
  constructor(config, store, rest, { WebSocketImpl = WebSocket, random = Math.random, log = console.error } = {}) {
    this.config = config;
    this.store = store;
    this.rest = rest;
    this.WebSocket = WebSocketImpl;
    this.random = random;
    this.log = log;
  }
  start() {
    if (!this.#stopped) return;
    this.#stopped = false;
    this.#connect();
  }
  stop() {
    this.#stopped = true;
    this.#syncGeneration += 1;
    clearTimeout(this.#heartbeatTimer);
    clearTimeout(this.#reconnectTimer);
    const socket = this.#socket;
    this.#socket = undefined;
    socket?.close(1000, 'Shutdown');
    this.store.setConnected(false);
  }
  ingestWebhook(raw) { this.#dispatch('MESSAGE_CREATE', raw); }
  #send(packet) {
    if (this.#socket?.readyState === 1) this.#socket.send(JSON.stringify(packet));
  }
  #clearSession() {
    this.#sessionId = undefined;
    this.#resumeUrl = undefined;
    this.#sequence = null;
  }
  #connect() {
    if (this.#stopped) return;
    const url = new URL(this.#resumeUrl ?? GATEWAY);
    url.searchParams.set('v', '10');
    url.searchParams.set('encoding', 'json');
    let socket;
    try { socket = new this.WebSocket(url.href); }
    catch { this.#scheduleReconnect(); return; }
    this.#socket = socket;
    this.#awaitingAck = false;
    // A connection that never sends HELLO must not remain stuck indefinitely.
    this.#heartbeatTimer = setTimeout(() => this.#reconnect(1000), 15000);
    socket.addEventListener('message', (event) => {
      if (socket !== this.#socket) return;
      try {
        if (typeof event.data !== 'string' || event.data.length > 2 * 1024 * 1024) throw new Error('Invalid gateway payload');
        const packet = JSON.parse(event.data);
        if (Number.isSafeInteger(packet.s)) this.#sequence = packet.s;
        this.#packet(packet);
      } catch {
        this.log('Discord Gateway sent an invalid payload; reconnecting.');
        this.#reconnect(1000);
      }
    });
    socket.addEventListener('error', () => {
      if (socket === this.#socket) this.#reconnect();
    });
    socket.addEventListener('close', (event) => {
      if (socket !== this.#socket) return;
      this.#socket = undefined;
      clearTimeout(this.#heartbeatTimer);
      this.#syncGeneration += 1;
      this.#syncing = false;
      this.#buffer = [];
      this.store.setConnected(false);
      if ([4004, 4010, 4011, 4012, 4013, 4014].includes(event.code)) {
        this.log(`Discord Gateway stopped (code ${event.code}). Check the bot token and Message Content Intent, then restart.`);
        this.#stopped = true;
        return;
      }
      if ([4007, 4009].includes(event.code)) this.#clearSession();
      this.#scheduleReconnect();
    });
  }
  #scheduleReconnect(delay) {
    if (this.#stopped || this.#reconnectTimer) return;
    const wait = delay ?? Math.min(30000, 1000 * (2 ** Math.min(this.#reconnectAttempt++, 5))) + Math.floor(this.random() * 1000);
    this.#reconnectTimer = setTimeout(() => {
      this.#reconnectTimer = undefined;
      this.#connect();
    }, wait);
  }
  #reconnect(delay) {
    clearTimeout(this.#heartbeatTimer);
    const socket = this.#socket;
    this.#socket = undefined;
    this.#syncGeneration += 1;
    this.#syncing = false;
    this.#buffer = [];
    this.store.setConnected(false);
    try { socket?.close(4000, 'Reconnect'); } catch { /* already closed */ }
    this.#scheduleReconnect(delay);
  }
  #heartbeat(interval) {
    if (this.#awaitingAck) { this.log('Discord Gateway heartbeat timed out; reconnecting.'); this.#reconnect(); return; }
    this.#send({ op: 1, d: this.#sequence });
    this.#awaitingAck = true;
    this.#heartbeatTimer = setTimeout(() => this.#heartbeat(interval), interval);
  }
  #packet(packet) {
    if (packet.op === 10) {
      clearTimeout(this.#heartbeatTimer);
      const interval = packet.d?.heartbeat_interval;
      if (!Number.isFinite(interval) || interval < 1000 || interval > 120000) throw new Error('Invalid heartbeat interval');
      this.#heartbeatTimer = setTimeout(() => this.#heartbeat(interval), Math.floor(this.random() * interval));
      if (this.#sessionId && this.#sequence !== null) {
        this.#send({ op: 6, d: { token: this.config.botToken, session_id: this.#sessionId, seq: this.#sequence } });
      } else {
        this.#send({ op: 2, d: { token: this.config.botToken, intents: INTENTS, properties: {
          os: process.platform, browser: 'SideglowRelay', device: 'SideglowRelay',
        } } });
      }
    } else if (packet.op === 11) {
      this.#awaitingAck = false;
    } else if (packet.op === 1) {
      this.#send({ op: 1, d: this.#sequence });
      this.#awaitingAck = true;
    } else if (packet.op === 7) {
      this.#reconnect(500);
    } else if (packet.op === 9) {
      if (!packet.d) this.#clearSession();
      this.#reconnect(1000 + Math.floor(this.random() * 4000));
    } else if (packet.op === 0) {
      if (packet.t === 'READY') {
        if (typeof packet.d?.session_id !== 'string') throw new Error('Missing session id');
        this.#sessionId = packet.d.session_id;
        const resume = new URL(packet.d.resume_gateway_url ?? GATEWAY);
        if (resume.protocol !== 'wss:' || !/^gateway(?:-[a-z0-9-]+)?\.discord\.gg$/.test(resume.hostname) ||
            resume.port || resume.username || resume.password) throw new Error('Invalid resume host');
        this.#resumeUrl = resume.href;
        this.#reconnectAttempt = 0;
        void this.#synchronize();
      } else if (packet.t === 'RESUMED') {
        this.#reconnectAttempt = 0;
        void this.#synchronize();
      } else this.#dispatch(packet.t, packet.d);
    }
  }
  async #synchronize() {
    const generation = ++this.#syncGeneration;
    this.#syncing = true;
    this.#buffer = [];
    this.store.setConnected(false);
    try {
      const history = await this.rest.history();
      if (this.#stopped || generation !== this.#syncGeneration) return;
      const safe = history.filter((message) => message.channel_id === this.config.channelId && SNOWFLAKE.test(message.id ?? ''));
      this.#raw = new Map(safe.map((message) => [message.id, message]));
      this.store.replaceHistory(safe.map(normalizeMessage));
      this.#syncing = false;
      const buffered = this.#buffer;
      this.#buffer = [];
      for (const event of buffered) this.#apply(event.type, event.data);
      this.store.setConnected(true);
    } catch {
      if (this.#stopped || generation !== this.#syncGeneration) return;
      this.log('Discord history synchronization failed. Check channel permissions; reconnecting.');
      this.#reconnect(5000);
    }
  }
  #dispatch(type, data) {
    if (!data || data.channel_id !== this.config.channelId ||
        !['MESSAGE_CREATE', 'MESSAGE_UPDATE', 'MESSAGE_DELETE', 'MESSAGE_DELETE_BULK'].includes(type)) return;
    if (this.#syncing) {
      if (this.#buffer.length >= 2000) { this.log('Discord synchronization buffer filled; reconnecting.'); this.#reconnect(1000); return; }
      this.#buffer.push({ type, data });
    } else this.#apply(type, data);
  }
  #apply(type, data) {
    if (type === 'MESSAGE_DELETE_BULK') {
      for (const id of Array.isArray(data.ids) ? data.ids : []) { this.#raw.delete(id); this.store.delete(id); }
    } else if (type === 'MESSAGE_DELETE') {
      this.#raw.delete(data.id);
      this.store.delete(data.id);
    } else if (SNOWFLAKE.test(data.id ?? '')) {
      const previous = this.#raw.get(data.id);
      // Edits outside the retained 200-message history do not reintroduce old messages.
      if (type === 'MESSAGE_UPDATE' && !previous) return;
      const merged = { ...previous, ...data };
      this.#raw.set(data.id, merged);
      this.store.upsert(normalizeMessage(merged));
      for (const id of this.#raw.keys()) if (!this.store.has(id)) this.#raw.delete(id);
    }
  }
}
