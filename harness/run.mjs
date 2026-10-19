// Browser harness: real Chromium tabs, real WebRTC data channels (signaled via
// a local relay), real pointer and keyboard events through the real UI. Five
// players (one phone-sized) plus a watcher play two rounds. Prints a
// scorecard and writes harness/out/scorecard.json + screenshots. Exit code 1
// if any gate fails.
import { chromium } from 'playwright';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { serve } from './serve.mjs';
import { createWsRelayServer } from '@trystero-p2p/ws-relay/server';

const here = (p) => fileURLToPath(new URL(p, import.meta.url));
const OUT = here('./out/');
const PORT = 8797, RELAY = 8798;
const room = `h${Date.now().toString(36)}`;
const url = `http://localhost:${PORT}/?signal=ws://localhost:${RELAY}#${room}`;

const metrics = {};
const gates = [];
const gate = (name, ok, detail = '') => {
  gates.push({ name, ok: !!ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? `  — ${detail}` : ''}`);
};
const metric = (name, value, unit = '') => {
  metrics[name] = value;
  console.log(`      ${name} = ${typeof value === 'number' ? +value.toFixed(2) : value}${unit}`);
};
const pct = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(p * s.length))]; };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const near = (a, b, tol = 1.5) => Math.abs(a[0] - b[0]) < tol && Math.abs(a[1] - b[1]) < tol;

await mkdir(OUT, { recursive: true });
const server = await serve({ port: PORT, relayPort: RELAY, quiet: true });
const browser = await chromium.launch({
  executablePath: process.env.CHROMIUM ?? undefined,
  args: ['--disable-features=WebRtcHideLocalIpsWithMdns'],
});
const errors = [];

async function open(label, viewport = { width: 1280, height: 860 }) {
  const ctx = await browser.newContext({ viewport, deviceScaleFactor: 1 });
  const page = await ctx.newPage();
  page.on('pageerror', (e) => errors.push(`${label}: ${e.message}`));
  page.on('console', (m) => m.type() === 'error' && errors.push(`${label}: ${m.text()}`));
  await page.goto(url);
  await page.waitForFunction(() => window.__cl);
  const P = {
    label, page, ctx, viewport,
    ev: (fn, arg) => page.evaluate(fn, arg),
    digest: () => page.evaluate(() => window.__cl.digest()),
    ui: () => page.evaluate(() => window.__cl.ui()),
    card: (i) => page.evaluate((i) => window.__cl.card(i), i),
    slot: (s) => page.evaluate((s) => window.__cl.slot(s), s),
    board: (x, y) => page.evaluate(([x, y]) => window.__cl.board(x, y), [x, y]),
    petal: (d) => page.evaluate((d) => window.__cl.petal(d), d),
    hub: () => page.evaluate(() => window.__cl.hub()),
    quarter: () => page.evaluate(() => window.__cl.quarter()),
    settle: () => page.evaluate(() => window.__cl.settle()),
    presses: () => page.evaluate(() => window.__cl.stats.presses),
    seat: async () => (await P.ui()).seat,
    async origin() {
      return page.evaluate(() => { const r = document.getElementById('c').getBoundingClientRect(); return [r.left, r.top]; });
    },
    async drag(from, to, { steps = 12 } = {}) {
      const o = await P.origin(), m = page.mouse;
      await m.move(o[0] + from[0], o[1] + from[1]);
      await m.down();
      for (let k = 1; k <= steps; k++) {
        const t = k / steps;
        await m.move(o[0] + from[0] + (to[0] - from[0]) * t, o[1] + from[1] + (to[1] - from[1]) * t);
      }
      await m.up();
    },
    async tap(at) {
      const o = await P.origin();
      await page.mouse.click(o[0] + at[0], o[1] + at[1]);
    },
    async joinViaSheet(name) {
      await page.waitForSelector('#joinSheet.open', { timeout: 5000 });
      await page.fill('#nameInput', name);
      await page.click('#joinBtn');
    },
    go: async () => { await page.click('#go'); if (await page.locator('#dealSheet.open').count()) await page.click('#dealConfirm'); },
    waitGoEnabled: () => page.waitForFunction(() => !document.getElementById('go').disabled && !document.getElementById('go').hidden),
  };
  return P;
}

/** Wait until `pred(digest)` holds on every page; returns ms elapsed. */
async function until(players, pred, timeout = 8000) {
  const t0 = performance.now();
  await Promise.all(players.map((p) => p.page.waitForFunction(
    ([src]) => new Function('d', `return (${src})(d)`)(window.__cl.digest()), [pred.toString()], { timeout, polling: 'raf' })));
  return performance.now() - t0;
}
const same = async (players) => {
  const ds = await Promise.all(players.map((p) => p.digest()));
  return ds.every((d) => JSON.stringify(d) === JSON.stringify(ds[0]));
};
const cur = (d) => d.puzzles[+d.phase.split(':')[1]];

/** A guesser drops card s on spot s (snaps) and turns it right, or a decoy into spot 0. */
async function solve(p, { decoyInSpot0 = false } = {}) {
  await p.settle();
  for (let s = 0; s < 4; s++) {
    if (cur(await p.digest()).mask & (1 << s)) continue;
    const card = decoyInSpot0 && s === 0 ? 4 : s;
    await p.drag((await p.card(card)).slice(0, 2), (await p.slot(s)).slice(0, 2));
    if (card !== s) continue;
    const want = await p.ev((s) => window.__cl.answerRot(s), s);
    for (let k = 0; k < 4; k++) {
      const c = cur(await p.digest()).cards[s];
      if (c[0] === 'slot' && c[2] === want) break;
      await p.tap((await p.card(s)).slice(0, 2));
    }
  }
}

try {
  console.log('\n# build');
  const wasm = await readFile(here('../web/core.wasm'));
  const mod = await WebAssembly.compile(wasm);
  metric('wasm_bytes', wasm.length, ' B');
  const shim = (await readFile(here('../web/main.js'), 'utf8')).split('\n').filter((l) => l.trim() && !l.trim().startsWith('//')).length;
  metric('js_shim_loc', shim);
  gate('wasm has zero JS imports (no bindgen glue)', WebAssembly.Module.imports(mod).length === 0);

  // ---- join ------------------------------------------------------------------------
  console.log('\n# join (5 players + 1 watcher, WebRTC mesh)');
  let t0 = performance.now();
  const names = ['Ana', 'Ben', 'Cy', 'Dee', 'Eve'];
  const players = await Promise.all([
    ...names.slice(0, 4).map((n) => open(n)),
    open('Eve-phone', { width: 390, height: 844 }),
  ]);
  const [A, B, C, D, E] = players;
  await E.page.waitForSelector('#joinSheet.open', { timeout: 5000 });
  await E.page.screenshot({ path: `${OUT}0-join-phone.png` });
  const joinPresses = [];
  for (const [k, p] of players.entries()) {
    const p0 = await p.presses();
    await p.joinViaSheet(names[k]);
    joinPresses.push((await p.presses()) - p0);
  }
  const W = await open('Watcher');
  await W.page.waitForSelector('#joinSheet.open');
  await W.page.click('#watchBtn');
  await Promise.all([...players, W].map((p) => p.page.waitForFunction(() => window.__cl.ui().players.length === 5, null, { timeout: 30000 })));
  metric('join_5_players_ms', performance.now() - t0, ' ms');
  gate('first visit asks for a name; joining is one tap', Math.max(...joinPresses) === 1);
  await A.page.waitForFunction(() => window.__cl.ui().watchers === 1, null, { timeout: 20000 })
    .then(() => gate('everyone sees the watcher', true), () => gate('everyone sees the watcher', false));

  // ---- writing ----------------------------------------------------------------------
  console.log('\n# writing (everyone at once)');
  // Seats can arrive in a sync before this page has a direct connection
  // to every player. Start deals to present players, so wait for presence.
  await Promise.all(players.map((p) => p.page.waitForFunction(() => window.__cl.ui().dealable.length === 5, null, { timeout: 30000 })));
  const startLabel = await A.page.textContent('#go'), lobbyLine = await A.page.textContent('#hint');
  gate('the lobby says how many players will be dealt in', startLabel === 'Start' && lobbyLine.includes('5 players here'), `${lobbyLine} | ${startLabel}`);
  await A.go(); // Start
  await until([...players, W], (d) => d.phase === 'writing' && d.round === 1 && d.dealt.length === 5);
  gate('one press deals everyone in, and fixes the roster', true);
  gate("watchers don't get a clover", (await W.ev(() => window.__cl.card(0)))[0] < 0);

  // ---- legibility: contrast, size, no stray selection --------------------------------
  {
    // Both themes: light (the default here) and dark.
    const canvasPairs = [], domPairs = [];
    for (const theme of ['light', 'dark']) {
    await A.ev((t) => window.__cl.setTheme(t), theme);
    canvasPairs.push(...(await A.ev(() => window.__cl.palette())).map(([n, f, b]) => [`${theme}: ${n}`, f, b]));
    domPairs.push(...(await A.ev((theme) => {
      const rgb = (c) => (c.match(/[\d.]+/g) || []).map(Number);
      const opaqueBg = (el) => {
        for (let e = el; e; e = e.parentElement) {
          const c = rgb(getComputedStyle(e).backgroundColor);
          if (c.length === 3 || c[3] === 1) return c.slice(0, 3);
        }
        return [20, 48, 31];
      };
      const hex = (c) => '#' + c.slice(0, 3).map((x) => Math.round(x).toString(16).padStart(2, '0')).join('');
      return ['#hint', '#go', '#decLabel', '#copyLink', '.danger', '.tip', '#themeBtn']
        .map((sel) => document.querySelector(sel))
        .filter((el) => el && el.offsetParent)
        .map((el) => [`${theme}: ${el.id || el.className}`, hex(rgb(getComputedStyle(el).color)), hex(opaqueBg(el)), parseFloat(getComputedStyle(el).fontSize)]);
    }, theme)));
    }
    await A.ev(() => window.__cl.setTheme('auto'));
    const lum = (h) => {
      const v = parseInt(h.slice(1), 16);
      const ch = (s) => { const c = ((v >> s) & 255) / 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
      return 0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0);
    };
    const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)]; return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
    const all = [...canvasPairs, ...domPairs].map(([n, f, b]) => [n, ratio(f, b)]);
    const worst = all.reduce((m, p) => (p[1] < m[1] ? p : m));
    metric('min_contrast_ratio', worst[1]);
    gate('all text meets WCAG AA contrast (≥ 4.5:1)', worst[1] >= 4.5, `worst: ${worst[0]} ${worst[1].toFixed(2)}`);
    const smallest = Math.min(...domPairs.map((p) => p[3]));
    gate('no UI text smaller than 14 px', smallest >= 14, `${smallest}px`);
    // Card words are laid out per card from measured widths: ~90% of the
    // deck renders at full size; the rest shrink evenly (never below 60%).
    await A.page.waitForFunction(() => window.__cl.wordFit(), null, { timeout: 5000 });
    const fit = await A.ev(() => window.__cl.wordFit());
    metric('deck_words_full_size', `${(fit.full * 100).toFixed(1)}%`);
    metric('deck_words_at_90pct_or_more', `${(fit.near * 100).toFixed(1)}%`);
    metric('smallest_word_scale', `${(fit.min * 100).toFixed(0)}%`);
    gate('~90% of deck words render at full size on their cards', fit.full >= 0.88, `${(fit.full * 100).toFixed(1)}%`);
    gate('every longer word fits by shrinking evenly (≥ 60%, no squeeze)', fit.min >= 0.6, `${(fit.min * 100).toFixed(0)}%`);
    const fontOk = await A.ev(() => document.fonts.check('700 26px Figtree') && document.fonts.check('600 26px Fraunces'));
    gate('brand fonts load from the site itself', fontOk);
    const sel = await E.ev(() => [getComputedStyle(document.getElementById('hint')).userSelect, getComputedStyle(document.getElementById('nameInput')).userSelect]);
    gate("bars can't be text-selected (no dictionary pop-ups); inputs can", sel[0] === 'none' && sel[1] !== 'none', sel.join(','));
  }

  // Tapping the side of a leaf opens its editor and never turns the board.
  {
    await B.settle();
    const q0 = await B.quarter();
    await B.tap((await B.board(150, 330)).slice(0, 2)); // lower-right part of the bottom leaf
    await B.page.waitForSelector('#clue', { state: 'visible' });
    await sleep(300);
    gate('tapping the side of a leaf edits it without turning the board', (await B.quarter()) === q0 && await B.page.isVisible('#clue'));
    const tf = await B.page.evaluate(() => getComputedStyle(document.getElementById('clue')).transform);
    const [a, b] = (tf.match(/-?[\d.e-]+/g) || [1, 0]).map(Number);
    gate("a leaf's editor lies along the leaf (bottom leaf: upside down)", Math.abs(Math.atan2(b, a)) > 3.0, tf);
    await B.page.keyboard.press('Escape');
  }

  const words = ['ocean', 'storm', 'royal', 'kitchen'];
  const pressCounts = [];
  await Promise.all(players.map(async (p, k) => {
    await p.settle();
    const p0 = await p.presses();
    await p.tap((await p.petal(0)).slice(0, 2));
    for (const w of words) {
      await p.page.waitForSelector('#clue', { state: 'visible' });
      await p.page.keyboard.type(`${w}${k}`);
      await p.page.keyboard.press('Enter');
    }
    if (k === 1) await p.page.click('#decUp'); // Ben bids 2 decoys
    await p.waitGoEnabled();
    pressCounts.push([(await p.presses()) - p0, k]);
    if (k !== 4) await p.go(); // Eve holds back
  }));
  await A.page.screenshot({ path: `${OUT}1-writing-ready.png` });
  await E.page.screenshot({ path: `${OUT}1-writing-phone.png` });
  const plain = pressCounts.filter(([, k]) => k !== 1).map(([n]) => n + 1);
  metric('presses_to_write_and_ready', Math.max(...plain));
  gate('a clover takes ≤ 6 presses (leaf, 4× Enter, Ready)', Math.max(...plain) <= 6, plain.join(','));
  await sleep(600);
  gate('guessing waits until everyone has submitted', (await A.digest()).phase === 'writing');
  await A.page.waitForFunction(() => document.querySelectorAll('#segs i.done').length === 4, null, { timeout: 3000 })
    .then(() => gate('the progress bar fills as people submit (4 of 5)', true), () => gate('the progress bar fills as people submit (4 of 5)', false));
  await A.page.waitForFunction(() => document.getElementById('hint').textContent.includes('Eve'), null, { timeout: 3000 }).catch(() => {});
  const label = await A.page.textContent('#hint');
  const proceed = await A.page.textContent('#alt');
  const eAlt = await E.page.evaluate(() => document.getElementById('alt').hidden);
  gate('the status names who is still writing; the override reads "Start anyway"', label.includes('Waiting for Eve') && proceed === 'Start anyway' && eAlt, `${label} | ${proceed} | Eve's own override hidden: ${eAlt}`);
  // Plain words that still fit: on the phone, the status line isn't cut off.
  const fits = await E.page.evaluate(() => { const h = document.getElementById('hint'); return [h.scrollWidth <= h.clientWidth + 1, h.textContent]; });
  gate("on a phone, the status line fits without being cut off", fits[0], fits[1]);
  // Submitted players can still edit.
  await A.settle();
  await A.tap((await A.petal(2)).slice(0, 2));
  await A.page.waitForSelector('#clue', { state: 'visible' });
  await A.page.fill('#clue', 'edited after submit');
  await A.page.keyboard.press('Enter');
  await until([B], (d) => d.subs.some((x) => x[2] === true && x.includes('edited after submit')))
    .then(() => gate('a submitted player can still edit; stays submitted', true), () => gate('a submitted player can still edit; stays submitted', false));
  t0 = performance.now();
  await E.go();
  await until([...players, W], (d) => d.phase === 'guessing:0', 10000);
  metric('last_ready_to_guessing_ms', performance.now() - t0, ' ms');
  const d1 = await A.digest();
  // Anyone can pick which clover is guessed: tap the status, choose; everyone follows.
  {
    await B.page.click('#hint');
    await B.page.waitForSelector('#cloverSheet.open');
    const rows = await B.page.$$eval('#cloverList .clover-row', (els) => els.length);
    await B.page.click('#cloverList .clover-row:nth-child(3)');
    const jumped = await until([...players, W], (d) => d.phase === 'guessing:2', 8000).then(() => true, () => false);
    await B.page.click('#hint');
    await B.page.waitForSelector('#cloverSheet.open');
    await B.page.click('#cloverList .clover-row:nth-child(1)');
    const back = await until([...players, W], (d) => d.phase === 'guessing:0', 8000).then(() => true, () => false);
    gate('tapping the status picks which clover everyone guesses (any order)', rows === d1.puzzles.length && jumped && back, `${rows} rows, jumped ${jumped}, back ${back}`);
  }
  gate('all 5 clovers queued; decoy bids kept', d1.puzzles.length === 5 && d1.subs.some((s) => s[1] === 2));
  gate('clues typed via Enter-to-next-leaf arrive intact', d1.subs.every((s) => s.slice(3).every((c, d) => c.startsWith(words[d]) || c === 'edited after submit')));

  // ---- take over a seat from another device ------------------------------------------
  console.log('\n# takeover');
  const A2 = await open('Ana-tablet', { width: 820, height: 1180 });
  await A2.joinViaSheet('ana');
  await A2.page.waitForFunction(() => window.__cl.ui().seated);
  gate('choosing an existing name takes over that seat', (await A2.seat()) === (await A.seat()) && (await A2.ui()).players.length === 5);

  // ---- guessing ----------------------------------------------------------------------
  console.log('\n# guessing');
  const all = [...players, A2, W];
  const bySeat = async (seat) => { for (const p of players) if ((await p.seat()) === seat) return p; };
  let Wr = await bySeat(cur(await A.digest()).writer);
  let guessers = players.filter((p) => p !== Wr);
  const [G, H] = guessers.filter((p) => p !== E);

  // Legibility and fill, measured where it matters: while guessing (tray shown).
  for (const p of [G, E]) {
    const word = await p.ev(() => window.__cl.wordPx());
    const boardPx = (await p.ev(() => window.__cl.cardPx() / window.__cl.zoom())) / 210 * 864; // 2 × EXTENT
    const side = Math.min(p.viewport.width, p.viewport.height);
    const tag = p === E ? 'phone_390x844' : 'desktop_1280x860';
    metric(`word_px_${tag}`, word, ' px');
    metric(`board_fill_${tag}`, boardPx / side);
  }
  gate('card words ≥ 16 px on desktop', metrics.word_px_desktop_1280x860 >= 16);
  gate('card words ≥ 14 px on a phone (bigger cards by default there)', metrics.word_px_phone_390x844 >= 14);
  gate('the board fills the phone width (≥ 95%)', metrics.board_fill_phone_390x844 >= 0.95);

  // Card size: per device, display only.
  {
    const d0 = JSON.stringify(await E.digest());
    const w0 = await E.ev(() => window.__cl.wordPx());
    await E.page.click('#menuBtn');
    await E.page.waitForSelector('#menuSheet.open');
    for (let k = 0; k < 8 && !(await E.page.isDisabled('#sizeUp')); k++) await E.page.click('#sizeUp');
    const atLimit = await E.page.isDisabled('#sizeUp') && await E.page.isVisible('#sizeNote') && !(await E.page.isDisabled('#sizeDown'));
    gate('at the size limit, + greys out and the menu says to pinch for more', atLimit);
    // The strategy guide opens from the menu and stays short.
    await E.page.click('#helpMenuBtn');
    await E.page.waitForSelector('#helpSheet.open');
    const helpWords = (await E.page.textContent('#helpSheet')).trim().split(/\s+/).length;
    await E.page.click('#helpClose');
    gate('"How to play well" opens from the menu (under 500 words)', helpWords > 150 && helpWords < 500, `${helpWords} words`);
    await E.page.click('#menuBtn');
    await E.page.waitForSelector('#menuSheet.open');
    await E.page.click('#closeMenu');
    const w1 = await E.ev(() => window.__cl.wordPx());
    metric('word_px_phone_max_size', w1, ' px');
    await sleep(200);
    gate('the card-size buttons enlarge cards on my screen only', w1 > w0 && JSON.stringify(await G.digest()) === d0
      && Math.abs((await G.ev(() => window.__cl.wordPx())) - metrics.word_px_desktop_1280x860) < 0.01, `${w0.toFixed(1)} → ${w1.toFixed(1)}`);
    // Even at the largest size, three tray cards fit across a phone without overlap.
    const px = await E.ev(() => window.__cl.cardPx());
    const tray = [];
    let onScreen = true;
    const cards = cur(await E.digest()).cards;
    for (let i = 0; i < cards.length; i++) {
      const [x, y] = await E.card(i);
      onScreen &&= x > 0 && x < 390 && y > 0 && y < 844;
      if (cards[i][0] === 'table') tray.push([x, y]);
    }
    const overlap = tray.some((a, i) => tray.some((b, j) => j > i && Math.abs(a[0] - b[0]) < px - 0.5 && Math.abs(a[1] - b[1]) < px - 0.5));
    gate('at the largest card size, tray cards on a phone still never overlap', !overlap && tray.length > 0, `${tray.length} in tray at ${px.toFixed(0)} px`);
    gate('at the largest card size every card is still on screen', onScreen);
  }

  // Pinch zoom is the board's own: two fingers zoom a camera over the board
  // (never the page, which could strand you), one finger keeps playing
  // through it, and 1× resets. A second finger cancels a drag in progress.
  {
    const ptr = (list) => E.ev((list) => {
      const c = document.getElementById('c'), r = c.getBoundingClientRect();
      for (const [type, id, px, py] of list) c.dispatchEvent(new PointerEvent(type, { pointerId: id, pointerType: 'touch', isPrimary: id === 1, clientX: r.left + px, clientY: r.top + py, bubbles: true }));
    }, list);
    await E.settle();
    const before = JSON.stringify(cur(await E.digest()).cards);
    const i = cur(await E.digest()).cards.findIndex((c) => c[0] === 'table');
    const [x, y] = await E.card(i);
    const [sx, sy] = await E.slot(0);
    const drag = [['pointerdown', 1, x, y]];
    for (let k = 1; k <= 6; k++) drag.push(['pointermove', 1, x + (sx - x) * k / 10, y + (sy - y) * k / 10]);
    await ptr([...drag, ['pointerdown', 2, x + 80, y + 80], ['pointermove', 1, sx, sy], ['pointerup', 1, sx, sy], ['pointerup', 2, x + 120, y + 120]]);
    await sleep(300);
    gate('a second finger cancels a drag: the card goes back, nothing turns', JSON.stringify(cur(await E.digest()).cards) === before);
    // Pinch out around the card.
    const steps = [['pointerdown', 1, x - 20, y - 20], ['pointerdown', 2, x + 20, y + 20]];
    for (let k = 1; k <= 8; k++) steps.push(['pointermove', 1, x - 20 - 4 * k, y - 20 - 4 * k], ['pointermove', 2, x + 20 + 4 * k, y + 20 + 4 * k]);
    await ptr([...steps, ['pointerup', 1, x - 52, y - 52], ['pointerup', 2, x + 52, y + 52]]);
    await sleep(100);
    const cam = await E.ev(() => window.__cl.camera());
    const page = await E.ev(() => visualViewport.scale);
    gate('pinching the board zooms the board, not the page', cam.s > 2 && page === 1 && JSON.stringify(cur(await E.digest()).cards) === before
      && await E.page.isVisible('#unzoom'), `board ${cam.s.toFixed(2)}×, page ${page}×`);
    // One finger still plays through the zoom: tapping the zoomed card turns it.
    const rot0 = cur(await E.digest()).cards[i][3];
    const [zx, zy] = [cam.x + x * cam.s, cam.y + y * cam.s];
    await ptr([['pointerdown', 1, zx, zy], ['pointerup', 1, zx, zy]]);
    await sleep(200);
    const rot1 = cur(await E.digest()).cards[i][3];
    for (let k = 0; k < 3; k++) await ptr([['pointerdown', 1, zx, zy], ['pointerup', 1, zx, zy]]);
    await sleep(200);
    gate('while zoomed, one finger still plays (a tap turns the card under it)', rot1 === (rot0 + 1) % 4 && JSON.stringify(cur(await E.digest()).cards) === before, `${rot0} → ${rot1}`);
    // One finger from an empty spot pans the zoomed view; the board doesn't turn.
    {
      const q0 = await E.quarter();
      const c0 = await E.ev(() => window.__cl.camera());
      const spot = await E.ev(() => {
        const c = document.getElementById('c').getBoundingClientRect(), cam = window.__cl.camera();
        for (let y = 60; y < c.height - 60; y += 20) for (let x = 60; x < c.width - 60; x += 20)
          if (!window.__cl.X.on_card((x - cam.x) / cam.s, (y - cam.y) / cam.s)) return [x, y];
        return null;
      });
      const [dx, dy] = [c0.x < -50 ? 50 : -50, c0.y < -50 ? 50 : -50];
      const moves = [['pointerdown', 1, spot[0], spot[1]]];
      for (let k = 1; k <= 8; k++) moves.push(['pointermove', 1, spot[0] + dx * k / 8, spot[1] + dy * k / 8]);
      await ptr([...moves, ['pointerup', 1, spot[0] + dx, spot[1] + dy]]);
      await sleep(400);
      const c1 = await E.ev(() => window.__cl.camera());
      gate('while zoomed, one finger off the cards pans instead of turning the board',
        Math.abs(c1.x - c0.x - dx) < 1 && Math.abs(c1.y - c0.y - dy) < 1 && (await E.quarter()) === q0 && JSON.stringify(cur(await E.digest()).cards) === before,
        `pan ${(c1.x - c0.x).toFixed(0)},${(c1.y - c0.y).toFixed(0)} of ${dx},${dy}; quarter ${q0} → ${await E.quarter()}`);
    }
    await E.page.click('#unzoom');
    gate('1× resets the board zoom', (await E.ev(() => window.__cl.camera().s)) === 1 && !(await E.page.isVisible('#unzoom')));
  }

  const wd = JSON.stringify(await Wr.digest());
  await Wr.settle();
  await Wr.drag((await Wr.card(0)).slice(0, 2), (await Wr.slot(0)).slice(0, 2));
  gate("the clover's writer cannot move cards", JSON.stringify(await Wr.digest()) === wd);

  const lat = [];
  for (let k = 0; k < 6; k++) {
    await G.settle();
    const s = k % 4;
    await G.drag((await G.card(0)).slice(0, 2), (await G.slot(s)).slice(0, 2));
    const t = performance.now();
    await until(all.filter((p) => p !== G), new Function('d', `const c=d.puzzles[0].cards[0];return c[0]==='slot'&&c[1]===${s}`));
    lat.push(performance.now() - t);
  }
  metric('sync_latency_p50_ms', pct(lat, 0.5), ' ms');
  metric('sync_latency_p95_ms', pct(lat, 0.95), ' ms');
  gate('a drop shows up on 6 other screens, p95 < 150 ms', pct(lat, 0.95) < 150);

  // Tentative placement: off-centre near a spot, and loose on the board.
  await G.settle();
  await G.drag((await G.card(1)).slice(0, 2), (await G.board(140 + 45, 140 + 40)).slice(0, 2));
  await G.drag((await G.card(2)).slice(0, 2), (await G.board(-20, 230)).slice(0, 2));
  await until(all, (d) => { const c = d.puzzles[0].cards; return c[1][0] === 'float' && c[2][0] === 'float'; });
  const tc = cur(await H.digest()).cards;
  gate('tentative placements stay exactly where dropped, for everyone',
    Math.abs(tc[1][1] - 185) < 2 && Math.abs(tc[1][2] - 180) < 2 && Math.abs(tc[2][2] - 230) < 2, JSON.stringify(tc.slice(1, 3)));
  gate('a card near a spot counts toward a check', cur(await H.digest()).claim[2] === 1);
  await H.page.screenshot({ path: `${OUT}2-guessing-tentative.png` });
  await E.page.screenshot({ path: `${OUT}2-guessing-phone.png` });

  // Card turns animate.
  await G.page.evaluate(() => new Promise(requestAnimationFrame));
  await G.tap((await G.card(3)).slice(0, 2));
  const target = (await G.card(3))[2];
  const mid = await G.page.evaluate(() => new Promise((r) => requestAnimationFrame(() => r(window.__cl.shownAngle(3)))));
  await sleep(600);
  const end = await G.ev(() => window.__cl.shownAngle(3));
  const gap = (a, b) => Math.abs((((a - b + Math.PI) % (2 * Math.PI)) + 2 * Math.PI) % (2 * Math.PI) - Math.PI);
  gate('turning a card animates instead of jumping', gap(mid, target) > 0.15 && gap(end, target) < 0.02, `mid off by ${gap(mid, target).toFixed(2)} rad`);

  // Turning the board: hub tap. Local only; board cards turn, tray cards stay.
  await G.settle();
  const n = cur(await G.digest()).cards.length;
  const before = await Promise.all([...Array(n).keys()].map((i) => G.card(i)));
  const otherBefore = await Promise.all([...Array(n).keys()].map((i) => H.card(i)));
  const dBefore = JSON.stringify(await G.digest());
  gate('the board starts each clover upright', (await G.quarter()) % 4 === 0);
  const q0 = await G.quarter();
  const [cx, cy] = await G.hub();
  await G.tap([cx, cy]);
  await G.settle();
  const after = await Promise.all([...Array(n).keys()].map((i) => G.card(i)));
  let boardOk = true, trayOk = true;
  cur(await G.digest()).cards.forEach((c, i) => {
    const [x, y] = before[i];
    if (c[0] !== 'table') boardOk &&= near(after[i], [cx - (y - cy), cy + (x - cx)]);
    else trayOk &&= near(after[i], before[i]);
  });
  gate('tapping ⟳ turns my view a quarter', (await G.quarter()) === q0 + 1);
  gate('cards on the board (snapped or loose) turn with it', boardOk);
  gate('cards in the tray stay put', trayOk);
  await sleep(150);
  const otherAfter = await Promise.all([...Array(n).keys()].map((i) => H.card(i)));
  gate("turning my board doesn't move anything on other screens", otherBefore.every((p, i) => near(p, otherAfter[i])));
  gate('board rotation is not shared state', JSON.stringify(await H.digest()) === dBefore);

  // First try with a decoy in spot 0, then fix it.
  await solve(G, { decoyInSpot0: true });
  await G.waitGoEnabled();
  // Record shakes on another screen as they happen (they last 0.6 s).
  await H.ev(() => { window.__shook = 0; const t = () => { window.__shook = Math.max(window.__shook, window.__cl.X.probe_shaking());
    if (window.__shook === 0) requestAnimationFrame(t); }; requestAnimationFrame(t); });
  await G.go();
  await until(all, (d) => d.puzzles[0].attempts === 1);
  const c1 = cur(await B.digest());
  gate('a miss boots the wrong card to the tray; right ones stay', c1.mask === 0b1110 && c1.cards[4][0] === 'table', `mask ${c1.mask}`);
  // (The one-time how-to tip may still be up from when guessing began; nothing else.)
  const toastNow = await H.page.$eval('#toast', (t) => t.classList.contains('show') ? t.textContent : '');
  gate('a miss comes with no commentary', (!toastNow || toastNow === (await H.page.textContent('#tipText'))) && (await H.page.textContent('#go')) === 'Guess', toastNow);
  const shook = await H.page.waitForFunction(() => window.__shook > 0, null, { timeout: 3000 }).then(() => H.ev(() => window.__shook), () => 0);
  gate('booted cards shake on every screen (simple motion for a miss)', shook > 0, `shaking ${shook}`);
  await solve(H);
  await H.waitGoEnabled();
  await H.go();
  await until(all, (d) => d.puzzles[0].attempts === 2 && d.puzzles[0].mask === 15);
  { const d = await A.digest(); gate('guessing keeps going until all four are right (no limit, no score)', d.puzzles[0].attempts === 2 && !('gameScore' in d)); }
  await G.page.screenshot({ path: `${OUT}3-clover-done.png` });

  for (let k = 1; k < 5; k++) {
    await guessers[0].go(); // Next clover
    await until(all, new Function('d', `return d.phase==='guessing:${k}'`));
    Wr = await bySeat(cur(await A.digest()).writer);
    guessers = players.filter((p) => p !== Wr);
    const g = guessers.find((p) => p !== E);
    await solve(g);
    await g.waitGoEnabled();
    await g.go();
    await until(all, new Function('d', `return d.puzzles[${k}].attempts===1`));
  }
  await A.go(); // Finish round
  await until(all, (d) => d.phase === 'end');
  const r1 = await A.digest();
  gate('every clover solved, same state everywhere', r1.puzzles.every((p) => p.mask === 15) && (await same(all)));
  // The finished round is kept on this device, and the home page lists the table.
  {
    await A.page.waitForFunction(() => JSON.parse(localStorage.getItem('shamrock:log') ?? '[]').length > 0, null, { timeout: 5000 }).catch(() => {});
    const log = await A.ev(() => JSON.parse(localStorage.getItem('shamrock:log') ?? '[]'));
    gate('a finished round is saved to this device\'s game log, answers included',
      log.length === 1 && log[0].boards.length === r1.puzzles.length && log[0].boards.every((b) => b.solved && b.text.split('\n').length >= 9), `${log.length} entries; ${JSON.stringify(log[0]?.boards.map((b) => [b.solved, b.guesses, b.text.split('\n').length]))} of ${r1.puzzles.length}`);
    const homePage = await A.page.context().newPage();
    await homePage.goto(`http://localhost:${PORT}/?signal=ws://localhost:${RELAY}`);
    await homePage.waitForSelector('#homeSheet.open');
    const rows = await homePage.$$eval('#tableList .tableRow b', (els) => els.map((e) => e.textContent));
    const pastGames = await homePage.textContent('#homeLog');
    gate('the home page lists my tables and past games', rows.includes(`#${room}`) && pastGames.includes('(1)'), `${rows.join(' ')} | ${pastGames}`);
    // Forget drops a table from the list (and its saved name), and it stays gone.
    await homePage.evaluate(() => { localStorage.setItem('shamrock:zzold:name', 'Old'); });
    await homePage.reload();
    await homePage.waitForSelector('#homeSheet.open');
    const before = await homePage.$$eval('#tableList .tableRow b', (els) => els.map((e) => e.textContent));
    await homePage.click('.tableItem:has(b:text("#zzold")) .forget');
    const after = await homePage.$$eval('#tableList .tableRow b', (els) => els.map((e) => e.textContent));
    await homePage.reload();
    await homePage.waitForSelector('#homeSheet.open');
    const reloaded = await homePage.$$eval('#tableList .tableRow b', (els) => els.map((e) => e.textContent));
    gate('Forget removes a table from my tables, for good', before.includes('#zzold') && !after.includes('#zzold') && !reloaded.includes('#zzold') && reloaded.includes(`#${room}`), `${before.join(' ')} → ${reloaded.join(' ')}`);
    // From the home page, tap a table; Back returns to the home page, Forward to the table.
    await homePage.click(`.tableItem:has(b:text("#${room}")) .tableRow`);
    await homePage.waitForFunction((r) => location.hash === `#${r}` && !document.getElementById('homeSheet').classList.contains('open') && window.__cl?.digest, room, { timeout: 8000 });
    await homePage.goBack();
    const backHome = await homePage.waitForSelector('#homeSheet.open', { timeout: 8000 }).then(() => true, () => false);
    await homePage.goForward();
    const forwardTable = await homePage.waitForFunction((r) => location.hash === `#${r}` && !document.getElementById('homeSheet').classList.contains('open'), room, { timeout: 8000 }).then(() => true, () => false);
    gate('Back from a table opened on the home page returns home (and Forward goes back in)', backHome && forwardTable, `back ${backHome}, forward ${forwardTable}`);
    // Starting a table: a suggested name and my usual name, both editable;
    // a typed name is used as typed (made link-safe), and I join on arrival.
    await homePage.goBack();
    await homePage.waitForSelector('#homeSheet.open');
    const suggested = await homePage.inputValue('#homeRoom');
    const myName = await homePage.inputValue('#homeName');
    await homePage.fill('#homeRoom', 'Family Night!');
    await homePage.click('#homeNew');
    const started = await homePage.waitForFunction(() => location.hash === '#family-night' && window.__cl?.ui?.().seated, null, { timeout: 8000 }).then(() => true, () => false);
    gate('starting a table suggests a name and uses my usual name; a typed name is kept (link-safe) and I join on arrival',
      /^[a-z]+-[a-z]+-[a-z]+$/.test(suggested) && myName === 'Ana' && started, `${suggested} | ${myName} | started ${started}`);
    // Opening another table's link: one tap, my name already filled in.
    const other = await A.page.context().newPage();
    await other.goto(`http://localhost:${PORT}/?signal=ws://localhost:${RELAY}#someone-elses-table`);
    await other.waitForSelector('#joinSheet.open');
    const pre = [await other.inputValue('#tableInput'), await other.inputValue('#nameInput')];
    gate('a new table link opens the join sheet with the table and my name filled in', pre[0] === 'someone-elses-table' && pre[1] === 'Ana', pre.join(' | '));
    await other.close();
    await homePage.close();
  }
  // The round's recap opens by itself: every clover's answer, one after another.
  {
    const open = await A.page.waitForSelector('#recapSheet.open', { timeout: 5000 }).then(() => true, () => false);
    const rows = await A.page.$$eval('#recapList .mini', (els) => els.length);
    gate('the end of a round opens a recap of every clover', open && rows === r1.puzzles.length, `open ${open}, ${rows} of ${r1.puzzles.length}`);
    await A.page.screenshot({ path: `${OUT}5-recap.png` });
    await A.page.click('#recapClose');
  }
  await A.page.screenshot({ path: `${OUT}5-end.png` });

  // ---- share boards: pick clovers, open the link at a fresh table ------------------------
  console.log('\n# share boards');
  await A.page.evaluate(() => document.getElementById('shareMenuBtn').click()); // Share boards (menu)
  await A.page.waitForSelector('#shareSheet.open');
  await A.page.click('#shareList .item:nth-child(2) input'); // leave one out
  await A.page.waitForFunction(() => !document.getElementById('shareCopy').disabled && document.getElementById('shareText').textContent.includes('?b='));
  const link = (await A.page.textContent('#shareText')).trim();
  // Nothing about the boards is readable from the link, raw or base64-decoded.
  {
    const payload = new URL(link).searchParams.get('b');
    const raw = Buffer.from(payload.replace(/-/g, '+').replace(/_/g, '/'), 'base64').toString('latin1').toLowerCase();
    const words = r1.subs.flatMap((x) => x.slice(3)).concat((await A.ev(() => window.__cl.X && [])) || []);
    const cardWords = await A.ev(() => { const d = window.__cl.digest(); return d.subs.length; });
    const leaks = words.filter((w) => w.length > 3 && (raw.includes(w.toLowerCase()) || link.toLowerCase().includes(w.toLowerCase())));
    gate("share links don't reveal clues or answers at a glance (encrypted)", leaks.length === 0 && cardWords > 0, leaks.join(','));
  }
  await A.page.screenshot({ path: `${OUT}7-share.png` });
  await A.page.click('#shareClose');
  metric('share_link_chars', link.length);
  gate('a four-clover share link stays under 1,200 characters (compressed)', link.length < 1200, `${link.length}`);
  // A share link is solo play: no room, no name, no peers; straight into the clovers.
  const sharedUrl = link.replace(/^https?:\/\/[^/]+\//, `http://localhost:${PORT}/`);
  const soloCtx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
  const solo = await soloCtx.newPage();
  solo.on('pageerror', (e) => errors.push(`solo: ${e.message}`));
  await solo.goto(sharedUrl);
  await solo.waitForFunction(() => window.__cl?.digest?.().phase === 'guessing:0', null, { timeout: 10000 });
  await sleep(1200);
  const fd = await solo.evaluate(() => window.__cl.digest());
  const soloUi = await solo.evaluate(() => ({ join: document.getElementById('joinSheet').classList.contains('open'),
    roster: document.getElementById('tableItems').offsetParent ? 'shown' : 'hidden', go: document.getElementById('go').textContent,
    remote: location.search.includes('signal') }));
  const srcClues = r1.subs.filter((x) => r1.puzzles.some((p, i) => i !== 1 && p.writer === x[0])).map((x) => x.slice(3).join('/'));
  const gotClues = fd.subs.map((x) => x.slice(3).join('/'));
  gate('a share link opens solo, straight into the chosen clovers (no room, no name)',
    !soloUi.join && soloUi.roster === 'hidden' && soloUi.go === 'Guess' && fd.puzzles.length === 4
    && srcClues.every((c) => gotClues.includes(c)), `${fd.puzzles.length} boards, join=${soloUi.join}, go=${soloUi.go}`);
  // Placing a shared board as its writer laid it out solves it first time.
  const Fr = { page: solo, ev: (fn, a) => solo.evaluate(fn, a), digest: () => solo.evaluate(() => window.__cl.digest()),
    card: (i) => solo.evaluate((i) => window.__cl.card(i), i), slot: (s) => solo.evaluate((s) => window.__cl.slot(s), s),
    settle: () => solo.evaluate(() => window.__cl.settle()),
    origin: () => solo.evaluate(() => { const r = document.getElementById('c').getBoundingClientRect(); return [r.left, r.top]; }) };
  Fr.drag = async (from, to) => { const o = await Fr.origin(), m = solo.mouse; await m.move(o[0] + from[0], o[1] + from[1]); await m.down();
    for (let k = 1; k <= 12; k++) await m.move(o[0] + from[0] + (to[0] - from[0]) * k / 12, o[1] + from[1] + (to[1] - from[1]) * k / 12); await m.up(); };
  Fr.tap = async (at) => { const o = await Fr.origin(); await solo.mouse.click(o[0] + at[0], o[1] + at[1]); };
  await solve(Fr);
  await solo.waitForFunction(() => !document.getElementById('go').disabled);
  await solo.click('#go');
  await solo.waitForFunction(() => window.__cl.digest().puzzles[0].attempts === 1, null, { timeout: 8000 });
  gate("a shared board checks against its writer's answer", (await solo.evaluate(() => window.__cl.digest())).puzzles[0].mask === 15);
  await solo.screenshot({ path: `${OUT}8-solo.png` });
  await soloCtx.close();

  // ---- next round keeps the table and the score -----------------------------------------
  console.log('\n# rounds and games');
  const nextLabel = await B.page.textContent('#go');
  // Ben's recap is open over the table; its Next round deals again.
  await B.page.waitForSelector('#recapSheet.open', { timeout: 5000 });
  await B.page.click('#recapNext');
  await B.page.waitForSelector('#dealSheet.open');
  const selectedNames = await B.page.locator('#dealList label').evaluateAll((rows) => rows.filter((r) => r.querySelector('input').checked).map((r) => r.querySelector('b').textContent));
  gate('next round shows the selected players before dealing', selectedNames.length === 5 && (await B.digest()).round === 1, selectedNames.join(', '));
  await B.page.click('#dealConfirm');
  await until(all, (d) => d.phase === 'writing' && d.round === 2);
  gate('the next round button says what it does', nextLabel === 'Next round', nextLabel);
  const r2 = await C.digest();
  gate('next round: same players', r2.seats.filter((s) => s[2]).length === 5);

  await D.ctx.close();
  // (Dee has left mid-writing; others would now see "Waiting for Dee (away)".)
  await A.page.waitForFunction(() => window.__cl.ui().players.find((p) => p.name === 'Dee')?.away, null, { timeout: 15000 })
    .then(() => gate('a player who leaves shows as away', true), () => gate('a player who leaves shows as away', false));
  await A.page.click('#menuBtn');
  await A.page.screenshot({ path: `${OUT}6-menu.png` });
  await A.page.click('#newGameBtn');
  const live = [A, B, C, E, A2, W];
  await until(live, (d) => d.phase === 'lobby' && d.round === 0);
  const g2 = await B.digest();
  const seated = g2.seats.filter((s) => s[2]);
  gate("New game starts over with whoever is here", seated.length === 4 && !seated.some((s) => s[1] === 'Dee'), seated.map((s) => s[1]).join(','));

  t0 = performance.now();
  const F = await open('Late');
  await F.page.waitForFunction((t) => JSON.stringify(window.__cl.digest()) === t, JSON.stringify(await A.digest()), { timeout: 30000 });
  metric('late_join_state_ms', performance.now() - t0, ' ms');
  gate('a late visitor receives the full table state', true);

  // ---- signaling down when people arrive: they must find each other without a reload ----
  console.log('\n# reconnecting');
  {
    const LATE = RELAY + 7;
    const url = `http://localhost:${PORT}/?signal=ws://localhost:${LATE}#late-relay`;
    const pages = [];
    for (const nm of ['Lu', 'Mo']) {
      const page = await (await browser.newContext({ viewport: { width: 1000, height: 800 } })).newPage();
      page.on('pageerror', (e) => errors.push(`${nm}: ${e.message}`));
      await page.goto(url);
      pages.push(page);
    }
    await sleep(2500);
    const lonely = await pages[0].textContent('#hint');
    gate('alone, the status line says it is looking for the others', /Waiting for others/.test(lonely), lonely);
    const relay = createWsRelayServer({ port: LATE, host: '127.0.0.1' });
    await relay.ready;
    t0 = performance.now();
    const met = await Promise.all(pages.map((p) => p.waitForFunction(() => window.__cl.peerCount() > 0, null, { timeout: 25000 }).then(() => true, () => false)));
    metric('meet_after_relay_returns_ms', performance.now() - t0, ' ms');
    gate('players who arrive while signaling is down find each other without a reload', met.every(Boolean));
    for (const p of pages) await p.context().close();
    await relay.close();
  }

  console.log('\n# tools for AI assistants');
  {
    // A stand-in for a browser with WebMCP: record what the page registers.
    const ctx = await browser.newContext();
    await ctx.addInitScript(() => {
      window.__registered = [];
      document.modelContext = { registerTool: async (t, opts) => { window.__registered.push(t); } };
    });
    const pg = await ctx.newPage();
    await pg.goto(`http://localhost:${PORT}/?net=local#webmcp-check`);
    await pg.waitForFunction(() => window.__registered?.length > 0, null, { timeout: 8000 }).catch(() => {});
    const reg = await pg.evaluate(async () => {
      const ok = window.__registered.filter((t) => /^[a-z_]+$/.test(t.name) && t.description?.length > 20
        && t.inputSchema?.type === 'object' && typeof t.execute === 'function');
      const table = window.__registered.find((t) => t.name === 'get_table');
      const out = table && await table.execute({});
      return { n: window.__registered.length, ok: ok.length, phase: out && JSON.parse(out.content[0].text).phase };
    });
    gate('with WebMCP, the page registers its twelve tools (name, description, schema) and they answer', reg.n === 12 && reg.ok === 12 && reg.phase === 'lobby', JSON.stringify(reg));
    await ctx.close();
  }

  console.log('\n# perf');
  await sleep(1000);
  const fms = await A.ev(() => window.__cl.stats.frameMs.slice(-300));
  metric('frame_cpu_p50_ms', pct(fms, 0.5), ' ms');
  metric('frame_cpu_p95_ms', pct(fms, 0.95), ' ms');
  gate('frame CPU (wasm + paint) p95 < 4 ms', pct(fms, 0.95) < 4);
  gate('no page errors', errors.length === 0, errors.slice(0, 3).join(' | '));
} catch (e) {
  gate('harness completed', false, e.stack?.split('\n').slice(0, 3).join(' '));
  for (const ctx of browser.contexts()) {
    for (const pg of ctx.pages()) {
      const st = await pg.evaluate(() => ({ ui: window.__cl?.ui(), hint: document.getElementById('hint').textContent })).catch((x) => String(x));
      console.log('  state', JSON.stringify(st).slice(0, 300));
      await pg.screenshot({ path: `${OUT}fail-${browser.contexts().indexOf(ctx)}.png` }).catch(() => {});
    }
  }
} finally {
  await browser.close();
  await server.close();
}

const passed = gates.filter((g) => g.ok).length;
console.log(`\n${passed}/${gates.length} gates passed`);
await writeFile(`${OUT}scorecard.json`, JSON.stringify({ at: new Date().toISOString(), passed, total: gates.length, gates, metrics }, null, 2));
process.exit(passed === gates.length ? 0 : 1);
