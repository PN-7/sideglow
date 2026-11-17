// SPDX-License-Identifier: MPL-2.0
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';

export class SendError extends Error {
  constructor(status, message, { uncertain = false, retryable = false, retryAfter } = {}) {
    super(message);
    this.status = status;
    this.uncertain = uncertain;
    this.retryable = retryable;
    this.retryAfter = retryAfter;
  }
}

const UNCERTAIN = '전송 결과를 확인할 수 없습니다. 중복 방지를 위해 재전송하지 않았습니다. Discord 채널에서 먼저 확인하세요.';

/** Persist intent before sending: a crash cannot silently replay a potentially delivered webhook POST. */
export class NonceLedger {
  #entries = new Map();
  #pending = new Map();
  #writeQueue = Promise.resolve();
  constructor(directory, { ttlMs = 24 * 60 * 60 * 1000, maxEntries = 10000, clock = Date.now } = {}) {
    this.directory = directory;
    this.file = path.join(directory, 'nonce-state.json');
    this.ttlMs = ttlMs;
    this.maxEntries = maxEntries;
    this.clock = clock;
  }
  async open() {
    await mkdir(this.directory, { recursive: true, mode: 0o700 });
    try {
      const text = await readFile(this.file, 'utf8');
      if (text.length > 8 * 1024 * 1024) throw new Error('Nonce state exceeds its size limit');
      const stored = JSON.parse(text);
      if (stored.version !== 1 || !Array.isArray(stored.entries) || stored.entries.length > this.maxEntries) {
        throw new Error('Nonce state has an unsupported format');
      }
      for (const entry of stored.entries) {
        if (!entry || typeof entry.key !== 'string' || typeof entry.fingerprint !== 'string' ||
            !Number.isFinite(entry.created) || !['pending', 'sent', 'failed', 'uncertain'].includes(entry.state)) {
          throw new Error('Nonce state is damaged');
        }
        if (entry.state === 'pending') entry.state = 'uncertain';
        this.#entries.set(entry.key, entry);
      }
      this.#prune();
      await this.#persist();
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
  }
  #prune() {
    const cutoff = this.clock() - this.ttlMs;
    for (const [key, entry] of this.#entries) {
      if (entry.created < cutoff && !this.#pending.has(key)) this.#entries.delete(key);
    }
  }
  #persist() {
    const text = JSON.stringify({ version: 1, entries: [...this.#entries.values()] });
    const task = this.#writeQueue.catch(() => {}).then(async () => {
      const temporary = `${this.file}.${process.pid}.tmp`;
      await writeFile(temporary, text, { encoding: 'utf8', mode: 0o600, flush: true });
      await rename(temporary, this.file);
    });
    this.#writeQueue = task;
    return task;
  }
  async run(userId, nonce, content, send) {
    this.#prune();
    const key = `${userId}:${nonce}`;
    const fingerprint = createHash('sha256').update(content).digest('hex');
    const existing = this.#entries.get(key);
    if (existing) {
      if (existing.fingerprint !== fingerprint) throw new SendError(409, '같은 전송 번호로 다른 문장을 보낼 수 없습니다.');
      const active = this.#pending.get(key);
      if (active) return { ...(await active), replayed: true };
      if (existing.state === 'sent') return { id: existing.id, replayed: true };
      if (existing.state === 'uncertain' || existing.state === 'pending') throw new SendError(409, UNCERTAIN, { uncertain: true });
      throw new SendError(existing.status ?? 502, existing.error ?? '전송하지 못했습니다.');
    }
    if (this.#entries.size >= this.maxEntries) throw new SendError(503, '전송 기록이 가득 찼습니다. 잠시 후 다시 시도하세요.', { retryable: true });
    const entry = { key, fingerprint, created: this.clock(), state: 'pending' };
    this.#entries.set(key, entry);
    const work = (async () => {
      try {
        await this.#persist();
      } catch {
        entry.state = 'failed';
        entry.status = 503;
        entry.error = '안전한 전송 기록을 저장하지 못해 메시지를 보내지 않았습니다.';
        throw new SendError(entry.status, entry.error);
      }
      try {
        const result = await send();
        if (!result || !/^[0-9]{16,20}$/.test(result.id ?? '')) throw new SendError(502, UNCERTAIN, { uncertain: true });
        entry.state = 'sent';
        entry.id = result.id;
        // If this write fails, disk still says pending; restart treats that as uncertain, never as safe to resend.
        await this.#persist();
        return { id: result.id, replayed: false };
      } catch (error) {
        if (entry.state === 'sent') throw new SendError(503, 'Discord에 전송되었지만 전송 기록 저장에 실패했습니다. 채널에서 확인하세요.', { uncertain: true });
        const failure = error instanceof SendError ? error : new SendError(502, UNCERTAIN, { uncertain: true });
        if (failure.retryable && !failure.uncertain) this.#entries.delete(key);
        else {
          entry.state = failure.uncertain ? 'uncertain' : 'failed';
          entry.status = failure.status;
          entry.error = failure.uncertain ? UNCERTAIN : failure.message;
        }
        try { await this.#persist(); } catch { /* The persisted pending marker remains conservative. */ }
        throw failure;
      }
    })();
    this.#pending.set(key, work);
    try { return await work; }
    finally { this.#pending.delete(key); }
  }
}
