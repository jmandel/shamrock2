// Browser HTTP cache regression: reloading the page used to keep a fresh but
// outdated core.wasm for ten minutes, including after the Reload warning.
// Use the real protocol-4 engine from history (or OLD_WASM for a shallow clone).
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { chromium, firefox } from 'playwright';
import { createWsRelayServer } from '@trystero-p2p/ws-relay/server';

const oldEngine = process.env.OLD_WASM ? await readFile(process.env.OLD_WASM)
  : execFileSync('git', ['show', 'b8924bc:web/core.wasm'], { maxBuffer: 4 * 1024 * 1024 });
const protocol = async (wasm) => (await WebAssembly.instantiate(wasm, {})).instance.exports.protocol();
const currentProtocol = await protocol(await readFile(new URL('../web/core.wasm', import.meta.url)));
assert.equal(await protocol(oldEngine), 4);
assert(currentProtocol > 4);
let old = true;
const requests = [], errors = [];
const server = createServer(async (req, res) => {
  const path = new URL(req.url, 'http://localhost').pathname;
  try {
    const wasm = path === '/core.wasm';
    const body = wasm && old ? oldEngine : await readFile(new URL(`../web${path === '/' ? '/index.html' : path}`, import.meta.url));
    const etag = `"${createHash('sha256').update(body).digest('hex')}"`;
    const fresh = req.headers['if-none-match'] === etag;
    const headers = {
      'content-type': wasm ? 'application/wasm' : path.endsWith('.js') ? 'text/javascript' : path.endsWith('.json') ? 'application/json' : 'text/html',
      'cache-control': 'max-age=600', etag,
    };
    if (wasm) requests.push({ old, status: fresh ? 304 : 200 });
    res.writeHead(fresh ? 304 : 200, headers); res.end(fresh ? undefined : body);
  } catch { res.writeHead(404).end(); }
});
await new Promise((resolve) => server.listen(8891, '127.0.0.1', resolve));
const relay = createWsRelayServer({ port: 8892, host: '127.0.0.1' }); await relay.ready;
const chrome = await chromium.launch({ executablePath: process.env.CHROMIUM });
const fox = process.env.FIREFOX ? await firefox.launch({ executablePath: process.env.FIREFOX }) : null;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(fn, label) {
  const start = performance.now();
  while (performance.now() - start < 30000) { if (await fn()) return; await sleep(200); }
  assert.fail(label);
}
const tool = (p, name, args = {}) => p.evaluate(([name, args]) => __cl.tools[name](args), [name, args]);
try {
  for (const browser of [chrome, fox].filter(Boolean)) {
    old = true;
    const contexts = [], pages = [], room = randomUUID();
    const url = `http://localhost:8891/?signal=ws://localhost:8892#${room}`;
    const open = async (name) => {
      const context = await browser.newContext(); contexts.push(context);
      const p = await context.newPage(); p.on('pageerror', (e) => errors.push(e.message));
      await p.goto(url); await p.waitForFunction(() => window.__cl?.tools);
      await tool(p, 'join_table', { name }); return p;
    };
    try {
      pages.push(await open('Ana'), await open('Ben'));
      await until(async () => (await Promise.all(pages.map((p) => p.evaluate(() => __cl.ui())))).every((t) => t.players.length === 2), 'old engines connect');
      // Protocol 4 predates the new roster checklist, so use its original Deal.
      await pages[0].evaluate(() => __cl.X.action(1));
      await until(async () => (await pages[1].evaluate(() => __cl.ui())).phase === 'writing', 'old engines enter clueing');
      for (const [i, p] of pages.entries()) await tool(p, 'write_clue', { leaf: 'top', text: `kept ${i}` });
      await sleep(2500);
      old = false;
      await pages[1].close({ runBeforeUnload: true });
      pages[1] = await open('Ben'); // A fresh browser receives the new release.
      await pages[0].waitForFunction(() => document.getElementById('toast').textContent.includes('A newer version'));
      assert.equal(await pages[0].evaluate(() => __cl.X.protocol()), 4);
      const beforeReload = requests.length;
      await pages[0].getByRole('button', { name: 'Reload', exact: true }).click();
      await pages[0].waitForFunction((expected) => window.__cl?.X.protocol() === expected, currentProtocol, { timeout: 10000 });
      assert(requests.slice(beforeReload).some((r) => !r.old && r.status === 200), 'Reload fetches the updated engine despite its fresh cached predecessor');
      await until(async () => (await tool(pages[0], 'get_table')).you === 'Ana', 'the returning player retakes their seat');
      for (const [i, p] of pages.entries()) {
        assert.equal((await tool(p, 'get_table')).clover.clues.top, `kept ${i}`, 'saved clues survive the engine upgrade');
        assert.equal((await p.evaluate(() => __cl.connectionReport())).current.protocolMismatch, false);
        assert.equal((await tool(p, 'write_clue', { leaf: 'right', text: `after ${i}` })).ok, true);
      }
      await until(async () => {
        const d = await Promise.all(pages.map((p) => p.evaluate(() => JSON.stringify(__cl.digest()))));
        return d[0] === d[1];
      }, 'the upgraded player can send and receive new clues');
      // A healthy reload still checks for updates, but reuses unchanged bytes.
      const beforeHealthyReload = requests.length;
      await pages[0].reload(); await pages[0].waitForFunction(() => window.__cl);
      assert(requests.slice(beforeHealthyReload).some((r) => !r.old && r.status === 304), 'unchanged engines use HTTP revalidation');
      console.log(`PASS ${browser === fox ? 'Firefox' : 'Chrome'}: genuine version warning → Reload → current engine, preserved clues, two-way edits; unchanged reload uses 304`);
    } finally { await Promise.all(contexts.map((c) => c.close())); }
  }
  assert.deepEqual(errors, []);
  console.log('ALL PASS');
} finally { await chrome.close(); await fox?.close(); await relay.close(); server.close(); }
