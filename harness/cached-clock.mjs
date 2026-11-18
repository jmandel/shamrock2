// Two devices whose copies of network.json come from a cache of different
// ages. GitHub Pages (Fastly) stamps `Date` when it serves a copy and adds
// `Age`; other caches keep the origin's `Date`. Either way both devices must
// still find each other.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { serve } from './serve.mjs';
import { serveNostr } from './nostr-relay.mjs';
import { randomUUID } from 'node:crypto';

const AGE_S = 400;
const server = await serve({ port: 8881, relayPort: 8882, quiet: true });
// Like relay.primal.net and relay.snort.social: tolerate skew, honour `since`.
const relay = await serveNostr({ port: 8883, maxClockSkewMs: 15 * 60000 });
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM ?? undefined });
const errors = [];
let failed = false;
try {
  for (const cache of ['github-pages', 'origin-date']) {
    const room = randomUUID(), contexts = [];
    try {
      const pages = await Promise.all([0, AGE_S].map(async (age) => {
        const context = await browser.newContext(); contexts.push(context);
        await context.route(/network\.json/, async (route) => {
          const response = await route.fetch();
          const date = cache === 'github-pages' ? Date.now() : Date.now() - age * 1000;
          await route.fulfill({ response, headers: { ...response.headers(), date: new Date(date).toUTCString(), age: String(age) } });
        });
        const page = await context.newPage();
        page.on('pageerror', (error) => errors.push(error.message));
        await page.goto(`http://localhost:8881/?relays=ws://localhost:8883#${room}`);
        await page.waitForFunction(() => window.__cl);
        return page;
      }));
      const started = Date.now();
      const met = await Promise.all(pages.map((p) => p.waitForFunction(() => __cl.peerCount() === 1, null, { timeout: 20000 }).then(() => true, () => false)));
      const offsets = await Promise.all(pages.map(async (p) => (await p.evaluate(() => __cl.connectionReport())).current.settings.clockOffsetMs / 1000));
      const ok = met.every(Boolean);
      console.log(`${ok ? 'PASS' : 'FAIL'} ${cache} cache, Age 0 and ${AGE_S}: ${ok ? `met in ${((Date.now() - started) / 1000).toFixed(1)}s` : 'never met'}; offsets ${offsets.join('s, ')}s`);
      if (!ok) failed = true;
    } finally { await Promise.all(contexts.map((c) => c.close())); }
  }
  assert.deepEqual(errors, []);
  assert(!failed, 'devices with differently aged cached clocks must meet');
} finally {
  await browser.close();
  await relay.close();
  await server.close();
}
