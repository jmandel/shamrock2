// Deterministic Nostr fixture: ephemeral events expire, and subscriptions honor
// `since`. This catches clock skew that the ordinary WS-signaling harness misses.
import assert from 'node:assert/strict';
import { WebSocketServer } from 'ws';
import { chromium, firefox } from 'playwright';
import { serve } from './serve.mjs';
import { randomUUID } from 'node:crypto';

const server = await serve({ port: 8867, relayPort: 8868, quiet: true });
const relay = new WebSocketServer({ port: 8869, host: '127.0.0.1' });
const subscriptions = new Map();
const counters = { accepted: 0, expired: 0 };
relay.on('connection', (socket) => {
  const subs = new Map(); subscriptions.set(socket, subs);
  socket.on('close', () => subscriptions.delete(socket));
  socket.on('message', (bytes) => {
    const [type, value, ...filters] = JSON.parse(bytes);
    if (type === 'REQ') { subs.set(value, filters); socket.send(JSON.stringify(['EOSE', value])); }
    else if (type === 'CLOSE') subs.delete(value);
    else if (type === 'EVENT') {
      const valid = Math.abs(value.created_at - Date.now() / 1000) < 5;
      socket.send(JSON.stringify(['OK', value.id, valid, valid ? '' : 'invalid: ephemeral event expired']));
      if (!valid) { counters.expired++; return; }
      counters.accepted++;
      for (const [recipient, peerSubscriptions] of subscriptions.entries()) {
        for (const [id, filters] of peerSubscriptions) if (filters.some((f) => value.created_at >= f.since && f.kinds.includes(value.kind) && value.tags.some(([key, tag]) => key === 'x' && f['#x'].includes(tag)))) {
          recipient.send(JSON.stringify(['EVENT', id, value]));
        }
      }
    }
  });
});
const chrome = await chromium.launch({ executablePath: process.env.CHROMIUM ?? undefined });
const fox = process.env.FIREFOX ? await firefox.launch({ executablePath: process.env.FIREFOX }) : null;
const errors = [];
try {
  for (const offset of [0, -180000, 180000]) {
    const room = randomUUID(), contexts = [];
    try {
      const pages = await Promise.all([chrome, fox || chrome].map(async (browser, i) => {
        const context = await browser.newContext({ timezoneId: i ? 'America/Los_Angeles' : 'America/Chicago' }); contexts.push(context);
        await context.addInitScript((offset) => { const now = Date.now.bind(Date); window.__originalDateNow = now; Date.now = () => now() + offset; }, i ? offset : 0);
        const page = await context.newPage();
        page.on('pageerror', (error) => errors.push(error.message));
        await page.goto(`http://localhost:8867/?relays=ws://localhost:8869#${room}`);
        await page.waitForFunction(() => window.__cl);
        return page;
      }));
      await Promise.all(pages.map((p) => p.waitForFunction(() => __cl.peerCount() === 1, null, { timeout: 30000 })));
      await pages[0].evaluate(() => __cl.joinAs('Ana'));
      await pages[1].evaluate(() => __cl.joinAs('Ben'));
      await Promise.all(pages.map((p) => p.waitForFunction(() => __cl.ui().dealable.length === 2)));
      const { current: report } = await pages[1].evaluate(() => __cl.connectionReport());
      assert.equal(report.settings.discoveryClock, 'server');
      assert(Math.abs(report.settings.clockOffsetMs + offset) < 2000);
      assert(report.relays[0].accepted > 0);
      assert.equal(report.relays[0].rejections, 0);
      assert(report.app.receivedPackets > 0);
      assert.equal(counters.expired, 0);
      if (offset) {
        await pages[1].evaluate(() => { Date.now = window.__originalDateNow; });
        assert((await pages[1].evaluate(() => __cl.connectionReport())).current.elapsedMs > 0);
        let refreshed;
        for (let i = 0; i < 40; i++) {
          refreshed = (await pages[1].evaluate(() => __cl.connectionReport())).current;
          if (Math.abs(refreshed.settings.clockOffsetMs) < 2000 && refreshed.events.some((e) => e.kind === 'discovery-clock')) break;
          await new Promise((resolve) => setTimeout(resolve, 250));
        }
        assert(refreshed.events.some((e) => e.kind === 'discovery-clock'), JSON.stringify(refreshed.settings));
        assert(Math.abs(refreshed.settings.clockOffsetMs) < 2000);
      }
      console.log(`PASS ${fox ? 'Firefox/Chrome' : 'Chrome'} discovery and game sync across Chicago/Los Angeles time zones with a ${offset / 1000}s clock offset`);
    } finally { await Promise.all(contexts.map((c) => c.close())); }
  }
  assert(counters.accepted > 0);
  assert.deepEqual(errors, []);
} finally {
  await chrome.close(); await fox?.close();
  for (const socket of relay.clients) socket.terminate();
  await new Promise((resolve) => relay.close(resolve));
  await server.close();
}
