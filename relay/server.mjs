// SPDX-License-Identifier: MPL-2.0
import http from 'node:http';
import https from 'node:https';
import { readFile } from 'node:fs/promises';
import { createHash, timingSafeEqual } from 'node:crypto';
import { SendError } from './nonce.mjs';

const MAX_BODY = 16 * 1024;

function json(response, status, value, extra = {}) {
  if (response.destroyed || response.writableEnded) return;
  response.writeHead(status, {
    'Content-Type': 'application/json; charset=utf-8',
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
    ...extra,
  });
  response.end(JSON.stringify(value));
}

async function readBody(request) {
  const declared = Number(request.headers['content-length']);
  if (Number.isFinite(declared) && declared > MAX_BODY) {
    request.resume();
    throw new SendError(413, '요청이 너무 큽니다.');
  }
  let length = 0;
  const chunks = [];
  for await (const chunk of request) {
    length += chunk.length;
    if (length > MAX_BODY) throw new SendError(413, '요청이 너무 큽니다.');
    chunks.push(chunk);
  }
  try { return JSON.parse(Buffer.concat(chunks).toString('utf8')); }
  catch { throw new SendError(400, '올바른 JSON 요청이 필요합니다.'); }
}

export function validateMessage(body) {
  if (!body || typeof body !== 'object' || Array.isArray(body) || typeof body.content !== 'string' ||
      !body.content.trim() || body.content.length > 2000 || /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(body.content)) {
    throw new SendError(400, '메시지는 비어 있지 않은 2,000자 이내 텍스트여야 합니다.');
  }
  if (typeof body.nonce !== 'string' || body.nonce.length < 1 || body.nonce.length > 128 || !/^[A-Za-z0-9_.:-]+$/.test(body.nonce)) {
    throw new SendError(400, '올바른 메시지 전송 번호가 필요합니다.');
  }
  if (body.romaji_japanese_input !== undefined && typeof body.romaji_japanese_input !== 'boolean') {
    throw new SendError(400, '로마자 일본어 선택은 켜기 또는 끄기 값이어야 합니다.');
  }
  return { content: body.content, nonce: body.nonce, romajiJapaneseInput: body.romaji_japanese_input === true };
}

class SendLimiter {
  #buckets = new Map();
  constructor(clock) { this.clock = clock; }
  take(id) {
    const now = this.clock();
    const previous = this.#buckets.get(id) ?? { tokens: 5, updated: now };
    previous.tokens = Math.min(5, previous.tokens + Math.max(0, now - previous.updated) / 1000);
    previous.updated = now;
    if (previous.tokens < 1) {
      this.#buckets.set(id, previous);
      throw new SendError(429, '너무 빠르게 보내고 있습니다. 잠시 후 다시 시도하세요.', { retryable: true, retryAfter: 1 });
    }
    previous.tokens -= 1;
    this.#buckets.set(id, previous);
  }
}

export function createHandler({ config, store, ledger, send, clock = Date.now }) {
  const polls = new Map();
  const sends = new Map();
  const limiter = new SendLimiter(clock);
  const authenticate = (request) => {
    const header = request.headers.authorization;
    if (typeof header !== 'string' || !/^Bearer [A-Za-z0-9_-]{32,256}$/.test(header)) return null;
    const digest = createHash('sha256').update(header.slice(7)).digest();
    let matched = null;
    for (const user of config.users) if (timingSafeEqual(digest, user.keyHash)) matched = user;
    return matched;
  };
  return async function handler(request, response) {
    try {
      // This API is intended for the native client, not third-party web pages.
      if (request.headers.origin !== undefined) { json(response, 403, { error: '브라우저 출처 요청은 허용되지 않습니다.' }); return; }
      const url = new URL(request.url, 'http://localhost');
      if (request.method === 'GET' && url.pathname === '/health') { json(response, 200, { ok: true }); return; }
      const user = authenticate(request);
      if (!user) { json(response, 401, { error: '접속 키를 확인하세요.' }); return; }
      if (request.headers['x-discord-channel'] !== config.channelId) {
        json(response, 403, { error: '이 서버에서 허용한 Discord 채널이 아닙니다.' }); return;
      }
      if (request.method === 'GET' && url.pathname === '/v1/events') {
        const rawAfter = url.searchParams.get('after') ?? '0';
        const rawWait = url.searchParams.get('wait') ?? '25000';
        const after = Number(rawAfter);
        const wait = Number(rawWait);
        if (!/^\d{1,16}$/.test(rawAfter) || !Number.isSafeInteger(after) ||
            !/^\d{1,5}$/.test(rawWait) || !Number.isInteger(wait) || wait > 25000) {
          json(response, 400, { error: '잘못된 이벤트 조회 범위입니다.' }); return;
        }
        if ((polls.get(user.id) ?? 0) >= 4) { json(response, 429, { error: '동시에 열린 채팅 연결이 너무 많습니다.' }, { 'Retry-After': '1' }); return; }
        polls.set(user.id, (polls.get(user.id) ?? 0) + 1);
        const abort = new AbortController();
        const close = () => abort.abort();
        response.once('close', close);
        try {
          const batch = await store.wait(after, wait, abort.signal);
          if (!abort.signal.aborted) json(response, 200, batch);
        } finally {
          response.off('close', close);
          polls.set(user.id, Math.max(0, (polls.get(user.id) ?? 1) - 1));
        }
        return;
      }
      if (request.method === 'POST' && url.pathname === '/v1/messages') {
        if (!/^application\/json(?:\s*;\s*charset=utf-8)?$/i.test(request.headers['content-type'] ?? '')) {
          json(response, 415, { error: 'application/json 형식이 필요합니다.' }); return;
        }
        if ((sends.get(user.id) ?? 0) >= 3) { json(response, 429, { error: '처리 중인 전송이 너무 많습니다.' }, { 'Retry-After': '1' }); return; }
        sends.set(user.id, (sends.get(user.id) ?? 0) + 1);
        try {
          const body = validateMessage(await readBody(request));
          // NUL cannot occur in accepted chat text, so this policy tag cannot
          // collide with another plain message. Legacy/off fingerprints stay valid.
          const identity = body.romajiJapaneseInput ? `\0romaji\0${body.content}` : body.content;
          const result = await ledger.run(user.id, body.nonce, identity, async () => {
            limiter.take(user.id);
            const name = body.romajiJapaneseInput
              ? `Sideglow [romaji] · ${Array.from(user.displayName).slice(0, 60).join('')}`
              : user.displayName.startsWith('Sideglow [romaji] · ')
                // Escape a nickname that itself starts with the reserved ON label.
                ? `Sideglow · ${Array.from(user.displayName).slice(0, 60).join('')}`
                : user.displayName;
            return await send(name, body.content);
          });
          json(response, result.replayed ? 200 : 201, { id: result.id });
        } finally { sends.set(user.id, Math.max(0, (sends.get(user.id) ?? 1) - 1)); }
        return;
      }
      json(response, 404, { error: '지원하지 않는 요청입니다.' });
    } catch (error) {
      if (error instanceof SendError) {
        json(response, error.status, { error: error.message }, error.retryAfter ? { 'Retry-After': String(error.retryAfter) } : {});
      } else json(response, 500, { error: '서버가 요청을 처리하지 못했습니다.' });
    }
  };
}

export async function createRelayServer(dependencies) {
  const handler = createHandler(dependencies);
  const tls = dependencies.config.listen.tls;
  const server = tls ? https.createServer({
    key: await readFile(tls.keyFile), cert: await readFile(tls.certFile), minVersion: 'TLSv1.2',
  }, handler) : http.createServer(handler);
  server.maxHeadersCount = 32;
  server.headersTimeout = 10000;
  server.requestTimeout = 15000;
  server.keepAliveTimeout = 5000;
  server.maxConnections = 512;
  return server;
}
