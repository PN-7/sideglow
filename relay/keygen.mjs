// SPDX-License-Identifier: MPL-2.0
import { randomBytes } from 'node:crypto';
process.stdout.write(`${randomBytes(32).toString('base64url')}\n`);
