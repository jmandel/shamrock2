// Fault scenarios in real browsers: what the Rust network simulator can't
// reach (the page's connection handling, reload and restore, the status
// line, real WebRTC and signaling). Each scenario is its own table with its
// own browser contexts (separate storage, like separate phones); moves go
// through the page's agent tools. Faults:
//   - the page's ?chaos hook: `cut` silences a page both ways while its
//     connections stay up (a phone in a tunnel), `delayMs` lags what arrives;
//   - the signaling relay going down and coming back;
//   - pages reloading, closing mid-gesture, and every page going away.
//
//   node harness/faults.mjs            exit code 1 if any check fails
import { chromium } from 'playwright';
import { serve } from './serve.mjs';

const PORT = 8941, RELAY = 8942;
const srv = await serve({ port: PORT, relayPort: RELAY, quiet: true });
const browser = await chromium.launch({ args: ['--disable-features=WebRtcHideLocalIpsWithMdns'] });
const fails = [], errors = [];
const check = (ok, what, detail = '') => { console.log(`${ok ? 'PASS' : 'FAIL'}  ${what}${detail ? `  — ${detail}` : ''}`); if (!ok) fails.push(what); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const until = async (fn, ms = 15000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { try { if (await fn()) return true; } catch {} await sleep(150); } return false; };
const url = (room, extra = '') => `http://localhost:${PORT}/?signal=ws://localhost:${RELAY}&chaos${extra}#${room}`;

/** A player: their own browser context (storage), one page at the table. */
async function player(name, room, { join = true } = {}) {
  const ctx = await browser.newContext({ viewport: { width: 900, height: 900 } });
  await ctx.addInitScript(() => { for (const k of ['guess', 'write']) localStorage.setItem(`shamrock:tip:${k}`, '1'); });
  const p = { name, ctx };
  p.open = async (extra = '&fresh') => {
    p.page = await ctx.newPage();
    p.page.on('pageerror', (e) => errors.push(`${name}: ${e.message}`));
    await p.page.goto(url(room, extra));
    await p.page.waitForFunction(() => window.__cl?.tools);
  };
  p.tool = (t, a = {}) => p.page.evaluate(([t, a]) => window.__cl.tools[t](a), [t, a]);
  p.table = () => p.tool('get_table');
  p.digest = () => p.page.evaluate(() => JSON.stringify(window.__cl.digest()));
  p.chaos = (c) => p.page.evaluate((c) => Object.assign(window.__cl.chaos, c), c);
  p.status = () => p.page.textContent('#hint');
  await p.open();
  if (join) await p.tool('join_table', { name });
  return p;
}
const live = (ps) => ps.filter((p) => !p.page.isClosed());
const agree = async (ps) => { const d = await Promise.all(live(ps).map((p) => p.digest())); return d.every((x) => x === d[0]); };
const seatedEverywhere = async (ps, n) => (await Promise.all(live(ps).map((p) => p.table()))).every((t) => t.players.length === n && t.seated);
const writeAll = async (p, word, submit = true) => {
  await until(async () => (await p.table()).phase === 'writing', 10000); // the deal reaches this screen first
  for (const leaf of ['top', 'right', 'bottom', 'left']) await p.tool('write_clue', { leaf, text: `${word} ${leaf}` });
  if (submit) await p.tool('submit_clover');
};
async function table(room, names) {
  const ps = [];
  for (const n of names) ps.push(await player(n, room));
  await until(() => seatedEverywhere(ps, names.length), 30000);
  return ps;
}
// Phones save every 2 s (and on the way out, which a closing page may not
// reach): wait until each has saved since `t` before closing it.
const savedSince = (ps, room, t) => Promise.all(ps.map((p) => until(() => p.page.evaluate(([r, t]) => (JSON.parse(localStorage.getItem(`shamrock:${r}:state`) ?? '{}').at ?? 0) > t, [room, t]), 15000)));
const done = async (ps) => { for (const p of ps) await p.ctx.close().catch(() => {}); };

// ONLY=text runs just the scenarios whose title contains it.
async function scenario(title, fn) {
  if (process.env.ONLY && !title.includes(process.env.ONLY)) return;
  console.log(`\n# ${title}`);
  try { await fn(); } catch (e) { check(false, `${title}: completed`, e.message.split('\n')[0]); }
}

await scenario('everyone joins at once, before anyone has connected', async () => {
  const room = `storm-${Date.now().toString(36)}`;
  // Four phones open the link together and sit down straight away.
  const ps = await Promise.all(['Ana', 'Ben', 'Cy', 'Dee'].map((n) => player(n, room)));
  check(await until(() => seatedEverywhere(ps, 4), 30000), 'all four end up seated on every screen');
  check(await until(() => agree(ps)), 'and every screen agrees');
  await ps[2].tool('start_round');
  for (const p of ps) await writeAll(p, p.name.toLowerCase());
  check(await until(async () => (await Promise.all(ps.map((p) => p.table()))).every((t) => t.phase === 'guessing'), 20000), 'a round dealt and submitted reaches guessing everywhere');
  await done(ps);
});

await scenario('a phone in a tunnel: cut off with its connections still up', async () => {
  const room = `tunnel-${Date.now().toString(36)}`;
  const ps = await table(room, ['Ana', 'Ben', 'Cy']);
  const [ana, ben, cy] = ps;
  await ana.tool('start_round');
  await until(async () => (await cy.table()).phase === 'writing');
  await cy.chaos({ cut: true });
  // Both sides carry on: Ana and Ben write and submit, Cy writes in the tunnel.
  await writeAll(ana, 'ana');
  await writeAll(ben, 'ben');
  await writeAll(cy, 'cy', false);
  await sleep(1500);
  check((await ana.table()).phase === 'writing', 'guessing waits for the player who is cut off');
  // Out of the tunnel. No connection opened or closed, so no full sync was
  // triggered: the table hashes in presence pings have to find the drift.
  await cy.chaos({ cut: false });
  const t0 = Date.now();
  check(await until(() => agree(ps), 20000), 'once back, every screen agrees again', `${((Date.now() - t0) / 1000).toFixed(1)} s`);
  const view = await ana.tool('get_table');
  check(view.readyCount === 2 && view.phase === 'writing', "Ana sees Cy's table as it is (two submitted, Cy still writing)", JSON.stringify({ phase: view.phase, ready: view.readyCount }));
  await cy.tool('submit_clover');
  check(await until(async () => (await Promise.all(ps.map((p) => p.table()))).every((t) => t.phase === 'guessing')), 'Cy submits and guessing starts everywhere');
  await done(ps);
});

await scenario('a laggy connection (600 ms behind)', async () => {
  const room = `lag-${Date.now().toString(36)}`;
  const ps = await table(room, ['Ana', 'Ben', 'Cy']);
  await ps[1].chaos({ delayMs: 600 });
  await ps[0].tool('start_round');
  for (const p of ps) await writeAll(p, p.name.toLowerCase());
  check(await until(async () => (await Promise.all(ps.map((p) => p.table()))).every((t) => t.phase === 'guessing'), 20000), 'a whole writing phase completes with one player lagging');
  // Guess the first clover from the laggy screen: the tools see what it sees.
  const lag = ps[1], t = await lag.table();
  if (!t.clover.isMine) {
    const labels = await lag.page.evaluate(() => window.__cl.agentView().cards.filter((c) => c._i < 4).sort((a, b) => a._i - b._i).map((c) => c.card));
    for (let s = 0; s < 4; s++) await lag.tool('place_card', { card: labels[s], spot: s, clover: t.clover.index });
  }
  check(await until(() => agree(ps), 15000), 'moves from the laggy screen reach everyone, and agree');
  await done(ps);
});

await scenario('reloading mid-writing', async () => {
  const room = `reload-${Date.now().toString(36)}`;
  const ps = await table(room, ['Ana', 'Ben', 'Cy']);
  const [ana, ben] = ps;
  await ana.tool('start_round');
  await until(async () => (await ben.table()).phase === 'writing');
  await ben.tool('write_clue', { leaf: 'top', text: 'kept' });
  await ben.tool('write_clue', { leaf: 'left', text: 'also kept' });
  await sleep(500);
  // A plain reload (no ?fresh): the page restores its save and rejoins as Ben.
  await ben.page.goto(url(room));
  await ben.page.waitForFunction(() => window.__cl?.tools);
  check(await until(async () => (await ben.table()).you === 'Ben' && (await ben.table()).seated, 10000), 'Ben is back in his seat without being asked');
  const clues = (await ben.table()).clover.clues;
  check(clues.top === 'kept' && clues.left === 'also kept', 'his half-written clues are still there', JSON.stringify(clues));
  check(await until(() => agree(ps)), 'and every screen agrees');
  await done(ps);
});

await scenario('everyone closes the table, then comes back', async () => {
  const room = `gone-${Date.now().toString(36)}`;
  const ps = await table(room, ['Ana', 'Ben', 'Cy']);
  await ps[0].tool('start_round');
  for (const p of ps) await writeAll(p, p.name.toLowerCase());
  await until(async () => (await ps[0].table()).phase === 'guessing');
  const before = await ps[0].digest();
  await sleep(2500); // past a save
  for (const p of ps) await p.page.close({ runBeforeUnload: true });
  // Only Cy comes back at first: no one else holds the table.
  await ps[2].open('');
  const t = await ps[2].table();
  check(t.phase === 'guessing' && t.players.length === 3, 'the first one back finds the table from their own save', JSON.stringify({ phase: t.phase, players: t.players.length }));
  await ps[0].open('');
  check(await until(async () => (await ps[0].digest()) === (await ps[2].digest())), 'the next one back agrees with them');
  check((await ps[0].digest()) === before, 'and nothing was lost');
  await done(ps);
});

await scenario('the signaling relay goes down mid-game', async () => {
  const room = `relay-${Date.now().toString(36)}`;
  const ps = await table(room, ['Ana', 'Ben']);
  await ps[0].tool('start_round');
  await srv.stopRelay();
  // Connected players keep playing over WebRTC.
  for (const p of ps) await writeAll(p, p.name.toLowerCase());
  check(await until(async () => (await Promise.all(ps.map((p) => p.table()))).every((t) => t.phase === 'guessing'), 15000), 'players already connected keep playing without the relay');
  // Someone new can't find the table until it's back.
  const cy = await player('Cy', room, { join: false });
  await sleep(3000);
  check((await cy.table()).players.length === 0, 'a newcomer meanwhile sees nobody', (await cy.status()).trim());
  check(/waiting|reconnecting/i.test(await cy.status()), 'and is told so in the status line', (await cy.status()).trim());
  await srv.startRelay();
  check(await until(async () => (await cy.table()).players.length === 2, 30000), 'when the relay is back, the newcomer finds the table (retrying on its own)');
  check(await until(() => agree([...ps, cy])), 'and receives the whole table');
  await done([...ps, cy]);
});

await scenario('a phone dies mid-drag', async () => {
  const room = `drag-${Date.now().toString(36)}`;
  const ps = await table(room, ['Ana', 'Ben', 'Cy']);
  await ps[0].tool('start_round');
  for (const p of ps) await writeAll(p, p.name.toLowerCase());
  await until(async () => (await Promise.all(ps.map((p) => p.table()))).every((t) => t.phase === 'guessing'));
  const writer = (await ps[0].table()).clover.writer;
  const [g, other] = ps.filter((p) => p.name !== writer);
  // A real finger on a real card, moved but never lifted.
  const at = await g.page.evaluate(() => { window.__cl.settle(); const r = document.getElementById('c').getBoundingClientRect(); const c = window.__cl.card(0); return [r.left + c[0], r.top + c[1]]; });
  await g.page.mouse.move(at[0], at[1]);
  await g.page.mouse.down();
  for (let k = 1; k <= 8; k++) await g.page.mouse.move(at[0] + 10 * k, at[1] + 6 * k);
  check(await until(() => other.page.evaluate(() => Object.keys(window.__cl.holds()).length > 0), 3000), 'the other guesser sees the card held');
  await g.ctx.close();
  check(await until(() => other.page.evaluate(() => Object.keys(window.__cl.holds()).length === 0), 8000), 'when that phone dies, the hold goes away');
  const labels = await other.page.evaluate(() => window.__cl.agentView().cards.filter((c) => c._i === 0).map((c) => c.card));
  const r = await other.tool('place_card', { card: labels[0], spot: 0, clover: (await other.table()).clover.index });
  check(r.ok, 'and the card can be moved by someone else', r.result);
  await done(ps);
});

await scenario('two hands reach for the same card', async () => {
  const room = `hands-${Date.now().toString(36)}`;
  const ps = await table(room, ['Ana', 'Ben', 'Cy']);
  await ps[0].tool('start_round');
  for (const p of ps) await writeAll(p, p.name.toLowerCase());
  await until(async () => (await Promise.all(ps.map((p) => p.table()))).every((t) => t.phase === 'guessing'));
  const writer = (await ps[0].table()).clover.writer;
  const [a, b] = ps.filter((p) => p.name !== writer);
  const watcher = ps.find((p) => p.name === writer);
  for (const p of ps) await p.page.evaluate(() => window.__cl.settle());
  const screen = (p, what, i) => p.page.evaluate(([what, i]) => { const r = document.getElementById('c').getBoundingClientRect(); const c = window.__cl[what](i); return [r.left + c[0], r.top + c[1]]; }, [what, i]);
  const where = (p, i) => p.page.evaluate((i) => { const d = window.__cl.digest(); return JSON.stringify(d.puzzles[+d.phase.split(':')[1]].cards[i]); }, i);
  const label = (p, i) => p.page.evaluate((i) => window.__cl.agentView().cards.find((c) => c._i === i).card, i);
  const toastText = (p) => p.page.evaluate(() => document.getElementById('toast').textContent);

  // One guesser holds card 1 (finger down, moving); the other can't take it.
  const at = await screen(a, 'card', 1);
  await a.page.mouse.move(...at);
  await a.page.mouse.down();
  for (let k = 1; k <= 6; k++) await a.page.mouse.move(at[0] + 8 * k, at[1] + 5 * k);
  await until(() => b.page.evaluate(() => window.__cl.holds()[1] !== undefined), 5000);
  const before = await where(b, 1);
  const r = await b.tool('place_card', { card: await label(b, 1), spot: 3, clover: (await b.table()).clover.index });
  check(!r.ok && r.result.includes(`${a.name} is moving`), `${b.name}'s tools say ${a.name} has the card`, r.result);
  const bAt = await screen(b, 'card', 1), bTo = await screen(b, 'slot', 3);
  await b.page.mouse.move(...bAt); await b.page.mouse.down();
  for (let k = 1; k <= 8; k++) await b.page.mouse.move(bAt[0] + (bTo[0] - bAt[0]) * k / 8, bAt[1] + (bTo[1] - bAt[1]) * k / 8);
  await b.page.mouse.up();
  check(await where(b, 1) === before, `${b.name}'s own drag doesn't move it`, `${before} → ${await where(b, 1)}`);
  check((await toastText(b)).includes(`${a.name} is moving this card`), `${b.name} is told who has it`, await toastText(b));
  await a.page.mouse.up();

  // Both grab card 2 at once, each 300 ms behind the other, toward different spots.
  for (const p of [a, b]) await p.chaos({ delayMs: 300 });
  await until(() => watcher.page.evaluate(() => Object.keys(window.__cl.holds()).length === 0), 5000);
  const [aFrom, bFrom, aTo, bTo2] = [await screen(a, 'card', 2), await screen(b, 'card', 2), await screen(a, 'slot', 1), await screen(b, 'slot', 3)];
  await Promise.all([[a, aFrom], [b, bFrom]].map(async ([p, f]) => { await p.page.mouse.move(...f); await p.page.mouse.down(); }));
  const seen = [];
  const sampling = (async () => { const t0 = Date.now(); while (Date.now() - t0 < 2600) { seen.push(await watcher.page.evaluate(() => window.__cl.holds()[2] ?? null)); await sleep(50); } })();
  for (let k = 1; k <= 24; k++) {
    await Promise.all([[a, aFrom, aTo], [b, bFrom, bTo2]].map(([p, f, t]) => p.page.mouse.move(f[0] + (t[0] - f[0]) * k / 24, f[1] + (t[1] - f[1]) * k / 24)));
    await sleep(60);
  }
  await sampling;
  const settled = seen.slice(-20).filter((x) => x !== null);
  check(settled.length > 0 && settled.every((x) => x === settled[0]), 'the watcher sees the card settle in one hand', JSON.stringify(seen));
  const keeper = (await a.page.evaluate(() => window.__cl.device)) < (await b.page.evaluate(() => window.__cl.device)) ? a : b;
  await Promise.all([a, b].map((p) => p.page.mouse.up()));
  for (const p of [a, b]) await p.chaos({ delayMs: 0 });
  check(await until(() => agree(ps), 15000), 'every screen agrees afterwards');
  const spot = (await watcher.page.evaluate(() => window.__cl.agentView().cards.find((c) => c._i === 2).at));
  check(spot === `slot ${keeper === a ? 1 : 3}`, `the card is where the keeper (${keeper.name}) dropped it`, spot);
  await done(ps);
});

await scenario('left alone, then joined again', async () => {
  const room = `alone-${Date.now().toString(36)}`;
  const ps = await table(room, ['Ana', 'Ben']);
  await ps[1].page.close();
  check(await until(async () => /waiting|reconnecting/i.test(await ps[0].status()), 10000), 'the one left behind is told they are on their own', (await ps[0].status()).trim());
  await ps[1].open('');
  check(await until(async () => !/waiting|reconnecting/i.test(await ps[0].status()), 15000), 'and the notice clears when someone is back', (await ps[0].status()).trim());
  check(await until(() => agree(ps)), 'with the table in agreement');
  await done(ps);
});

await scenario('two groups played apart in one room, then meet, and one moves away', async () => {
  const room = `two-${Date.now().toString(36)}`;
  const happenedAt = (p, r) => p.page.evaluate((r) => JSON.parse(localStorage.getItem(`shamrock:${r}:happened`) ?? '[]').map((e) => e.text), r);
  const toastText = (p) => p.page.evaluate(() => document.getElementById('toast').textContent);
  // Ana, Ben and Eve start writing, then put their phones away.
  const ab = await table(room, ['Ana', 'Ben', 'Eve']);
  const eve = ab.pop();
  await ab[0].tool('start_round');
  await until(async () => (await ab[0].table()).phase === 'writing');
  await ab[0].tool('write_clue', { leaf: 'top', text: 'moonlight' });
  await ab[1].tool('write_clue', { leaf: 'top', text: 'starlight' });
  await savedSince([...ab, eve], room, Date.now());
  for (const p of [...ab, eve]) await p.page.close({ runBeforeUnload: true });
  // Later Cy and Dee use the same room and get a round further.
  const cd = await table(room, ['Cy', 'Dee']);
  await cd[0].tool('start_round');
  for (const p of cd) await writeAll(p, p.name.toLowerCase());
  await until(async () => (await cd[0].table()).phase === 'guessing');
  await cd[0].page.evaluate(() => document.getElementById('redealBtn').click());
  await cd[0].page.waitForSelector('#dealSheet.open');
  await cd[0].page.click('#dealConfirm');
  await until(async () => (await cd[1].table()).round === 2);
  await savedSince(cd, room, Date.now());
  for (const p of cd) await p.page.close({ runBeforeUnload: true });
  // Ana, Ben, Cy and Dee come back at once, each phone with its own save.
  const all = [...ab, ...cd];
  await Promise.all(all.map((p) => p.open('')));
  check(await until(() => agree(all), 30000), 'one table on every screen');
  const t = await ab[0].table();
  check(t.round === 2 && t.players.length === 5, 'the game further along carries on, with everyone seated', JSON.stringify({ round: t.round, players: t.players.length }));
  await until(async () => /Past games/.test(await toastText(ab[0])), 5000);
  const toast = await toastText(ab[0]);
  check(/Cy and Dee have also been playing at this table/.test(toast) && /Past games/.test(toast) && /Move us/.test(toast), 'Ana is told what happened, and offered a move', toast);
  check((await happenedAt(ab[0], room)).some((x) => /also been playing/.test(x)), 'and it is listed under What happened');
  const log = await ab[0].page.evaluate(() => JSON.parse(localStorage.getItem('shamrock:log') ?? '[]'));
  check(log.some((e) => e.finished === false && e.boards.some((b) => b.text.includes('moonlight'))), 'her half-written round is in Past games, marked unfinished');
  for (const p of cd) { const h = await happenedAt(p, room); check(!h.some((x) => /also been playing/.test(x)), `${p.name} lost nothing to the merge, so is told nothing about it`, JSON.stringify(h)); }

  // Ana moves her group.
  await ab[0].page.click('#toast button');
  check(await until(() => ab[0].page.evaluate((r) => location.hash.slice(1) !== r, room), 10000), 'Ana goes to a new table');
  const newRoom = await ab[0].page.evaluate(() => decodeURIComponent(location.hash.slice(1)));
  await ab[0].page.waitForFunction(() => window.__cl?.tools);
  await until(async () => (await ab[0].table()).you === 'Ana', 5000); // rejoins under her name
  const at = await ab[0].table();
  check(at.round === 1 && at.phase === 'writing' && at.you === 'Ana' && at.clover.clues.top === 'moonlight', `at #${newRoom} she is back in round 1, seated, her clue there`, JSON.stringify({ round: at.round, phase: at.phase, you: at.you, top: at.clover.clues.top }));
  // Ben is offered the move, and follows.
  check(await until(async () => new RegExp(`Ana moved your group to #${newRoom}`).test(await toastText(ab[1])), 10000), 'Ben is told where his group went', await toastText(ab[1]));
  check((await happenedAt(ab[1], room)).some((x) => /moved your group/.test(x)), 'and it is listed under What happened');
  await ab[1].page.click('#toast button');
  await until(() => ab[1].page.evaluate((r) => location.hash.slice(1) === r, newRoom), 10000);
  await ab[1].page.waitForFunction(() => window.__cl?.tools);
  await until(async () => (await ab[1].table()).you === 'Ben', 5000);
  check(await until(() => agree(ab), 20000), 'Ana and Ben are one table again at the new room');
  const bt = await ab[1].table();
  check(bt.clover.clues.top === 'starlight' && bt.players.length === 3, "Ben's clue is there, with their group's seats", JSON.stringify({ top: bt.clover.clues.top, players: bt.players.map((x) => x.name) }));
  // Nobody else was offered it, and the old room carries on.
  for (const p of cd) check(!(await happenedAt(p, room)).some((x) => /moved/.test(x)), `${p.name} was never offered the move`);
  check((await cd[0].table()).round === 2, 'the old room is untouched');
  // Eve was away through all this. Opening the room later, from her save,
  // she is offered the move too.
  await eve.open('');
  check(await until(async () => (await happenedAt(eve, room)).some((x) => new RegExp(`moved your group to #${newRoom}`).test(x)), 20000), 'Eve, back later from her save, is offered the move', JSON.stringify(await happenedAt(eve, room)));
  await done([...all, eve]);
});

await scenario('a page on an older version', async () => {
  const room = `ver-${Date.now().toString(36)}`;
  const [ana, ben] = await table(room, ['Ana', 'Ben']);
  await ana.tool('start_round');
  await until(async () => (await ben.table()).phase === 'writing');
  // Ben's page hears a Hello from a newer version of the game.
  await ben.page.evaluate(() => {
    const { X } = window.__cl;
    const b = new Uint8Array(15), v = new DataView(b.buffer);
    b[0] = 20; v.setUint32(1, 12345, true); v.setUint32(5, 0, true); v.setUint32(9, 0, true); v.setUint16(13, X.protocol() + 1, true);
    new Uint8Array(X.memory.buffer, X.inbox_reserve(b.length), b.length).set(b);
    X.on_message(b.length, 99);
  });
  check(await until(async () => /newer version/.test(await ben.page.evaluate(() => document.getElementById('toast').textContent)), 5000), 'it is asked to reload');
  await ben.tool('write_clue', { leaf: 'top', text: 'unsent' });
  await sleep(2500);
  check((await ana.table()).readyCount === 0 && !(await ana.digest()).includes('unsent'), 'and its changes no longer go out (they could be misread)');
  await done([ana, ben]);
});

check(errors.length === 0, 'no page errors', errors.slice(0, 3).join(' | '));
await browser.close();
await srv.close();
console.log(fails.length ? `\n${fails.length} failed` : '\nall passed');
process.exit(fails.length ? 1 : 0);
