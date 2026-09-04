import { Queue, Worker } from 'bullmq';
import Redis from 'ioredis';
import {
  getNetworkConfig,
  createHorizonServer,
  getSorobanRpc,
  categorizeEventType,
  mapToEventType,
  normalizeEventPayload,
} from '@mizpah-pulse/stellar';
import { prisma, Prisma } from '@mizpah-pulse/database';
import { v4 as uuidv4 } from 'uuid';
import { startWebhookWorker } from './webhook-worker';
import { startHealthServer, updateHealth, recordProcessedEvent } from './health-check';
import { runEventRetention, parseRetentionDays, RETENTION_INTERVAL_MS } from './retention';
import type { RawStellarEvent } from '@mizpah-pulse/types';

/**
 * MizpahPulse Event Ingestion Engine
 *
 * Monitors Stellar blockchain activity via Horizon SSE and Soroban RPC polling.
 * Pushes raw events into a Redis-backed queue for async processing.
 */

// ──────────────────────────────────────────────
// Queue Configuration
// ──────────────────────────────────────────────
const REDIS_URL = process.env.REDIS_URL || 'redis://localhost:6379';

const connection = {
  url: REDIS_URL,
};

const RAW_EVENT_QUEUE = 'raw-stellar-events';
const PROCESSED_EVENT_QUEUE = 'processed-events';
const DEAD_LETTER_QUEUE = 'dead-letter-events';

const rawEventQueue = new Queue<RawStellarEvent>(RAW_EVENT_QUEUE, {
  connection,
  defaultJobOptions: {
    attempts: 3,
    backoff: { type: 'exponential', delay: 1000 },
    removeOnComplete: 1000,
    removeOnFail: 5000,
  },
});

const processedEventQueue = new Queue(PROCESSED_EVENT_QUEUE, { connection });
const deadLetterQueue = new Queue(DEAD_LETTER_QUEUE, { connection });

// Redis Pub/Sub client (lazy init in main)
let pubClient: Redis | null = null;
const REDIS_CHANNEL = 'mizpah-pulse:events';

// Track active timers for graceful shutdown
const activeTimers: ReturnType<typeof setInterval>[] = [];

// ──────────────────────────────────────────────
// Horizon SSE Stream
// ──────────────────────────────────────────────

// Guards against duplicate streams. The old implementation called
// startHorizonStream() again from the error handler without closing the
// previous stream, so every reconnect leaked another SSE connection (and
// each one re-queued the same transactions).
let horizonStreamActive = false;
let horizonReconnectAttempts = 0;

async function startHorizonStream() {
  if (horizonStreamActive) return;
  horizonStreamActive = true;

  const config = getNetworkConfig();
  console.log(`[Ingester] Starting Horizon SSE stream on ${config.network} (${config.horizonUrl})`);

  let lastPagingToken: string | null = null;

  // Fetch latest ledger to start streaming from
  try {
    const server = createHorizonServer();
    const latestLedger = await server.ledgers().order('desc').limit(1).call();
    if (latestLedger.records[0]) {
      lastPagingToken = latestLedger.records[0].paging_token;
      console.log(`[Ingester] Starting from ledger paging token: ${lastPagingToken}`);
    }
  } catch (err) {
    console.warn('[Ingester] Could not fetch latest ledger, starting from "now"');
  }

  const cursor = lastPagingToken ?? 'now';
  updateHealth({ horizonConnected: true });

  const streamServer = createHorizonServer();

  // Reset the backoff counter once the stream has been up for a while.
  const resetBackoff = setInterval(() => {
    horizonReconnectAttempts = 0;
    clearInterval(resetBackoff);
  }, 5 * 60_000);
  resetBackoff.unref?.();

  let closed = false;
  let lastMessageAt = Date.now();
  const closeStream = () => {
    if (closed) return;
    closed = true;
    horizonStreamActive = false;
    clearInterval(watchdog);
  };

  // Watchdog: if the SSE connection goes silent (no heartbeat or message) for
  // 90 seconds, treat it as a dead stream and restart. Horizon SSE connections
  // normally emit keepalives, so this only fires when the socket was dropped
  // without an error event.
  const watchdog = setInterval(() => {
    if (closed) return;
    if (Date.now() - lastMessageAt > 90_000) {
      console.warn('[Ingester] Horizon SSE stream silent — restarting');
      updateHealth({ horizonConnected: false });
      closeStream();
      setTimeout(startHorizonStream, 5000);
    }
  }, 30_000);
  watchdog.unref?.();

  streamServer
    .transactions()
    .cursor(cursor)
    .stream({
      onmessage: async (tx) => {
        lastMessageAt = Date.now();
        const rawEvent: RawStellarEvent = {
          source: 'HORIZON',
          type: 'transaction',
          payload: tx as unknown as Record<string, unknown>,
          capturedAt: new Date().toISOString(),
          pagingToken: tx.paging_token,
          transactionHash: tx.id,
        };

        await rawEventQueue.add(`horizon-${uuidv4()}`, rawEvent);
      },
      onerror: (err) => {
        console.error('[Ingester] Horizon SSE error:', err);
        updateHealth({ horizonConnected: false });
        closeStream();
        // Attempt reconnect with capped exponential backoff (5s → 10s → 20s … max 5m)
        const delay = Math.min(5000 * 2 ** horizonReconnectAttempts, 300_000);
        horizonReconnectAttempts++;
        setTimeout(startHorizonStream, delay);
      },
    });
}

// ──────────────────────────────────────────────
// Soroban RPC Event Polling
// ──────────────────────────────────────────────
async function startSorobanPolling() {
  const config = getNetworkConfig();
  console.log(`[Ingester] Starting Soroban RPC polling on ${config.sorobanRpcUrl}`);

  const { getSorobanRpc } = await import('@mizpah-pulse/stellar');
  const rpc = getSorobanRpc();

  let lastLedger = 0;

  try {
    const latest = await rpc.getLatestLedger();
    lastLedger = latest.sequence;
    console.log(`[Ingester] Starting Soroban poll from ledger: ${lastLedger}`);
  } catch {
    console.warn('[Ingester] Could not get latest Soroban ledger, starting from 0');
  }

  const timer = setInterval(async () => {
    try {
      const response = await rpc.getEvents({
        startLedger: lastLedger > 0 ? lastLedger - 1 : undefined,
        limit: 100,
        filters: [],
      });

      if (response.events && response.events.length > 0) {
        for (const evt of response.events) {
          const rawEvent: RawStellarEvent = {
            source: 'SOROBAN_RPC',
            type: 'contract_event',
            payload: evt as unknown as Record<string, unknown>,
            capturedAt: new Date().toISOString(),
            pagingToken: evt.pagingToken,
            transactionHash: evt.txHash,
          };

          await rawEventQueue.add(`soroban-${uuidv4()}`, rawEvent);
        }

        const maxLedger = Math.max(...response.events.map((e) => e.ledger));
        if (maxLedger > lastLedger) {
          lastLedger = maxLedger;
        }

        console.log(
          `[Ingester] Polled ${response.events.length} Soroban events, ledger: ${lastLedger}`,
        );
      }
    } catch (err) {
      console.error('[Ingester] Soroban polling error:', err);
    }
  }, 5000);

  activeTimers.push(timer);
  return () => clearInterval(timer);
}

// ──────────────────────────────────────────────
// Event Processing Worker
// ──────────────────────────────────────────────
const eventProcessor = new Worker<RawStellarEvent>(
  RAW_EVENT_QUEUE,
  async (job) => {
    const rawEvent = job.data;
    console.log(`[Processor] Processing event: ${rawEvent.type} from ${rawEvent.source}`);

    try {
      // Deduplication check
      if (rawEvent.pagingToken) {
        const existing = await prisma.event.findUnique({
          where: { pagingToken: rawEvent.pagingToken },
        });
        if (existing) {
          console.log(`[Processor] Duplicate event skipped: ${rawEvent.pagingToken}`);
          return { status: 'duplicate' };
        }
      }

      // Parse and normalize the event
      const category = categorizeEventType(rawEvent.type);
      const eventType = mapToEventType(rawEvent.type);
      const normalizedPayload = normalizeEventPayload(rawEvent.payload);

      // Store processed event
      const stored = await prisma.event.create({
        data: {
          eventType,
          source: rawEvent.source,
          category,
          transactionHash: rawEvent.transactionHash ?? 'unknown',
          ledgerSequence: BigInt(
            (normalizedPayload.ledger as number) ?? (normalizedPayload.ledger_attr as number) ?? 0,
          ),
          pagingToken: rawEvent.pagingToken ?? `gen-${uuidv4()}`,
          timestamp: rawEvent.capturedAt,
          accountId: normalizedPayload.source_account as string,
          payload: normalizedPayload as Prisma.InputJsonValue,
        },
      });

      console.log(`[Processor] Stored event: ${stored.id} (${eventType})`);
      recordProcessedEvent();

      // Publish to Redis Pub/Sub for real-time WebSocket broadcast
      try {
        await pubClient?.publish(
          REDIS_CHANNEL,
          JSON.stringify({
            channel: 'feed:all',
            eventType: stored.eventType,
            data: {
              eventId: stored.id,
              category: stored.category,
              accountId: stored.accountId,
              amount: stored.amount,
              assetCode: stored.assetCode,
              timestamp: stored.timestamp,
            },
            timestamp: new Date().toISOString(),
            sequence: Date.now(),
          }),
        );
      } catch (pubErr) {
        console.error('[Processor] Redis publish error:', pubErr);
      }

      // Queue for downstream processing (notifications, webhooks, WebSocket broadcast)
      await processedEventQueue.add(`processed-${stored.id}`, {
        eventId: stored.id,
        eventType: stored.eventType,
        category: stored.category,
      });

      return { status: 'processed', eventId: stored.id };
    } catch (err) {
      console.error(`[Processor] Error processing event:`, err);
      throw err;
    }
  },
  {
    connection,
    concurrency: 5,
    limiter: { max: 50, duration: 1000 },
  },
);

// Dead letter handler
eventProcessor.on('failed', async (job, err) => {
  if (job && job.attemptsMade >= (job.opts.attempts ?? 3)) {
    console.error(`[Processor] Event failed permanently: ${job.id}`, err);
    await deadLetterQueue.add(`dlq-${job.id}`, { ...job.data, error: err.message });
  }
});

// ──────────────────────────────────────────────
// Startup
// ──────────────────────────────────────────────
async function main() {
  console.log('═══════════════════════════════════════════');
  console.log('  MizpahPulse — Event Ingestion Engine');
  console.log('═══════════════════════════════════════════');

  // Verify database connection
  try {
    await prisma.$connect();
    console.log('[Ingester] Database connected');
  } catch (err) {
    console.error('[Ingester] Database connection failed:', err);
    process.exit(1);
  }

  // Verify Redis connection
  try {
    const redis = new Redis(REDIS_URL);
    await redis.ping();
    await redis.quit();
    console.log('[Ingester] Redis connected');

    // Initialize Redis Pub/Sub client for broadcasting processed events
    pubClient = new Redis(REDIS_URL);
    await pubClient.ping();
    console.log('[Ingester] Redis Pub/Sub connected');
  } catch (err) {
    console.error('[Ingester] Redis connection failed:', err);
    process.exit(1);
  }

  // Start Horizon SSE streaming
  startHorizonStream().catch((err) =>
    console.error('[Ingester] Horizon stream failed to start:', err),
  );

  // Start Soroban event polling
  startSorobanPolling().catch((err) =>
    console.error('[Ingester] Soroban polling failed to start:', err),
  );

  // Mark Soroban polling as live once it initializes
  updateHealth({ sorobanPolling: true });

  console.log('[Ingester] Event ingestion engine is running');

  // Start webhook delivery worker
  const stopWebhookWorker = startWebhookWorker();

  // Start health server (exposes /health via HEALTH_PORT, default 8080)
  const stopHealthServer = startHealthServer();

  // Start event retention/pruning (issue #53). Disabled unless
  // EVENT_RETENTION_DAYS is set; runs once on startup, then on an interval.
  await startRetentionJob();

  // ──────────────────────────────────────────────
  // Event Retention / Pruning (issue #53)
  // ──────────────────────────────────────────────
  async function startRetentionJob(): Promise<void> {
    const retentionDays = parseRetentionDays(process.env.EVENT_RETENTION_DAYS);
    if (retentionDays === null) {
      console.log(
        '[Retention] Disabled — set EVENT_RETENTION_DAYS (e.g. 90) to enable pruning of old events',
      );
      return;
    }

    const run = async () => {
      try {
        const result = await runEventRetention(retentionDays);
        console.log(
          `[Retention] Pruned ${result.deleted} events older than ${result.retentionDays} days ` +
            `(cutoff ${result.cutoff.toISOString()}, ${result.batches} batch(es))`,
        );
      } catch (err) {
        console.error('[Retention] Pruning error:', err);
      }
    };

    await run();
    const timer = setInterval(run, RETENTION_INTERVAL_MS);
    activeTimers.push(timer);
  }

  // Periodically refresh queue depth for health reporting
  const queueTimer = setInterval(async () => {
    try {
      const counts = await rawEventQueue.getJobCounts();
      updateHealth({ queueSize: (counts.waiting ?? 0) + (counts.active ?? 0) });
    } catch {
      // Health metrics are best-effort
    }
  }, 30_000);
  activeTimers.push(queueTimer);

  // Graceful shutdown
  const shutdown = async () => {
    console.log('[Ingester] Shutting down...');
    stopWebhookWorker();
    stopHealthServer();
    activeTimers.forEach((t) => clearInterval(t));
    await pubClient?.quit();
    await rawEventQueue.close();
    await processedEventQueue.close();
    await deadLetterQueue.close();
    await eventProcessor.close();
    await prisma.$disconnect();
    process.exit(0);
  };

  process.on('SIGTERM', shutdown);
  process.on('SIGINT', shutdown);
}

main();
