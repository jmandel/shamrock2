// A small Nostr relay for browser tests. Its silent-socket fault keeps the TCP
// connection open while discarding messages, as a stale mobile path can do.
import { WebSocketServer } from 'ws';

export async function serveNostr({ port, maxClockSkewMs = 5000 } = {}) {
  const server = new WebSocketServer({ port, host: '127.0.0.1' });
  await new Promise((resolve) => server.once('listening', resolve));
  const clients = new Map();
  const counters = { accepted: 0, expired: 0, probes: 0 };
  const matches = (event, f) => (!f.since || event.created_at >= f.since)
    && (!f.ids || f.ids.includes(event.id))
    && (!f.kinds || f.kinds.includes(event.kind))
    && (!f['#x'] || event.tags.some(([key, tag]) => key === 'x' && f['#x'].includes(tag)));
  server.on('connection', (socket) => {
    const row = { subscriptions: new Map(), silent: false, probeDelayMs: 0 };
    clients.set(socket, row);
    socket.on('close', () => clients.delete(socket));
    socket.on('message', (bytes) => {
      if (row.silent) return;
      const [type, value, ...filters] = JSON.parse(bytes);
      if (type === 'REQ') {
        row.subscriptions.set(value, filters);
        const probe = filters.some((f) => f.ids?.includes('0'.repeat(64)));
        if (probe) counters.probes++;
        const reply = () => { if (socket.readyState === 1 && !row.silent) socket.send(JSON.stringify(['EOSE', value])); };
        if (probe && row.probeDelayMs) setTimeout(reply, row.probeDelayMs).unref();
        else reply();
      } else if (type === 'CLOSE') row.subscriptions.delete(value);
      else if (type === 'EVENT') {
        const valid = Math.abs(value.created_at * 1000 - Date.now()) < maxClockSkewMs;
        socket.send(JSON.stringify(['OK', value.id, valid, valid ? '' : 'invalid: ephemeral event expired']));
        if (!valid) { counters.expired++; return; }
        counters.accepted++;
        for (const [recipient, peer] of clients) if (!peer.silent) {
          for (const [id, filters] of peer.subscriptions) if (filters.some((f) => matches(value, f))) {
            recipient.send(JSON.stringify(['EVENT', id, value]));
          }
        }
      }
    });
  });
  return {
    clients, counters,
    async close() {
      for (const socket of server.clients) socket.terminate();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
