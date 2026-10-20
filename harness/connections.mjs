// Exercise connection negotiation itself, including the case the ordinary
// game harness misses: signaling succeeds but WebRTC cannot find a route.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdir, open } from 'node:fs/promises';
import { chromium } from 'playwright';
import { serve } from './serve.mjs';

const PORT = 8837, SIGNAL = 8838, TURN = 8839;
const base = `http://localhost:${PORT}/`;
const server = await serve({ port: PORT, relayPort: SIGNAL, quiet: true });
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM ?? undefined });
const errors = [];
const contexts = [];
let turn, turnLog;

async function page(room, { delay = false, block = false, config, ice } = {}) {
  const context = await browser.newContext();
  contexts.push(context);
  await context.addInitScript(({ delay, block, relay }) => {
    window.__blockIce = block;
    window.__rtc = [];
    const Native = RTCPeerConnection;
    window.RTCPeerConnection = class extends Native {
      constructor(config) {
        super(window.__blockIce ? { ...config, iceServers: [], iceTransportPolicy: 'relay' }
          : relay ? { ...config, iceTransportPolicy: 'relay' } : config);
        window.__rtc.push(this);
      }
      async setRemoteDescription(description) {
        if (delay && description.type === 'answer') await new Promise((r) => setTimeout(r, 14000));
        return super.setRemoteDescription(description);
      }
    };
  }, { delay, block, relay: !!ice });
  if (config) await context.route('**/network.json', (route) => route.fulfill({ json: config }));
  if (ice) await context.route('**/test-ice', (route) => route.fulfill({ json: ice }));
  const p = await context.newPage();
  p.on('pageerror', (e) => errors.push(e.message));
  await p.goto(`${base}?signal=ws://localhost:${SIGNAL}#${room}`);
  await p.waitForFunction(() => window.__cl);
  return p;
}

const connected = (pages) => Promise.all(pages.map((p) => p.waitForFunction(() => __cl.peerCount() > 0, null, { timeout: 40000 })));
const close = (pages) => Promise.all(pages.map((p) => p.context().close()));

try {
  {
    const room = `slow-${randomUUID()}`;
    const pages = await Promise.all([page(room, { delay: true }), page(room, { delay: true })]);
    await connected(pages);
    for (const p of pages) assert.equal(await p.evaluate(() => __cl.stats.joins), 1);
    for (const p of pages) {
      const report = await p.evaluate(() => __cl.connectionReport());
      assert.equal(report.current.settings.turnServers, 0);
      assert(report.current.attempts.some((a) => a.state === 'connected' && a.path?.kind === 'direct'));
      assert(report.current.relays.some((r) => r.state === 'open'));
    }
    console.log('PASS slow negotiation survives past the old 12-second room reset');
    console.log('PASS diagnostics distinguish a selected direct path from unconfigured TURN');
    await close(pages);
  }
  {
    const room = `blocked-${randomUUID()}`;
    const pages = await Promise.all([page(room, { block: true }), page(room, { block: true })]);
    await Promise.all(pages.map((p) => p.waitForFunction(() => document.getElementById('hint').textContent.includes("Couldn't connect"), null, { timeout: 65000 })));
    for (const p of pages) {
      assert.equal(await p.evaluate(() => __cl.peerCount()), 0);
      assert.match(await p.evaluate(() => __cl.stats.connectionError), /after exchanging SDP/);
      assert.equal(await p.evaluate(() => __cl.stats.joins), 1);
      const report = await p.evaluate(() => __cl.connectionReport());
      assert(report.current.discovered > 0);
      assert.equal(report.current.app.connectedPeers, 0);
      assert(report.current.events.some((e) => e.kind === 'join-error' && e.category === 'route-failed'));
      assert(!JSON.stringify(report).includes(room), 'reports must not contain raw room names');
      await p.evaluate(() => { window.__blockIce = false; });
    }
    console.log('PASS a real failed ICE negotiation reports connection failure, not an empty room');
    // A new reachable peer arrives after the route becomes available. The
    // existing pages must clear their error without reloading or losing state.
    pages.push(await page(room));
    await connected(pages);
    for (const p of pages) assert.equal(await p.evaluate(() => document.getElementById('hint').textContent.includes("Couldn't connect")), false);
    console.log('PASS a successful connection clears the failure on the existing page');
    await close(pages);
  }
  {
    const room = `config-down-${randomUUID()}`;
    const pages = await Promise.all([page(room, { config: { iceServersUrl: './missing-credentials' } }), page(room)]);
    await connected(pages);
    const report = await pages[0].evaluate(() => __cl.connectionReport());
    assert.equal(report.current.settings.state, 'failed');
    assert.equal(report.current.settings.source, 'credentials');
    assert.equal(report.current.settings.status, 404);
    await pages[0].reload();
    await pages[0].waitForFunction(() => window.__cl);
    const reloaded = await pages[0].evaluate(() => __cl.connectionReport());
    assert.equal(reloaded.previous[0].session, report.current.session);
    assert.equal(reloaded.previous[0].settings.status, 404);
    console.log('PASS unavailable relay credentials still allow a direct connection');
    console.log('PASS a reload preserves the previous connection report');
    await close(pages);
  }
  {
    await server.stopRelay();
    const p = await page(`discovery-down-${randomUUID()}`);
    await p.evaluate(() => __cl.watch());
    await p.click('#menuBtn');
    await p.click('#connectionBtn');
    await p.waitForFunction(() => document.getElementById('connectionHeadline').textContent.includes('Cannot reach room discovery'), null, { timeout: 20000 });
    assert.equal((await p.evaluate(() => __cl.connectionReport())).current.discovered, 0);
    // Clipboard denial must leave a selectable report on the phone.
    await p.evaluate(() => { Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { throw new Error('denied'); } } }); });
    await p.click('#connectionCopy');
    await p.waitForFunction(() => !document.getElementById('connectionReport').hidden);
    assert.equal(JSON.parse(await p.inputValue('#connectionReport')).current.schema, 1);
    const download = p.waitForEvent('download');
    await p.click('#connectionSave');
    assert.match((await download).suggestedFilename(), /^shamrock-connection-.*\.json$/);
    await server.startRelay();
    await p.waitForFunction(async () => (await __cl.connectionReport()).current.relays.some((r) => r.state === 'open'), null, { timeout: 40000 });
    console.log('PASS discovery outage, clipboard fallback, report download and relay recovery');
    await close([p]);
  }
  if (process.env.TURN_BIN) {
    await mkdir(new URL('./out/', import.meta.url), { recursive: true });
    turnLog = await open(new URL('./out/turn.log', import.meta.url), 'w');
    const credential = randomUUID();
    turn = spawn(process.env.TURN_BIN, [
      '-n', '--no-cli', '--no-tls', '--listening-ip=127.0.0.1', '--relay-ip=127.0.0.1',
      `--listening-port=${TURN}`, '--min-port=8840', '--max-port=8859', '--allow-loopback-peers',
      '--no-multicast-peers', '--realm=shamrock-harness', `--user=shamrock:${credential}`, '--lt-cred-mech',
      '--relay-threads=1', '--log-file=stdout', `--pidfile=/tmp/shamrock-turn-${process.pid}.pid`,
    ], { stdio: ['ignore', turnLog.fd, turnLog.fd] });
    await new Promise((resolve, reject) => {
      turn.once('error', reject);
      turn.once('exit', (code) => reject(new Error(`TURN exited with ${code}`)));
      setTimeout(resolve, 1000);
    });
    const room = `turn-${randomUUID()}`;
    // TCP transport covers the fallback needed when UDP is blocked.
    const ice = [{ urls: `turn:127.0.0.1:${TURN}?transport=tcp`, username: 'shamrock', credential }];
    const pages = await Promise.all([page(room, { config: { iceServersUrl: './test-ice' }, ice }), page(room, { config: { iceServers: ice }, ice })]);
    await connected(pages);
    for (const p of pages) {
      assert.equal(await p.evaluate(async () => {
        for (const pc of window.__rtc) {
          const stats = await pc.getStats();
          for (const row of stats.values()) {
            if (row.type === 'candidate-pair' && row.state === 'succeeded' && stats.get(row.localCandidateId)?.candidateType === 'relay') return true;
          }
        }
        return false;
      }), true, 'connection must actually use TURN');
    }
    await pages[0].evaluate(() => __cl.joinAs('Ana'));
    await pages[1].evaluate(() => __cl.joinAs('Ben'));
    await Promise.all(pages.map((p) => p.waitForFunction(() => __cl.ui().dealable.length === 2)));
    for (const p of pages) {
      const report = await p.evaluate(() => __cl.connectionReport());
      assert.equal(report.current.settings.turnServers, 1);
      assert(report.current.attempts.some((a) => a.state === 'connected' && a.path?.kind === 'turn'));
      for (const secret of [credential, '127.0.0.1', 'Ana', 'Ben', room]) assert(!JSON.stringify(report).includes(secret), `report leaked ${secret}`);
    }
    console.log('PASS game state syncs over TURN/TCP with direct connections disabled');
    console.log('PASS diagnostics verify TURN is actually in use without exposing credentials or ICE addresses');
    await close(pages);
  } else console.log('SKIP TURN transport (set TURN_BIN to a coturn executable)');
  assert.deepEqual(errors, []);
} finally {
  await browser.close();
  await server.close();
  if (turn && turn.exitCode === null) {
    const exited = new Promise((resolve) => turn.once('exit', resolve));
    turn.kill();
    await exited;
  }
  await turnLog?.close();
}
