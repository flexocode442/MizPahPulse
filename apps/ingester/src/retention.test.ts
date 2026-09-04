import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  getRetentionCutoff,
  parseRetentionDays,
  runEventRetention,
  type RetentionDb,
} from './retention';

test('getRetentionCutoff subtracts retention days', () => {
  const now = new Date('2026-09-04T12:00:00Z');
  const cutoff = getRetentionCutoff(90, now);
  assert.equal(cutoff.toISOString(), '2026-06-06T12:00:00.000Z');
});

test('parseRetentionDays returns null when disabled or invalid', () => {
  assert.equal(parseRetentionDays(undefined), null);
  assert.equal(parseRetentionDays(''), null);
  assert.equal(parseRetentionDays('0'), null);
  assert.equal(parseRetentionDays('-5'), null);
  assert.equal(parseRetentionDays('abc'), null);
});

test('parseRetentionDays parses a positive number of days', () => {
  assert.equal(parseRetentionDays('90'), 90);
  assert.equal(parseRetentionDays('90.9'), 90);
});

test('runEventRetention deletes stale events in bounded batches', async () => {
  const staleIds = Array.from({ length: 1200 }, (_, i) => `evt-${i}`);
  const deletedIds: string[] = [];
  const db: RetentionDb = {
    findMany: async ({ take }) => {
      const slice = staleIds.slice(deletedIds.length, deletedIds.length + take);
      return slice.map((id) => ({ id }));
    },
    deleteMany: async ({ where }) => {
      deletedIds.push(...where.id.in);
      return { count: where.id.in.length };
    },
  };

  const result = await runEventRetention(90, 500, db, new Date('2026-09-04T12:00:00Z'));

  assert.equal(result.retentionDays, 90);
  assert.equal(result.deleted, 1200);
  assert.equal(result.batches, 3);
  assert.equal(deletedIds.length, 1200);
  assert.equal(new Set(deletedIds).size, 1200);
});

test('runEventRetention stops when nothing is stale', async () => {
  const db: RetentionDb = {
    findMany: async () => [],
    deleteMany: async () => ({ count: 0 }),
  };

  const result = await runEventRetention(90, 500, db, new Date());
  assert.equal(result.deleted, 0);
  assert.equal(result.batches, 0);
});
