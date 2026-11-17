// SPDX-License-Identifier: MPL-2.0
import { createHash } from 'node:crypto';
import { mkdtemp, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

export const CHANNEL = '123456789012345678';
export const OTHER = '223456789012345678';
export const KEY = 'A'.repeat(43);
export function config() {
  return {
    channelId: CHANNEL, botToken: 'test-bot-token-not-real',
    webhookUrl: `https://discord.com/api/webhooks/${CHANNEL}/TEST-NOT-REAL`,
    listen: { host: '127.0.0.1', port: 8787 },
    users: [{ id: 'alice', displayName: '앨리스 · Overlay', keyHash: createHash('sha256').update(KEY).digest() }],
  };
}
export function message(id = '323456789012345678', extra = {}) {
  return {
    id, channel_id: CHANNEL, content: '원문', timestamp: '2026-11-11T00:00:00.000Z',
    author: { id: '423456789012345678', username: 'Alice', bot: false }, ...extra,
  };
}
export async function temporary(t) {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'sideglow-relay-test-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return directory;
}
export async function until(predicate, timeoutMs = 1000) {
  const start = Date.now();
  while (!predicate()) {
    if (Date.now() - start > timeoutMs) throw new Error('Timed out waiting for test state');
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}
