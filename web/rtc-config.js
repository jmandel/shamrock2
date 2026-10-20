// Deployment settings, separate from room links and saved games. A credential
// endpoint can mint browser-scoped TURN credentials without exposing its API key.
let serverClock = null;
export const discoveryTime = () => serverClock ? serverClock.ms + performance.now() - serverClock.tick : Date.now();

function sampleClock(response, start, tick) {
  const duration = performance.now() - tick;
  const date = Date.parse(response.headers.get('Date'));
  const age = Number(response.headers.get('Age') || 0);
  // HTTP Date has one-second precision; Age accounts for a cached reply.
  if (!Number.isFinite(date) || !Number.isFinite(age) || age < 0 || Math.abs(Date.now() - start - duration) >= 1000) return null;
  serverClock = { ms: date + age * 1000 + 500, tick: tick + duration / 2 };
  return { clockOffsetMs: Math.round((serverClock.ms - start - duration / 2) / 1000) * 1000, clockUncertaintyMs: Math.ceil(duration / 2) + 1000, discoveryClock: 'server' };
}

export async function refreshDiscoveryClock() {
  const url = new URL('./network.json', import.meta.url);
  url.searchParams.set('clock', crypto.randomUUID());
  const start = Date.now(), tick = performance.now();
  const response = await fetch(url, { method: 'HEAD', cache: 'no-store', signal: AbortSignal.timeout(5000) });
  const clock = response.ok && sampleClock(response, start, tick);
  if (!clock) throw new Error('Server clock unavailable');
  return clock;
}

export async function loadIceServers(report = () => {}) {
  serverClock = null;
  let stage = 'configuration';
  let clock = { clockOffsetMs: null, clockUncertaintyMs: null, discoveryClock: 'device' };
  const get = async (url) => {
    const start = Date.now(), tick = performance.now();
    const response = await fetch(url, { cache: 'no-store', signal: AbortSignal.timeout(5000) });
    // Compare only our own site's response, never a credential provider.
    if (stage === 'configuration') clock = sampleClock(response, start, tick) ?? clock;
    if (!response.ok) {
      const error = new Error('Could not load connection relay settings');
      error.status = response.status;
      throw error;
    }
    return response.json();
  };
  try {
    const config = await get(new URL('./network.json', import.meta.url));
    if (config.iceServersUrl) stage = 'credentials';
    const data = config.iceServersUrl ? await get(new URL(config.iceServersUrl, import.meta.url)) : config;
    const servers = Array.isArray(data) ? data : data.iceServers;
    if (!Array.isArray(servers) || servers.some((server) => {
      const urls = typeof server?.urls === 'string' ? [server.urls] : server?.urls;
      return !Array.isArray(urls) || !urls.length || urls.some((url) => typeof url !== 'string' || !/^stuns?:|^turns?:/.test(url))
        || (urls.some((url) => /^turns?:/.test(url)) && (typeof server.username !== 'string' || typeof server.credential !== 'string'));
    })) throw new Error('Invalid connection relay settings');
    report({ state: 'loaded', source: stage, turnServers: servers.filter((s) => [s.urls].flat().some((url) => /^turns?:/.test(url))).length, ...clock });
    return servers;
  } catch (error) {
    report({ state: 'failed', source: stage, status: error.status ?? null, turnServers: 0, ...clock });
    throw error;
  }
}
