/**
 * Database seed script for development and testing.
 * Populates the database with representative sample data.
 *
 * Usage:
 *   npx tsx packages/database/prisma/seed.ts
 */

import { PrismaClient } from '@prisma/client';
import { v4 as uuidv4 } from 'uuid';

const prisma = new PrismaClient();

const SAMPLE_ACCOUNTS = [
  'GABC1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ',
  'GDEF1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ',
  'GHIJ1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ',
  'GKLM1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ',
  'GNOP1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ',
];

const SAMPLE_CONTRACTS = [
  'CA7G1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ',
  'CB3X1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ',
  'CD9Y1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ',
];

const EVENT_TYPES = [
  'PAYMENT',
  'CREATE_ACCOUNT',
  'SOROBAN_INVOKE',
  'DEX_TRADE',
  'NFT_TRANSFER',
  'TOKEN_TRANSFER',
  'LIQUIDITY_POOL_DEPOSIT',
  'MANAGE_BUY_OFFER',
  'CLAWBACK',
];

async function main() {
  console.log('🌱 Seeding database...');

  // Clean up existing data
  await prisma.monitoredEvent.deleteMany();
  await prisma.event.deleteMany();
  await prisma.transaction.deleteMany();
  await prisma.webhookDelivery.deleteMany();
  await prisma.webhookSubscription.deleteMany();
  await prisma.apiKey.deleteMany();
  await prisma.monitoredWallet.deleteMany();
  await prisma.asset.deleteMany();
  await prisma.auditLog.deleteMany();
  await prisma.notificationPreference.deleteMany();
  await prisma.user.deleteMany();

  console.log('  ✓ Cleaned existing data');

  // Seed assets
  const assets = [
    {
      code: 'USDC',
      issuer: 'GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN',
      type: 'CREDIT_ALPHANUM12',
    },
    {
      code: 'USDT',
      issuer: 'GCQTGZQQ5ANDQ6NGTRFNOCN4R5ZOIUM4JBF7AGVYMTEGFY6MOY5KY6CJ',
      type: 'CREDIT_ALPHANUM12',
    },
    {
      code: 'EURMTL',
      issuer: 'GACS6TAAA65RQCINSOWTR4WVKUCOP7HCC5HV7PXBJEW4MKIAICXPYJWX',
      type: 'CREDIT_ALPHANUM12',
    },
  ];

  for (const asset of assets) {
    await prisma.asset.upsert({
      where: { code_issuer: { code: asset.code, issuer: asset.issuer } },
      update: asset,
      create: asset,
    });
  }
  console.log(`  ✓ Created ${assets.length} assets`);

  // Seed a webhook subscription
  const webhook = await prisma.webhookSubscription.create({
    data: {
      userId: 'demo-user',
      endpoint: 'https://example.com/webhooks/stellar',
      secret: `whsec_${uuidv4()}`,
      events: JSON.stringify(['PAYMENT', 'DEX_TRADE', 'SOROBAN_INVOKE']),
      isActive: true,
    },
  });
  console.log('  ✓ Created demo webhook subscription');

  // Seed the demo user (issue #55). Existing tables reference `userId` as a
  // plain string; the demo rows use 'demo-user' to stay compatible.
  await prisma.user.upsert({
    where: { address: SAMPLE_ACCOUNTS[0] },
    update: {},
    create: {
      id: 'demo-user',
      address: SAMPLE_ACCOUNTS[0],
      displayName: 'Demo User',
    },
  });
  console.log('  ✓ Created demo user');

  // Seed events spread across 7 days (issue #54). Each day follows a
  // realistic hourly pattern: more activity during business hours (08:00-
  // 21:00), quiet overnight. The seed cleans up existing rows first, so
  // re-running it is idempotent.
  const now = new Date();
  const DAY_MS = 24 * 60 * 60 * 1000;
  const HOUR_MS = 60 * 60 * 1000;
  const events = [];

  let index = 0;
  for (let day = 6; day >= 0; day--) {
    // Anchor each day at local midnight so the hourly pattern maps to real
    // wall-clock hours: busy during 08:00–21:00, quiet overnight.
    const dayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate() - day);
    for (let hour = 0; hour < 24; hour++) {
      const isPeak = hour >= 8 && hour <= 21;
      const perHour = isPeak ? 3 : 1;
      for (let slot = 0; slot < perHour; slot++) {
        const eventType = EVENT_TYPES[index % EVENT_TYPES.length] || 'PAYMENT';
        const accountId = SAMPLE_ACCOUNTS[index % SAMPLE_ACCOUNTS.length];
        const contractId =
          eventType === 'SOROBAN_INVOKE'
            ? SAMPLE_CONTRACTS[index % SAMPLE_CONTRACTS.length]
            : undefined;
        const timestamp = new Date(dayStart.getTime() + hour * HOUR_MS + slot * 20 * 60_000);
        // Never seed events in the future (today's remaining slots).
        if (timestamp.getTime() > now.getTime()) continue;

        events.push({
          eventType,
          source: index % 2 === 0 ? 'HORIZON' : 'SOROBAN_RPC',
          category:
            eventType === 'PAYMENT'
              ? 'PAYMENT'
              : eventType === 'SOROBAN_INVOKE'
                ? 'CONTRACT'
                : eventType === 'DEX_TRADE'
                  ? 'DEX'
                  : eventType === 'NFT_TRANSFER'
                    ? 'NFT'
                    : eventType === 'TOKEN_TRANSFER'
                      ? 'TOKEN'
                      : 'ACCOUNT',
          transactionHash: `0x${uuidv4().replace(/-/g, '')}`,
          ledgerSequence: BigInt(5_000_000 + index),
          pagingToken: `paging-${uuidv4()}`,
          timestamp,
          accountId,
          contractId,
          assetCode: ['PAYMENT', 'TOKEN_TRANSFER'].includes(eventType) ? 'XLM' : undefined,
          amount: ['PAYMENT', 'TOKEN_TRANSFER'].includes(eventType)
            ? String(Math.floor(Math.random() * 1000) + 1)
            : undefined,
          payload: {
            type: eventType,
            source_account: accountId,
          },
          processedAt: timestamp,
          createdAt: timestamp,
          updatedAt: timestamp,
        });
        index++;
      }
    }
  }

  // Create the parent Transaction rows first: the Event → Transaction
  // foreign key requires each referenced hash to exist. Without this the
  // seed fails with a P2003 constraint violation on every fresh database.
  const transactions = events.map((event) => ({
    hash: event.transactionHash,
    sourceAccount: event.accountId ?? SAMPLE_ACCOUNTS[0],
    fee: String(Math.floor(Math.random() * 500) + 100), // stroops
    operationCount: Math.floor(Math.random() * 5) + 1,
    successful: true,
    ledgerSequence: event.ledgerSequence,
    createdAt: event.timestamp,
    envelopeXdr: null,
    resultXdr: null,
    signatures: JSON.stringify([]),
  }));
  await prisma.transaction.createMany({ data: transactions, skipDuplicates: true });
  console.log(`  ✓ Created ${transactions.length} transactions`);

  for (const event of events) {
    await prisma.event.create({ data: event });
  }
  console.log(`  ✓ Created ${events.length} events spanning 7 days`);

  // Seed audit logs
  const auditActions = ['API_REQUEST', 'WEBHOOK_DELIVERY', 'WALLET_CONNECT', 'CONTRACT_INVOKE'];
  for (let i = 0; i < 20; i++) {
    await prisma.auditLog.create({
      data: {
        userId: 'demo-user',
        action: auditActions[i % auditActions.length] || 'API_REQUEST',
        resource: 'event',
        resourceId: uuidv4(),
        details: { seed: true, index: i },
        ipAddress: '127.0.0.1',
        userAgent: 'SeedScript/1.0',
        createdAt: new Date(now.getTime() - i * 60_000),
      },
    });
  }
  console.log('  ✓ Created 20 audit log entries');

  // Seed notification preferences
  await prisma.notificationPreference.create({
    data: {
      userId: 'demo-user',
      channels: JSON.stringify(['websocket', 'email']),
      events: JSON.stringify(['PAYMENT', 'SOROBAN_INVOKE', 'NFT_TRANSFER']),
      enabled: true,
    },
  });
  console.log('  ✓ Created notification preferences');

  console.log('\n✅ Database seeded successfully!');
}

main()
  .catch((e) => {
    console.error('❌ Seed failed:', e);
    process.exit(1);
  })
  .finally(async () => {
    await prisma.$disconnect();
  });
