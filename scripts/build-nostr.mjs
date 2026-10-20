// Trystero 0.26.0 timestamps both announcements and subscription filters with
// the device clock. A slow clock makes ephemeral events expire at public relays.
// Keep this small, checked patch reproducible; never change Date globally.
import { readFile, realpath, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';

const source = await realpath('node_modules/@trystero-p2p/nostr/dist/index.mjs');
const code = await readFile(source, 'utf8');
const needle = 'const now = () => Math.floor(Date.now() / 1e3);';
if (code.split(needle).length !== 2) throw new Error('Review the Nostr clock patch for this Trystero version');
const patched = code.replace(needle, `let discoveryClock = Date.now;
export const setDiscoveryClock = (clock) => { discoveryClock = clock; };
const now = () => Math.floor(discoveryClock() / 1e3);`);
const temp = await mkdtemp(join(dirname(source), '.shamrock-build-'));
try {
  const entry = join(temp, 'index.mjs');
  await writeFile(entry, patched);
  execFileSync('bun', ['build', entry, '--target=browser', '--format=esm', '--minify', '--outfile', resolve('web/vendor/trystero-nostr.js')], { stdio: 'inherit' });
} finally { await rm(temp, { recursive: true, force: true }); }
