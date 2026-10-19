// A real multi-device table: review next-round players, retain away phones,
// select a subset, persist it across reloads/dealers, and reject stale dialogs.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { serve } from './serve.mjs';
const PORT = 8993, RELAY = 8994;
const server = await serve({ port: PORT, relayPort: RELAY, quiet: true });
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM, args: ['--disable-features=WebRtcHideLocalIpsWithMdns'] });
const url = `http://localhost:${PORT}/?signal=ws://localhost:${RELAY}#roster-${Date.now()}`;
const errors = [], devices = [];
const out = new URL('./out/rounds/', import.meta.url).pathname;
await mkdir(out, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(fn) { for (let i = 0; i < 150; i++) { if (await fn()) return; await sleep(100); } throw Error('condition did not settle'); }
async function device(name, at = url) {
  const ctx = await browser.newContext({ viewport: { width: 384, height: 760 }, deviceScaleFactor: 1.5, hasTouch: true, colorScheme: 'dark' });
  const page = await ctx.newPage(); page.on('pageerror', (e) => errors.push(`${name}: ${e.message}`));
  await page.goto(at); await page.waitForFunction(() => window.__cl?.tools);
  const d = { name, ctx, page, tool: (name, args = {}) => page.evaluate(([n, a]) => window.__cl.tools[n](a), [name, args]), digest: () => page.evaluate(() => window.__cl.digest()) };
  await d.tool('join_table', { name }); devices.push(d); return d;
}
async function rows(d) {
  return d.page.locator('#dealList label').evaluateAll((els) => els.map((e) => ({ name: e.querySelector('b').textContent, selected: e.querySelector('input').checked, note: e.querySelector('small').textContent })));
}
const checkbox = (d, name) => d.page.locator('#dealList label').filter({ has: d.page.locator('b', { hasText: new RegExp(`^${name}$`) }) }).locator('input');
async function menuDeal(d) {
  await d.page.evaluate(() => document.querySelectorAll('.overlay').forEach((e) => e.classList.remove('open')));
  await d.page.click('#menuBtn'); await d.page.click('#redealBtn'); await d.page.waitForSelector('#dealSheet.open');
}
async function solve(d) {
  const t = await d.tool('get_table');
  for (let s = 0; s < 4; s++) {
    const label = await d.page.evaluate((s) => window.__cl.agentView().cards.find((c) => c._i === s).card, s);
    await d.tool('place_card', { card: label, spot: s, clover: t.clover.index });
    const want = await d.page.evaluate((s) => window.__cl.answerRot(s), s);
    for (let k = 0; k < 4; k++) {
      const dg = await d.digest(), c = dg.puzzles[+dg.phase.split(':')[1]].cards[s];
      if (c[0] === 'slot' && c[2] === want) break;
      await d.tool('turn_card', { card: label, clover: t.clover.index });
    }
  }
  assert.equal((await d.tool('check_guess', { clover: t.clover.index })).ok, true);
  await d.tool('next_clover');
}
try {
  const A = await device('Ana'), B = await device('Ben'), C = await device('Carrie');
  await A.page.waitForFunction(() => window.__cl.ui().players.length === 3 && window.__cl.peerCount() === 2);
  await A.page.click('#go'); await A.page.waitForSelector('#dealSheet.open');
  assert.equal((await rows(A)).filter((r) => r.selected).length, 3);
  assert.equal((await A.digest()).round, 0, 'opening the picker does not deal');
  await A.page.click('#dealConfirm');
  await until(async () => (await B.digest()).round === 1);
  await C.ctx.close();
  await A.page.waitForFunction(() => window.__cl.ui().players.find((p) => p.name === 'Carrie')?.away);
  for (const d of [A, B]) {
    for (const leaf of ['top', 'right', 'bottom', 'left']) await d.tool('write_clue', { leaf, text: `${d.name} ${leaf}` });
    await d.tool('submit_clover');
  }
  await A.page.click('#alt');
  await until(async () => (await A.tool('get_table')).phase === 'guessing');
  for (let k = 0; k < 2; k++) {
    const t = await A.tool('get_table'); const d = t.clover.isMine ? B : A;
    await until(async () => (await d.tool('get_table')).clover.index === t.clover.index);
    await solve(d);
    await until(async () => { const now = await A.tool('get_table'); return now.phase === 'end' || now.clover.index !== t.clover.index; });
  }
  await A.page.waitForSelector('#recapSheet.open');
  const D = await device('Dan');
  await A.page.waitForFunction(() => window.__cl.ui().players.length === 4);
  await A.page.click('#recapNext'); await A.page.waitForSelector('#dealSheet.open');
  let list = await rows(A);
  assert.equal(list.length, 4); assert.ok(list.every((r) => r.selected));
  assert.match(list.find((r) => r.name === 'Carrie').note, /Away.*played last round/);
  assert.match(list.find((r) => r.name === 'Dan').note, /Here.*new/);
  await checkbox(A, 'Ben').uncheck();
  const E = await device('Eve');
  await until(async () => (await rows(A)).length === 5);
  list = await rows(A);
  assert.equal(list.find((r) => r.name === 'Ben').selected, false, 'new arrivals do not reset explicit choices');
  assert.equal(list.find((r) => r.name === 'Eve').selected, true);
  assert.equal(await A.page.textContent('#dealConfirm'), 'Deal to 4 players');
  await A.page.screenshot({ path: out + 'players-384-dark.png' });
  await A.page.setViewportSize({ width: 320, height: 568 });
  await A.page.screenshot({ path: out + 'players-320-dark.png' });
  await A.page.click('#dealConfirm');
  await until(async () => (await D.digest()).round === 2 && (await B.digest()).round === 2);
  const dg = await A.digest(), ben = await B.page.evaluate(() => window.__cl.ui().seat);
  assert.equal(dg.dealt.length, 4); assert.ok(!dg.dealt.includes(ben)); assert.ok(dg.sittingOut.includes(ben));
  assert.equal((await B.tool('get_table')).phase, 'writing');
  assert.equal(await B.page.evaluate(() => window.__cl.ui().inRound), false);
  assert.equal(await B.page.textContent('#hint'), 'Sitting out this round');
  await D.page.reload(); await D.page.waitForFunction(() => window.__cl?.ui().seated);
  await menuDeal(D); list = await rows(D);
  assert.equal(list.find((r) => r.name === 'Ben').selected, false, 'sitout persists after reload on another dealer');
  assert.equal(list.find((r) => r.name === 'Carrie').selected, true, 'away players remain selected next time');
  for (const r of list) await checkbox(D, r.name).uncheck();
  assert.equal(await D.page.locator('#dealConfirm').isDisabled(), true);
  await D.page.click('#dealCancel'); assert.equal((await D.digest()).round, 2);
  await menuDeal(A); await menuDeal(D);
  await checkbox(D, 'Ben').check(); await D.page.click('#dealConfirm');
  await A.page.waitForFunction(() => window.__cl.digest().round === 3 && !document.getElementById('dealSheet').classList.contains('open'));
  assert.equal((await A.digest()).dealt.length, 5);
  assert.deepEqual((await A.digest()).sittingOut, []);
  const L = await device('Player 01', url + '-limit');
  for (let i = 2; i <= 13; i++) await L.tool('join_table', { name: `Player ${String(i).padStart(2, '0')}` });
  await L.page.click('#go'); await L.page.waitForSelector('#dealSheet.open');
  for (const r of await rows(L)) await checkbox(L, r.name).check();
  assert.equal(await L.page.locator('#dealConfirm').isDisabled(), true, '13 selected cannot silently become 12');
  await checkbox(L, 'Player 13').uncheck();
  assert.equal(await L.page.textContent('#dealConfirm'), 'Deal to 12 players');
  await L.page.click('#dealConfirm');
  assert.equal((await L.digest()).dealt.length, 12);
  assert.deepEqual(errors, []);
  console.log('PASS named checklist, away defaults, late arrivals, explicit subset, reload, another dealer, empty selection, cancel, stale dialog, reselection, and 12-player limit');
} finally { await browser.close(); await server.close(); }
