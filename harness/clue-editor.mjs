// A focused clue field must stay on its leaf while a real touch drag turns
// the clover. Touch drags retain focus even when a mouse click would blur it.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { serve } from './serve.mjs';

const PORT = 8985, RELAY = 8986;
const server = await serve({ port: PORT, relayPort: RELAY, quiet: true });
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM });
const errors = [];
const out = new URL('./out/clue-editor/', import.meta.url).pathname;
await mkdir(out, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function checkField(page, leaf, text) {
  const s = await page.evaluate((leaf) => {
    const c = window.__cl, input = document.getElementById('clue');
    const r = input.getBoundingClientRect(), canvas = document.getElementById('c').getBoundingClientRect();
    const camera = c.camera(), p = c.petal(leaf), a = c.board(0, 0), b = c.board(100, 0);
    const expected = [canvas.left + camera.x + p[0] * camera.s, canvas.top + camera.y + p[1] * camera.s];
    const matrix = new DOMMatrix(getComputedStyle(input).transform);
    const angle = Math.atan2(matrix.b, matrix.a) - Math.atan2(b[1] - a[1], b[0] - a[0]) - leaf * Math.PI / 2;
    return {
      visible: r.width > 0, focused: document.activeElement === input, value: input.value,
      selection: [input.selectionStart, input.selectionEnd],
      distance: Math.hypot(r.x + r.width / 2 - expected[0], r.y + r.height / 2 - expected[1]),
      angle: Math.abs(Math.atan2(Math.sin(angle), Math.cos(angle))),
    };
  }, leaf);
  assert.ok(s.visible && s.focused, 'rotation retains the open, focused editor');
  assert.equal(s.value, text, 'repositioning preserves the uncommitted draft');
  assert.deepEqual(s.selection, [1, 4], 'repositioning preserves the text selection');
  assert.ok(s.distance < 1.5, `editor stays on leaf ${leaf}: off by ${s.distance.toFixed(1)}px`);
  assert.ok(s.angle < 0.02, `editor follows the leaf angle: off by ${s.angle.toFixed(2)}rad`);
}

async function spin(page, cdp, leaf, text, dir) {
  const [cx, cy, radius] = await page.evaluate(() => {
    const c = window.__cl, r = document.getElementById('c').getBoundingClientRect();
    const [x, y] = c.hub(), p = c.petal(0);
    return [r.x + x, r.y + y, Math.hypot(p[0] - x, p[1] - y) * 0.65];
  });
  for (let k = 0; k <= 18; k++) {
    const angle = dir * k / 18 * Math.PI / 2;
    await cdp.send('Input.dispatchTouchEvent', {
      type: k ? 'touchMove' : 'touchStart',
      touchPoints: [{ x: cx + radius * Math.cos(angle), y: cy + radius * Math.sin(angle), id: 1 }],
    });
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await checkField(page, leaf, text);
  }
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  // Check the animated settling too, without jumping it to its final pose.
  for (let k = 0; k < 8; k++) { await sleep(60); await checkField(page, leaf, text); }
}

try {
  for (const [width, height] of [[390, 844], [844, 390]]) {
    const context = await browser.newContext({ viewport: { width, height }, hasTouch: true, isMobile: true });
    const page = await context.newPage();
    page.on('pageerror', (e) => errors.push(e.message));
    await page.goto(`http://localhost:${PORT}/?net=local#editor-${width}`);
    await page.waitForFunction(() => window.__cl?.tools);
    await page.evaluate(() => window.__cl.tools.join_table({ name: 'Ana' }));
    assert.equal((await page.evaluate(() => window.__cl.tools.start_round())).ok, true);
    await page.evaluate(() => window.__cl.settle());
    const cdp = await context.newCDPSession(page);
    for (let leaf = 0; leaf < 4; leaf++) {
      const text = `draft for leaf ${leaf}`;
      const point = await page.evaluate((leaf) => {
        const r = document.getElementById('c').getBoundingClientRect(), p = window.__cl.petal(leaf);
        return [r.x + p[0], r.y + p[1]];
      }, leaf);
      await page.touchscreen.tap(...point);
      await page.locator('#clue').fill(text);
      await page.locator('#clue').evaluate((input) => input.setSelectionRange(1, 4));
      await checkField(page, leaf, text);
      const q = await page.evaluate(() => window.__cl.quarter());
      await spin(page, cdp, leaf, text, leaf % 2 ? -1 : 1);
      assert.notEqual(await page.evaluate(() => window.__cl.quarter()), q, 'the touch gesture actually rotates the board');
      // A keyboard opening or a phone orientation change must also keep
      // the same draft attached, without selecting all of its text again.
      await page.setViewportSize({ width, height: height - 80 });
      await sleep(100); await checkField(page, leaf, text);
      await page.setViewportSize({ width, height });
      await sleep(100); await checkField(page, leaf, text);
      if (leaf === 0) await page.screenshot({ path: `${out}${width}-rotated.png` });
      await page.keyboard.press('Enter');
      const clues = await page.evaluate(() => window.__cl.digest().subs[0].slice(3));
      assert.equal(clues[leaf], text, 'Enter commits to the original leaf after rotation');
      for (let d = leaf + 1; d < 4; d++) assert.equal(clues[d], '', 'other leaves stay untouched');
      // Enter may advance to another leaf; Escape must leave it closed,
      // including after another board rotation and its animation frames.
      await page.keyboard.press('Escape');
      await page.evaluate(() => window.__cl.X.rotate_view(1));
      await sleep(800);
      assert.equal(await page.locator('#clue').isVisible(), false, 'a canceled editor stays closed');
    }
    // Plain taps still dismiss and save the field before turning the hub.
    const point = await page.evaluate(() => {
      const r = document.getElementById('c').getBoundingClientRect(), p = window.__cl.petal(0);
      return [r.x + p[0], r.y + p[1]];
    });
    await page.touchscreen.tap(...point);
    await page.locator('#clue').fill('saved by blur');
    const hub = await page.evaluate(() => {
      const r = document.getElementById('c').getBoundingClientRect(), p = window.__cl.hub();
      return [r.x + p[0], r.y + p[1]];
    });
    await page.touchscreen.tap(...hub);
    await sleep(800);
    assert.equal(await page.locator('#clue').isVisible(), false);
    assert.equal(await page.evaluate(() => window.__cl.digest().subs[0][3]), 'saved by blur');
    console.log(`PASS ${width}×${height}: four leaves, both spin directions, animation, resize, draft/selection, Enter and blur`);
    await context.close();
  }
  assert.deepEqual(errors, []);
  console.log('ALL PASS');
} finally { await browser.close(); await server.close(); }
