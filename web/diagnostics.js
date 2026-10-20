// Browser transport diagnostics only. Never retain SDP, ICE addresses,
// credentials, room links, player names, clues, or packet contents.
const VERSION = '2026-10-19-connections-2';
const STORE = 'shamrock:connection-reports:v1';
const MAX_EVENTS = 160, MAX_ATTEMPTS = 48;
const types = new Set(['host', 'srflx', 'prflx', 'relay']);
const candidateType = (value) => types.has(value) ? value : 'unknown';
const errorName = (error) => ['AbortError', 'TimeoutError', 'TypeError', 'OperationError', 'InvalidStateError', 'NotSupportedError'].includes(error?.name) ? error.name : 'Error';
export function rejectionCategory(reason) {
  if (/duplicate|already (have|stored|exists)/i.test(reason)) return 'duplicate';
  if (/timestamp|created[_ -]?at|creation (date|time)|clock|future|too (old|new|early|late)|too far|expired|time.{0,16}(skew|range|window)/i.test(reason)) return 'clock';
  if (/signature|schnorr|sig check/i.test(reason)) return 'signature';
  if (/rate|limit|too many/i.test(reason)) return 'rate-limit';
  if (/auth|restricted|blocked|whitelist|trust|proof.of.work|^pow:/i.test(reason)) return 'restricted';
  if (/invalid|malformed|bad event|unsupported|not supported/i.test(reason)) return 'invalid-event';
  return 'other';
}
const hash = async (text) => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text))), (b) => b.toString(16).padStart(2, '0')).join('').slice(0, 16);
const endpoint = (url) => {
  try {
    const parsed = new URL(url);
    return /^[\d.]+$/.test(parsed.hostname) || parsed.hostname.includes(':') ? `${parsed.protocol}//[address]${parsed.port ? `:${parsed.port}` : ''}` : parsed.origin;
  } catch { return 'invalid-url'; }
};

export function createConnectionDiagnostics({ room, mode, protocol, sample }) {
  const started = Date.now(), startedTick = performance.now(), session = crypto.randomUUID();
  const events = [], attempts = new Map(), sockets = new Map(), observed = new WeakSet();
  let previous = [];
  try { previous = JSON.parse(sessionStorage.getItem(STORE) || '[]').slice(0, 2); } catch {}
  let roomTag = 'pending', stateTag = null, stateValue = null, nextAttempt = 0;
  let socketSource = null, saveTimer = null, polling = false;
  let lastReceived = null, received = 0, discovered = 0;
  let settings = { state: 'loading', turnServers: 0 }, fatal = null, mismatch = false;
  let lastSample = {};
  const elapsed = () => Math.round(performance.now() - startedTick);
  const event = (kind, data = {}) => {
    events.push({ ms: elapsed(), kind, ...data });
    if (events.length > MAX_EVENTS) events.splice(0, events.length - MAX_EVENTS);
    if (!saveTimer) saveTimer = setTimeout(() => { saveTimer = null; persist(); }, 2000);
  };
  const readSample = () => {
    try {
      const { state, ...safe } = sample();
      lastSample = safe;
      if (state !== stateValue) {
        stateValue = state;
        hash(state).then((tag) => { if (state === stateValue) stateTag = tag; }).catch(() => {});
      }
      if (safe.receivedPackets > received) { received = safe.receivedPackets; lastReceived = elapsed(); }
    } catch { /* The core may still be starting. */ }
    return lastSample;
  };
  const current = () => {
    const app = readSample();
    return {
      schema: 1, client: VERSION, protocol, session, startedAt: new Date(started).toISOString(),
      capturedAt: new Date().toISOString(), elapsedMs: elapsed(), roomTag, mode,
      browser: navigator.userAgent, online: navigator.onLine, visibility: document.visibilityState,
      settings, fatal, protocolMismatch: mismatch, discovered,
      app: { ...app, stateTag, lastReceivedAgoMs: lastReceived === null ? null : elapsed() - lastReceived },
      relays: [...sockets.values()].map(({ socket, ...row }) => ({ ...row, state: ['connecting', 'open', 'closing', 'closed'][socket.readyState] })),
      attempts: [...attempts.values()].map(({ pc, ...row }) => row), events: events.slice(),
    };
  };
  const persist = () => {
    try { sessionStorage.setItem(STORE, JSON.stringify([current(), ...previous])); } catch {}
  };
  hash(`shamrock-p2p:${room}`).then((tag) => { roomTag = tag; persist(); }).catch(() => { roomTag = 'unavailable'; });

  async function inspect(row) {
    try {
      const stats = await row.pc.getStats();
      let selected;
      for (const s of stats.values()) if (s.type === 'transport' && s.selectedCandidatePairId) selected = stats.get(s.selectedCandidatePairId);
      if (!selected) for (const s of stats.values()) if (s.type === 'candidate-pair' && s.nominated && s.state === 'succeeded') selected = s;
      const pairs = [];
      for (const s of stats.values()) if (s.type === 'candidate-pair') pairs.push({
        state: s.state, nominated: !!s.nominated,
        local: candidateType(stats.get(s.localCandidateId)?.candidateType),
        remote: candidateType(stats.get(s.remoteCandidateId)?.candidateType),
        requestsSent: s.requestsSent ?? 0, responsesReceived: s.responsesReceived ?? 0,
      });
      row.pairs = pairs.slice(0, 12);
      if (selected) {
        const local = stats.get(selected.localCandidateId), remote = stats.get(selected.remoteCandidateId);
        const path = {
          kind: local?.candidateType === 'relay' || remote?.candidateType === 'relay' ? 'turn' : 'direct',
          local: candidateType(local?.candidateType), remote: candidateType(remote?.candidateType),
          protocol: ['udp', 'tcp'].includes(local?.protocol) ? local.protocol : 'unknown',
          relayProtocol: ['udp', 'tcp', 'tls'].includes(local?.relayProtocol) ? local.relayProtocol : null,
        };
        if (JSON.stringify(path) !== JSON.stringify(row.path)) event('selected-path', { attempt: row.id, ...path });
        row.path = path;
        row.bytesSent = selected.bytesSent ?? 0;
        row.bytesReceived = selected.bytesReceived ?? 0;
        row.rttMs = Number.isFinite(selected.currentRoundTripTime) ? Math.round(selected.currentRoundTripTime * 1000) : null;
      }
      row.statsAtMs = elapsed();
    } catch { /* Stats can disappear when a browser closes a connection. */ }
  }
  // Use Trystero's supported rtcPolyfill hook; leave browser globals alone.
  const Native = globalThis.RTCPeerConnection;
  const PeerConnection = Native && class extends Native {
    constructor(config) {
      super(config);
      const row = { id: ++nextAttempt, pc: this, createdMs: elapsed(), state: this.connectionState, ice: this.iceConnectionState, gathering: this.iceGatheringState, remoteSeen: false, candidates: {}, candidateErrors: [], pairs: [], path: null };
      attempts.set(row.id, row);
      this._diagnosticRow = row;
      // Trystero preallocates offers. They are not evidence of another player.
      for (const [name, field, property] of [['connectionstatechange', 'state', 'connectionState'], ['iceconnectionstatechange', 'ice', 'iceConnectionState'], ['icegatheringstatechange', 'gathering', 'iceGatheringState']]) {
        this.addEventListener(name, () => {
          row[field] = this[property];
          if (row.remoteSeen) event(name, { attempt: row.id, value: row[field] });
          if (field !== 'gathering' && row.remoteSeen) void inspect(row);
        });
      }
      this.addEventListener('icecandidate', ({ candidate }) => {
        if (candidate) { const type = candidateType(candidate.type); row.candidates[type] = (row.candidates[type] || 0) + 1; }
      });
      this.addEventListener('icecandidateerror', ({ errorCode }) => {
        if (Number.isInteger(errorCode) && !row.candidateErrors.includes(errorCode)) row.candidateErrors.push(errorCode);
      });
      while (attempts.size > MAX_ATTEMPTS) {
        const old = [...attempts.values()].find((a) => a.state === 'closed') ?? [...attempts.values()].find((a) => !a.remoteSeen);
        if (!old) break;
        attempts.delete(old.id);
      }
    }
    async setRemoteDescription(description) {
      const row = this._diagnosticRow;
      if (!row.remoteSeen) { row.remoteSeen = true; discovered++; event('peer-discovered', { attempt: row.id }); }
      try { return await super.setRemoteDescription(description); }
      catch (error) { event('remote-description-error', { attempt: row.id, error: errorName(error) }); throw error; }
    }
    close() {
      const row = this._diagnosticRow;
      if (row.remoteSeen) void inspect(row);
      super.close();
      row.state = 'closed'; row.ice = 'closed';
    }
  };

  const observeSockets = () => {
    if (!socketSource) return;
    for (const [url, socket] of Object.entries(socketSource())) {
      if (observed.has(socket)) continue;
      observed.add(socket);
      const key = endpoint(url), old = sockets.get(key);
      const row = { endpoint: key, socket, opens: old?.opens ?? 0, reconnects: old ? old.reconnects + 1 : 0,
        messages: old?.messages ?? 0, accepted: old?.accepted ?? 0, receivedEvents: old?.receivedEvents ?? 0,
        subscriptionsReady: old?.subscriptionsReady ?? 0, notices: old?.notices ?? 0,
        rejections: old?.rejections ?? 0, reasons: old?.reasons ?? {}, closeCode: null };
      sockets.set(key, row);
      event('relay-socket', { endpoint: key, state: socket.readyState });
      if (socket.readyState === 1) row.opens++;
      socket.addEventListener('open', () => { row.opens++; event('relay-open', { endpoint: key }); });
      socket.addEventListener('error', () => event('relay-error', { endpoint: key }));
      socket.addEventListener('close', ({ code }) => { row.closeCode = code; event('relay-close', { endpoint: key, code }); });
      socket.addEventListener('message', ({ data }) => {
        row.messages++;
        if (mode !== 'nostr' || typeof data !== 'string' || data.length > 65536) return;
        try {
          const msg = JSON.parse(data);
          if (!Array.isArray(msg)) return;
          if (msg[0] === 'OK' && msg[2] === true) row.accepted++;
          if (msg[0] === 'EVENT') row.receivedEvents++;
          if (msg[0] === 'EOSE') row.subscriptionsReady++;
          // Store categories, never the server's arbitrary text or event IDs.
          const rejected = msg[0] === 'OK' && msg[2] === false;
          if (msg[0] === 'NOTICE' || rejected || msg[0] === 'CLOSED') {
            row.notices++;
            if (rejected || msg[0] === 'CLOSED') row.rejections++;
            const reason = String(msg.at(-1));
            const category = rejectionCategory(reason);
            row.reasons[category] = (row.reasons[category] || 0) + 1;
            event('relay-notice', { endpoint: key, category, rejected, type: msg[0] });
          }
        } catch {}
      });
    }
  };
  const poll = async () => {
    if (polling) return;
    polling = true;
    try {
      observeSockets();
      await Promise.all([...attempts.values()].filter((a) => a.remoteSeen && a.state !== 'closed').map(inspect));
      persist();
    } finally { polling = false; }
  };
  setInterval(() => { try { observeSockets(); } catch {} }, 1000);
  setInterval(() => { void poll().catch(() => {}); }, 5000);
  for (const kind of ['online', 'offline', 'pageshow', 'pagehide']) addEventListener(kind, () => { event(kind); persist(); });
  document.addEventListener('visibilitychange', () => { event('visibility', { value: document.visibilityState }); persist(); });
  event('start', { mode });

  return {
    PeerConnection, event,
    trackRelays(source) { socketSource = source; observeSockets(); },
    settings(value) { settings = value; event('ice-settings', value); },
    clock(value) { settings = { ...settings, ...value }; event('discovery-clock', value); },
    failed(error) { fatal = errorName(error); event('startup-failed', { error: fatal }); },
    joinError(error) { event('join-error', { category: /after exchanging SDP/.test(error) ? 'route-failed' : /password/.test(error) ? 'password' : 'other' }); },
    protocolMismatch() { mismatch = true; event('protocol-mismatch'); },
    async report() { await poll(); return { current: current(), previous }; },
    snapshot: current,
    clearPrevious() { previous = []; persist(); },
  };
}

export function connectionView(report) {
  const connected = report.app.connectedPeers || 0;
  const active = report.attempts.filter((a) => a.state === 'connected');
  const turn = active.filter((a) => a.path?.kind === 'turn').length;
  const direct = active.filter((a) => a.path?.kind === 'direct').length;
  const open = report.relays.filter((r) => r.state === 'open').length;
  const accepted = report.relays.reduce((n, r) => n + (r.accepted || 0), 0);
  const rejected = report.relays.reduce((n, r) => n + Math.max(0, r.rejections - (r.reasons?.duplicate || 0)), 0);
  const offset = report.settings.clockOffsetMs;
  const clockWrong = Number.isFinite(offset) && Math.abs(offset) > Math.max(30000, report.settings.clockUncertaintyMs * 2);
  let headline, advice;
  if (report.mode === 'inactive') { headline = 'This page is not a multiplayer table'; advice = 'Open a table link to connect with friends.'; }
  else if (!report.online) { headline = 'This device is offline'; advice = 'Reconnect to the internet, then keep this table open.'; }
  else if (report.fatal) { headline = 'Connection setup failed'; advice = 'Copy this report, then reload the page.'; }
  else if (report.protocolMismatch) { headline = 'A player has a newer game version'; advice = 'Reload this page to use the same version.'; }
  else if (connected) { headline = `Connected to ${connected} other ${connected === 1 ? 'device' : 'devices'}`; advice = report.app.receivedPackets ? 'Game data has arrived. Compare the room code and reports if a player is still missing.' : 'Connected, but no game data has arrived yet. Keep this screen open for a few seconds.'; }
  else if (report.mode === 'local') { headline = 'Waiting for another tab'; advice = 'This test mode only connects tabs in the same browser.'; }
  else if (clockWrong && report.settings.discoveryClock !== 'server') { headline = 'The device clock may be preventing discovery'; advice = 'Set this device’s date and time automatically, then reload. Discovery messages are timestamped.'; }
  else if (!open && report.elapsedMs > 10000) { headline = 'Cannot reach room discovery'; advice = 'Keep this screen open on both devices, then copy a report from each.'; }
  else if (report.discovered) { headline = 'Found another device; no game connection'; advice = 'The connection may still be negotiating, or the network may be blocking it. Copy a report from both devices.'; }
  else if (rejected && !accepted) { headline = 'Discovery messages are being rejected'; advice = 'The relay sockets opened, but no outgoing discovery messages have been accepted. Copy this report from both devices.'; }
  else { headline = 'No other device found yet'; advice = 'Open the exact same invite link on each device and keep both pages visible. Compare the room code below.'; }
  return { headline, advice, rows: [
    ['Room code', report.roomTag],
    ['Discovery', report.mode === 'local' ? 'Same-browser tabs' : `${open} of ${report.relays.length} relay sockets open`],
    ...(report.mode === 'nostr' ? [['Discovery messages', `${accepted} accepted · ${Math.max(0, rejected)} rejected`]] : []),
    ['Device clock', !Number.isFinite(offset) ? 'Server comparison unavailable' : clockWrong ? `${Math.round(Math.abs(offset) / 1000)} seconds ${offset < 0 ? 'ahead of' : 'behind'} server${report.settings.discoveryClock === 'server' ? ' · discovery corrected' : ''}` : 'Close to server time'],
    ['Connection route', connected ? `${direct} direct · ${turn} TURN${connected > direct + turn ? ` · ${connected - direct - turn} checking` : ''}` : 'No active connection'],
    ['TURN fallback', report.settings.state === 'failed' ? 'Settings failed to load' : report.settings.state === 'loading' ? 'Loading settings…' : report.settings.turnServers ? `Configured${turn ? ' · in use' : ' · not in use'}` : 'Not configured'],
    ['Game packets', `${report.app.sentPackets || 0} sent · ${report.app.receivedPackets || 0} received`],
    ['Last game data', report.app.lastReceivedAgoMs === null ? 'None received' : `${Math.floor(report.app.lastReceivedAgoMs / 1000)} seconds ago`],
    ['Game state code', report.app.stateTag || 'Checking…'],
    ['Version', `${report.client} · protocol ${report.protocol}`],
  ] };
}
