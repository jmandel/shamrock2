// Regression cases from reports where sockets opened but discovery was rejected.
import assert from 'node:assert/strict';
import { rejectionCategory, connectionView } from '../web/diagnostics.js';
import { loadIceServers, discoveryTime, refreshDiscoveryClock } from '../web/rtc-config.js';

for (const [text, category] of [
  ['invalid: event is too old', 'clock'],
  ['invalid: event creation date is too far off from the current time', 'clock'],
  ['invalid: event timestamp is in the future', 'clock'],
  ['invalid: signature verification failed', 'signature'],
  ['rate-limited: slow down', 'rate-limit'],
  ['Policy violated and pubkey is not in our web of trust.', 'restricted'],
  ['duplicate: already have this event', 'duplicate'],
  ['invalid: malformed event', 'invalid-event'],
  ['arbitrary text with a player name and address', 'other'],
]) assert.equal(rejectionCategory(text), category);

const originalFetch = globalThis.fetch, originalNow = Date.now;
const timestamp = originalNow();
let report;
try {
  Date.now = () => originalNow() + 180000;
  globalThis.fetch = async () => new Response('{"iceServers":[]}', { headers: { Date: new Date(timestamp).toUTCString() } });
  await loadIceServers((value) => { report = value; });
  assert(Math.abs(report.clockOffsetMs + 180000) < 2000);
  assert(report.clockUncertaintyMs >= 1000);
  assert(Math.abs(discoveryTime() - originalNow()) < 2000);
  // Correcting the device clock during play must not double-correct discovery.
  Date.now = originalNow;
  assert(Math.abs(discoveryTime() - originalNow()) < 2000);

  // Cached HTTP responses are not evidence of a wrong device clock.
  Date.now = originalNow;
  globalThis.fetch = async () => new Response('{"iceServers":[]}', { headers: { Date: new Date(timestamp - 90000).toUTCString(), Age: '90' } });
  await loadIceServers((value) => { report = value; });
  assert(Math.abs(report.clockOffsetMs) < 2000);

  globalThis.fetch = async () => new Response('{"iceServers":[]}');
  await loadIceServers((value) => { report = value; });
  assert.equal(report.clockOffsetMs, null);
  assert.equal(discoveryTime(), Date.now());

  let count = 0;
  globalThis.fetch = async () => ++count === 1
    ? new Response('{"iceServersUrl":"./credentials"}', { headers: { Date: new Date(timestamp).toUTCString() } })
    : new Response('{"iceServers":[{"urls":"turn:private-host.example","username":"private-user","credential":"private-secret"}]}', { headers: { Date: new Date(timestamp + 500000).toUTCString() } });
  await loadIceServers((value) => { report = value; });
  assert(Math.abs(report.clockOffsetMs) < 2000, 'only the site clock is used');
  assert.equal(report.turnServers, 1);
  assert(!JSON.stringify(report).includes('private-'));
  globalThis.fetch = async (_url, options) => {
    assert.equal(options.method, 'HEAD');
    return new Response(null, { headers: { Date: new Date(originalNow()).toUTCString() } });
  };
  Date.now = () => originalNow() - 90000;
  const refreshed = await refreshDiscoveryClock();
  assert(Math.abs(refreshed.clockOffsetMs - 90000) < 2000);
  assert(Math.abs(discoveryTime() - originalNow()) < 2000);
} finally { globalThis.fetch = originalFetch; Date.now = originalNow; }

const sample = {
  mode: 'nostr', online: true, elapsedMs: 30000, discovered: 0, settings: { state: 'loaded', turnServers: 0 },
  attempts: [], app: { connectedPeers: 0, lastReceivedAgoMs: null },
  relays: [{ state: 'open', accepted: 0, rejections: 5, reasons: { clock: 5 } }],
};
assert.equal(connectionView(sample).headline, 'Discovery messages are being rejected');
sample.relays[0].accepted = 2;
assert.equal(connectionView(sample).headline, 'No other device found yet');
sample.settings.clockOffsetMs = 180000;
sample.settings.clockUncertaintyMs = 1001;
assert.equal(connectionView(sample).headline, 'The device clock may be preventing discovery');
assert(connectionView(sample).rows.some(([label, value]) => label === 'Device clock' && value.includes('behind')));
sample.settings.discoveryClock = 'server';
assert.equal(connectionView(sample).headline, 'No other device found yet');
assert(connectionView(sample).rows.some(([label, value]) => label === 'Device clock' && value.includes('discovery corrected')));
sample.app.connectedPeers = 1;
sample.attempts = [{ state: 'connected', candidates: { relay: 3 }, path: { kind: 'direct' } }];
sample.settings.turnServers = 1;
assert.equal(connectionView(sample).headline, 'Connected to 1 other device');
assert(connectionView(sample).rows.some(([label, value]) => label === 'TURN fallback' && value === 'Configured · not in use'));
console.log('PASS rejection reasons, server clock comparison, cached responses, privacy and selected-route wording');
