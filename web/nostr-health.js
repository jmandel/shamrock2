// A suspended phone can retain an OPEN WebSocket after its network path has
// gone away. Trystero retries closed sockets; verify idle ones so it gets that
// opportunity. This touches discovery only, never a live peer or its ICE work.
const IDLE_MS = 30000, REPLY_MS = 15000;

export function watchNostrRelays(getSockets, event = () => {}) {
  const watched = new Map();
  let timer = null, lastPoll = performance.now();
  const closeProbe = (row) => {
    if (!row.pending) return;
    if (row.socket.readyState === WebSocket.OPEN) row.socket.send(JSON.stringify(['CLOSE', row.id]));
    row.pending = null;
  };
  const probe = (row, now) => {
    if (row.pending || row.socket.readyState !== WebSocket.OPEN) return;
    row.id = `shamrock-health-${crypto.randomUUID()}`;
    row.pending = { at: now };
    // A valid, empty lookup gets EOSE without publishing anything, knowing the
    // room, or relying on the relay to send unsolicited WebSocket pings.
    row.socket.send(JSON.stringify(['REQ', row.id, { ids: ['0'.repeat(64)], limit: 1 }]));
  };
  const poll = () => {
    const now = performance.now(), sockets = Object.values(getSockets());
    // Sleep or a suspended event loop may happen without a visibility event.
    if (now - lastPoll > 5000) for (const row of watched.values()) if (row.pending) row.pending.at = now;
    lastPoll = now;
    for (const [socket, row] of watched) if (!sockets.includes(socket)) {
      socket.removeEventListener('message', row.receive);
      watched.delete(socket);
    }
    sockets.forEach((socket, index) => {
      let row = watched.get(socket);
      if (!row) {
        row = { socket, id: null, pending: null, lastMessage: -Infinity };
        row.receive = ({ data }) => {
          row.lastMessage = performance.now();
          if (!row.pending || typeof data !== 'string') return;
          try {
            const message = JSON.parse(data);
            if ((message[0] === 'EOSE' || message[0] === 'CLOSED') && message[1] === row.id) closeProbe(row);
          } catch { /* Trystero handles the relay's other messages. */ }
        };
        socket.addEventListener('message', row.receive);
        watched.set(socket, row);
      }
      if (socket.readyState !== WebSocket.OPEN) { row.pending = null; return; }
      if (row.pending && now - row.pending.at >= REPLY_MS) {
        row.pending = null;
        event('relay-health-timeout', { relay: index + 1 });
        socket.close(); // Its replacement resubscribes and announces the room.
      } else if (!row.pending && now - row.lastMessage >= IDLE_MS) probe(row, now);
    });
  };
  return {
    check() {
      if (timer === null) return;
      poll();
      for (const row of watched.values()) probe(row, performance.now());
    },
    resume() {
      if (timer !== null) return;
      // Give each socket a fresh reply window after sleep or a network change.
      for (const row of watched.values()) { closeProbe(row); row.lastMessage = -Infinity; }
      poll();
      timer = setInterval(poll, 1000);
    },
    pause() {
      clearInterval(timer); timer = null;
      for (const row of watched.values()) closeProbe(row);
    },
  };
}
