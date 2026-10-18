// One group link, staggered arrivals, separate browser storage and real
// WebRTC. Identity is selected through the UI; clues use the existing tools.
// CHROMIUM=/usr/bin/chromium node harness/invites.mjs
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright';
import { serve } from './serve.mjs';

const PORT = 8951, RELAY = 8952;
const base = `http://localhost:${PORT}/?signal=ws://localhost:${RELAY}&rejoin=2000`;
const server = await serve({ port: PORT, relayPort: RELAY, quiet: true });
let browser;
const errors = [];
const out = new URL('./out/invites/', import.meta.url).pathname;
const tool = (p, name, args = {}) => p.evaluate(([n, a]) => window.__cl.tools[n](a), [name, args]);
const table = (p) => tool(p, 'get_table');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pass = (text) => console.log(`PASS  ${text}`);

async function context(viewport = { width: 390, height: 844 }) {
  const ctx = await browser.newContext({ viewport });
  await ctx.addInitScript(() => {
    for (const k of ['write', 'guess']) localStorage.setItem(`shamrock:tip:${k}`, '1');
    // A usual name from another room must not choose a seat in this invite.
    localStorage.setItem('shamrock:name', 'Ana');
  });
  return ctx;
}
async function open(ctx, url) {
  const p = await ctx.newPage();
  p.on('pageerror', (e) => { errors.push(e.message); console.error(`Page error: ${e.message}`); });
  await p.goto(url);
  await p.waitForFunction(() => window.__cl?.tools);
  return p;
}
async function group(names) {
  const ctx = await context();
  const page = await open(ctx, base);
  await page.fill('#homeName', names[0]);
  await page.fill('#homeRoom', 'meet-later');
  await page.check('#homePlan');
  await page.fill('#homeFriends', names.slice(1).join('\n'));
  await page.click('#homeInvite');
  await page.waitForURL((u) => u.searchParams.has('i') && !!u.hash);
  await page.waitForFunction((name) => window.__cl?.ui().seated && window.__cl.ui().name === name, names[0]);
  return { ctx, page, url: page.url() };
}
async function choose(page, name) {
  await page.waitForSelector('#joinSheet.open');
  await page.locator('.inviteSeat').filter({ has: page.locator('strong', { hasText: `I'm ${name}` }) }).click();
  await page.waitForFunction((n) => window.__cl.ui().seated && window.__cl.ui().name === n, name);
}
async function write(page, name, leaves = ['top', 'right', 'bottom', 'left']) {
  for (const leaf of leaves) assert.equal((await tool(page, 'write_clue', { leaf, text: `${name} ${leaf}` })).ok, true);
}
async function saveAndAge(page, days = 30) {
  // Wait for the latest snapshot, not just any previously saved state.
  await page.waitForFunction(() => {
    const r = decodeURIComponent(location.hash.slice(1));
    const kept = JSON.parse(localStorage.getItem(`shamrock:${r}:state`) || 'null');
    const X = window.__cl.X, n = X.snapshot();
    const s = btoa(String.fromCharCode(...new Uint8Array(X.memory.buffer, X.scratch_ptr(), n)))
      .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    return kept?.s === s;
  });
  await page.evaluate((days) => {
    const k = `shamrock:${decodeURIComponent(location.hash.slice(1))}:state`;
    const kept = JSON.parse(localStorage.getItem(k));
    kept.at = Date.now() - days * 86400_000;
    localStorage.setItem(k, JSON.stringify(kept));
  }, days);
}
async function agree(pages) {
  // Wait for a common digest, including auto-proceed stamps and identities.
  const until = Date.now() + 20_000;
  do {
    const ds = await Promise.all(pages.map((p) => p.evaluate(() => JSON.stringify(window.__cl.digest()))));
    if (ds.every((d) => d === ds[0])) return;
    await sleep(100);
  } while (Date.now() < until);
  assert.fail('tables did not converge');
}

async function place(page, card, slot, answer = false) {
  await page.evaluate(({ card, slot, answer }) => {
    const g = window.__cl, X = g.X;
    g.settle();
    const [x, y] = g.card(card), [tx, ty] = g.slot(slot);
    X.pointer_down(x, y);
    for (let k = 1; k <= 12; k++) X.pointer_move(x + (tx - x) * k / 12, y + (ty - y) * k / 12);
    X.pointer_up(tx, ty);
    if (answer) for (let k = 0; k < 4; k++) {
      if (g.digest().puzzles[g.ui().k].cards[card][2] === g.answerRot(slot)) break;
      g.settle();
      const [cx, cy] = g.card(card);
      X.pointer_down(cx, cy); X.pointer_up(cx, cy);
    }
  }, { card, slot, answer });
}

async function solve(page) {
  for (let s = 0; s < 4; s++) await place(page, s, s, true);
  const checked = await tool(page, 'check_guess', { clover: (await table(page)).clover.index });
  assert.equal(checked.ok, true, checked.result);
  await page.waitForFunction(() => window.__cl.ui().done);
}

try {
  await mkdir(out, { recursive: true });
  browser = await chromium.launch({ executablePath: process.env.CHROMIUM ?? undefined,
    args: ['--disable-features=WebRtcHideLocalIpsWithMdns'] });
  const names = ['Ana', 'Ben', 'Cy', 'Dee'];
  const creator = await group(names);
  const invite = creator.url;
  const contexts = [creator.ctx];
  await write(creator.page, 'Ana', ['top']);
  await creator.page.click('#menuBtn');
  assert.equal(await creator.page.textContent('#linkText'), invite);
  await creator.page.evaluate(() => { navigator.clipboard.writeText = async (s) => { window.copiedInvite = s; }; });
  await creator.page.click('#copyLink');
  assert.equal(await creator.page.evaluate(() => window.copiedInvite), invite);
  await creator.page.click('#closeMenu');
  await saveAndAge(creator.page);
  await creator.page.close();
  pass('creator reserves four clovers and copies one group link before leaving');

  for (const name of names.slice(1)) {
    const ctx = await context(); contexts.push(ctx);
    const p = await open(ctx, invite);
    await p.waitForSelector('#joinSheet.open');
    assert.equal(await p.locator('#inviteJoinTitle').textContent(), 'Who are you?');
    assert.deepEqual(await p.locator('.inviteSeat strong').allTextContents(), names.map((n) => `I'm ${n}`));
    assert.equal(await p.locator('#joinNameField').isVisible(), false);
    assert.equal(await p.locator('#joinSubmit').isVisible(), false);
    await sleep(1700);
    assert.equal((await table(p)).seated, false, 'a name remembered from another table cannot auto-join');
    if (name === 'Ben') await p.screenshot({ path: `${out}choose-name-phone.png` });
    await choose(p, name);
    assert.equal((await table(p)).players.length, 4);
    assert.equal(await p.evaluate(() => window.__cl.peerCount()), 0);
    await write(p, name);
    assert.equal((await tool(p, 'submit_clover')).ok, true);
    await sleep(300);
    assert.equal((await table(p)).phase, 'writing', 'a lone submission must wait for the reserved group');
    await saveAndAge(p);
    await p.close();
  }
  pass('three friends choose their names and submit separately while everyone else is offline');

  const pages = await Promise.all(contexts.map((ctx) => open(ctx, invite)));
  await Promise.all(pages.map((p, k) => p.waitForFunction((n) => window.__cl.ui().seated && window.__cl.ui().name === n, names[k])));
  await Promise.all(pages.map((p) => p.waitForFunction(() => window.__cl.ui().readyCount === 3)));
  await agree(pages);
  for (const [k, p] of pages.entries()) {
    const t = await table(p);
    assert.equal(t.phase, 'writing');
    assert.equal(t.clover.clues.top, `${names[k]} top`);
    assert.equal(await p.evaluate(() => window.__cl.digest().dealt.length), 4);
  }
  await write(pages[0], 'Ana', ['right', 'bottom', 'left']);
  await tool(pages[0], 'submit_clover');
  await Promise.all(pages.map((p) => p.waitForFunction(() => window.__cl.ui().phase === 'guessing' && window.__cl.ui().n === 4)));
  await agree(pages);
  pass('month-old drafts restore, all four submissions merge, and guessing starts with all four clovers');

  const lateCtx = await context(), late = await open(lateCtx, invite);
  await late.waitForFunction(() => window.__cl.ui().phase === 'guessing');
  await late.waitForSelector('#joinSheet.open');
  await late.locator('.inviteSeat').filter({ has: late.locator('strong', { hasText: "I'm Ana" }) }).click();
  assert.equal(await late.locator('#seatConfirm').isVisible(), true);
  assert.equal((await table(late)).seated, false);
  assert.match(await late.textContent('#seatConfirmText'), /Ana is already playing/);
  await late.screenshot({ path: `${out}occupied-seat-phone.png` });
  await late.setViewportSize({ width: 740, height: 360 });
  await late.click('#seatChooseAnother');
  assert.equal(await late.locator('.inviteSeat').count(), 4);
  await late.locator('.inviteSeat').filter({ has: late.locator('strong', { hasText: "I'm Ben" }) }).click();
  await late.click('#seatConfirmJoin');
  assert.equal((await table(late)).you, 'Ben');
  assert.equal((await table(late)).phase, 'guessing');
  await lateCtx.close();
  await pages[1].close();
  const home = await open(contexts[1], base);
  const savedLink = home.locator('.tableRow').filter({ hasText: new URL(invite).hash.slice(1) });
  assert.equal(await savedLink.getAttribute('href'), invite);
  await savedLink.click();
  await home.waitForFunction(() => window.__cl?.ui().seated && window.__cl.ui().name === 'Ben');
  assert.equal((await table(home)).phase, 'guessing');
  for (const ctx of contexts) await ctx.close();
  pass('original and saved table links retain progress and identity; occupied names require deliberate confirmation');

  // Only Ben and Cy reconvene; Ana returns during guessing, Dee after
  // the available clovers have all been solved. Everyone uses one link.
  const partial = await group(names);
  await partial.ctx.close();
  const groupCtx = await Promise.all(names.map(() => context()));
  const live = [];
  for (const k of [1, 2]) {
    live[k] = await open(groupCtx[k], partial.url);
    await choose(live[k], names[k]);
    await write(live[k], names[k]);
    await tool(live[k], 'submit_clover');
  }
  await live[1].waitForFunction(() => window.__cl.ui().readyCount === 2);
  assert.equal(await live[1].textContent('#alt'), 'Start anyway');
  await live[1].click('#alt');
  await Promise.all([1, 2].map((k) => live[k].waitForFunction(() => window.__cl.ui().phase === 'guessing' && window.__cl.ui().n === 2)));
  await place(live[2], 0, 3);
  await agree([live[1], live[2]]);
  const current = await live[1].evaluate(() => window.__cl.digest().puzzles[window.__cl.ui().k]);
  live[0] = await open(groupCtx[0], partial.url);
  await choose(live[0], 'Ana');
  await live[0].waitForFunction(() => window.__cl.ui().strip.go?.label === 'Add my clover');
  assert.equal((await table(live[0])).phase, 'writing');
  assert.ok(Object.values((await table(live[0])).clover.leafWords).every(Boolean));
  await sleep(600);
  await live[0].screenshot({ path: `${out}late-writer-phone.png` });
  for (const [width, height] of [[320, 568], [390, 844]]) {
    await live[0].setViewportSize({ width, height });
    assert.equal(await live[0].evaluate(() => ['go', 'alt'].every((id) => {
      const b = document.getElementById(id), r = b.getBoundingClientRect();
      return r.x >= 0 && r.right <= innerWidth && b.scrollWidth <= b.clientWidth;
    })), true, `both late-arrival actions fit at ${width} px`);
  }
  await live[0].click('#alt');
  assert.equal((await table(live[0])).phase, 'guessing');
  assert.equal((await table(live[0])).clover.writer, 'Ben');
  assert.equal(await live[0].textContent('#alt'), 'Write my clover');
  await live[0].click('#alt');
  await write(live[0], 'Ana', ['top', 'right']);
  await saveAndAge(live[0]);
  await live[0].close();
  live[0] = await open(groupCtx[0], partial.url);
  await live[0].waitForFunction(() => window.__cl.ui().name === 'Ana' && window.__cl.ui().strip.go?.label === 'Add my clover');
  assert.equal((await table(live[0])).clover.clues.top, 'Ana top');
  await write(live[0], 'Ana', ['bottom', 'left']);
  await live[0].click('#go');
  await Promise.all(live.map((p) => p.waitForFunction(() => window.__cl.ui().phase === 'guessing' && window.__cl.ui().n === 3)));
  await agree(live);
  for (const p of live) {
    assert.equal((await table(p)).clover.writer, 'Ben');
    assert.deepEqual(await p.evaluate(() => window.__cl.digest().puzzles[window.__cl.ui().k]), current);
  }
  pass('two friends start; a late writer can guess, resume a saved draft and add it without changing the current board');
  for (let k = 0; k < 3; k++) {
    const who = (await table(live[0])).clover.writer;
    const p = live.find((_, i) => names[i] !== who);
    await solve(p);
    await agree(live);
    await tool(p, 'next_clover');
    await agree(live);
  }
  await Promise.all(live.map((p) => p.waitForSelector('#recapSheet.open')));
  live[3] = await open(groupCtx[3], partial.url);
  await choose(live[3], 'Dee');
  await live[3].waitForFunction(() => window.__cl.ui().strip.go?.label === 'Add my clover');
  await write(live[3], 'Dee');
  await live[3].click('#go');
  await Promise.all(live.map((p) => p.waitForFunction(() => window.__cl.ui().phase === 'guessing' && window.__cl.ui().n === 4)));
  await agree(live);
  for (const p of live) {
    assert.equal((await table(p)).clover.writer, 'Dee');
    await p.waitForFunction(() => getComputedStyle(document.getElementById('recapSheet')).opacity === '0'
      && getComputedStyle(document.getElementById('recapSheet')).pointerEvents === 'none');
    assert.equal(await p.evaluate(() => window.__cl.digest().puzzles.filter((p) => p.mask === 15).length), 3);
  }
  await solve(live[0]);
  await agree(live);
  await tool(live[0], 'next_clover');
  await Promise.all(live.map((p) => p.waitForSelector('#recapSheet.open')));
  assert.equal(await live[0].evaluate(() => window.__cl.digest().puzzles.filter((p) => p.mask === 15).length), 4);
  for (const ctx of groupCtx) await ctx.close();
  pass('a submission after the recap reopens guessing, preserves three solved clovers and ends with all four solved');

  // Ordinary tables also retain old saves. No invitation is needed.
  const normalCtx = await context();
  const ordinary = `${base}#ordinary-${Date.now()}`;
  let normal = await open(normalCtx, ordinary);
  await normal.waitForSelector('#joinSheet.open');
  await normal.fill('#nameInput', 'Normal');
  await normal.click('#joinBtn');
  await normal.click('#go');
  await write(normal, 'Normal', ['top']);
  await saveAndAge(normal, 365);
  await normal.close();
  normal = await open(normalCtx, ordinary);
  await normal.waitForFunction(() => window.__cl.ui().seated);
  assert.equal((await table(normal)).clover.clues.top, 'Normal top');
  await normal.click('#menuBtn');
  await normal.click('#newGameBtn');
  await saveAndAge(normal);
  await normal.close();
  normal = await open(normalCtx, ordinary);
  await normal.waitForFunction(() => window.__cl.ui().seated);
  assert.equal((await table(normal)).phase, 'lobby');
  await normalCtx.close();
  pass('a year-old ordinary game restores; an explicit New game stays reset after reopening');

  const many = ['Alexanderthecloverwriter', ...Array.from({ length: 11 }, (_, i) => `Friend ${i + 1}`)];
  const max = await group(many);
  await max.page.close();
  const lastCtx = await context({ width: 360, height: 640 });
  const last = await open(lastCtx, max.url);
  await last.waitForSelector('#joinSheet.open');
  assert.equal(await last.locator('.inviteSeat').count(), 12);
  assert.equal(await last.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await last.screenshot({ path: `${out}twelve-names-phone.png` });
  for (const [width, height] of [[320, 568], [740, 360]]) {
    await last.setViewportSize({ width, height });
    await last.locator('.inviteSeat').last().scrollIntoViewIfNeeded();
    assert.equal(await last.evaluate(() => {
      const b = document.querySelector('.inviteSeat:last-child'), r = b.getBoundingClientRect();
      return b.contains(document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2));
    }), true, `last seat is reachable at ${width}×${height}`);
  }
  await choose(last, many[11]);
  assert.equal((await table(last)).players.length, 12);
  assert.equal((await table(last)).you, many[11]);
  await lastCtx.close(); await max.ctx.close();
  pass('twelve full names fit a scrollable picker on a small phone and the last seat opens its clover');

  const invalidCtx = await context();
  const invalid = await open(invalidCtx, base);
  await invalid.check('#homePlan');
  await invalid.fill('#homeName', 'Ana');
  await invalid.fill('#homeFriends', 'ana\nBen');
  await invalid.click('#homeInvite');
  assert.match(await invalid.textContent('#homeError'), /different name/);
  assert.equal(new URL(invalid.url()).hash, '');
  await invalid.close();
  const wrong = new URL(invite); wrong.hash = 'wrong-room';
  const broken = new URL(invite); broken.searchParams.set('i', 'broken!');
  const truncated = new URL(invite); truncated.hash = '';
  for (const link of [wrong.href, broken.href, truncated.href]) {
    const p = await open(invalidCtx, link);
    await p.waitForSelector('#inviteErrorSheet.open');
    assert.equal(await p.evaluate(() => window.__cl.peerCount()), 0);
    await p.close();
  }
  await invalidCtx.close();
  const blockedCtx = await context();
  await blockedCtx.addInitScript(() => {
    const set = Storage.prototype.setItem;
    window.allowStateSave = false;
    Storage.prototype.setItem = function(k, v) {
      if (k.endsWith(':state') && !window.allowStateSave) throw new DOMException('full', 'QuotaExceededError');
      return set.call(this, k, v);
    };
  });
  const blocked = await open(blockedCtx, invite);
  await choose(blocked, 'Cy');
  await write(blocked, 'Cy', ['top']);
  await blocked.waitForFunction(() => document.getElementById('toast').textContent.includes("couldn't save"));
  await blocked.evaluate(() => { window.allowStateSave = true; });
  await saveAndAge(blocked);
  await blockedCtx.close();
  assert.deepEqual(errors, []);
  pass('duplicate identities and damaged or mismatched links fail clearly, without opening another game');
  pass('failed local saves are reported and retried when storage becomes available');
} finally {
  await browser?.close();
  await server.close();
}
