//! Raw C-ABI exports for the browser shim. No wasm-bindgen: JS reads the
//! command buffer / outbox straight out of linear memory and writes inbound
//! bytes and text into `inbox`.

pub mod app;
pub mod json;
pub mod invite;
pub mod model;
pub mod pages;
pub mod words;

use app::App;

struct Globals {
    app: App,
    inbox: Vec<u8>,
    scratch: Vec<u8>,
    preview: (Vec<f32>, Vec<u8>),
    probe: [f32; 4],
}

static mut G: Option<Globals> = None;

fn g() -> &'static mut Globals {
    // Single-threaded wasm; the shim never re-enters.
    unsafe { (*core::ptr::addr_of_mut!(G)).as_mut().expect("init not called") }
}

fn inbox_str(len: u32) -> String {
    let ib = &g().inbox;
    String::from_utf8_lossy(&ib[..(len as usize).min(ib.len())]).into_owned()
}

fn to_scratch(s: String) -> u32 {
    g().scratch = s.into_bytes();
    g().scratch.len() as u32
}

/// Returns this device's id.
#[no_mangle]
pub extern "C" fn init(seed_lo: u32, seed_hi: u32) -> u32 {
    let app = App::new((seed_hi as u64) << 32 | seed_lo as u64);
    let id = app.device;
    unsafe {
        *core::ptr::addr_of_mut!(G) = Some(Globals { app, inbox: Vec::new(), scratch: Vec::new(), preview: (Vec::new(), Vec::new()), probe: [0.0; 4] });
    }
    id
}

#[no_mangle]
pub extern "C" fn resize(w: f32, h: f32, dpr: f32) {
    let a = &mut g().app;
    (a.w, a.h, a.dpr) = (w.max(1.0), h.max(1.0), dpr.max(0.5));
}

/// Advances animation, flushes network, renders. Returns command count.
#[no_mangle]
pub extern "C" fn frame(now: f64) -> u32 {
    g().app.frame(now);
    g().app.cmds.len() as u32
}

/// 1 if the last `frame` drew exactly what the one before it did.
#[no_mangle]
pub extern "C" fn unchanged() -> u32 {
    g().app.unchanged as u32
}

#[no_mangle]
pub extern "C" fn cmd_ptr() -> *const f32 {
    g().app.cmds.as_ptr()
}

#[no_mangle]
pub extern "C" fn str_ptr() -> *const u8 {
    g().app.strs.as_ptr()
}

/// A read-only clover view (JSON in inbox) drawn with the gameplay renderer.
/// Separate buffers keep the live board and its frame cache untouched.
#[no_mangle]
pub extern "C" fn clover_preview(len: u32) -> u32 {
    let Some(view) = json::Json::parse(&inbox_str(len)) else { return 0 };
    g().preview = g().app.clover_preview(&view);
    g().preview.0.len() as u32
}

#[no_mangle]
pub extern "C" fn preview_cmd_ptr() -> *const f32 {
    g().preview.0.as_ptr()
}

#[no_mangle]
pub extern "C" fn preview_str_ptr() -> *const u8 {
    g().preview.1.as_ptr()
}

#[no_mangle]
pub extern "C" fn outbox_ptr() -> *const u8 {
    g().app.outbox.as_ptr()
}

#[no_mangle]
pub extern "C" fn outbox_len() -> u32 {
    g().app.outbox.len() as u32
}

#[no_mangle]
pub extern "C" fn outbox_clear() {
    g().app.outbox.clear();
}

/// Returns a buffer of at least `len` bytes for JS to fill before a call
/// that consumes the inbox.
#[no_mangle]
pub extern "C" fn inbox_reserve(len: u32) -> *mut u8 {
    let ib = &mut g().inbox;
    ib.resize(len as usize, 0);
    ib.as_mut_ptr()
}

/// Inbound packet (in the inbox) from the peer on connection `link`.
#[no_mangle]
pub extern "C" fn on_message(len: u32, link: u32) {
    let bytes = core::mem::take(&mut g().inbox);
    g().app.on_packet(&bytes[..(len as usize).min(bytes.len())], link);
    g().inbox = bytes;
}

/// How many connections to other devices are open (for the status strip).
#[no_mangle]
pub extern "C" fn set_links(n: u32) {
    g().app.links = n;
    if n > 0 {
        g().app.connection_failed = false;
    }
}

/// A discovered peer could not connect (as distinct from nobody being here).
#[no_mangle]
pub extern "C" fn set_connection_failed(failed: u32) {
    g().app.connection_failed = failed != 0 && g().app.links == 0;
}

/// Solo play from a share link (no table, no connections).
#[no_mangle]
pub extern "C" fn set_solo() {
    g().app.solo = true;
}

/// Restore a saved table from this protocol version. Saves do not expire.
/// Returns 1 if it was used.
#[no_mangle]
pub extern "C" fn restore(len: u32, version: u32) -> u32 {
    let bytes = core::mem::take(&mut g().inbox);
    let ok = g().app.restore(&bytes[..(len as usize).min(bytes.len())], version);
    g().inbox = bytes;
    ok as u32
}

#[no_mangle]
pub extern "C" fn create_invite(len: u32) -> u32 {
    let input = inbox_str(len);
    let result = g().app.create_invite(&input);
    to_scratch(result)
}

/// Room and decoded invite JSON, separated by U+001F. An empty result
/// means success; otherwise the core returns the explanation for the page.
#[no_mangle]
pub extern "C" fn load_invite(len: u32) -> u32 {
    let input = inbox_str(len);
    let (room, text) = input.split_once('\u{1f}').unwrap_or(("", ""));
    let result = g().app.load_invite(room, text).err().unwrap_or_default();
    to_scratch(result)
}

#[no_mangle]
pub extern "C" fn on_link_lost(link: u32) {
    g().app.on_link_lost(link);
}

#[no_mangle]
pub extern "C" fn on_peer_join() {
    g().app.on_peer_join();
}

#[no_mangle]
pub extern "C" fn pointer_down(x: f32, y: f32) {
    g().app.pointer_down(x, y);
}

#[no_mangle]
pub extern "C" fn pointer_move(x: f32, y: f32) {
    g().app.pointer_move(x, y);
}

#[no_mangle]
pub extern "C" fn pointer_up(x: f32, y: f32) {
    g().app.pointer_up(x, y);
}

/// 1 if a press at (x, y) CSS px would pick up a card.
#[no_mangle]
pub extern "C" fn on_card(x: f32, y: f32) -> u32 {
    g().app.on_card(x, y) as u32
}

/// A gesture the browser took over (pinch zoom): undo it, act on nothing.
#[no_mangle]
pub extern "C" fn pointer_cancel() {
    g().app.pointer_cancel();
}

#[no_mangle]
pub extern "C" fn rotate_view(dir: i32) {
    g().app.rotate_view(dir);
}

#[no_mangle]
pub extern "C" fn action(code: u32) {
    g().app.action(code);
}

/// Name is in the inbox. Joining with an existing name takes that seat over.
#[no_mangle]
pub extern "C" fn join(len: u32) {
    let s = inbox_str(len);
    g().app.join(&s);
}

#[no_mangle]
pub extern "C" fn watch() {
    g().app.watch();
}

#[no_mangle]
pub extern "C" fn remove_seat(id: u32) {
    g().app.remove_seat(id);
}

/// Jump to clover `k` while guessing (shared: everyone follows).
#[no_mangle]
pub extern "C" fn jump(k: u32) {
    g().app.jump(k as usize);
}

/// Theme for this device's drawing: 1 dark, 0 light.
#[no_mangle]
pub extern "C" fn set_theme(dark: u32) {
    g().app.set_theme(dark != 0);
}

/// Card zoom as drawn now: the chosen size, capped so tray cards fit.
#[no_mangle]
pub extern "C" fn card_zoom() -> f32 {
    g().app.card_zoom()
}

/// Local card size (display only). Returns the clamped value.
#[no_mangle]
pub extern "C" fn set_zoom(z: f32) -> f32 {
    g().app.set_zoom(z);
    g().app.zoom
}

/// 0 = reduced motion (no deal-in or glow).
#[no_mangle]
pub extern "C" fn set_motion(on: u32) {
    g().app.motion = on != 0;
}

/// Text/background colour pairs used on the canvas (JSON) → scratch.
#[no_mangle]
pub extern "C" fn palette() -> u32 {
    to_scratch(g().app.palette_json())
}

/// Boards given in answer order (inbox text) → scrambled form → scratch.
#[no_mangle]
pub extern "C" fn scramble_boards(len: u32) -> u32 {
    let t = inbox_str(len);
    to_scratch(g().app.scramble_boards(&t))
}

/// Shareable (scrambled) text for this round's clovers selected by `mask` → scratch.
#[no_mangle]
pub extern "C" fn export_boards(mask: u32) -> u32 {
    to_scratch(g().app.export_boards(mask))
}

/// This round's clovers for the game log (see `App::review_json`).
#[no_mangle]
pub extern "C" fn review() -> u32 {
    to_scratch(g().app.review_json())
}

/// An imported clover (text in the inbox; see `model::Custom::load`).
/// Returns 1 if accepted.
#[no_mangle]
pub extern "C" fn load_custom(len: u32) -> u32 {
    let ok = model::Custom::load(&inbox_str(len));
    g().app.custom_room = ok;
    ok as u32
}

#[no_mangle]
pub extern "C" fn set_decoys(n: u32) {
    g().app.set_decoys(n.min(255) as u8);
}

/// Clue text is in the inbox. `advance` != 0 opens the next empty leaf.
#[no_mangle]
pub extern "C" fn set_clue(idx: u32, len: u32, advance: u32) {
    let s = inbox_str(len);
    g().app.set_clue(idx as u8, &s, advance != 0);
}

/// Re-send the clue editor geometry for leaf `d` (after a resize).
#[no_mangle]
pub extern "C" fn edit_clue(d: u32) {
    g().app.edit_clue((d % 4) as u8);
}

/// UI state (JSON) → scratch; returns its length.
#[no_mangle]
pub extern "C" fn ui() -> u32 {
    to_scratch(g().app.ui_json())
}

/// Canonical shared-state digest (JSON) → scratch; returns its length.
#[no_mangle]
pub extern "C" fn digest() -> u32 {
    to_scratch(g().app.shared.digest())
}

#[no_mangle]
pub extern "C" fn scratch_ptr() -> *const u8 {
    g().scratch.as_ptr()
}

// ---- harness probes --------------------------------------------------------

#[no_mangle]
pub extern "C" fn settle() {
    g().app.settle();
}

#[no_mangle]
pub extern "C" fn view_quarter() -> i32 {
    g().app.q
}

#[no_mangle]
pub extern "C" fn probe_ptr() -> *const f32 {
    g().probe.as_ptr()
}

fn probe2((x, y): (f32, f32)) {
    g().probe = [x, y, 0.0, 0.0];
}

#[no_mangle]
pub extern "C" fn probe_card(i: u32) {
    let (x, y, a) = g().app.card_screen(i as usize);
    g().probe = [x, y, a, 0.0];
}

#[no_mangle]
pub extern "C" fn probe_slot(s: u32) {
    probe2(g().app.slot_screen((s as usize).min(model::NSLOTS - 1)));
}

#[no_mangle]
pub extern "C" fn probe_board(x: f32, y: f32) {
    probe2(g().app.board_screen(x, y));
}

#[no_mangle]
pub extern "C" fn probe_petal(d: u32) {
    probe2(g().app.petal_screen((d % 4) as u8));
}

#[no_mangle]
pub extern "C" fn probe_hub() {
    probe2(g().app.hub_screen());
}

/// How many cards are shaking (booted by a guess) right now.
#[no_mangle]
pub extern "C" fn probe_shaking() -> u32 {
    g().app.shaking() as u32
}

#[no_mangle]
pub extern "C" fn probe_shown_angle(i: u32) -> f32 {
    g().app.shown_angle(i as usize)
}

/// Widest a card word may be (card units) before it would crowd a corner.
/// The whole table, to keep on this device (see `App::snapshot`); feed it
/// back with `on_message` after a reload.
#[no_mangle]
pub extern "C" fn snapshot() -> u32 {
    let b = g().app.snapshot();
    g().scratch = b;
    g().scratch.len() as u32
}

/// Move this device's group to the room named in the inbox (see
/// `App::record_move`). Returns the group's table to seed the new room with
/// (in scratch), or 0 if this device didn't come from another table.
#[no_mangle]
pub extern "C" fn record_move(len: u32) -> u32 {
    let room = inbox_str(len);
    match g().app.record_move(&room) {
        Some(b) => {
            g().scratch = b;
            g().scratch.len() as u32
        }
        None => 0,
    }
}

/// The table this device came from, to follow its group's move (scratch), or 0.
#[no_mangle]
pub extern "C" fn origin_snapshot() -> u32 {
    match g().app.origin_snapshot() {
        Some(b) => {
            g().scratch = b;
            g().scratch.len() as u32
        }
        None => 0,
    }
}

/// The message format version (saves are kept only for the same one).
#[no_mangle]
pub extern "C" fn protocol() -> u32 {
    crate::model::PROTOCOL as u32
}

/// Cards other players are dragging right now, as JSON {card: seat} (tests).
#[no_mangle]
pub extern "C" fn holds() -> u32 {
    let h = g().app.holds();
    to_scratch(format!("{{{}}}", h.iter().map(|(i, s)| format!("\"{i}\":{s}")).collect::<Vec<_>>().join(",")))
}

/// The current clover for an automated player (see `App::agent_view_json`).
#[no_mangle]
pub extern "C" fn agent_view() -> u32 {
    to_scratch(g().app.agent_view_json())
}

/// Card word size, board units.
#[no_mangle]
pub extern "C" fn word_size() -> f32 {
    app::WORD_SIZE
}

/// Every card word (deck and imported clovers), newline-separated, for the
/// shell to measure in the card font.
#[no_mangle]
pub extern "C" fn word_list() -> u32 {
    let mut ws: Vec<&str> = words::CARDS.iter().flatten().copied().collect();
    ws.extend(model::custom_words());
    ws.sort_unstable();
    ws.dedup();
    to_scratch(ws.join("\n"))
}

/// Measured card-word widths (`word\twidth` lines, at size 100; in the inbox).
#[no_mangle]
pub extern "C" fn set_word_widths(len: u32) {
    let s = inbox_str(len);
    g().app.set_word_widths(&s);
}

/// Sizes the layout gives a card's four words (`a|b|c|d` in the inbox),
/// as a JSON array of fractions of full size.
#[no_mangle]
pub extern "C" fn card_word_scales(len: u32) -> u32 {
    let s = inbox_str(len);
    let w: Vec<&str> = s.split('|').collect();
    if w.len() != 4 {
        return to_scratch("[]".into());
    }
    let sizes = g().app.card_word_sizes(&[w[0], w[1], w[2], w[3]]);
    to_scratch(format!("[{}]", sizes.map(|x| (x / app::WORD_SIZE).to_string()).join(",")))
}

/// Over the deck: (share of words at full size, smallest scale), as JSON.
#[no_mangle]
pub extern "C" fn word_fit() -> u32 {
    let (full, near, min) = g().app.word_fit();
    to_scratch(format!("{{\"full\":{full},\"near\":{near},\"min\":{min}}}"))
}

/// Rendered size of card words, in CSS px.
#[no_mangle]
pub extern "C" fn probe_word_px() -> f32 {
    app::WORD_SIZE * g().app.scale() * g().app.card_zoom()
}

/// Rendered card edge length, in CSS px.
#[no_mangle]
pub extern "C" fn probe_card_px() -> f32 {
    model::CARD * g().app.scale() * g().app.card_zoom()
}

/// Harness only: correct rotation for spot `s` of the clover being guessed
/// (255 if none). Submissions are shared state, so this is no secret from
/// devtools anyway; the game is played on trust.
#[no_mangle]
pub extern "C" fn answer_rot(s: u32) -> u32 {
    let a = &g().app;
    match a.scene().writer {
        Some(w) if matches!(a.shared.phase(), model::Phase::Guessing(_)) => {
            a.shared.deal(w).sol_rot[(s as usize) % model::NSLOTS] as u32
        }
        _ => 255,
    }
}

// ---- what the page shows ----------------------------------------------------

/// A sheet's content (`app::SHEET_*`), as JSON.
#[no_mangle]
pub extern "C" fn sheet(kind: u32) -> u32 {
    to_scratch(g().app.sheet_json(kind))
}

/// Parts of the inbox text, separated by U+001F.
fn inbox_parts(len: u32) -> Vec<String> {
    inbox_str(len).split('\u{1f}').map(String::from).collect()
}

/// Past games (inbox: stored log, U+001F, an EV_KEEP payload, U+001F, room)
/// with that round kept; the new log to store. Solo play keeps nothing.
#[no_mangle]
pub extern "C" fn keep_round(len: u32, now: f64) -> u32 {
    let p = inbox_parts(len);
    let log = p.first().cloned().unwrap_or_default();
    if g().app.solo || p.len() < 3 {
        return to_scratch(log);
    }
    to_scratch(pages::keep_into(&log, &p[1], &p[2], now))
}

/// Past games as listed (inbox: the stored log).
#[no_mangle]
pub extern "C" fn past_games(len: u32) -> u32 {
    to_scratch(pages::past_games_view(&inbox_str(len)))
}

/// "What happened" with a notice added (inbox: stored list, U+001F, notice JSON).
#[no_mangle]
pub extern "C" fn happened_add(len: u32, now: f64) -> u32 {
    let p = inbox_parts(len);
    to_scratch(pages::happened_add(p.first().map_or("", |x| x), p.get(1).map_or("", |x| x), now))
}

/// This device's tables as listed (inbox: room → {name, at}).
#[no_mangle]
pub extern "C" fn tables(len: u32) -> u32 {
    to_scratch(pages::tables_view(&inbox_str(len)))
}

/// A typed table name made link-safe (inbox: the typed name).
#[no_mangle]
pub extern "C" fn norm_room(len: u32) -> u32 {
    to_scratch(pages::norm_room(&inbox_str(len)))
}

/// A suggested table name, from a random number.
#[no_mangle]
pub extern "C" fn suggest_room(random: u32) -> u32 {
    to_scratch(pages::suggest_room(random))
}
