// Layout audit: drive the app through every screen and sheet with stress
// data (24-character names, a 32-character table name, 40-character clues,
// six players), and at each one check the page at phone, landscape, tablet
// and desktop sizes for generic layout faults:
//   offscreen   a visible element sticks out past the screen edge
//   hscroll     the page scrolls sideways
//   clipped     text is cut off with no "…" (ellipsis is reported apart)
//   spills      text runs outside its own box
//   covered     a control's centre is under something else
//   overlap     two controls overlap
//   small       a control smaller than 32 px across
//   sheet       an open sheet is taller than the screen but can't scroll
//   squeezed    canvas text squashed sideways (beyond its 60% shrink)
// Screenshots go to harness/out/layout/. Exit 1 if any fault is found.
//
//   node harness/layout.mjs          ONLY=state runs the audit at states matching it
import { chromium } from 'playwright';
import { mkdir, rm } from 'node:fs/promises';
import { serve } from './serve.mjs';

const PORT = 8951, RELAY = 8952;
const OUT = new URL('./out/layout/', import.meta.url).pathname;
await rm(OUT, { recursive: true, force: true });
await mkdir(OUT, { recursive: true });
await serve({ port: PORT, relayPort: RELAY, quiet: true });
const browser = await chromium.launch({ args: ['--disable-features=WebRtcHideLocalIpsWithMdns'] });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const until = async (fn, ms = 20000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { try { if (await fn()) return true; } catch {} await sleep(150); } return false; };
const ROOM = 'the-extraordinarily-long-table-x'.slice(0, 32);
const base = `http://localhost:${PORT}/?signal=ws://localhost:${RELAY}`;
const url = (extra = '') => `${base}${extra}#${ROOM}`;
const VIEWPORTS = [[320, 568], [360, 640], [390, 844], [740, 360], [820, 1180], [1280, 800]];
const NAMES = ['Bartholomew Wigglesworth', 'Maximiliana Rosencrantz', 'WWWWWWWWWWWWWWWWWWWWWWWW', 'Christopher Montgomery', 'Anastasia Featherstonehaugh'.slice(0, 24), 'Jo'];
const CLUES = ['internationalization', 'the quick brown fox jumps over lazy dogs', 'Constantinople', 'tea'];
const errors = [];

// Canvas text: remember how much each string had to be squeezed sideways.
const canvasProbe = () => {
  window.__squeeze = new Map();
  const fill = CanvasRenderingContext2D.prototype.fillText;
  CanvasRenderingContext2D.prototype.fillText = function (text, x, y, maxw) {
    if (maxw !== undefined) {
      const w = this.measureText(text).width;
      if (w > maxw * 1.02) {
        const k = `${text} @ ${this.font.split(' ').slice(-2).join(' ')}`;
        window.__squeeze.set(k, Math.min(window.__squeeze.get(k) ?? 1, maxw / w));
      }
    }
    return fill.call(this, text, x, y, maxw);
  };
};

async function device(label, vp = [390, 844]) {
  const ctx = await browser.newContext({ viewport: { width: vp[0], height: vp[1] }, deviceScaleFactor: 1, hasTouch: true });
  await ctx.addInitScript(canvasProbe);
  const d = { label, ctx };
  d.open = async (u) => {
    d.page = await ctx.newPage();
    d.page.on('pageerror', (e) => errors.push(`${label}: ${e.message}`));
    await d.page.goto(u);
    await d.page.waitForFunction(() => window.__cl?.tools);
    return d;
  };
  d.tool = (t, a = {}) => d.page.evaluate(([t, a]) => window.__cl.tools[t](a), [t, a]);
  d.table = () => d.tool('get_table');
  d.digest = () => d.page.evaluate(() => window.__cl.digest());
  return d;
}

// The generic checks, run inside the page. When a sheet is open only the
// sheet is checked (the page beneath it was checked in its own state).
function audit() {
  const vw = innerWidth, vh = innerHeight, out = [];
  const sheet = document.querySelector('.overlay.open');
  const root = sheet ?? document.body;
  const shown = (el) => {
    if (!sheet && el.closest('.overlay')) return false;
    if (el.closest('[hidden]')) return false;
    const det = el.closest('details:not([open])');
    if (det && !el.closest('summary') && el !== det) return false;
    const r = el.getBoundingClientRect(), cs = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && cs.visibility !== 'hidden' && +cs.opacity !== 0;
  };
  const scroller = (el) => {
    for (let e = el.parentElement; e && e !== root.parentElement; e = e.parentElement) {
      const o = getComputedStyle(e);
      if (/(auto|scroll)/.test(o.overflowY + o.overflowX) && e.scrollHeight + e.scrollWidth > e.clientHeight + e.clientWidth + 1) return e;
    }
    return null;
  };
  const name = (el) => `${el.id ? `#${el.id}` : el.tagName.toLowerCase() + (typeof el.className === 'string' && el.className ? `.${el.className.split(' ')[0]}` : '')}`
    + ` "${(el.value || el.textContent || el.getAttribute('aria-label') || '').trim().replace(/\s+/g, ' ').slice(0, 36)}"`;
  const els = [...root.querySelectorAll('*')].filter(shown);
  if (document.documentElement.scrollWidth > vw + 1) out.push(['hscroll', `page is ${document.documentElement.scrollWidth}px wide`]);
  for (const el of els) {
    if (el.closest('svg') && el.tagName !== 'svg') continue;
    const r = el.getBoundingClientRect(), sc = scroller(el);
    if (!sc && (r.right > vw + 1 || r.left < -1 || r.bottom > vh + 1 || r.top < -1)) out.push(['offscreen', name(el), `${Math.round(r.left)},${Math.round(r.top)}–${Math.round(r.right)},${Math.round(r.bottom)}`]);
    if (sc) { const b = sc.getBoundingClientRect(); if (r.right > b.right + 1 || r.left < b.left - 1) out.push(['offscreen', name(el), `sideways out of its scrolling box`]); }
    // (Inputs scroll their own text, so they aren't checked for clipping.)
    const ownText = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    if (ownText && el.clientWidth > 0 && el.scrollWidth > el.clientWidth + 1) {
      const cs = getComputedStyle(el);
      out.push([cs.textOverflow === 'ellipsis' ? 'ellipsis' : cs.overflowX === 'visible' ? 'spills' : 'clipped', name(el), `${el.scrollWidth} in ${el.clientWidth}px`]);
    }
  }
  const ctl = els.filter((e) => e.matches('button, input, a[href], .pick'));
  for (const b of ctl) {
    // A control inside a label is tapped through the whole label.
    const r = b.getBoundingClientRect(), t = b.matches('input[type=checkbox], input[type=radio]') && b.closest('label') ? b.closest('label').getBoundingClientRect() : r;
    if (Math.min(t.width, t.height) < 32) out.push(['small', name(b), `${Math.round(t.width)}×${Math.round(t.height)}`]);
    const sc = scroller(b), box = sc ? sc.getBoundingClientRect() : { left: 0, top: 0, right: vw, bottom: vh };
    const cx = r.left + r.width / 2, cy = r.top + r.height / 2;
    if (cx > box.left && cy > box.top && cx < box.right && cy < box.bottom) {
      const hit = document.elementFromPoint(cx, cy);
      if (hit && !b.contains(hit) && !hit.contains(b) && !(b.labels && [...b.labels].some((l) => l.contains(hit)))) out.push(['covered', name(b), `by ${name(hit)}`]);
    }
  }
  for (let i = 0; i < ctl.length; i++) for (let j = i + 1; j < ctl.length; j++) {
    if (ctl[i].contains(ctl[j]) || ctl[j].contains(ctl[i])) continue;
    // Scrolled-out checkbox rows have boxes behind the fixed footer, but
    // those pixels are clipped. Compare the portions actually visible.
    const visible = (el) => {
      const r = el.getBoundingClientRect(), sc = scroller(el);
      if (!sc) return r;
      const b = sc.getBoundingClientRect();
      return { left: Math.max(r.left, b.left), right: Math.min(r.right, b.right), top: Math.max(r.top, b.top), bottom: Math.min(r.bottom, b.bottom) };
    };
    const a = visible(ctl[i]), c = visible(ctl[j]);
    if (Math.min(a.right, c.right) - Math.max(a.left, c.left) > 2 && Math.min(a.bottom, c.bottom) - Math.max(a.top, c.top) > 2) out.push(['overlap', name(ctl[i]), name(ctl[j])]);
  }
  if (sheet) {
    const s = sheet.querySelector('.sheet');
    if (s.scrollHeight > s.clientHeight + 1 && !/(auto|scroll)/.test(getComputedStyle(s).overflowY)) out.push(['sheet', `#${sheet.id}`, "taller than the screen and can't scroll"]);
  }
  for (const [k, v] of window.__squeeze) if (v < 0.8) out.push(['squeezed', k, `${Math.round(v * 100)}% wide`]);
  window.__squeeze.clear();
  return out;
}

const findings = new Map(); // "kind | what" → { detail, where: [] }
let shots = 0;
async function snap(state, d, { viewports = VIEWPORTS, settle = 350 } = {}) {
  if (process.env.ONLY && !state.includes(process.env.ONLY)) return;
  const orig = d.page.viewportSize();
  for (const [w, h] of viewports) {
    await d.page.setViewportSize({ width: w, height: h });
    await sleep(settle);
    await d.page.evaluate(() => window.__squeeze.clear());
    await d.page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    for (const [kind, what, detail = ''] of await d.page.evaluate(audit)) {
      const key = `${kind} | ${what}`;
      if (!findings.has(key)) findings.set(key, { kind, what, detail, where: [] });
      findings.get(key).where.push(`${state}@${w}×${h}`);
    }
    await d.page.screenshot({ path: `${OUT}${state}-${w}x${h}.png` });
    shots++;
  }
  await d.page.setViewportSize(orig);
}

// Solve the clover on screen through the agent tools.
async function solve(d) {
  const t = await d.table();
  if (t.clover?.isMine) return false;
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
  await d.tool('check_guess', { clover: t.clover.index });
  return true;
}

try {
  // ---- home, before ever playing --------------------------------------------------
  const A = await device('auditor');
  await A.open(base);
  await A.page.waitForSelector('#homeSheet.open');
  await snap('01-home-empty', A);

  // ---- six players sit down (the auditor last, so its join sheet lists them) ------------
  const others = [];
  for (const n of NAMES.slice(1)) {
    const d = await device(n);
    await d.open(url());
    await d.tool('join_table', { name: n });
    others.push(d);
  }
  await A.page.goto(url());
  await A.page.waitForFunction(() => window.__cl?.tools);
  await A.page.waitForSelector('#joinSheet.open');
  await until(async () => (await A.page.$$('#seatList > *')).length >= 5);
  await snap('02-join-sheet', A);
  await A.tool('join_table', { name: NAMES[0] });
  const all = [A, ...others];
  await until(async () => (await Promise.all(all.map((d) => d.table()))).every((t) => t.players.length === 6 && t.seated), 40000);
  await snap('03-lobby', A);
  await A.page.click('#menuBtn'); await A.page.waitForSelector('#menuSheet.open');
  await snap('04-menu', A);
  await A.page.click('#helpMenuBtn'); await A.page.waitForSelector('#helpSheet.open');
  await snap('05-help', A);
  await A.page.click('#helpClose');
  await A.page.evaluate(() => document.querySelectorAll('.overlay').forEach((o) => o.classList.remove('open')));

  // ---- writing ------------------------------------------------------------------------
  await A.page.click('#go');
  await A.page.waitForSelector('#dealSheet.open');
  await snap('05b-choose-players', A);
  await A.page.click('#dealConfirm');
  await until(async () => (await Promise.all(all.map((d) => d.table()))).every((t) => t.phase === 'writing'));
  await snap('06-writing-tip', A);
  // The clue editor open on a leaf, with a long clue typed in.
  await A.page.evaluate(() => window.__cl.settle());
  const o = await A.page.evaluate(() => { const r = document.getElementById('c').getBoundingClientRect(); return [r.left, r.top]; });
  const leaf = await A.page.evaluate(() => window.__cl.petal(0));
  await A.page.mouse.click(o[0] + leaf[0], o[1] + leaf[1]);
  await A.page.waitForSelector('#clue', { state: 'visible' });
  await A.page.keyboard.type(CLUES[0]);
  await snap('07-writing-editor', A, { viewports: [[390, 844]] });
  await A.page.keyboard.press('Escape');
  for (const [k, leaf] of ['top', 'right', 'bottom', 'left'].entries()) await A.tool('write_clue', { leaf, text: CLUES[k] });
  await A.tool('set_decoys', { count: 4 });
  await snap('08-writing-done', A);
  for (const d of others.slice(0, 3)) {
    for (const [k, leaf] of ['top', 'right', 'bottom', 'left'].entries()) await d.tool('write_clue', { leaf, text: CLUES[(k + 1) % 4] });
    await d.tool('submit_clover');
  }
  await A.tool('submit_clover');
  await until(async () => (await A.table()).readyCount === 4);
  await snap('09-waiting', A);
  await A.page.click('#menuBtn'); await A.page.waitForSelector('#menuSheet.open');
  await snap('10-menu-submitted', A, { viewports: [[360, 640]] });
  await A.page.evaluate(() => document.querySelectorAll('.overlay').forEach((o) => o.classList.remove('open')));
  for (const d of others.slice(3)) {
    for (const [k, leaf] of ['top', 'right', 'bottom', 'left'].entries()) await d.tool('write_clue', { leaf, text: CLUES[(k + 2) % 4] });
    await d.tool('submit_clover');
  }

  // ---- guessing -----------------------------------------------------------------------
  await until(async () => (await Promise.all(all.map((d) => d.table()))).every((t) => t.phase === 'guessing'));
  // On someone else's clover (long clues on the leaves), then on your own.
  const mine = (await A.table()).clover.isMine;
  if (mine) await A.tool('choose_clover', { clover: 1 });
  await sleep(500);
  await snap('11-guessing', A);
  await A.page.click('#hint'); await A.page.waitForSelector('#cloverSheet.open');
  await snap('12-clover-list', A);
  await A.page.evaluate(() => document.querySelectorAll('.overlay').forEach((o) => o.classList.remove('open')));
  await solve(A);
  await until(async () => (await A.table()).clover.solved === true || (await A.page.textContent('#hint')).startsWith('Solved'));
  await snap('13-solved', A);
  // The writer's own view while the others guess.
  const n = (await A.digest()).puzzles.length;
  for (let k = 0; k < n; k++) {
    await A.tool('choose_clover', { clover: k });
    if ((await A.table()).clover.isMine) { await sleep(400); await snap('14-guessing-mine', A, { viewports: [[360, 640]] }); break; }
  }
  // Solve every clover (each by someone who didn't write it), then finish.
  for (let k = 0; k < n; k++) {
    await A.tool('choose_clover', { clover: k });
    await sleep(300);
    const t = await A.table();
    if (t.clover?.solved) continue;
    const solver = t.clover.isMine ? others[0] : A;
    await until(async () => (await solver.digest()).phase === `guessing:${k}`);
    await solve(solver);
    await sleep(300);
  }
  await A.tool('next_clover');
  await until(async () => (await A.table()).phase === 'end', 20000);
  await A.page.waitForSelector('#recapSheet.open', { timeout: 5000 }).catch(() => {});
  await snap('15-recap', A);
  await A.page.evaluate(() => document.querySelectorAll('.overlay').forEach((o) => o.classList.remove('open')));
  await snap('16-end', A);
  await A.page.click('#hint'); await A.page.waitForSelector('#recapSheet.open');
  await A.page.click('#recapShare'); await A.page.waitForSelector('#shareSheet.open');
  await A.page.click('#shareList > :nth-child(1)').catch(() => {});
  await A.page.click('#shareList > :nth-child(2)').catch(() => {});
  await sleep(300);
  await snap('17-share', A);
  const link = await A.page.textContent('#shareText');
  await A.page.evaluate(() => document.querySelectorAll('.overlay').forEach((o) => o.classList.remove('open')));
  await A.page.click('#menuBtn'); await A.page.waitForSelector('#menuSheet.open');
  await A.page.click('#logMenuBtn'); await A.page.waitForSelector('#logSheet.open');
  await snap('18-past-games', A);
  await A.page.evaluate(() => document.querySelectorAll('.overlay').forEach((o) => o.classList.remove('open')));

  // ---- something changed by someone else: a notice, and the menu's list ---------------
  await others[1].page.evaluate(() => document.querySelectorAll('.overlay').forEach((o) => o.classList.remove('open')));
  await others[1].page.click('#menuBtn'); await others[1].page.waitForSelector('#menuSheet.open');
  await others[1].page.click('#newGameBtn');
  await until(async () => (await A.page.textContent('#toast')).length > 0, 8000);
  await snap('19-notice', A, { viewports: [[320, 568], [390, 844]], settle: 50 });
  await A.page.click('#menuBtn'); await A.page.waitForSelector('#menuSheet.open');
  await snap('20-menu-happened', A, { viewports: [[360, 640]] });
  await A.page.evaluate(() => document.querySelectorAll('.overlay').forEach((o) => o.classList.remove('open')));

  // ---- a late joiner watching, and home with tables -----------------------------------
  const W = await device('watcher');
  await W.open(url());
  await W.page.waitForSelector('#joinSheet.open');
  await W.page.click('#watchBtn');
  await sleep(1500);
  await snap('21-watching', W, { viewports: [[320, 568], [390, 844]] });
  await A.page.click('#menuBtn'); await A.page.waitForSelector('#menuSheet.open');
  await A.page.click('#homeMenuBtn'); await A.page.waitForSelector('#homeSheet.open');
  await snap('22-home-tables', A);

  // ---- a share link: solo play ---------------------------------------------------------
  if (link?.startsWith('http')) {
    const S = await device('solo');
    await S.open(link.replace(/^https?:\/\/[^/]+\/[^?#]*/, `http://localhost:${PORT}/`));
    await sleep(800);
    await snap('23-solo', S, { viewports: [[320, 568], [390, 844], [1280, 800]] });
  } else console.log('(no share link found; skipped solo)');
} catch (e) {
  console.log(`audit stopped early: ${e.message.split('\n')[0]}`);
  errors.push(e.message.split('\n')[0]);
}

// ---- report ------------------------------------------------------------------------
const order = ['hscroll', 'offscreen', 'covered', 'overlap', 'clipped', 'spills', 'sheet', 'squeezed', 'small', 'ellipsis'];
const list = [...findings.values()].sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind));
for (const f of list) {
  const states = [...new Set(f.where.map((w) => w.split('@')[0]))], sizes = [...new Set(f.where.map((w) => w.split('@')[1]))];
  console.log(`${f.kind.padEnd(9)} ${f.what}  — ${f.detail}\n          in ${states.join(', ')} at ${sizes.join(', ')}`);
}
const faults = list.filter((f) => f.kind !== 'ellipsis');
console.log(`\n${shots} screens checked; ${faults.length} faults, ${list.length - faults.length} places cut with "…"`);
if (errors.length) console.log(`page errors:\n  ${errors.join('\n  ')}`);
await browser.close();
process.exit(faults.length || errors.length ? 1 : 0);
