// Two real WebRTC players, using the live game's Nostr transport, leave and
// return during clueing. Check recovery and new game data in both directions.
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { chromium, firefox } from 'playwright';
import { serve } from './serve.mjs';
import { serveNostr } from './nostr-relay.mjs';

const server = await serve({ port: 8871, relayPort: 8872, quiet: true });
const relay = await serveNostr({ port: 8873 });
const chrome = await chromium.launch({ executablePath: process.env.CHROMIUM });
const fox = process.env.FIREFOX ? await firefox.launch({ executablePath: process.env.FIREFOX }) : null;
const errors = [];
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(fn, label, timeout = 65000) {
  const start = performance.now();
  while (performance.now() - start < timeout) { if (await fn()) return; await sleep(200); }
  assert.fail(label);
}
const tool = (p, name, args = {}) => p.evaluate(([name, args]) => __cl.tools[name](args), [name, args]);
const table = (p) => tool(p, 'get_table');
const report = (p) => p.evaluate(async () => (await __cl.connectionReport()).current);

try {
  for (const mode of ['reopen', 'home', 'silent-reopen', 'silent-home', 'live-peer', 'background']) {
    if (process.env.ONLY && mode !== process.env.ONLY) continue;
    if (process.env.PUBLIC && !['reopen', 'home'].includes(mode)) continue;
    const room = randomUUID(), contexts = [], pages = [];
    const url = `http://localhost:8871/${process.env.PUBLIC ? '' : '?relays=ws://localhost:8873'}#${room}`;
    const open = async (context) => {
      const p = await context.newPage();
      p.on('pageerror', (e) => errors.push(e.message));
      await p.goto(url); await p.waitForFunction(() => window.__cl?.tools);
      await until(async () => (await report(p)).relays.some((r) => r.state === 'open'), 'discovery socket opens');
      return p;
    };
    try {
      for (const [i, name] of ['Ana', 'Ben'].entries()) {
        const context = await (i && fox ? fox : chrome).newContext(); contexts.push(context);
        pages.push(await open(context));
        assert.equal((await tool(pages[i], 'join_table', { name })).ok, true);
      }
      await until(async () => (await Promise.all(pages.map(table))).every((t) => t.players.length === 2), 'both players join');
      assert.equal((await tool(pages[0], 'start_round')).ok, true);
      await until(async () => (await table(pages[1])).phase === 'writing', 'both players enter clueing');
      for (const [i, p] of pages.entries()) assert.equal((await tool(p, 'write_clue', { leaf: 'top', text: `kept ${i}` })).ok, true);
      await sleep(2500); // Both committed clues have reached the save timer.
      const original = await report(pages[0]);
      const [stayingSocket] = relay.clients.keys();
      const stayingRelay = relay.clients.get(stayingSocket);
      if (mode.startsWith('silent-') || mode === 'live-peer') stayingRelay.silent = true;
      const start = performance.now();
      if (mode.endsWith('reopen')) {
        await pages[1].close({ runBeforeUnload: true });
        pages[1] = await open(contexts[1]);
      } else if (mode.endsWith('home')) {
        await pages[1].click('#menuBtn');
        await pages[1].click('#homeMenuBtn');
        await pages[1].waitForFunction(() => document.body.classList.contains('home'));
        await pages[1].locator('#tableList .tableRow').filter({ hasText: room }).click();
        await pages[1].waitForFunction(() => window.__cl?.tools && !document.body.classList.contains('home'));
      } else if (mode === 'background') {
        // A pending probe must get a fresh reply window after backgrounding.
        // Visibility is controlled here; the sockets and WebRTC remain real.
        stayingRelay.probeDelayMs = 17000;
        await pages[0].evaluate(() => {
          Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' });
          document.dispatchEvent(new Event('visibilitychange'));
          Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
          document.dispatchEvent(new Event('visibilitychange'));
        });
        await sleep(2000);
        await pages[0].evaluate(() => {
          Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' });
          document.dispatchEvent(new Event('visibilitychange'));
        });
        await sleep(16000);
        stayingRelay.probeDelayMs = 0;
        await pages[0].evaluate(() => {
          Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
          document.dispatchEvent(new Event('visibilitychange'));
        });
      }
      if (mode.startsWith('silent-') || mode === 'live-peer') {
        await until(async () => (await report(pages[0])).events.some((e) => e.kind === 'relay-health-timeout'), 'silent discovery socket gets a retry');
        await until(() => !relay.clients.has(stayingSocket), 'stale discovery socket is replaced');
      }
      await until(async () => (await Promise.all(pages.map(table))).every((t) => t.you && t.players.every((p) => !p.away)), 'both players reconnect and retake their seats');
      for (const [i, p] of pages.entries()) {
        assert.equal((await table(p)).phase, 'writing');
        assert.equal((await table(p)).clover.clues.top, `kept ${i}`, 'clue drafts survive the return');
        assert.equal((await tool(p, 'write_clue', { leaf: 'right', text: `after ${i}` })).ok, true);
      }
      await until(async () => {
        const digests = await Promise.all(pages.map((p) => p.evaluate(() => JSON.stringify(__cl.digest()))));
        return digests[0] === digests[1];
      }, 'new clues travel in both directions');
      const current = await report(pages[0]);
      assert.equal(await pages[0].evaluate(() => __cl.stats.joins), 1, 'the staying player never resets the room or cancels ICE');
      if (mode === 'live-peer' || mode === 'background') {
        assert.equal(current.discovered, original.discovered, 'the healthy peer connection is preserved');
        assert.equal(current.events.filter((e) => e.kind === 'game-peer-left').length, 0);
      }
      if (mode === 'background') {
        assert(!current.events.some((e) => e.kind === 'relay-health-timeout'), 'background time does not cause a false timeout');
        assert(relay.clients.has(stayingSocket), 'a responsive discovery socket stays open');
      }
      console.log(`PASS ${mode}: two players recover the same clues and sync new edits (${((performance.now() - start) / 1000).toFixed(1)}s)`);
    } finally { await Promise.all(contexts.map((c) => c.close())); }
    await until(() => relay.clients.size === 0, 'scenario sockets close');
  }
  assert.deepEqual(errors, []);
  console.log('ALL PASS');
} finally { await chrome.close(); await fox?.close(); await relay.close(); await server.close(); }
