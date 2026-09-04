/**
 * Event retention / pruning for the ingester (issue #53).
 *
 * Old events are deleted in bounded batches so a single query never locks a
 * large slice of the table. The job is disabled by default; set
 * `EVENT_RETENTION_DAYS` (e.g. `90`) to enable pruning of events older than
 * the cutoff.
 */

import { prisma } from '@mizpah-pulse/database';

export const RETENTION_BATCH_SIZE = 500;
export const RETENTION_INTERVAL_MS = 6 * 60 * 60 * 1000; // every 6 hours

export interface RetentionResult {
  retentionDays: number;
  cutoff: Date;
  deleted: number;
  batches: number;
}

/** Minimal Prisma surface used by `runEventRetention` (injectable for tests). */
export interface RetentionDb {
  findMany(args: {
    where: { timestamp: { lt: Date } };
    select: { id: true };
    take: number;
  }): Promise<Array<{ id: string }>>;
  deleteMany(args: { where: { id: { in: string[] } } }): Promise<{ count: number }>;
}

/** Compute the retention cutoff (`now - retentionDays`). */
export function getRetentionCutoff(retentionDays: number, now: Date = new Date()): Date {
  return new Date(now.getTime() - retentionDays * 24 * 60 * 60 * 1000);
}

/**
 * Parse `EVENT_RETENTION_DAYS`. Returns `null` when unset or invalid so the
 * job stays disabled; a positive number of days enables pruning.
 */
export function parseRetentionDays(raw: string | undefined): number | null {
  if (raw === undefined || raw.trim() === '') return null;
  const days = Number(raw);
  if (!Number.isFinite(days) || days <= 0) return null;
  return Math.floor(days);
}

/**
 * Delete events older than `retentionDays` in batches of `batchSize`.
 * Returns a summary for logging. Safe to call repeatedly; converges when no
 * stale events remain.
 */
export async function runEventRetention(
  retentionDays: number,
  batchSize: number = RETENTION_BATCH_SIZE,
  // The Prisma event delegate structurally matches `RetentionDb` for the
  // specific call shapes used here; the cast keeps the narrow interface for
  // testability while defaulting to the real client in production.
  db: RetentionDb = prisma as unknown as RetentionDb,
  now: Date = new Date(),
): Promise<RetentionResult> {
  const cutoff = getRetentionCutoff(retentionDays, now);
  let deleted = 0;
  let batches = 0;

  for (;;) {
    const stale = await db.findMany({
      where: { timestamp: { lt: cutoff } },
      select: { id: true },
      take: batchSize,
    });
    if (stale.length === 0) break;
    await db.deleteMany({ where: { id: { in: stale.map((e) => e.id) } } });
    deleted += stale.length;
    batches++;
    if (stale.length < batchSize) break;
  }

  return { retentionDays, cutoff, deleted, batches };
}
