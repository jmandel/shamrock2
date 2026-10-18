// Platform shim: everything here is plumbing. Game rules, sync, layout,
// hit-testing, animation and drawing decisions live in core.wasm (Rust).

import { agentTools, offerTools } from './agent.js';

const params = new URLSearchParams(location.search);
// No room in the link (and no shared boards): the home page, listing this
// device's tables and past games, with a button to start a new table.
const home = !location.hash.slice(1) && !params.has('b') && !params.has('i');
const roomId = decodeURIComponent(location.hash.slice(1));
// Fault tests (?chaos): `cut` silences this page both ways while its
// connections stay up (a phone in a tunnel); `delayMs` lags what arrives.
const chaos = params.has('chaos') ? { cut: false, delayMs: 0 } : null;

const $ = (id) => document.getElementById(id);
const canvas = $('c'), ctx = canvas.getContext('2d'), clueInput = $('clue');
const enc = new TextEncoder(), dec = new TextDecoder();
const remember = (key, value) => { try { value == null ? localStorage.removeItem(key) : localStorage.setItem(key, value); return true; } catch { return false; } };
const recall = (key) => { try { return localStorage.getItem(key); } catch { return null; } };

const { instance } = await WebAssembly.instantiateStreaming(fetch('core.wasm'), {});
// Frames run only while something is changing (see the frame loop below):
// every call into the core wakes them, except drawing and the questions that
// change nothing. (One missed here costs at most a 250 ms wait, not a stale screen.)
const QUIET = new Set([
  'memory', 'frame', 'unchanged', 'cmd_ptr', 'str_ptr', 'outbox_ptr', 'outbox_len', 'outbox_clear', 'scratch_ptr', 'inbox_reserve', 'probe_ptr',
  'ui', 'sheet', 'digest', 'holds', 'agent_view', 'snapshot', 'origin_snapshot', 'palette', 'protocol', 'on_card', 'card_zoom', 'view_quarter',
  'word_size', 'word_list', 'word_fit', 'card_word_scales', 'answer_rot', 'review', 'export_boards', 'scramble_boards', 'past_games', 'tables',
  'norm_room', 'suggest_room', 'probe_card', 'probe_slot', 'probe_board', 'probe_petal', 'probe_hub', 'probe_shaking', 'probe_shown_angle',
  'probe_word_px', 'probe_card_px',
]);
let framePending = false, framesStarted = false;
const X = Object.fromEntries(Object.entries(instance.exports).map(([k, v]) =>
  [k, typeof v !== 'function' || QUIET.has(k) ? v : (...a) => { const r = v(...a); wake(); return r; }]));
const seed = crypto.getRandomValues(new Uint32Array(2));
const device = X.init(seed[0], seed[1]) >>> 0; // u32 comes back as i32
const reduceMotion = matchMedia('(prefers-reduced-motion: reduce)');
X.set_motion(reduceMotion.matches ? 0 : 1);
reduceMotion.addEventListener?.('change', (e) => X.set_motion(e.matches ? 0 : 1));

const bytes = (ptr, len) => new Uint8Array(X.memory.buffer, ptr, len);
const writeInbox = (u8) => { bytes(X.inbox_reserve(u8.length), u8.length).set(u8); return u8.length; };
const writeText = (s) => writeInbox(enc.encode(s));
const scratchJson = (len) => JSON.parse(dec.decode(bytes(X.scratch_ptr(), len)));

// ---- canvas: replay the command buffer ------------------------------------

const SANS = 'Figtree, system-ui, -apple-system, sans-serif';
const FONTS = [
  (px) => `400 ${px}px ${SANS}`,
  (px) => `700 ${px}px ${SANS}`,
  (px) => `600 ${px}px Fraunces, Georgia, serif`,
];
// Fonts ship with the site (web/fonts). Repaint happens every frame, so
// canvas text switches over as soon as they're ready.
document.fonts?.load(`700 20px Figtree`).catch(() => {});
document.fonts?.load(`600 20px Fraunces`).catch(() => {});

const colors = new Map();
const css = (c) => {
  let s = colors.get(c);
  if (!s) colors.set(c, (s = `rgba(${c >>> 24},${(c >>> 16) & 255},${(c >>> 8) & 255},${(c & 255) / 255})`));
  return s;
};

function paint(n) {
  const f = new Float32Array(X.memory.buffer, X.cmd_ptr(), n);
  const u = new Uint32Array(X.memory.buffer, X.cmd_ptr(), n);
  const strs = X.str_ptr();
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  for (let i = 0; i < n; ) {
    switch (f[i]) {
      case 1: // Rust's transform, under this device's zoom camera.
        ctx.setTransform(cam.s, 0, 0, cam.s, cam.x * devicePixelRatio, cam.y * devicePixelRatio);
        ctx.transform(f[i + 1], f[i + 2], f[i + 3], f[i + 4], f[i + 5], f[i + 6]); i += 7; break;
      case 2: { // rrect x y w h r fill stroke lw
        ctx.beginPath(); ctx.roundRect(f[i + 1], f[i + 2], f[i + 3], f[i + 4], f[i + 5]);
        if (u[i + 6] & 255) { ctx.fillStyle = css(u[i + 6]); ctx.fill(); }
        if (u[i + 7] & 255) { ctx.strokeStyle = css(u[i + 7]); ctx.lineWidth = f[i + 8]; ctx.stroke(); }
        i += 9; break;
      }
      case 3: { // circle x y r fill stroke lw
        ctx.beginPath(); ctx.arc(f[i + 1], f[i + 2], f[i + 3], 0, Math.PI * 2);
        if (u[i + 4] & 255) { ctx.fillStyle = css(u[i + 4]); ctx.fill(); }
        if (u[i + 5] & 255) { ctx.strokeStyle = css(u[i + 5]); ctx.lineWidth = f[i + 6]; ctx.stroke(); }
        i += 7; break;
      }
      case 4: { // text x y size color face off len maxw
        // Too wide for its room: shrink the type evenly (not the canvas's
        // sideways squeeze), down to 60%; squeeze only past that. A clue of
        // several words that would shrink a lot goes onto two lines instead.
        const text = dec.decode(bytes(strs + f[i + 6], f[i + 7]));
        const face = f[i + 5], full = f[i + 3], maxw = f[i + 8];
        ctx.font = FONTS[face](full);
        let lines = [text], wide = ctx.measureText(text).width;
        if (face === 2 && wide > maxw / 0.8 && text.includes(' ')) {
          const cut = [...text.matchAll(/ /g)].map((m) => m.index).reduce((a, b) => (Math.abs(b - text.length / 2) < Math.abs(a - text.length / 2) ? b : a));
          lines = [text.slice(0, cut), text.slice(cut + 1)];
          wide = Math.max(...lines.map((l) => ctx.measureText(l).width));
        }
        const size = wide > maxw ? Math.max(full * 0.6, full * maxw / wide) : full;
        ctx.font = FONTS[face](size);
        ctx.fillStyle = css(u[i + 4]);
        lines.forEach((l, k) => ctx.fillText(l, f[i + 1], f[i + 2] + (k - (lines.length - 1) / 2) * size * 1.05, maxw));
        i += 9; break;
      }
      case 6: { // clover leaf w h fill stroke lw: tip at origin, lobes toward -y
        const w = f[i + 1], h = f[i + 2];
        ctx.beginPath();
        ctx.moveTo(0, 0);
        ctx.bezierCurveTo(-0.15 * w, -0.25 * h, -1.0 * w, -0.45 * h, -0.85 * w, -0.78 * h);
        ctx.bezierCurveTo(-0.72 * w, -1.02 * h, -0.2 * w, -1.03 * h, 0, -0.93 * h);
        ctx.bezierCurveTo(0.2 * w, -1.03 * h, 0.72 * w, -1.02 * h, 0.85 * w, -0.78 * h);
        ctx.bezierCurveTo(1.0 * w, -0.45 * h, 0.15 * w, -0.25 * h, 0, 0);
        ctx.fillStyle = css(u[i + 3]); ctx.fill();
        ctx.strokeStyle = css(u[i + 4]); ctx.lineWidth = f[i + 5]; ctx.stroke();
        i += 6; break;
      }
      case 5: ctx.setTransform(1, 0, 0, 1, 0, 0); ctx.fillStyle = css(u[i + 1]); ctx.fillRect(0, 0, canvas.width, canvas.height); i += 2; break;
      default: throw new Error(`bad draw command ${f[i]} at ${i}`);
    }
  }
}

// ---- outbox: network packets, UI requests ---------------------------------

const stats = { joins: 0, sentPackets: 0, sentBytes: 0, recvPackets: 0, recvBytes: 0, frameMs: [], presses: 0, paints: 0 };
let net = null;

let toastTimer = 0;
/** A one-line notice; `action` = [label, onclick] adds a button to it. */
function toast(msg, ms = 4500, action = null) {
  const t = $('toast');
  t.textContent = msg;
  if (action) {
    const b = Object.assign(document.createElement('button'), { textContent: action[0] });
    b.onclick = () => { t.classList.remove('show'); action[1](); };
    t.append(b);
  }
  t.classList.toggle('act', !!action);
  t.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove('show'), ms);
}

function drainOutbox() {
  const len = X.outbox_len();
  if (!len) return;
  const out = bytes(X.outbox_ptr(), len).slice();
  X.outbox_clear();
  const dv = new DataView(out.buffer);
  for (let p = 0; p < len; ) {
    const kind = out[p], n = dv.getUint32(p + 1, true), body = out.subarray(p + 5, p + 5 + n);
    p += 5 + n;
    if (kind === 1) {
      stats.sentPackets++; stats.sentBytes += n;
      if (!chaos?.cut) net?.send(body);
    } else if (kind === 2) {
      const b = new DataView(body.buffer, body.byteOffset, body.byteLength);
      const [x, y, w, h, angle] = [1, 5, 9, 13, 17].map((o) => b.getFloat32(o, true));
      openClueEditor(body[0], x, y, w, h, angle, dec.decode(body.subarray(21)));
    } else if (kind === 3) {
      toast(dec.decode(body));
    } else if (kind === 5) {
      keepRound(dec.decode(body));
    } else if (kind === 4 || kind === 6) {
      // A change someone else made, or a move of this group: worded by the core.
      if (!solo && !home) happened(dec.decode(body));
    } else if (kind === 7) {
      toast('A newer version of Shamrock is at this table. Reload to keep playing.', 1e9, ['Reload', () => location.reload()]);
    }
  }
}

// Presence follows connections: each peer connection gets a numeric link id.
const linkId = (s) => { let h = 0x811c9dc5; for (const c of String(s)) h = Math.imul(h ^ c.charCodeAt(0), 0x01000193); return (h >>> 0) || 1; };

function receive(u8, link) {
  if (chaos?.cut) return;
  if (chaos?.delayMs) return void setTimeout(() => deliver(u8, link), chaos.delayMs);
  deliver(u8, link);
}
function deliver(u8, link) {
  stats.recvPackets++; stats.recvBytes += u8.length;
  X.on_message(writeInbox(u8), link);
}

// ---- input ------------------------------------------------------------------

// Zoom is the app's own, per device, on the board area only: two fingers
// (or ctrl+wheel / a trackpad pinch) zoom and pan a camera over the canvas,
// so the dock and menu stay put and you can't get stuck zoomed in. Rust
// never knows: drawing goes through the camera and pointers come back
// through its inverse. One finger always plays. A second finger cancels
// whatever the first started, and nothing acts again until all have lifted.
const cam = { s: 1, x: 0, y: 0 };
let goAt = null; // see placeGo
const CAM_MAX = 4;
const view = () => canvas.getBoundingClientRect();
const local = (e) => { const r = view(); return [(e.clientX - r.left - cam.x) / cam.s, (e.clientY - r.top - cam.y) / cam.s]; };
function setCam(s, x, y) {
  const r = view();
  cam.s = Math.min(CAM_MAX, Math.max(1, s));
  // Keep the board area covered: no blank edges past the content.
  cam.x = Math.min(0, Math.max(r.width * (1 - cam.s), x));
  cam.y = Math.min(0, Math.max(r.height * (1 - cam.s), y));
  if (cam.s < 1.01) Object.assign(cam, { s: 1, x: 0, y: 0 });
  $('unzoom').hidden = cam.s === 1;
  if (editing >= 0) placeEditor();
  placeGo();
  wake();
}
/** Zoom by `k` about the canvas point (px, py), CSS px. */
const zoomAt = (k, px, py) => {
  const s = Math.min(CAM_MAX, Math.max(1, cam.s * k));
  setCam(s, px - (px - cam.x) * s / cam.s, py - (py - cam.y) * s / cam.s);
};
$('unzoom').onclick = () => setCam(1, 0, 0);

const fingers = new Map();
// While zoomed, one finger dragged from anywhere but a card pans the view
// (instead of spinning the board); a press that doesn't move is still a tap.
let pan = null; // { x, y, cx, cy, moved }
let pinch = null; // { d, mx, my, s, x, y } at the start of a two-finger gesture
let pinching = false;
const at = (e) => { const r = view(); return [e.clientX - r.left, e.clientY - r.top]; };
const spread = () => {
  const [a, b] = [...fingers.values()];
  return { d: Math.hypot(a[0] - b[0], a[1] - b[1]) || 1, mx: (a[0] + b[0]) / 2, my: (a[1] + b[1]) / 2 };
};
canvas.addEventListener('pointerdown', (e) => {
  try { canvas.setPointerCapture(e.pointerId); } catch {}
  fingers.set(e.pointerId, at(e));
  if (fingers.size === 2) {
    if (!pinching && !pan) { X.pointer_cancel(); drainOutbox(); }
    pan = null;
    pinching = true;
    pinch = { ...spread(), s: cam.s, x: cam.x, y: cam.y };
    return;
  }
  if (fingers.size > 2 || pinching) return;
  const [px, py] = at(e);
  if (cam.s > 1 && !X.on_card(...local(e))) { pan = { x: px, y: py, cx: cam.x, cy: cam.y, moved: false }; return; }
  X.pointer_down(...local(e));
});
canvas.addEventListener('pointermove', (e) => {
  if (!fingers.has(e.pointerId)) { if (e.pointerType === 'mouse') X.pointer_move(...local(e)); return; }
  fingers.set(e.pointerId, at(e));
  if (pan && !pinching) {
    const [px, py] = at(e);
    pan.moved ||= Math.hypot(px - pan.x, py - pan.y) > 8;
    if (pan.moved) setCam(cam.s, pan.cx + px - pan.x, pan.cy + py - pan.y);
    return;
  }
  if (!pinching) return X.pointer_move(...local(e));
  if (fingers.size !== 2 || !pinch) return;
  const now = spread(), s = pinch.s * now.d / pinch.d;
  // The board point under the fingers' midpoint follows the midpoint.
  setCam(s, now.mx - (pinch.mx - pinch.x) * s / pinch.s, now.my - (pinch.my - pinch.y) * s / pinch.s);
});
const lift = (e, act) => {
  if (!fingers.delete(e.pointerId)) return;
  if (pinching) {
    if (fingers.size === 2) pinch = { ...spread(), s: cam.s, x: cam.x, y: cam.y };
    if (!fingers.size) { pinching = false; pinch = null; }
    return;
  }
  if (pan) {
    // A pan that never moved was a tap (a leaf to edit, ⟳ to turn).
    const tap = act && !pan.moved;
    pan = null;
    if (!tap) return;
    X.pointer_down(...local(e));
  }
  if (act) { stats.presses++; X.pointer_up(...local(e)); } else X.pointer_cancel();
  drainOutbox();
};
canvas.addEventListener('pointerup', (e) => lift(e, true));
canvas.addEventListener('pointercancel', (e) => lift(e, false));
canvas.addEventListener('wheel', (e) => {
  // ctrl+wheel and trackpad pinches zoom the board; a plain wheel pans when zoomed.
  if (!e.ctrlKey && cam.s === 1) return;
  e.preventDefault();
  const [px, py] = at(e);
  if (e.ctrlKey) zoomAt(Math.exp(-e.deltaY / 200), px, py);
  else setCam(cam.s, cam.x - e.deltaX, cam.y - e.deltaY);
}, { passive: false });
// Safari's own page pinch: the board handles pinches itself.
for (const g of ['gesturestart', 'gesturechange']) document.addEventListener(g, (e) => e.preventDefault());
canvas.addEventListener('contextmenu', (e) => e.preventDefault());
addEventListener('keydown', (e) => {
  if (e.target instanceof HTMLInputElement) return;
  if (e.key === 'q') X.rotate_view(-1);
  if (e.key === 'e' || e.key === ' ') X.rotate_view(1);
});
// What the buttons do, by name (the Rust core's ACT_ codes).
const ACT = { deal: 1, ready: 2, check: 3, next: 4, proceed: 5, redeal: 6, newGame: 7, write: 8, joinGuessing: 9 };
const act = (name) => { stats.presses++; X.action(ACT[name]); drainOutbox(); renderUi(true); };

let editing = -1;
let editorAt = null;
function placeEditor() {
  if (!editorAt) return;
  const [x, y, w, h, angle] = editorAt;
  // Under the zoom camera: centre moves with it, the box scales with it.
  Object.assign(clueInput.style, {
    display: 'block', left: `${cam.x + x * cam.s - w / 2}px`, top: `${cam.y + y * cam.s - h / 2}px`, width: `${w}px`, height: `${h}px`,
    transform: `rotate(${angle}rad) scale(${cam.s})`,
  });
}
function openClueEditor(idx, x, y, w, h, angle, text) {
  const reposition = editing === idx && clueInput.style.display === 'block';
  editing = idx;
  editorAt = [x, y, w, h, angle];
  placeEditor();
  if (reposition) return; // keep what's being typed
  clueInput.value = text;
  clueInput.focus();
  clueInput.select();
}
function closeClueEditor(commit, advance = false) {
  if (editing < 0) return;
  const idx = editing;
  editing = -1;
  // Stay visible while Rust decides whether to open the next leaf: hiding a
  // focused input fires a late blur that would close the next leaf.
  if (commit) X.set_clue(idx, writeText(clueInput.value), advance ? 1 : 0);
  drainOutbox();
  if (editing < 0) clueInput.style.display = 'none';
}
clueInput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') { stats.presses++; closeClueEditor(true, true); }
  if (e.key === 'Escape') closeClueEditor(false);
});
clueInput.addEventListener('blur', () => closeClueEditor(true));

new ResizeObserver(() => {
  const r = canvas.getBoundingClientRect(), dpr = devicePixelRatio || 1;
  canvas.width = Math.round(r.width * dpr);
  canvas.height = Math.round(r.height * dpr);
  X.resize(r.width, r.height, dpr);
  setCam(cam.s, cam.x, cam.y);
  // Layout shifts must not interrupt typing.
  if (editing >= 0) { X.edit_clue(editing); drainOutbox(); }
}).observe(canvas);

// ---- joining, watching, menu ------------------------------------------------------

const nameKey = `shamrock:${roomId}:name`;

// Shared boards travel inside the link (?b=…, made by "Share boards") and
// open as solo play on whatever device follows it: no room, no name, no
// connection. Links must not give answers away by accident: Rust scrambles
// each board (cards shuffled and randomly turned, the answer on a separate
// line); here that text is only compressed and AES-GCM encrypted under a key
// derived from the link's #fragment. Every device has to decode it to play,
// so this stops accidental peeking, not determined devtools digging.
const solo = params.has('b');
const b64 = {
  enc: (u8) => btoa(String.fromCharCode(...u8)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, ''),
  dec: (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0)),
};
const shareKey = async (salt) => crypto.subtle.importKey(
  'raw', await crypto.subtle.digest('SHA-256', enc.encode(`shamrock-share-v1:${salt}`)), 'AES-GCM', false, ['encrypt', 'decrypt']);
const squeeze = async (u8, mode) => new Uint8Array(await new Response(
  new Blob([u8]).stream().pipeThrough(mode === 'in' ? new CompressionStream('deflate-raw') : new DecompressionStream('deflate-raw'))).arrayBuffer());
const scratchText = (len) => dec.decode(bytes(X.scratch_ptr(), len));

/** Scrambled board text (from Rust) → link payload. Byte 2 = deflated text. */
async function seal(text, salt) {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const data = new Uint8Array([2, ...(await squeeze(enc.encode(text), 'in'))]);
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, await shareKey(salt), data));
  return b64.enc(new Uint8Array([...iv, ...ct]));
}
/** Link payload → scrambled board texts, for Rust to load. Older links (JSON,
 *  plain or deflated) are rewritten into the same scrambled text form. */
async function unseal(payload, salt) {
  const raw = b64.dec(payload);
  const pt = new Uint8Array(await crypto.subtle.decrypt({ name: 'AES-GCM', iv: raw.slice(0, 12) }, await shareKey(salt), raw.slice(12)));
  if (pt[0] === 2) return dec.decode(await squeeze(pt.slice(1), 'out')).split('\n\n');
  const json = pt[0] === 1 ? await squeeze(pt.slice(1), 'out') : pt;
  return JSON.parse(dec.decode(json)).map(({ n, c, k, a }) => [n, ...c, ...k.map((w) => w.join('|')), `@ ${a.flat().join(' ')}`].join('\n'));
}

// Card words are laid out per card from their real widths: measure every
// card word (deck and any imported clovers) once the card font is in.
let wordsMeasured = false;
async function measureWords() {
  await document.fonts?.load(`700 100px Figtree`).catch(() => {});
  const c = document.createElement('canvas').getContext('2d');
  c.font = FONTS[1](100);
  const list = scratchText(X.word_list()).split('\n');
  c.textBaseline = 'middle';
  // Width and how far below the centre line each word's letters reach.
  // …and, letter by letter, where each sits and how far it descends, so a
  // corner only counts the letters actually near it.
  const line = (w) => {
    const t = c.measureText(w);
    const letters = [...w].map((ch, i) => `${c.measureText(w.slice(0, i + 1)).width.toFixed(1)}:${c.measureText(ch).actualBoundingBoxDescent.toFixed(1)}`).join(' ');
    return `${w}\t${t.width.toFixed(2)}\t${t.actualBoundingBoxDescent.toFixed(2)}\t${letters}`;
  };
  X.set_word_widths(writeText(list.map(line).join('\n')));
  wordsMeasured = true;
}

let customs = 0; // clovers loaded from a share link
const loadBoard = (text) => { customs += X.load_custom(writeText(text)); };
if (solo) {
  try { (await unseal(params.get('b'), roomId)).forEach(loadBoard); }
  catch { setTimeout(() => toast("This shared-boards link is damaged; check it was copied in full.", 8000), 500); }
}
measureWords();

/** A link that plays `boards` (texts in answer order, e.g. from a screenshot). */
async function shareFromText(boards) {
  const scrambled = scratchText(X.scramble_boards(writeText(boards.join('\n\n'))));
  return linkFor(scrambled);
}
async function linkFor(scrambled) {
  const salt = Math.random().toString(36).slice(2, 8);
  return `${location.origin}${location.pathname}?b=${await seal(scrambled, salt)}#${salt}`;
}
const shareLink = async (mask) => linkFor(scratchText(X.export_boards(mask)));
// To the home page, where a new table gets its name (and yours).
const ownTable = () => { location.href = `${location.origin}${location.pathname}`; };
function openShare() {
  const list = $('shareList');
  list.replaceChildren(...sheet('share').rows.map((label, i) => {
    const row = document.createElement('label');
    row.className = 'item';
    row.innerHTML = `<input type="checkbox" checked data-i="${i}"> <span class="grow">${esc(label)}</span>`;
    return row;
  }));
  const update = async () => {
    let mask = 0;
    list.querySelectorAll('input:checked').forEach((c) => (mask |= 1 << +c.dataset.i));
    $('shareCopy').disabled = true;
    $('shareText').textContent = mask ? await shareLink(mask) : '';
    $('shareCopy').disabled = !mask;
  };
  list.onchange = update;
  update();
  closeSheets();
  $('shareSheet').classList.add('open');
}
$('shareCopy').onclick = async () => {
  try { await navigator.clipboard.writeText($('shareText').textContent); toast('Link copied'); }
  catch { getSelection().selectAllChildren($('shareText')); toast('Select the link above and copy it'); }
};
$('shareClose').onclick = () => closeSheets();
$('shareMenuBtn').onclick = openShare;
function joinAs(name) {
  name = name.trim();
  if (!name) return;
  X.join(writeText(name));
  drainOutbox();
  remember(nameKey, name);
  remember(NAME, name);
  noteTable(name);
  closeSheets();
  pendingSeat = null;
  renderUi(true);
}
function watch() {
  X.watch();
  drainOutbox();
  remember(nameKey, '');
  closeSheets();
  renderUi(true);
}
const closeSheets = () => document.querySelectorAll('.overlay').forEach((o) => o.classList.remove('open'));
let pendingSeat = null, lastJoin = '';
function renderJoin(force = false) {
  const j = sheet('join'), key = JSON.stringify(j);
  if (!force && lastJoin === key) return;
  lastJoin = key;
  const invited = !!j.invited;
  $('joinSheet').classList.toggle('invited', invited);
  $('inviteJoinHead').hidden = !invited;
  for (const id of ['joinTableField', 'joinNameField', 'joinAdvice', 'joinSubmit']) $(id).hidden = invited;
  $('seatList').hidden = !!pendingSeat;
  $('seatConfirm').hidden = !pendingSeat;
  $('inviteJoinTitle').textContent = invited ? j.heading : '';
  $('inviteJoinIntro').textContent = j.intro ?? '';
  $('seatList').replaceChildren(
    ...(!invited && j.heading ? [Object.assign(document.createElement('p'), { textContent: j.heading })] : []),
    ...j.rows.map((p) => {
      const b = document.createElement('button');
      b.type = 'button'; b.className = invited ? 'inviteSeat' : 'menuBtn';
      b.innerHTML = invited ? `<strong>${esc(p.label)}</strong><small>${esc(p.note)}</small>`
        : `<span class="chip${p.away ? ' away' : ''}"><i style="background:${esc(p.color)}"></i>${esc(p.name)}</span>${p.note ? ` <small>${esc(p.note)}</small>` : ''}`;
      b.onclick = () => {
        // Re-read presence at the moment of the click: someone may have
        // arrived since this row was drawn.
        const choice = sheet('join').rows.find((r) => r.name === p.name);
        if (choice?.confirm) {
          pendingSeat = p.name;
          $('seatConfirmText').textContent = choice.confirm;
          $('seatConfirmJoin').textContent = choice.confirmLabel;
          renderJoin(true);
          $('seatChooseAnother').focus();
        } else joinAs(p.name);
      };
      return b;
    }));
}
function openJoin() {
  const s = ui();
  pendingSeat = null;
  renderJoin(true);
  $('tableInput').value = roomId;
  $('nameInput').value = s.name ?? recall(nameKey) ?? myName();
  $('joinSheet').classList.add('open');
  // A remembered name means one tap on Join; only an empty one needs typing.
  if (!$('joinNameField').hidden && !$('nameInput').value) setTimeout(() => $('nameInput').focus(), 50);
}
$('seatConfirmJoin').onclick = () => { if (pendingSeat) joinAs(pendingSeat); };
$('seatChooseAnother').onclick = () => { pendingSeat = null; renderJoin(true); };
$('joinForm').onsubmit = (e) => {
  e.preventDefault();
  if (sheet('join').invited) return; // invited names are chosen explicitly
  stats.presses++;
  // Editing the table name switches tables (joining that one instead).
  const room = normRoom($('tableInput').value);
  if (room && room !== roomId) return startAt(room, $('nameInput').value);
  joinAs($('nameInput').value);
};
$('watchBtn').onclick = watch;
$('menuBtn').onclick = () => {
  const s = ui();
  renderHappened();
  $('unsubmitBtn').hidden = !s.menu.unsubmit;
  $('linkText').textContent = tableUrl(roomId);
  // Everyone at the table, by name, with what they're up to.
  const who = sheet('players');
  $('playerList').replaceChildren(...who.rows.map((p) => {
    const row = document.createElement('div');
    row.className = 'who-row';
    const label = document.createElement('span');
    label.className = 'grow';
    label.innerHTML = `<b>${esc(p.name)}</b>${esc(p.you)}<br><small>${esc(p.doing)}</small>`;
    row.append(label);
    if (p.removable) {
      const rm = Object.assign(document.createElement('button'), { textContent: 'Remove' });
      rm.onclick = () => { X.remove_seat(p.id); drainOutbox(); row.remove(); };
      row.append(rm);
    }
    return row;
  }), ...(who.watching ? [Object.assign(document.createElement('div'), { className: 'who-row', textContent: who.watching })] : []));
  $('watchMenuBtn').hidden = !s.menu.watch;
  syncSize();
  $('menuSheet').classList.add('open');
};
$('closeMenu').onclick = closeSheets;
document.querySelectorAll('.overlay').forEach((o) => o.addEventListener('click', (e) => {
  if (e.target !== o || ['homeSheet', 'inviteErrorSheet'].includes(o.id) || (o.id === 'joinSheet' && sheet('join').invited)) return;
  closeSheets();
  if (home) openHome();
}));
$('copyLink').onclick = async () => {
  try { await navigator.clipboard.writeText(tableUrl(roomId)); toast('Invite link copied'); }
  catch { getSelection().selectAllChildren($('linkText')); toast('Select the link above and copy it'); }
};
$('switchBtn').onclick = () => { closeSheets(); openJoin(); };
$('watchMenuBtn').onclick = watch;
$('redealBtn').onclick = () => { closeSheets(); act('redeal'); };
$('newGameBtn').onclick = () => { closeSheets(); act('newGame'); };
$('unsubmitBtn').onclick = () => { closeSheets(); act('ready'); };
$('freshBtn').onclick = () => { closeSheets(); freshTable(); };

// Card size is per device (display only): bigger cards just overlap more.
const zoomKey = 'shamrock:zoom';
let zoom = X.set_zoom(+(recall(zoomKey) ?? (Math.min(innerWidth, innerHeight) < 520 ? 1.25 : 1)));
const setZoom = (z) => { stats.presses++; zoom = X.set_zoom(z); remember(zoomKey, zoom); syncSize(); };
// The size buttons step from what's drawn (a saved size can exceed what fits
// right now) and grey out when a step would change nothing; past the limit,
// pinch zoom takes over.
const SIZE_STEP = 0.15;
function sizeWouldChange(dz) {
  const was = X.card_zoom(), saved = zoom;
  X.set_zoom(was + dz);
  const changed = Math.abs(X.card_zoom() - was) > 1e-3;
  X.set_zoom(saved);
  return changed;
}
function syncSize() {
  const up = sizeWouldChange(SIZE_STEP), down = sizeWouldChange(-SIZE_STEP);
  $('sizeUp').disabled = !up;
  $('sizeDown').disabled = !down;
  $('sizeNote').hidden = up;
}
// Theme: follow the device's light/dark setting, or a choice made in the
// menu (kept per device). Rust draws the board in the matching palette.
const themeKey = 'shamrock:theme';
const systemDark = matchMedia('(prefers-color-scheme: dark)');
let themePref = recall(themeKey) ?? 'auto';
function applyTheme() {
  const dark = themePref === 'dark' || (themePref === 'auto' && systemDark.matches);
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
  X.set_theme(dark ? 1 : 0);
  document.querySelector('meta[name=theme-color]')?.setAttribute('content', getComputedStyle(document.documentElement).getPropertyValue('--bar').trim());
  $('themeBtn').textContent = `Theme: ${themePref}`;
}
systemDark.addEventListener('change', applyTheme);
$('themeBtn').onclick = () => {
  themePref = { auto: 'light', light: 'dark', dark: 'auto' }[themePref] ?? 'auto';
  remember(themeKey, themePref === 'auto' ? null : themePref);
  applyTheme();
};
applyTheme();

// Full screen where the browser allows it (Android, desktop; not iPhone,
// where "Add to Home Screen" gives the same via the manifest).
const fullBtn = $('fullBtn');
const fsEl = document.documentElement;
fullBtn.hidden = !(document.fullscreenEnabled && fsEl.requestFullscreen);
const syncFull = () => { fullBtn.textContent = document.fullscreenElement ? 'Exit full screen' : 'Full screen'; };
document.addEventListener('fullscreenchange', syncFull);
fullBtn.onclick = () => {
  closeSheets();
  (document.fullscreenElement ? document.exitFullscreen() : fsEl.requestFullscreen({ navigationUI: 'hide' })).catch(() => {});
};
syncFull();
$('sizeDown').onclick = () => setZoom(X.card_zoom() - SIZE_STEP);
$('sizeUp').onclick = () => setZoom(X.card_zoom() + SIZE_STEP);

const setDecoys = (d) => { X.set_decoys(Math.max(0, Math.min(4, d))); drainOutbox(); renderUi(true); };
$('decDown').onclick = () => { stats.presses++; setDecoys(ui().decoys - 1); };
$('decUp').onclick = () => { stats.presses++; setDecoys(ui().decoys + 1); };

// ---- HTML controls mirror the Rust UI state ------------------------------------

const ui = () => scratchJson(X.ui());
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
// Text from the core with names marked \u0001…\u0002 for bold, as HTML.
const marked = (t) => esc(t).replace(/\u0001/g, '<b>').replace(/\u0002/g, '</b>');
// What a sheet shows, as the core has it (see app/page.rs).
const SHEET = { join: 1, players: 2, clovers: 3, share: 4, recap: 5 };
const sheet = (kind) => scratchJson(X.sheet(SHEET[kind]));
// Core functions over text (parts joined by U+001F), returning text.
const coreText = (fn, parts, ...args) => scratchText(fn(writeText(parts.join('\u001f')), ...args));
let lastUi = '';

function renderUi(force = false) {
  const s = ui();
  const key = JSON.stringify(s);
  if (!force && key === lastUi) return;
  lastUi = key;
  if ($('joinSheet').classList.contains('open')) renderJoin();

  // The strip as the Rust core words it (see app/page.rs): this only
  // shows it. Side effects of the page's own stay here: tips once per
  // device, and the recap opening once by itself when a round ends.
  const st = s.strip;
  if (st.tip) tip(st.tip);
  if (s.phase !== 'end' && recapShown === s.round) {
    $('recapSheet').classList.remove('open');
    recapShown = null;
  }
  if (!home && s.phase === 'end' && recapShown !== s.round && (!invitePayload || s.seated || recall(nameKey) === '')) {
    recapShown = s.round; setTimeout(() => { if (ui().phase === 'end' && ui().round === s.round) openRecap(); }, 600);
  }
  // Submission progress: a thin bar along the top of the strip.
  const segs = $('segs');
  segs.hidden = !st.segs;
  if (!segs.hidden) {
    segs.replaceChildren(...s.progress.map((p) => {
      const seg = document.createElement('i');
      seg.className = `${p.ready ? 'done' : ''}${p.away ? ' away' : ''}`;
      seg.style.color = p.color;
      seg.title = `${p.name}: ${p.ready ? 'submitted' : 'writing'}${p.away ? ' (away)' : ''}`;
      return seg;
    }));
  }
  const DO = { join: openJoin, ownTable };
  const press = (b) => DO[b.act] ?? (() => act(b.act));
  $('hint').classList.toggle('pick', !!st.tap);
  $('hint').onclick = st.tap === 'clovers' ? openClovers : st.tap === 'recap' ? openRecap : null;
  $('invite').hidden = !st.invite;
  const goBtn = $('go');
  goBtn.hidden = !st.go;
  goBtn.textContent = st.go?.label ?? '';
  goBtn.disabled = !st.go?.enabled;
  goBtn.onclick = st.go ? press(st.go) : null;
  goAt = st.goAt;
  placeGo();
  const altBtn = $('alt');
  altBtn.hidden = !st.alt;
  altBtn.textContent = st.alt?.label ?? '';
  altBtn.onclick = st.alt ? press(st.alt) : null;
  $('shareMenuBtn').hidden = !s.menu.share;
  $('decoys').hidden = !st.decoys;
  $('decLabel').textContent = st.decoyLabel;
  fitHint(st.hint.map((h) => (st.pulse ? '<span class="pulse"></span>' : '') + marked(h)));
}

// The core may place the main button by the clover instead of in the strip
// (canvas px), and it follows the zoom camera.
function placeGo() {
  const goBtn = $('go'), c = canvas.getBoundingClientRect();
  const [x, y, w] = goAt ?? [], px = cam.x + x * cam.s, py = cam.y + y * cam.s;
  // Zoomed in so its spot is off screen: back to the strip, always in reach.
  const near = !!goAt && px > 60 && px < c.width - 60 && py > 30 && py < c.height - 30;
  goBtn.classList.toggle('near', near);
  document.body.classList.toggle('goNear', near);
  if (!near) { goBtn.removeAttribute('style'); return; }
  Object.assign(goBtn.style, { left: `${c.left + px}px`, top: `${c.top + py}px`, fontSize: `${Math.min(26, Math.max(17, w / 8))}px` });
}

// Show the longest version of the status that fits beside the buttons (the
// last one is cut with … if even that doesn't). Re-fit when the width changes.
let hintVariants = [];
function fitHint(hint) {
  hintVariants = [hint].flat();
  const h = $('hint');
  for (const v of hintVariants) {
    h.innerHTML = v;
    if (h.scrollWidth <= h.clientWidth + 1) break;
  }
  h.title = hintVariants[0] ? h.ownerDocument.createRange().createContextualFragment(hintVariants[0]).textContent : '';
}
addEventListener('resize', () => fitHint(hintVariants));

// How-to tips: shown once per device as a toast that the next touch on the
// board dismisses, and kept in the ⋯ menu.
function tip(kind) {
  const TIPS = {
    write: 'Tap a leaf and write a clue linking the two words facing it. Enter jumps to the next leaf. Then choose decoys and Submit.',
    guess: 'Drag cards onto the spots (they snap when close), tap a card to turn it, then Guess. Wrong cards bounce back.',
  };
  $('tipText').textContent = TIPS[kind];
  const key = `shamrock:tip:${kind}`;
  if (recall(key)) return;
  remember(key, '1');
  setTimeout(() => toast(TIPS[kind], 9000), 300);
}
canvas.addEventListener('pointerdown', () => $('toast').classList.remove('show'));

// ---- choosing the clover, inviting ---------------------------------------------

function openClovers() {
  const list = $('cloverList');
  list.replaceChildren(...sheet('clovers').rows.map((r, i) => {
    const b = Object.assign(document.createElement('button'), { className: `clover-row${r.now ? ' now' : ''}` });
    b.innerHTML = `<span>${marked(r.label)}</span><small>${esc(r.note)}</small>`;
    b.onclick = () => { stats.presses++; X.jump(i); drainOutbox(); closeSheets(); renderUi(true); };
    return b;
  }));
  $('cloverSheet').classList.add('open');
}
$('cloverClose').onclick = closeSheets;
$('invite').onclick = async () => {
  stats.presses++;
  const url = tableUrl(roomId);
  try {
    if (navigator.share) return await navigator.share({ title: 'Shamrock', text: 'Join my Shamrock table', url });
  } catch (e) { if (e?.name === 'AbortError') return; }
  try { await navigator.clipboard.writeText(url); toast('Invite link copied'); }
  catch { toast(url, 9000); }
};

// ---- this device's tables and game log (localStorage only) ------------------------

const TABLES = 'shamrock:tables', LOG = 'shamrock:log';
const readJson = (k, d) => { try { return JSON.parse(recall(k)) ?? d; } catch { return d; } };
// Table names are the link (the core suggests them, and makes typed ones
// link-safe: "Family Night" -> family-night).
const suggestRoom = () => scratchText(X.suggest_room((Math.random() * 2 ** 32) >>> 0));
const normRoom = (t) => scratchText(X.norm_room(writeText(t)));
// Your name, carried across tables as the default.
const NAME = 'shamrock:name';
const myName = () => recall(NAME) ?? myTables().name ?? '';
/** Go to `room` as `name`: remembered for that table, so it joins on arrival. */
function startAt(room, name, invite) {
  name = name.trim();
  if (name) { remember(NAME, name); remember(`shamrock:${room}:name`, name); }
  goTo(room, invite);
}
// Links to other tables keep the network this page is on (a private
// signaling relay, a relay list, test switches), and nothing else.
const NET_PARAMS = ['signal', 'relays', 'net', 'rejoin', 'chaos'];
const netQuery = () => { const q = new URLSearchParams(NET_PARAMS.filter((k) => params.has(k)).map((k) => [k, params.get(k)])).toString(); return q ? `?${q}` : ''; };
const tableUrl = (room, invite = room === roomId ? invitePayload : recall(`shamrock:${room}:invite`)) => {
  const q = new URLSearchParams(netQuery());
  if (room && invite) q.set('i', invite);
  return `${location.origin}${location.pathname}${q.size ? `?${q}` : ''}${room ? `#${room}` : ''}`;
};
// The page is one table (or the home page) for its whole life: when the
// #room changes, as with Back/Forward between the home page and a table,
// start over as whatever the address now says.
const goTo = (room, invite) => { location.href = tableUrl(room, invite); };
addEventListener('hashchange', () => { if (decodeURIComponent(location.hash.slice(1)) !== roomId) location.reload(); });
const when = (t) => (t ? new Date(t).toLocaleString([], { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }) : 'earlier');
function noteTable(name) {
  const t = readJson(TABLES, {});
  t[roomId] = { name, at: Date.now() };
  remember(TABLES, JSON.stringify(t));
}
function myTables() {
  const t = readJson(TABLES, {});
  try { // Visits from before this list only left a saved name.
    for (let i = 0; i < localStorage.length; i++) {
      const m = localStorage.key(i).match(/^shamrock:(.+):name$/);
      if (m && !t[m[1]]) t[m[1]] = { name: recall(m[0]), at: 0 };
    }
  } catch {}
  return scratchJson(X.tables(writeText(JSON.stringify(t))));
}
/** Drop a table from this device's list (and the name saved for it there).
 *  Its past games stay under Past games; the invite link still works. */
function forgetTable(room) {
  const t = readJson(TABLES, {});
  delete t[room];
  remember(TABLES, JSON.stringify(t));
  remember(`shamrock:${room}:name`, null);
  remember(`shamrock:${room}:state`, null);
  remember(`shamrock:${room}:invite`, null);
}
/** Keep a finished round's clovers (answers included) on this device. */
/** Keep (or update) a round in Past games, by its key (table and round).
 *  Rust raises this when a round with anything in it ends or stops being
 *  current, for any reason, and again if a late change reaches it. */
function keepRound(k) {
  if (home) return;
  remember(LOG, coreText(X.keep_round, [recall(LOG) ?? '[]', k, roomId], Date.now()));
}

// ---- what happened at this table ------------------------------------------------

// Every change of round someone else made, as one line, kept on this device
// (the last 10, in the menu) besides the notice itself.
const HAPPENED = `shamrock:${roomId}:happened`;
const ACTIONS = { move: () => moveGroup(), follow: (a) => followMove(a.room) };
function happened(notice) {
  const { text, action } = JSON.parse(notice);
  toast(text, action ? 20000 : 8000, action && [action.label, () => ACTIONS[action.kind](action)]);
  remember(HAPPENED, coreText(X.happened_add, [recall(HAPPENED) ?? '[]', notice], Date.now()));
}
function renderHappened() {
  const list = readJson(HAPPENED, []);
  $('happenedEmpty').hidden = list.length > 0;
  $('happenedList').replaceChildren(...list.map((e) => {
    const d = Object.assign(document.createElement('div'), { className: 'happened' });
    d.innerHTML = `<small>${when(e.at)}</small>${esc(e.text)}`;
    if (e.action) {
      const b = Object.assign(document.createElement('button'), { className: 'menuBtn', textContent: e.action.label });
      b.onclick = () => { closeSheets(); ACTIONS[e.action.kind](e.action); };
      d.append(b);
    }
    return d;
  }));
}

// ---- moving to a fresh table ------------------------------------------------------

/** Open `room` with `seed` (a table, as a sync packet) as its saved table,
 *  and this player's name, so it arrives seated where it left off. */
function seedRoom(room, seed) {
  remember(`shamrock:${room}:state`, JSON.stringify({ at: Date.now(), v: X.protocol(), s: b64.enc(seed) }));
  const name = ui().name ?? myName();
  if (name) { remember(`shamrock:${room}:name`, name); remember(NAME, name); }
}
const scratchBytes = (len) => bytes(X.scratch_ptr(), len).slice();
/** Two tables met here: take my group (the table I came from) to a new room,
 *  and leave word here for the rest of the group. */
function moveGroup() {
  // Moving says who did it: sit down under your name first if the page
  // hasn't yet (it rejoins 1.5 s after opening).
  if (!ui().seated && recall(nameKey)) joinAs(recall(nameKey));
  const room = suggestRoom();
  const len = X.record_move(writeText(room));
  if (!len) return drainOutbox(); // the core says why
  const seed = scratchBytes(len);
  drainOutbox();
  seedRoom(room, seed);
  setTimeout(() => goTo(room), 800); // let the word reach the others first
}
/** Follow my group's move, bringing my own copy of our table. */
function followMove(room) {
  const len = X.origin_snapshot();
  if (len) seedRoom(room, scratchBytes(len));
  goTo(room);
}
/** Split off: this table, as it is, at a new room; send the link to anyone. */
function freshTable() {
  const room = suggestRoom();
  seedRoom(room, scratchBytes(X.snapshot()));
  try { sessionStorage.setItem('shamrock:fresh', room); } catch {}
  goTo(room);
}

/** A finished clover, read-only, as the core describes it (pages.rs):
 *  each clue on its leaf with the two words it linked, round the four
 *  answer cards showing only their outward words. */
function cloverHtml(v) {
  const leaf = (d) => `<span class="leafTag c${d}"><b>${esc(v.leaves[d].clue)}</b><small>${esc(v.leaves[d].words)}</small></span>`;
  const card = (sp) => `<div class="mc">${v.cards[sp].map((w, e) => (w ? `<span class="${'trbl'[e]}">${esc(w)}</span>` : '')).join('')}</div>`;
  return `<div class="mini">
    <div class="who"><span>${esc(v.title)}</span><span class="badge${v.first ? ' first' : ''}">${esc(v.badge)}</span></div>
    <div class="mclover">${leaf(0)}${leaf(1)}${leaf(2)}${leaf(3)}<div class="mgrid">${[0, 1, 3, 2].map(card).join('')}</div></div>
    ${v.decoys ? `<div class="decoys">${esc(v.decoys)}</div>` : ''}
  </div>`;
}
/** The end of a round: every clover's answer, one after another. */
function openRecap() {
  const r = sheet('recap');
  if (!r.boards.length) return;
  $('recapHead').textContent = r.head;
  $('recapStats').innerHTML = r.stats.map(([v, k]) => `<div class="stat"><b>${esc(v)}</b>${esc(k)}</div>`).join('');
  $('recapList').innerHTML = r.boards.map(cloverHtml).join('');
  $('recapList').scrollTop = 0;
  $('recapShare').hidden = !r.share;
  $('recapNext').hidden = !r.next;
  $('recapNext').disabled = !r.nextEnabled;
  closeSheets();
  $('recapSheet').classList.add('open');
}
$('recapShare').onclick = openShare;
$('recapNext').onclick = () => { closeSheets(); act('deal'); };
$('recapClose').onclick = () => closeSheets();
let recapShown = null;
function openLog() {
  const log = scratchJson(X.past_games(writeText(recall(LOG) ?? '[]')));
  const list = $('logList');
  list.replaceChildren(...log.rows.map((e) => {
    const d = document.createElement('details');
    d.className = 'logEntry';
    d.innerHTML = `<summary><b>${when(e.at)}</b> · ${esc(e.line)}<br><small>${esc(e.who)}</small></summary>
      ${e.boards.map(cloverHtml).join('')}
      <div class="row"><button class="go replay"${e.replay ? '' : ' hidden'}>${esc(e.replay ?? '')}</button><button class="forget">Delete</button></div>`;
    // Only clovers with all four clues can be played; half-written ones are for reading.
    d.querySelector('.replay').onclick = async () => { location.href = await shareFromText(e.replayTexts); };
    d.querySelector('.forget').onclick = () => {
      remember(LOG, JSON.stringify(readJson(LOG, []).filter((x) => x.id !== e.id)));
      d.remove();
    };
    return d;
  }));
  $('logEmpty').hidden = log.rows.length > 0;
  closeSheets();
  $('logSheet').classList.add('open');
}
function openHome() {
  const tables = myTables().rows;
  $('tableList').replaceChildren(...tables.map((t) => {
    const row = document.createElement('div');
    row.className = 'tableItem';
    const a = document.createElement('a');
    a.className = 'item tableRow';
    a.href = tableUrl(t.room);
    a.onclick = (e) => { e.preventDefault(); goTo(t.room); };
    a.innerHTML = `<span class="grow"><b>${esc(t.title)}</b><br><small>${esc(t.by)} · ${when(t.at)}</small></span><span aria-hidden="true">›</span>`;
    const drop = Object.assign(document.createElement('button'), { className: 'forget', textContent: 'Forget', title: `Forget #${t.room} on this device` });
    drop.onclick = () => { forgetTable(t.room); openHome(); };
    row.append(a, drop);
    return row;
  }));
  $('tablesHead').hidden = !tables.length;
  const past = scratchJson(X.past_games(writeText(recall(LOG) ?? '[]'))).homeButton;
  $('homeLog').hidden = !past;
  $('homeLog').textContent = past ?? '';
  if (!$('homeRoom').value) $('homeRoom').value = suggestRoom();
  if (!$('homeName').value) $('homeName').value = myName();
  $('homeSheet').classList.add('open');
}
$('homeReroll').onclick = () => { $('homeRoom').value = suggestRoom(); };
$('homePlan').onchange = () => {
  $('inviteSetup').hidden = !$('homePlan').checked;
  $('homeNew').hidden = $('homePlan').checked;
  $('homeInvite').hidden = !$('homePlan').checked;
  $('homeError').hidden = true;
};
$('homeForm').onsubmit = (e) => {
  e.preventDefault();
  if ($('homePlan').checked) {
    const i = scratchJson(X.create_invite(writeText(JSON.stringify({ room: $('homeRoom').value, name: $('homeName').value, friends: $('homeFriends').value }))));
    if (i.error) { $('homeError').textContent = i.error; $('homeError').hidden = false; return; }
    const payload = b64.enc(enc.encode(JSON.stringify(i)));
    remember(`shamrock:${i.room}:invite`, payload);
    try { sessionStorage.setItem('shamrock:fresh', i.room); } catch {}
    return startAt(i.room, i.names[0], payload);
  }
  startAt(normRoom($('homeRoom').value) || suggestRoom(), $('homeName').value);
};
$('inviteErrorHome').onclick = () => goTo('');
$('homeLog').onclick = openLog;
$('logClose').onclick = () => { closeSheets(); if (home) openHome(); };
$('logMenuBtn').onclick = openLog;
// The strategy guide: from the menu, or from the home page.
const openHelp = () => { closeSheets(); $('helpSheet').classList.add('open'); };
$('helpMenuBtn').onclick = openHelp;
$('homeHelp').onclick = openHelp;
$('helpClose').onclick = () => { closeSheets(); if (home) openHome(); };
$('homeMenuBtn').onclick = () => goTo('');

// ---- network ----------------------------------------------------------------

const peers = new Set();
const joined = (id) => { peers.add(id); X.set_links(peers.size); X.on_peer_join(); drainOutbox(); renderUi(true); };
const left = (id) => { peers.delete(id); X.set_links(peers.size); X.on_link_lost(linkId(id)); renderUi(true); };

async function connect() {
  const mode = params.get('net');
  if (mode === 'local') {
    // Same-browser tabs; handy offline. Same shape as the WebRTC path.
    const bc = new BroadcastChannel(`shamrock:${roomId}`);
    const hello = (to) => bc.postMessage({ hello: device, to, from: device });
    bc.onmessage = ({ data }) => {
      if (data.hello) {
        if (!peers.has(data.hello)) { joined(data.hello); hello(data.hello); }
      } else if (data.bye) left(data.bye);
      else receive(new Uint8Array(data.bytes), linkId(data.from));
    };
    addEventListener('pagehide', () => bc.postMessage({ bye: device }));
    hello();
    return { send: (u8) => bc.postMessage({ bytes: u8, from: device }) };
  }
  const signal = params.get('signal');
  const lib = signal ? await import('./vendor/trystero-ws.js') : await import('./vendor/trystero-nostr.js');
  // Players find each other through public Nostr relays. Trystero's default
  // picks 5 hobby relays by hashing the app id, and when they're down or
  // slow nobody meets until a reload happens to get through; these are
  // large, widely run open relays, all used at once.
  const config = { appId: 'shamrock-p2p', relayConfig: { urls: signal ? [signal] : RELAYS } };
  let room, action;
  const enter = () => {
    room = lib.joinRoom(config, roomId);
    action = room.makeAction('m');
    action.onMessage = (data, { peerId }) => receive(new Uint8Array(data), linkId(peerId));
    room.onPeerJoin = joined;
    room.onPeerLeave = left;
    stats.joins++;
  };
  enter();
  // Alone? Announce again from scratch (what a reload did), soon at first,
  // then backing off; and at once when the page comes back to the
  // foreground, since phones drop sockets in background tabs.
  let since = performance.now(), wait = REJOIN_FIRST_MS;
  const rejoin = () => {
    try { room.leave(); } catch {}
    enter();
    since = performance.now();
  };
  setInterval(() => {
    if (peers.size) { since = performance.now(); wait = REJOIN_FIRST_MS; return; }
    if (performance.now() - since > wait) { wait = Math.min(wait * 1.6, 30000); rejoin(); }
  }, 1000);
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible' && !peers.size) rejoin(); });
  addEventListener('pagehide', () => room.leave());
  return { send: (u8) => peers.size && action.send(u8), rejoin };
}
// ?relays=host1,host2 overrides the list (for testing from a phone).
const RELAYS = params.get('relays')?.split(',').map((h) => (h.includes('://') ? h : `wss://${h}`))
  ?? ['wss://relay.damus.io', 'wss://nos.lol', 'wss://relay.primal.net', 'wss://nostr.mom', 'wss://relay.snort.social', 'wss://offchain.pub'];
const REJOIN_FIRST_MS = +(params.get('rejoin') ?? 12000);

// ---- this table, kept on the device -------------------------------------------

// The whole table is saved here (every couple of seconds while it changes,
// and as the page goes away) and restored on load, so a table survives even
// if every device that held it reloads. A stale save merges like any other
// sync, so it can't undo newer moves. Saves are restored regardless of
// age. Starting over is an explicit New game.
let invitePayload = null, inviteError = '';
if (!solo && !home) {
  const payload = params.get('i') ?? recall(`shamrock:${roomId}:invite`);
  if (payload !== null) {
    try {
      if (payload.length > 6000) throw new Error('oversize');
      inviteError = coreText(X.load_invite, [roomId, dec.decode(b64.dec(payload))]);
      if (!inviteError) {
        invitePayload = payload;
        remember(`shamrock:${roomId}:invite`, payload);
      }
    } catch { inviteError = 'This invite is damaged. Ask for the original link.'; }
  }
}
const stateKey = `shamrock:${roomId}:state`;
let savedState = '', saveWarning = false;
function saveTable() {
  // The snapshot first: it replaces the scratch buffer, which may move.
  const s = b64.enc(scratchBytes(X.snapshot()));
  if (s !== savedState) {
    if (remember(stateKey, JSON.stringify({ at: Date.now(), v: X.protocol(), s }))) {
      savedState = s;
      saveWarning = false;
    } else if (!saveWarning) {
      toast("This browser couldn't save your game. Keep this tab open until you've connected with the others.", 20000);
      saveWarning = true;
    }
  }
}
if (!solo && !home && !inviteError) {
  const kept = readJson(stateKey, null);
  // The core checks the version and merges without any age cutoff.
  if (kept?.s) {
    try { if (X.restore(writeInbox(b64.dec(kept.s)), kept.v ?? 0)) savedState = kept.s; } catch {}
  }
  setInterval(saveTable, 2000);
  addEventListener('pagehide', saveTable);
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden') saveTable(); });
}

// Solo play (a share link) never connects to anyone.
if (!solo && !home && !inviteError) connect().then((n) => { net = n; }, (e) => toast(`Can't connect: ${e.message}`, 10000));

// Returning to this room re-takes your seat; a first visit asks. Wait a
// moment first so we've heard the table's state (and existing names).
if (inviteError) {
  $('inviteErrorText').textContent = inviteError;
  $('inviteErrorSheet').classList.add('open');
} else if (home) {
  document.body.classList.add('home');
  openHome();
} else if (solo) {
  // Just play: an anonymous local seat, straight into the shared clovers.
  document.body.classList.add('solo');
  X.set_solo();
  X.join(writeText('You'));
  if (customs) X.action(ACT.deal);
  drainOutbox();
} else {
  const saved = params.has('fresh') ? null : recall(nameKey);
  if (saved) noteTable(saved);
  if (saved && (!invitePayload || sheet('join').rows.some((p) => p.name === saved))) setTimeout(() => joinAs(saved), 1500);
  else if (saved && invitePayload) setTimeout(openJoin, 900);
  else if (saved === null) setTimeout(() => { if (!ui().seated) openJoin(); }, 900);
}
// Just split off to this fresh table: say where we are and offer the invite.
try {
  if (!home && sessionStorage.getItem('shamrock:fresh') === roomId) {
    sessionStorage.removeItem('shamrock:fresh');
    setTimeout(() => toast(`Your game is here at #${roomId}. Send the link to whoever should join.`, 15000, ['Invite', () => $('invite').onclick()]), 1700);
  }
} catch {}

// ---- frame loop ---------------------------------------------------------------

// The core draws every frame, but the page paints only what changed, and
// once nothing has for half a second, frames stop: a 4 Hz tick keeps the
// table's timers (presence, auto-start, pointers fading) going, and wakes
// frames again the moment it draws something new. Input and messages wake
// them too (through `X`). The tick also runs in background tabs, where
// frames don't.
let quiet = 0, lastStep = 0, painted = '';
function step(t) {
  lastStep = performance.now();
  const t0 = lastStep;
  const n = X.frame(t);
  // Canvas size, zoom camera and fonts change the picture without the core.
  const shell = `${canvas.width}x${canvas.height} ${cam.s} ${cam.x} ${cam.y} ${document.fonts?.status}`;
  const fresh = !X.unchanged() || shell !== painted;
  if (fresh) {
    paint(n);
    painted = shell;
    stats.paints++;
    if (stats.frameMs.push(performance.now() - t0) > 600) stats.frameMs.shift();
  }
  quiet = fresh ? 0 : quiet + 1;
  drainOutbox();
  // The page's own controls follow too (it only touches what changed).
  renderUi();
  return fresh;
}
function tick(t) {
  framePending = false;
  step(t);
  if (quiet < 30) wake();
}
function wake() {
  if (framePending || !framesStarted) return;
  framePending = true;
  requestAnimationFrame(tick);
}
setInterval(() => {
  if (performance.now() - lastStep < 200) return;
  if (step(performance.now())) wake();
}, 250);
renderUi(true);
framesStarted = true;
wake();

// ---- harness hooks (read-only probes + a settle switch) ------------------------

const probe = (fn, ...a) => { fn(...a); return Array.from(new Float32Array(X.memory.buffer, X.probe_ptr(), 3)); };

// ---- tools for AI assistants (WebMCP; see agent.js) ----------------------------

const tools = agentTools({
  X, ui, act, joinAs, writeText, probe,
  agentView: () => scratchJson(X.agent_view()),
  review: () => scratchJson(X.review()),
  status: () => $('hint').textContent,
  sync: () => { drainOutbox(); renderUi(true); },
  trayPoint: () => [canvas.clientWidth / 2, canvas.clientHeight - 60],
});
if (!home) offerTools(tools);

window.__cl = {
  chaos,
  holds: () => scratchJson(X.holds()),
  tools: Object.fromEntries(tools.map((t) => [t.name, t.run])),
  toolSpecs: () => tools.map(({ run, ...spec }) => spec),
  agentView: () => scratchJson(X.agent_view()),
  myTables: () => myTables(),
  setTheme: (t) => { themePref = t; applyTheme(); },
  peerCount: () => peers.size,
  camera: () => ({ ...cam }),
  device, X, stats, peers, ui, joinAs, watch,
  digest: () => scratchJson(X.digest()),
  card: (i) => probe(X.probe_card, i),
  slot: (s) => probe(X.probe_slot, s),
  board: (x, y) => probe(X.probe_board, x, y),
  petal: (d) => probe(X.probe_petal, d),
  hub: () => probe(X.probe_hub),
  shownAngle: (i) => X.probe_shown_angle(i),
  quarter: () => X.view_quarter(),
  wordPx: () => X.probe_word_px(),
  cardPx: () => X.probe_card_px(),
  zoom: () => zoom,
  settle: () => X.settle(),
  answerRot: (s) => X.answer_rot(s),
  palette: () => scratchJson(X.palette()),
  shareFromText,
  unseal,
  wordFit: () => wordsMeasured && scratchJson(X.word_fit()),
  wordScales: (card) => scratchJson(X.card_word_scales(writeText(card.join('|')))),
  wordSize: () => X.word_size(),
  measure: async (words, size = X.word_size()) => {
    await document.fonts.load(`700 ${size}px Figtree`);
    const c = document.createElement('canvas').getContext('2d');
    c.font = FONTS[1](size);
    return words.map((w) => [w, c.measureText(w).width]);
  },
};
