// Deployment settings, separate from room links and saved games. A credential
// endpoint can mint browser-scoped TURN credentials without exposing its API key.
export async function loadIceServers(report = () => {}) {
  let stage = 'configuration';
  const get = async (url) => {
    const response = await fetch(url, { cache: 'no-store', signal: AbortSignal.timeout(5000) });
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
    report({ state: 'loaded', source: stage, turnServers: servers.filter((s) => [s.urls].flat().some((url) => /^turns?:/.test(url))).length });
    return servers;
  } catch (error) {
    report({ state: 'failed', source: stage, status: error.status ?? null, turnServers: 0 });
    throw error;
  }
}
