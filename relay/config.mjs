// SPDX-License-Identifier: MPL-2.0
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';

const SNOWFLAKE = /^[0-9]{16,20}$/;
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

function requiredSecret(env, name, label) {
  if (!ENV_NAME.test(name ?? '')) throw new Error(`${label}: invalid environment variable name`);
  const value = env[name]?.trim();
  if (!value) throw new Error(`${label}: environment variable ${name} is not set`);
  return value;
}

export function validateConfig(raw, baseDirectory, env = process.env) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('Configuration must be a JSON object');
  if (!SNOWFLAKE.test(raw.channelId ?? '')) throw new Error('channelId must be a Discord channel ID');
  const botToken = requiredSecret(env, raw.botTokenEnv ?? 'DISCORD_BOT_TOKEN', 'Bot token');
  const webhookUrl = requiredSecret(env, raw.webhookUrlEnv ?? 'DISCORD_WEBHOOK_URL', 'Webhook URL');
  let webhook;
  try { webhook = new URL(webhookUrl); } catch { throw new Error('Webhook URL is invalid'); }
  if (webhook.protocol !== 'https:' || webhook.hostname !== 'discord.com' || webhook.port ||
      webhook.username || webhook.password || webhook.search || webhook.hash ||
      !/^\/api(?:\/v10)?\/webhooks\/[0-9]{16,20}\/[A-Za-z0-9._-]+$/.test(webhook.pathname)) {
    throw new Error('Webhook URL must be an HTTPS discord.com incoming webhook URL, without query parameters');
  }
  if (!Array.isArray(raw.users) || raw.users.length < 1 || raw.users.length > 100) {
    throw new Error('Configure between 1 and 100 users');
  }
  const ids = new Set();
  const keys = new Set();
  const users = raw.users.map((user) => {
    if (!/^[A-Za-z0-9_-]{1,64}$/.test(user.id ?? '') || ids.has(user.id)) {
      throw new Error('Every user needs a unique id (letters, digits, underscore, or hyphen)');
    }
    if (typeof user.displayName !== 'string' || !user.displayName.trim() || user.displayName.length > 80 ||
        /[\u0000-\u001f\u007f]/.test(user.displayName)) throw new Error(`Invalid displayName for ${user.id}`);
    const key = requiredSecret(env, user.accessKeyEnv, `Access key for ${user.id}`);
    if (!/^[A-Za-z0-9_-]{32,256}$/.test(key)) {
      throw new Error(`Access key for ${user.id} must contain 32–256 URL-safe characters; generate a random key`);
    }
    const keyHash = createHash('sha256').update(key).digest();
    const hex = keyHash.toString('hex');
    if (keys.has(hex)) throw new Error('Each user must have a different access key');
    keys.add(hex);
    ids.add(user.id);
    return Object.freeze({ id: user.id, displayName: user.displayName.trim(), keyHash });
  });
  const host = raw.listen?.host ?? '127.0.0.1';
  const port = raw.listen?.port ?? 8787;
  if (typeof host !== 'string' || !host || host.length > 253 || /\s/.test(host)) throw new Error('Invalid listen.host');
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('listen.port must be 1–65535');
  let tls;
  if (raw.listen?.tls) {
    const value = raw.listen.tls;
    if (typeof value.keyFile !== 'string' || !value.keyFile || typeof value.certFile !== 'string' || !value.certFile) {
      throw new Error('TLS requires keyFile and certFile');
    }
    tls = { keyFile: path.resolve(baseDirectory, value.keyFile), certFile: path.resolve(baseDirectory, value.certFile) };
  } else if (host !== '127.0.0.1' && host !== '::1') {
    throw new Error('Plain HTTP may bind only to 127.0.0.1 or ::1; configure TLS for remote access');
  }
  if (raw.stateDirectory !== undefined && (typeof raw.stateDirectory !== 'string' || !raw.stateDirectory)) {
    throw new Error('stateDirectory must be a directory path');
  }
  return Object.freeze({
    channelId: raw.channelId, botToken, webhookUrl,
    users, listen: { host, port, tls },
    stateDirectory: path.resolve(baseDirectory, raw.stateDirectory ?? '.state'),
  });
}

export async function loadConfig(filename, env = process.env) {
  const absolute = path.resolve(filename);
  let raw;
  try { raw = JSON.parse(await readFile(absolute, 'utf8')); }
  catch (error) { throw new Error(`Cannot read configuration: ${error.code ?? 'invalid JSON'}`); }
  return validateConfig(raw, path.dirname(absolute), env);
}
