// Long names on a 360 px phone: three players with 24-character names (one
// all W's, the widest letter) play through lobby, writing, waiting and
// guessing. Gates: no button ever runs off the screen, and with real-world
// long names the status line shows a version that fits rather than cutting
// off. Screenshots of the strip go to harness/out/long/. Exit 1 on failure.
import { chromium } from 'playwright';
import { serve } from './serve.mjs';
const PORT = 8807, RELAY = 8808, room = `L${Date.now().toString(36)}`;
const url = `http://localhost:${PORT}/?signal=ws://localhost:${RELAY}#${room}`;
const OUT = new URL('./out/long/', import.meta.url).pathname;
await (await import('node:fs/promises')).mkdir(OUT, { recursive: true });
await serve({ port: PORT, relayPort: RELAY, quiet: true });
const browser = await chromium.launch({ args: ['--disable-features=WebRtcHideLocalIpsWithMdns'] });
const names = ['Bartholomew Wigglesworth', 'WWWWWWWWWWWWWWWWWWWWWWWW', 'Maximiliana Rosencrantz'];
const ps = [];
for (const n of names) {
  const ctx = await browser.newContext({ viewport: { width: 360, height: 740 }, deviceScaleFactor: 1 });
  const page = await ctx.newPage(); await page.goto(url); await page.waitForFunction(() => window.__cl);
  await page.waitForSelector('#joinSheet.open'); await page.fill('#nameInput', n); await page.click('#joinBtn');
  ps.push(page);
}
await Promise.all(ps.map((p) => p.waitForFunction(() => window.__cl?.ui().players.length === 3, null, { timeout: 30000 })));
const report = [];
async function shot(tag, k = 0) {
  await new Promise((r) => setTimeout(r, 400));
  const p = ps[k];
  const m = await p.evaluate(() => {
    const h = document.getElementById('hint'), W = innerWidth;
    const btns = ['go', 'alt'].map((id) => document.getElementById(id)).filter((b) => b && !b.hidden && b.offsetParent);
    return { text: h.textContent, cut: h.scrollWidth > h.clientWidth + 1, hintW: h.clientWidth,
      btns: btns.map((b) => `${b.textContent}[${Math.round(b.getBoundingClientRect().width)}]`).join(' '),
      overflow: Math.max(0, ...btns.map((b) => b.getBoundingClientRect().right - W)), pageScroll: document.documentElement.scrollWidth > W };
  });
  report.push({ tag, who: names[k], ...m });
  await p.screenshot({ path: `${OUT}${tag}.png`, clip: { x: 0, y: 640, width: 360, height: 100 } }).catch(() => p.screenshot({ path: `${OUT}${tag}.png` }));
}
const [A, B, C] = ps;
await shot('1-lobby');
await A.click('#go');
await A.click('#dealConfirm');
await Promise.all(ps.map((p) => p.waitForFunction(() => window.__cl.digest().phase === 'writing')));
async function write(p, n = 4, mid) {
  await p.waitForFunction(() => window.__cl.card(0)[0] >= 0);
  await p.evaluate(() => window.__cl.settle());
  const o = await p.evaluate(() => { const r = document.getElementById('c').getBoundingClientRect(); return [r.left, r.top]; });
  const at = await p.evaluate(() => window.__cl.petal(0));
  await p.mouse.click(o[0] + at[0], o[1] + at[1]);
  for (let i = 0; i < n; i++) { await p.waitForSelector('#clue', { state: 'visible' }); await p.keyboard.type(`clue${i}`); await p.keyboard.press('Enter'); if (i === 1 && mid) { await p.keyboard.press('Escape'); await mid(); await p.evaluate(() => window.__cl.settle()); const q = await p.evaluate(() => window.__cl.petal(0)); await p.mouse.click(o[0] + q[0], o[1] + q[1]); } }
  await p.keyboard.press('Escape').catch(() => {});
}
await write(C, 2); await shot('2-writing-half', 2);
await write(A); await shot('3-writing-done');
await A.click('#go'); await shot('4-submitted');
await write(B); await B.click('#go');
await A.waitForFunction(() => !document.getElementById('alt').hidden);
await shot('5-waiting-alt');                       // waiting for Maximiliana, Start without Maximiliana
await shot('5b-waiting-self', 2);                  // Maximiliana's own view
await A.click('#alt');
await Promise.all(ps.map((p) => p.waitForFunction(() => window.__cl.digest().phase.startsWith('guessing'))));
await shot('6-guessing', 2);
for (let k = 0; k < 3; k++) await shot(`7-guessing-view${k}`, k);
// Each clover in turn (picked from the status), seen by someone who isn't its writer.
const n = await A.evaluate(() => window.__cl.digest().puzzles.length);
for (let i = 1; i < n; i++) {
  await C.click('#hint'); await C.waitForSelector('#cloverSheet.open');
  await C.click(`#cloverList .clover-row:nth-child(${i + 1})`);
  await C.waitForFunction((i) => window.__cl.digest().phase === `guessing:${i}`, i);
  for (let k = 0; k < 3; k++) await shot(`8-clover${i}-view${k}`, k);
}
console.table(report.map(({ tag, who, text, cut, hintW, btns, overflow, pageScroll }) => ({ tag, who: who.slice(0, 6), text, cut, hintW, btns, overflow, pageScroll })));
const gates = [
  ['no button runs off the screen, and the page never scrolls sideways', report.every((r) => r.overflow === 0 && !r.pageScroll)],
  ['real-world long names: the status line fits without "…"', report.every((r) => !r.cut || r.text.includes('WWW'))],
  ['the stress name is cut with "…", not dropped', report.some((r) => r.text.includes('WWW…'))],
  ['the clover line falls back to a shorter version that fits', report.some((r) => /Bartholomew's clover, 2 of 2/.test(r.text))],
];
for (const [name, ok] of gates) console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}`);
await browser.close(); process.exit(gates.every(([, ok]) => ok) ? 0 : 1);
