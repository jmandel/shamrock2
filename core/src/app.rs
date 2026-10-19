//! Local (per-device) state: identity, presence, view rotation, layout,
//! pointer interaction, animation and rendering. Rendering emits a flat f32
//! command buffer that the JS shim replays onto a Canvas2D context.

use crate::model::*;
use core::f32::consts::{FRAC_PI_2, PI, TAU};
use std::collections::{BTreeMap, BTreeSet};

mod page;
pub use page::*;

// Outbox event kinds (to JS).
pub const EV_NET: u8 = 1;
pub const EV_EDIT_CLUE: u8 = 2;
pub const EV_STATUS: u8 = 3;
/// The round changed under this device (not by its own hand): JSON
/// {kind, by, n, dealtIn, names, text, action}; kind is next | missed |
/// crossed | newGame | otherTable, text the sentence to show, action null or
/// {kind, label} for its button. One per change, for the notice and history.
pub const EV_NOTICE: u8 = 4;
/// A round to keep (or update) in this device's Past games, by `key`: JSON
/// {key, n, finished, players, boards}. Raised when a round with anything in
/// it ends or stops being current, for any reason, and again when a late
/// change reaches it.
pub const EV_KEEP: u8 = 5;
/// Someone moved this device's group (the table it came from) to another
/// room: JSON {room, by, text, action}. Raised once.
pub const EV_MOVED: u8 = 6;
/// A peer speaks a newer protocol: this page should reload. Raised once.
pub const EV_OUTDATED: u8 = 7;

// Draw commands.
const C_XFORM: f32 = 1.0;
const C_RRECT: f32 = 2.0;
const C_CIRCLE: f32 = 3.0;
const C_TEXT: f32 = 4.0;
const C_CLEAR: f32 = 5.0;
/// Heart-shaped clover leaf: tip at the origin, lobes toward -y.
const C_LEAF: f32 = 6.0;

// Text faces (the shim maps these to fonts).
const F_SANS_BOLD: u8 = 1;
const F_SERIF: u8 = 2;

// Palettes, one per theme. Every text/background pair is checked for WCAG AA
// contrast in both by the harness (see `palette_json`).
#[derive(Debug)]
pub struct Palette {
    pub bg: u32,
    pub tray: u32,
    pub tray_edge: u32,
    pub leaf: u32,
    pub leaf_active: u32,
    pub leaf_glow: u32,
    pub leaf_edge: u32,
    pub centre: u32,
    pub centre_edge: u32,
    pub paper: u32,
    pub paper_edge: u32,
    pub ink: u32,
    pub clue_ink: u32,
    pub keyword_ink: u32,
    pub hub: u32,
    pub gold: u32,
    /// The mark on a placement a check already rejected.
    pub tried: u32,
}

/// Daylight: a pale sage table, white cards, the clover as before.
pub const LIGHT: Palette = Palette {
    bg: 0xe6eee2ff,
    tray: 0xd9e4d4ff,
    tray_edge: 0xb9cdb3ff,
    leaf: 0xa8dbb0ff,
    leaf_active: 0xd6f1daff,
    leaf_glow: 0xe9f8cfff,
    leaf_edge: 0x4f8a5eff,
    centre: 0x2e6b45ff,
    centre_edge: 0x24573aff,
    paper: 0xffffffff,
    paper_edge: 0xc9bf9fff,
    ink: 0x1c1710ff,
    clue_ink: 0x0b2414ff,
    keyword_ink: 0x1d4a2bff,
    hub: 0x173a27ff,
    gold: 0xd99a1eff,
    tried: 0xa63d2cff,
};

/// Night: a near-neutral charcoal table, muted leaves, and cards dimmed a
/// little so they don't glare.
pub const DARK: Palette = Palette {
    bg: 0x111613ff,
    tray: 0x18201bff,
    tray_edge: 0x2a362eff,
    leaf: 0x86b38fff,
    leaf_active: 0xa9cdb0ff,
    leaf_glow: 0xcfe6b8ff,
    leaf_edge: 0x2c4a35ff,
    centre: 0x22432fff,
    centre_edge: 0x2f5a3fff,
    paper: 0xe8e3d5ff,
    paper_edge: 0x8f876eff,
    ink: 0x1c1710ff,
    clue_ink: 0x0b2414ff,
    keyword_ink: 0x10301bff,
    hub: 0x0e1f15ff,
    gold: 0xf2c14eff,
    tried: 0xa63d2cff,
};

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |s: u32| -> u32 {
        let (x, y) = (((a >> s) & 255) as f32, ((b >> s) & 255) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

const DRAG_THRESHOLD: f32 = 6.0;
/// A released board spin coasts this long at its last speed before settling.
const FLING_S: f32 = 0.18;
const FLING_WINDOW_MS: f64 = 120.0;
const CURSOR_TTL_MS: f64 = 4000.0;
const HELLO_EVERY_MS: f64 = 2000.0;
/// At most one full resync per this long when tables drift apart.
const RESYNC_EVERY_MS: f64 = 5000.0;
/// Card word size, board units (a card is 210 across). Words are laid out
/// per card from their measured widths (the shell measures every word in the
/// card font, see `set_word_widths`): a word may run into a corner whenever
/// the words on both neighbouring edges are short enough to stay clear of it,
/// so ~90% of the deck renders at full size; the rest shrink evenly to fit.
pub const WORD_SIZE: f32 = 35.0;
/// Gap between a card's edge and the top of its words' letters.
const WORD_EDGE: f32 = 3.0;
/// Figtree Bold, per unit of size, around the text's centre line (measured):
/// the tallest letters reach 0.45 above it, and ascender-to-descender ink
/// spans 0.96.
const WORD_ASC: f32 = 0.45;
const WORD_INK: f32 = 0.96;
/// How far along its edge a word may reach from the centre: the card's
/// rounded corner (radius 18) at the letters' top.
const WORD_REACH: f32 = 96.0;
/// A card word's ink at size 100: its width, and for each letter where it
/// sits along the word (from the word's left) and how far it descends below
/// the centre line.
#[derive(Clone, Debug)]
pub struct WordInk {
    pub width: f32,
    pub letters: Vec<(f32, f32, f32)>,
}

/// Clear space kept between a word's end and the letters of the word on the
/// neighbouring edge (so they never read as touching).
const WORD_GAP: f32 = 6.0;
/// Room for a word that stays out of both corners, which are the squares
/// where the neighbouring edges' words sit.
pub const WORD_SAFE_W: f32 = 2.0 * (HALF - WORD_EDGE - WORD_INK * WORD_SIZE - WORD_GAP);
/// Tray depth (short axis) in board units: two rows of cards plus air.
/// Clue size on a leaf, board units: as big as the band between the centre
/// circle (300) and the notch between the lobes (400) allows. Longer clues
/// shrink evenly to fit the leaf's width (see the shell's text command).
const CLUE_SIZE: f32 = 74.0;

/// Gap between loose cards in the tray, CSS px.
const TRAY_GAP: f32 = 4.0;
/// Gap between the tray's edge and its cards, CSS px.
const TRAY_INSET: f32 = 2.0;
/// Screen margin, CSS px: just enough that nothing sits flush on the bezel.
const PAD: f32 = 3.0;
/// Per-device card size range. Up to `ZOOM_TOUCH` cards in neighbouring
/// spots don't overlap; beyond it they may, if a player wants them that big.
pub const ZOOM_MIN: f32 = 0.7;
pub const ZOOM_TOUCH: f32 = 2.0 * SLOT_OFF / CARD;
pub const ZOOM_MAX: f32 = 1.6;

/// 2D affine matrix in canvas `setTransform(a, b, c, d, e, f)` order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct M {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl M {
    pub const ID: M = M { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };
    pub fn t(x: f32, y: f32) -> M {
        M { e: x, f: y, ..M::ID }
    }
    /// Clockwise on screen (y points down).
    pub fn r(th: f32) -> M {
        let (s, c) = th.sin_cos();
        M { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }
    pub fn s(k: f32) -> M {
        M { a: k, d: k, ..M::ID }
    }
    /// self ∘ o (apply `o` first).
    pub fn then(self, o: M) -> M {
        M {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }
    pub fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }
    pub fn inv(self) -> M {
        let det = self.a * self.d - self.b * self.c;
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        M { a, b, c, d, e: -(a * self.e + c * self.f), f: -(b * self.e + d * self.f) }
    }
    pub fn angle(self) -> f32 {
        self.b.atan2(self.a)
    }
}

fn quarter(q: i32) -> u8 {
    q.rem_euclid(4) as u8
}

fn turn(r: u8) -> f32 {
    r as f32 * FRAC_PI_2
}

/// Shortest signed difference between two angles.
fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

const PALETTE: [u32; 8] =
    [0xe0457bff, 0x2f80edff, 0xf2994aff, 0x9b51e0ff, 0x27ae60ff, 0x00a3a3ff, 0xd4a017ff, 0xeb5757ff];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Target {
    Card(usize),
    /// A card another player's hand is holding: the press does nothing.
    Held(usize),
    Petal(u8),
    Hub,
    Board,
    Empty,
}

struct Press {
    sx: f32,
    sy: f32,
    target: Target,
    /// Card centre minus pointer, in CSS px.
    grab: (f32, f32),
    /// Where the dragged card started (a cancelled drag puts it back).
    origin: Option<Card>,
    moved: bool,
    /// (pointer angle about the board centre, view angle) while spinning.
    spin: Option<(f32, f32)>,
    /// Recent (time ms, view angle) samples of a spin, for its fling.
    trail: Vec<(f64, f32)>,
}

pub struct RemoteCursor {
    pub seat: u32,
    pub kind: u8,
    pub x: f32,
    pub y: f32,
    /// The card it is dragging, if any.
    pub card: Option<u8>,
    pub seen: f64,
}

/// Where things go on this screen.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// CSS px per board unit.
    pub k: f32,
    /// Board centre, CSS px.
    pub bc: (f32, f32),
    /// Tray rect (x, y, w, h) and whether its long axis is horizontal.
    pub tray: Option<(f32, f32, f32, f32, bool)>,
    /// Card zoom actually drawn: this device's card size, capped so three
    /// cards always fit across the tray.
    pub z: f32,
}

/// Displayed (animated) pose of a card: in board units when on the board,
/// in tray coords otherwise; angle in radians.
#[derive(Clone, Copy, Debug)]
struct Pose {
    board: bool,
    x: f32,
    y: f32,
    a: f32,
    /// Scale (deal-in grows cards from small).
    s: f32,
    /// Seconds left before this card starts moving (deal-in stagger).
    wait: f32,
}

/// What the board shows right now, for this device.
pub struct Scene {
    pub writer: Option<u32>,
    pub cards: Vec<Card>,
    pub claim: [Option<usize>; NSLOTS],
    pub words: Vec<[&'static str; 4]>,
    pub clues: [String; NSLOTS],
    pub movable: Vec<bool>,
    pub locked: u8,
}

pub struct App {
    /// Card-word metrics at size 100 in the card font (see `WordInk`).
    word_w: std::collections::HashMap<String, WordInk>,
    /// Solved card-word layouts, keyed by the card's words.
    word_layouts: std::cell::RefCell<std::collections::HashMap<String, [(f32, f32); 4]>>,
    /// Colours for the current theme (display only, per device).
    pub pal: &'static Palette,
    pub shared: Shared,
    /// The immutable starting deal carried by this room's group invite.
    /// Local metadata: all shared state still uses ordinary round/seat ops.
    pub invitation: Option<crate::invite::Invite>,
    /// This device (Lamport tie-break, presence).
    pub device: u32,
    /// The seat this device plays, if any (None = watching).
    pub seat: Option<u32>,
    /// A late writer can help guess, then return to their unfinished clover.
    guessing_with_group: bool,
    clock: u64,
    rng: Rng,
    /// Target view rotation in quarter turns (local only).
    pub q: i32,
    /// Animated view angle in radians.
    pub angle: f32,
    pub w: f32,
    pub h: f32,
    pub dpr: f32,
    press: Option<Press>,
    editing: Option<u8>,
    /// Device → (seat, link it arrives over). Presence follows the
    /// connection, not a timer, so a backgrounded tab (no frames, no
    /// heartbeats) still counts as here until its connection actually drops.
    pub peers: BTreeMap<u32, (u32, u32)>,
    pub cursors: BTreeMap<u32, RemoteCursor>,
    my_cursor: (u8, f32, f32),
    cursor_dirty: bool,
    last_cursor_ms: f64,
    last_hello_ms: f64,
    /// Peers whose table hash differed from ours: device → (theirs, ours).
    drift: BTreeMap<u32, (u32, u32)>,
    /// The table as it was just before the round started changing (set by
    /// `begin_change`, consumed by `finish_change`).
    before_change: Option<Shared>,
    /// The last table this device was pushed out of, kept so late changes
    /// to its round still land (and update Past games).
    pub aside: Option<Shared>,
    /// The table this device came from when it last joined another table in
    /// this room: its group, for moving (`record_move`, `EV_MOVED`).
    pub origin: Option<Shared>,
    move_prompted: bool,
    /// A peer speaks a newer protocol: stop sending table changes.
    pub outdated: bool,
    /// Late changes merged into a replaced round (tests' coverage).
    pub late_merges: u32,
    last_resync_ms: f64,
    dirty_cards: BTreeSet<(u32, u8)>,
    last_phase: Phase,
    poses: BTreeMap<(u32, u8), Pose>,
    dt: f32,
    /// Local card size multiplier (display only; positions are shared).
    pub zoom: f32,
    /// Decorative motion on (off for prefers-reduced-motion).
    pub motion: bool,
    /// This room has an imported clover to open with (see `Custom`).
    pub custom_room: bool,
    /// Solo play from a share link: no table, no connections.
    pub solo: bool,
    /// Connections open to other devices right now (told by the shell).
    pub links: u32,
    /// Discovery found another device, but its connection attempt failed.
    /// Local transport status only; never saved or sent to other players.
    pub connection_failed: bool,
    /// Soft glow on the leaves after a clover is completed (1 → 0).
    glow: f32,
    last_done: Option<(u32, bool)>,
    /// (writer, guesses) last seen, to spot a guess made anywhere.
    last_guess: Option<(u32, u8)>,
    /// Cards booted by the last guess: when their shake started.
    shakes: BTreeMap<(u32, u8), f64>,
    /// The last card I couldn't take because someone else held it: (card,
    /// their seat, when), to pulse it in their colour.
    refused: Option<(usize, u32, f64)>,
    solved_at: f64,
    pub now: f64,
    pub outbox: Vec<u8>,
    pub cmds: Vec<f32>,
    pub strs: Vec<u8>,
    /// The last frame's commands and strings, to tell whether this one
    /// draws anything different (`unchanged`).
    last_cmds: Vec<f32>,
    last_strs: Vec<u8>,
    /// This frame's commands are the same as the last frame's: there is
    /// nothing new to paint.
    pub unchanged: bool,
}

impl App {
    pub fn new(seed: u64) -> App {
        let mut rng = Rng::new(seed);
        let device = (rng.next() as u32).max(1);
        App {
            shared: Shared::default(),
            invitation: None,
            device,
            seat: None,
            guessing_with_group: false,
            clock: 0,
            rng,
            q: 0,
            angle: 0.0,
            w: 800.0,
            h: 1000.0,
            dpr: 1.0,
            press: None,
            editing: None,
            peers: BTreeMap::new(),
            cursors: BTreeMap::new(),
            my_cursor: (0, 0.0, 0.0),
            cursor_dirty: false,
            last_cursor_ms: 0.0,
            last_hello_ms: f64::NEG_INFINITY,
            drift: BTreeMap::new(),
            before_change: None,
            aside: None,
            origin: None,
            move_prompted: false,
            outdated: false,
            late_merges: 0,
            last_resync_ms: f64::NEG_INFINITY,
            dirty_cards: BTreeSet::new(),
            last_phase: Phase::Lobby,
            poses: BTreeMap::new(),
            dt: 0.0,
            zoom: 1.0,
            motion: true,
            custom_room: false,
            solo: false,
            links: 0,
            connection_failed: false,
            glow: 0.0,
            last_done: None,
            last_guess: None,
            shakes: BTreeMap::new(),
            refused: None,
            pal: &DARK,
            word_w: Default::default(),
            word_layouts: Default::default(),
            solved_at: f64::NEG_INFINITY,
            now: 0.0,
            outbox: Vec::new(),
            cmds: Vec::new(),
            strs: Vec::new(),
            last_cmds: Vec::new(),
            last_strs: Vec::new(),
            unchanged: false,
        }
    }

    fn stamp(&mut self) -> Stamp {
        self.clock += 1;
        Stamp { clock: self.clock, peer: self.device }
    }

    /// Prepare one invite for the whole group; no game is changed here.
    pub fn create_invite(&mut self, input: &str) -> String {
        let stamp = self.stamp();
        let seed = self.rng.next() as u32;
        match crate::invite::Invite::create(input, stamp, seed) {
            Ok(i) => i.json(),
            Err(e) => format!("{{\"error\":{}}}", json_str(&e)),
        }
    }

    /// Seed before restoring a local save or connecting to peers. Reopening
    /// the original invite merges its old ops, so it cannot reset progress.
    pub fn load_invite(&mut self, room: &str, text: &str) -> Result<(), String> {
        let invite = crate::invite::Invite::parse(text, room)?;
        let packet = invite.snapshot();
        self.invitation = Some(invite);
        self.on_packet(&packet, 0);
        Ok(())
    }

    // ---- outbox -----------------------------------------------------------

    fn event(&mut self, kind: u8, payload: &[u8]) {
        self.outbox.push(kind);
        self.outbox.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        self.outbox.extend_from_slice(payload);
    }

    fn send(&mut self, msgs: &[Msg]) {
        let mut b = Vec::new();
        // On an outdated page, table changes stay local (they could be misread).
        for m in msgs.iter().filter(|m| !(self.outdated && matches!(m, Msg::Op(_)))) {
            encode(m, &mut b);
        }
        if b.is_empty() {
            return;
        }
        self.event(EV_NET, &b);
    }

    fn status(&mut self, s: &str) {
        self.event(EV_STATUS, s.as_bytes());
    }

    fn op(&self, body: Body) -> Op {
        Op { round: self.shared.round, body }
    }

    /// Apply a locally-originated op and broadcast it.
    fn commit(&mut self, body: Body) {
        let op = self.op(body);
        self.shared.apply(&op);
        self.send(&[Msg::Op(op)]);
    }

    /// Local card write; broadcast is coalesced to once per frame.
    fn set_card(&mut self, w: u32, i: usize, loc: Loc, rot: u8) {
        let stamp = self.stamp();
        let op = self.op(Body::Card(w, i as u8, Card { loc, rot, stamp }));
        self.shared.apply(&op);
        self.dirty_cards.insert((w, i as u8));
    }

    fn flush_cards(&mut self) {
        if !self.dirty_cards.is_empty() {
            let msgs = self.card_msgs();
            self.send(&msgs);
        }
    }

    /// Ops for every card written since the last broadcast.
    fn card_msgs(&mut self) -> Vec<Msg> {
        std::mem::take(&mut self.dirty_cards)
            .into_iter()
            .filter_map(|k| self.shared.cards.get(&k).map(|c| Msg::Op(self.op(Body::Card(k.0, k.1, *c)))))
            .collect()
    }

    fn hello(&mut self) {
        self.last_hello_ms = self.now;
        let (device, seat, sum) = (self.device, self.seat.unwrap_or(0), self.state_sum());
        self.send(&[Msg::Hello { device, seat, sum, ver: PROTOCOL }]);
    }

    // ---- network in ---------------------------------------------------------

    /// A packet from the peer on connection `link`.
    pub fn on_packet(&mut self, bytes: &[u8], link: u32) {
        let mut late = false;
        for m in decode(bytes) {
            match m {
                Msg::Op(op) => {
                    let s = match &op.body {
                        Body::Sub(_, s) => s.stamp,
                        Body::Seat(_, s) => s.stamp,
                        Body::Move(_, m) => m.stamp,
                        Body::Start(s) | Body::Next(_, s) | Body::Focus(_, s) => *s,
                        Body::Card(_, _, c) => c.stamp,
                        Body::Check(_, c) => c.stamp,
                        Body::None => op.round.stamp,
                    };
                    self.clock = self.clock.max(s.clock).max(op.round.stamp.clock);
                    // A late change to a round this device was pushed out of
                    // still lands there.
                    for kept in [&mut self.aside, &mut self.origin].into_iter().flatten() {
                        if op.round.stamp == kept.round.stamp && op.round.stamp != self.shared.round.stamp {
                            late |= kept.apply(&op);
                        }
                    }
                    if op.round.rank() > self.shared.round.rank() {
                        self.begin_change();
                    }
                    self.shared.apply(&op);
                }
                Msg::Hello { device, seat, sum, ver } if device != self.device => {
                    self.peers.insert(device, (seat, link));
                    if ver > PROTOCOL && !self.outdated {
                        self.outdated = true;
                        self.event(EV_OUTDATED, b"{}");
                    }
                    self.compare_sums(device, sum);
                }
                Msg::Cursor { device, seat, kind, x, y, card } if device != self.device => {
                    let now = self.now;
                    self.peers.insert(device, (seat, link));
                    self.cursors.insert(device, RemoteCursor { seat, kind, x, y, card, seen: now });
                }
                _ => {}
            }
        }
        // Explained once the whole packet is in: a full sync sends the round
        // before the seats of the players in it.
        self.finish_change(false);
        if late {
            self.late_merges += 1;
            if let Some(json) = self.aside.as_ref().and_then(keep_json) {
                self.event(EV_KEEP, json.as_bytes());
            }
        }
        self.check_moved();
    }

    /// The round is about to change: remember the table as it was.
    fn begin_change(&mut self) {
        if self.before_change.is_none() {
            self.before_change = Some(self.shared.clone());
        }
    }

    /// The round changed (or didn't, after all). Keep the round that left in
    /// Past games, keep it aside for late changes, and unless this device did
    /// it, explain what happened: one rule for every cause.
    fn finish_change(&mut self, by_me: bool) {
        let Some(old) = self.before_change.take() else { return };
        if old.round.stamp == self.shared.round.stamp {
            return;
        }
        self.new_round_local();
        let new = self.shared.round;
        let kept = keep_json(&old);
        let kept_unfinished = kept.is_some() && old.phase() != Phase::End;
        if let Some(json) = kept {
            self.event(EV_KEEP, json.as_bytes());
        }
        let from = old.round;
        let kind = if from.table == Stamp::default() {
            None // arriving: this device had no table yet
        } else if new.table != from.table {
            Some("otherTable")
        } else if new.epoch > from.epoch {
            Some("newGame")
        } else if new.parent == from.stamp {
            Some("next")
        } else if new.parent == from.parent {
            Some("crossed")
        } else {
            Some("missed")
        };
        if kind == Some("otherTable") {
            self.origin = Some(old.clone());
            self.move_prompted = false;
        }
        if from.table != Stamp::default() {
            self.aside = Some(old.clone());
        }
        let Some(kind) = kind.filter(|_| !by_me) else { return };
        // The other group: seated here now, but not at the table we had.
        let names: Vec<String> = if kind == "otherTable" {
            self.shared
                .seats
                .iter()
                .filter(|(id, x)| x.active && !old.seats.get(id).is_some_and(|o| o.active) && Some(**id) != self.seat)
                .map(|(_, x)| x.name.clone())
                .collect()
        } else {
            Vec::new()
        };
        let (text, action) = self.notice_text(kind, &names, kept_unfinished);
        let action = action.map_or("null".into(), |(k, l)| format!("{{\"kind\":\"{k}\",\"label\":{}}}", json_str(l)));
        let names: Vec<String> = names.iter().map(|n| json_str(n)).collect();
        let by = if new.dealer != 0 { json_str(&self.name_of(new.dealer)) } else { "null".into() };
        let dealt_in = self.seat.is_some_and(|me| new.players().contains(&me));
        let json = format!(
            "{{\"kind\":\"{kind}\",\"by\":{by},\"n\":{},\"dealtIn\":{dealt_in},\"names\":[{}],\"text\":{},\"action\":{action}}}",
            new.n,
            names.join(","),
            json_str(&text)
        );
        self.event(EV_NOTICE, json.as_bytes());
    }

    /// If this device's group has moved to another room, say so, once.
    fn check_moved(&mut self) {
        if self.move_prompted {
            return;
        }
        let Some(t) = self.origin.as_ref().map(|o| o.round.table) else { return };
        if let Some(m) = self.shared.moves.get(&t).cloned() {
            self.move_prompted = true;
            let by = self.name_of(m.by);
            let json = format!(
                "{{\"room\":{},\"by\":{},\"text\":{},\"action\":{{\"kind\":\"follow\",\"label\":{},\"room\":{}}}}}",
                json_str(&m.room),
                json_str(&by),
                json_str(&format!("{by} moved your group to #{}.", m.room)),
                json_str(&format!("Go to #{}", m.room)),
                json_str(&m.room)
            );
            self.event(EV_MOVED, json.as_bytes());
        }
    }

    /// Move this device's group (the table it came from) to `room`: record
    /// it here for the rest of the group, and return that table, to seed the
    /// new room with. None if this device didn't come from another table.
    pub fn record_move(&mut self, room: &str) -> Option<Vec<u8>> {
        let Some(origin) = self.origin.as_ref() else {
            self.status("Only a group whose game was set aside here can move.");
            return None;
        };
        let (t, snap) = (origin.round.table, snapshot_of(origin));
        let (by, stamp) = (self.seat.unwrap_or(0), self.stamp());
        self.move_prompted = true;
        self.commit(Body::Move(t, Moved { room: clamp_text(room, MAX_TEXT), by, stamp }));
        Some(snap)
    }

    /// The table this device came from (to follow a move with its own copy).
    pub fn origin_snapshot(&self) -> Option<Vec<u8>> {
        self.origin.as_ref().map(snapshot_of)
    }

    fn new_round_local(&mut self) {
        self.guessing_with_group = false;
        self.editing = None;
        self.press = None;
        self.dirty_cards.clear();
        self.poses.clear();
    }

    /// The connection `link` closed: everyone who spoke over it is gone.
    pub fn on_link_lost(&mut self, link: u32) {
        let gone: Vec<u32> = self.peers.iter().filter(|(_, (_, l))| *l == link).map(|(d, _)| *d).collect();
        for d in gone {
            self.peers.remove(&d);
            self.cursors.remove(&d);
        }
    }

    /// A hash of the whole table (FNV-1a over the snapshot).
    fn state_sum(&self) -> u32 {
        self.snapshot().iter().fold(0x811c_9dc5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193))
    }

    /// Changes go straight from whoever made them to each peer, and nobody
    /// passes them on: one sent while a link was down, by a device that then
    /// left, would never reach the far side if its other links stayed up.
    /// So presence pings carry a hash of the table, and when a peer's differs
    /// from ours twice running with neither side changing in between (not
    /// just changes in flight), we send everything, at most every few seconds.
    fn compare_sums(&mut self, device: u32, theirs: u32) {
        let mine = self.state_sum();
        if theirs == mine {
            self.drift.remove(&device);
            return;
        }
        if self.drift.insert(device, (theirs, mine)) == Some((theirs, mine)) && self.now - self.last_resync_ms >= RESYNC_EVERY_MS {
            self.last_resync_ms = self.now;
            self.drift.clear();
            self.on_peer_join();
        }
    }

    /// The whole table as one packet, for this device to keep (localStorage)
    /// and feed back to `on_packet` after a reload: if every device that
    /// held the table reloads or closes, the table survives. A stale save
    /// merges like any other sync, so it can't undo newer moves.
    pub fn snapshot(&self) -> Vec<u8> {
        snapshot_of(&self.shared)
    }

    pub fn on_peer_join(&mut self) {
        let mut msgs: Vec<Msg> = self.shared.all_ops().into_iter().map(Msg::Op).collect();
        msgs.push(Msg::Hello { device: self.device, seat: self.seat.unwrap_or(0), sum: self.state_sum(), ver: PROTOCOL });
        self.send(&msgs);
    }

    // ---- seats ----------------------------------------------------------------

    /// Sit down as `name`. An existing name takes over that seat.
    pub fn join(&mut self, name: &str) {
        let name = clamp_text(name, 24);
        if name.is_empty() {
            return;
        }
        let id = seat_id(&name);
        let stamp = self.stamp();
        self.commit(Body::Seat(id, Seat { name, active: true, stamp }));
        self.seat = Some(id);
        self.guessing_with_group = false;
        self.hello();
    }

    /// While this device is here, its seat stays active: if someone took it
    /// out (Remove, or a New game made while this device was cut off), it
    /// sits back down.
    fn keep_my_seat(&mut self) {
        let Some(id) = self.seat else { return };
        if let Some(seat) = self.shared.seats.get(&id).filter(|s| !s.active).cloned() {
            let stamp = self.stamp();
            self.commit(Body::Seat(id, Seat { active: true, stamp, ..seat }));
        }
    }

    pub fn watch(&mut self) {
        self.seat = None;
        self.hello();
    }

    /// Take a (usually absent) player out of the game.
    pub fn remove_seat(&mut self, id: u32) {
        if let Some(s) = self.shared.seats.get(&id).cloned() {
            let stamp = self.stamp();
            self.commit(Body::Seat(id, Seat { active: false, stamp, ..s }));
            if self.seat == Some(id) {
                self.watch();
            }
        }
    }

    /// Seats with a device here right now (including mine).
    pub fn present(&self) -> BTreeSet<u32> {
        let mut v: BTreeSet<u32> = self
            .peers
            .values()
            .filter(|(seat, _)| *seat != 0)
            .map(|(s, _)| *s)
            .collect();
        v.extend(self.seat);
        v
    }

    pub fn watchers(&self) -> usize {
        self.peers.values().filter(|(seat, _)| *seat == 0).count()
            + self.seat.is_none() as usize
    }

    pub fn color_of(&self, seat: u32) -> u32 {
        let players = self.shared.players();
        let k = players.iter().position(|&x| x == seat).unwrap_or(seat as usize);
        PALETTE[k % PALETTE.len()]
    }

    pub fn name_of(&self, seat: u32) -> String {
        self.shared.seats.get(&seat).map(|s| s.name.clone()).unwrap_or_else(|| "someone".into())
    }

    fn my_sub(&self) -> Sub {
        self.seat.and_then(|s| self.shared.latest(s).cloned()).unwrap_or_else(|| Sub { decoys: 1, ..Sub::default() })
    }

    pub fn can_write(&self) -> bool {
        self.shared.round.custom == 0 && self.seat.is_some_and(|s| self.shared.round_players().contains(&s)
            && (self.shared.start.is_none() || !self.shared.is_ready(s)))
    }

    /// The shared table can guess while this device writes its own clover.
    pub fn view_phase(&self) -> Phase {
        if self.shared.start.is_some() && self.can_write() && !self.guessing_with_group {
            Phase::Writing
        } else {
            self.shared.phase()
        }
    }

    fn seated_writing(&self) -> Option<u32> {
        self.seat.filter(|_| self.view_phase() == Phase::Writing && self.can_write())
    }

    // ---- what is on the board -------------------------------------------------

    pub fn scene(&self) -> Scene {
        let s = &self.shared;
        let empty = || Scene {
            writer: None,
            cards: Vec::new(),
            claim: [None; NSLOTS],
            words: Vec::new(),
            clues: Default::default(),
            movable: Vec::new(),
            locked: 0,
        };
        match self.view_phase() {
            Phase::Lobby | Phase::End => empty(),
            Phase::Writing => {
                let Some(me) = self.seated_writing() else { return empty() };
                let d = s.deal(me);
                Scene {
                    writer: Some(me),
                    cards: (0..NSLOTS)
                        .map(|i| Card { loc: Loc::Slot(i as u8), rot: d.sol_rot[i], stamp: Stamp::default() })
                        .collect(),
                    claim: [Some(0), Some(1), Some(2), Some(3)],
                    words: d.words[..NSLOTS].to_vec(),
                    clues: self.my_sub().clues,
                    movable: vec![false; NSLOTS],
                    locked: 0b1111,
                }
            }
            Phase::Guessing(k) => {
                let w = s.puzzles()[k];
                let d = s.deal(w);
                let (cards, claim) = s.layout(w);
                let check = s.check(w);
                let locked = if check.done() { 0b1111 } else { check.mask };
                let can = self.seat.is_some_and(|me| me != w) && !check.done();
                Scene {
                    writer: Some(w),
                    movable: (0..cards.len()).map(|i| can && (i >= NSLOTS || locked & (1 << i) == 0)).collect(),
                    words: d.words[..cards.len()].to_vec(),
                    clues: s.effective(w).map(|x| x.clues.clone()).unwrap_or_default(),
                    cards,
                    claim,
                    locked,
                }
            }
        }
    }

    // ---- geometry -----------------------------------------------------------

    /// Board fills the screen; while guessing, a tray for loose cards goes
    /// below (portrait) or beside (landscape) it.
    pub fn layout(&self) -> Layout {
        // A saved big card size must not make tray cards overlap: cap the
        // zoom so three fit across. Shrinking the zoom only grows the board
        // scale, so a few rounds settle it, and the final cap uses the final k.
        let mut l = self.layout_at(self.zoom);
        for _ in 0..4 {
            let Some((_, _, tw, th, horiz)) = l.tray else { return l };
            let cap = (if horiz { tw } else { th }) - 2.0 * TRAY_INSET - (TRAY_PER_LINE as f32 - 1.0) * TRAY_GAP;
            let cap = cap / (TRAY_PER_LINE as f32 * CARD * l.k);
            if l.z <= cap {
                return l;
            }
            // A smaller zoom grows k; the next pass re-checks against it.
            l = self.layout_at(cap);
        }
        let Some((_, _, tw, th, horiz)) = l.tray else { return l };
        let cap = ((if horiz { tw } else { th }) - 2.0 * TRAY_INSET - (TRAY_PER_LINE as f32 - 1.0) * TRAY_GAP) / (TRAY_PER_LINE as f32 * CARD * l.k);
        Layout { z: l.z.min(cap), ..l }
    }

    fn layout_at(&self, zoom: f32) -> Layout {
        let (w, h) = (self.w, self.h);
        let ext = 2.0 * FIT_EXTENT;
        if !matches!(self.view_phase(), Phase::Guessing(_)) {
            let k = ((w - 2.0 * PAD).min(h - 2.0 * PAD) / ext).max(0.05);
            return Layout { k, bc: (w / 2.0, h / 2.0), tray: None, z: zoom };
        }
        // The tray holds every card of the clover without overlap, three to a
        // row (portrait) or column (landscape) at this device's card size; the
        // board shrinks only as far as that needs.
        let n = match self.view_phase() {
            Phase::Guessing(i) => self.shared.ncards(self.shared.puzzles()[i]),
            _ => NSLOTS,
        };
        let lines = n.div_ceil(TRAY_PER_LINE).max(1) as f32;
        let card = CARD * zoom;
        // Tray length for `lines` cards of `k`: matches `tray_px`'s margins.
        let tray_len = |k: f32| lines * (card * k + TRAY_GAP) + 2.0 * TRAY_INSET - TRAY_GAP;
        let fixed = lines * TRAY_GAP + 2.0 * TRAY_INSET - TRAY_GAP;
        if h >= w {
            let k = ((w - 2.0 * PAD) / ext).min((h - 3.0 * PAD - fixed) / (ext + lines * card)).max(0.05);
            let bh = ext * k;
            let tray_y = PAD + bh + PAD;
            Layout { k, bc: (w / 2.0, PAD + bh / 2.0), tray: Some((PAD, tray_y, w - 2.0 * PAD, h - tray_y - PAD, true)), z: zoom }
        } else {
            let k = ((h - 2.0 * PAD) / ext).min((w - 3.0 * PAD - fixed) / (ext + lines * card)).max(0.05);
            let bw = ext * k;
            // Keep the group centred; spare width goes to the margins.
            let tray_w = tray_len(k).max(w - bw - 3.0 * PAD).min(tray_len(k) + card * k);
            let left = ((w - bw - tray_w - PAD) / 2.0).max(PAD);
            Layout {
                k,
                bc: (left + bw / 2.0, h / 2.0),
                tray: Some((left + bw + PAD, PAD, tray_w, h - 2.0 * PAD, false)),
                z: zoom,
            }
        }
    }

    pub fn scale(&self) -> f32 {
        self.layout().k
    }

    /// Board units → CSS px.
    pub fn board_m(&self) -> M {
        let l = self.layout();
        M::t(l.bc.0, l.bc.1).then(M::s(l.k)).then(M::r(self.angle))
    }

    /// Tray coords (0..1 along long, short axis) → CSS px of a card centre.
    pub fn tray_px(&self, u: f32, v: f32) -> (f32, f32) {
        let l = self.layout();
        let Some((x, y, w, h, horiz)) = l.tray else { return (-1000.0, -1000.0) };
        let half = HALF * l.k * l.z + TRAY_INSET;
        let along = |o: f32, len: f32, t: f32| o + half + t.clamp(0.0, 1.0) * (len - 2.0 * half).max(0.0);
        if horiz { (along(x, w, u), along(y, h, v)) } else { (along(x, w, v), along(y, h, u)) }
    }

    /// CSS px → tray coords (clamped).
    pub fn px_tray(&self, px: f32, py: f32) -> (f32, f32) {
        let l = self.layout();
        let Some((x, y, w, h, horiz)) = l.tray else { return (0.5, 0.5) };
        let half = HALF * l.k * l.z + TRAY_INSET;
        let t = |o: f32, len: f32, p: f32| ((p - o - half) / (len - 2.0 * half).max(1.0)).clamp(0.0, 1.0);
        if horiz { (t(x, w, px), t(y, h, py)) } else { (t(y, h, py), t(x, w, px)) }
    }

    /// Card zoom as drawn now (see `Layout::z`).
    pub fn card_zoom(&self) -> f32 {
        self.layout().z
    }

    /// Guess clover `k` next (anyone, any order; solved ones show their answer).
    pub fn jump(&mut self, k: usize) {
        if matches!(self.view_phase(), Phase::Guessing(_)) && k < self.shared.puzzles().len() {
            let s = self.stamp();
            self.commit(Body::Focus(self.shared.puzzles()[k], s));
        }
    }

    /// Card-word metrics at size 100, one word per line:
    /// `word\twidth[\tdescent[\tx1:d1 x2:d2 …]]` — the last field gives each
    /// letter's right edge (from the word's left) and descent below the
    /// centre line. Missing fields fall back to the whole word's descent.
    pub fn set_word_widths(&mut self, text: &str) {
        self.word_layouts.borrow_mut().clear();
        for line in text.lines() {
            let mut f = line.split('\t');
            let (Some(w), Some(Ok(width))) = (f.next(), f.next().map(str::parse::<f32>)) else { continue };
            let desc = f.next().and_then(|d| d.parse::<f32>().ok()).unwrap_or((WORD_INK - WORD_ASC) * 100.0);
            let mut letters = Vec::new();
            let mut x0 = 0.0;
            for g in f.next().unwrap_or("").split(' ') {
                if let Some((x1, d)) = g.split_once(':') {
                    if let (Ok(x1), Ok(d)) = (x1.parse::<f32>(), d.parse::<f32>()) {
                        letters.push((x0, x1, d));
                        x0 = x1;
                    }
                }
            }
            if letters.is_empty() {
                letters.push((0.0, width, desc));
            }
            self.word_w.insert(w.to_string(), WordInk { width, letters });
        }
    }

    fn word_ink(&self, w: &str) -> WordInk {
        self.word_w.get(w).cloned().unwrap_or_else(|| {
            let width = w.chars().count() as f32 * 56.0;
            WordInk { width, letters: vec![(0.0, width, (WORD_INK - WORD_ASC) * 100.0)] }
        })
    }

    /// How deep a word's letters reach from the card edge, per unit of size,
    /// at one end: only the letters that sit in the corner zone count (the
    /// part of the edge that a neighbouring word's letters could share at
    /// full size). Letters' tops line up WORD_ASC above the centre line, so
    /// depth = that plus the deepest descent among those letters.
    fn end_depth(ink: &WordInk, plus_end: bool) -> f32 {
        // Zone: past this distance from the word's centre (size-100 units).
        let zone = (HALF - WORD_EDGE - WORD_INK * WORD_SIZE - WORD_GAP) * 100.0 / WORD_SIZE;
        let half = ink.width / 2.0;
        let desc = ink
            .letters
            .iter()
            .filter(|&&(x0, x1, _)| if plus_end { x1 - half >= zone } else { half - x0 >= zone })
            .map(|l| l.2)
            .fold(0.0f32, f32::max);
        WORD_ASC + desc / 100.0
    }

    /// Each edge word's size on a card (top, right, bottom, left) and its
    /// offset along the edge (always 0: words stay centred), cached per card.
    pub fn card_word_layout(&self, words: &[&str; 4]) -> [(f32, f32); 4] {
        let key = words.join("\u{1}");
        if let Some(l) = self.word_layouts.borrow().get(&key) {
            return *l;
        }
        let l = self.solve_card_words(words).map(|s| (s, 0.0));
        self.word_layouts.borrow_mut().insert(key, l);
        l
    }

    /// Sizes for a card's four centred words, solved exactly.
    ///
    /// With x the fraction of full size, a word's half-width is a·x and its
    /// ink depth from the edge d·x (both measured per word). Two neighbours
    /// collide when both reach into their shared corner, so at each corner
    /// one of them must yield: its half-width plus the other's depth fits
    /// C = HALF − WORD_EDGE − WORD_GAP. That is linear in x. For each of the
    /// 16 ways to pick who yields at each corner, minimise the squared
    /// shrinkage Σ(1 − x)² (a deep cut to one word costs more than two light
    /// trims) under those constraints and x ≤ 1 (and the edge's length), by
    /// checking the KKT conditions of every small set of tight constraints.
    /// Keep the best; ties go to the bigger smallest word.
    fn solve_card_words(&self, words: &[&str; 4]) -> [f32; 4] {
        let ink = words.map(|w| self.word_ink(w));
        let a: [f32; 4] = core::array::from_fn(|j| ink[j].width / 100.0 * WORD_SIZE / 2.0);
        // Depth at each end: `dp` at the +x end (toward corner j), `dm` at
        // the −x end (toward corner j−1).
        let dp: [f32; 4] = core::array::from_fn(|j| Self::end_depth(&ink[j], true) * WORD_SIZE);
        let dm: [f32; 4] = core::array::from_fn(|j| Self::end_depth(&ink[j], false) * WORD_SIZE);
        let c = HALF - WORD_EDGE - WORD_GAP;
        let cap: [f32; 4] = core::array::from_fn(|j| (WORD_REACH / a[j]).min(1.0));
        // Corner k between word k and word k+1; `yields(k, j)`: word j's end
        // must clear the other's letters.
        // At corner k, word k's +x end meets word k+1's −x end.
        let collides = |x: &[f32; 4], k: usize| {
            let n = (k + 1) % 4;
            a[k] * x[k] > c - dm[n] * x[n] + 1e-4 && a[n] * x[n] > c - dp[k] * x[k] + 1e-4
        };
        if (0..4).all(|k| !collides(&[1.0; 4], k)) && cap.iter().all(|&u| u >= 1.0) {
            return [WORD_SIZE; 4];
        }
        let mut best: Option<([f32; 4], (f32, f32))> = None;
        for who in 0..16u32 {
            // Constraint rows: coefficients over x, and the bound.
            let mut rows: Vec<([f32; 4], f32)> = Vec::new();
            for k in 0..4 {
                let n = (k + 1) % 4;
                // Who yields: its end must clear the other's letters there.
                let mut r = [0.0; 4];
                if who & (1 << k) != 0 {
                    r[k] = a[k];
                    r[n] = dm[n];
                } else {
                    r[n] = a[n];
                    r[k] = dp[k];
                }
                rows.push((r, c));
            }
            for j in 0..4 {
                let mut r = [0.0; 4];
                r[j] = 1.0;
                rows.push((r, cap[j]));
            }
            if let Some(x) = min_squared_shrink(&rows) {
                let score = (-x.iter().map(|v| (1.0 - v).powi(2)).sum::<f32>(), x.iter().copied().fold(f32::MAX, f32::min));
                if best.as_ref().is_none_or(|b| score > b.1) {
                    best = Some((x, score));
                }
            }
        }
        let x = best.map_or([0.6; 4], |b| b.0);
        x.map(|v| v.max(0.5) * WORD_SIZE)
    }

    /// Just the sizes (see `card_word_layout`).
    pub fn card_word_sizes(&self, words: &[&str; 4]) -> [f32; 4] {
        self.card_word_layout(words).map(|x| x.0)
    }

    /// Over the whole deck: the share of words at full size, the share at 90%
    /// or more, and the smallest scale any word gets.
    pub fn word_fit(&self) -> (f32, f32, f32) {
        let (mut full, mut near, mut min, mut n) = (0, 0, 1.0f32, 0);
        for c in crate::words::CARDS.iter() {
            for size in self.card_word_sizes(c) {
                n += 1;
                full += (size >= WORD_SIZE - 1e-3) as u32;
                near += (size >= 0.9 * WORD_SIZE - 1e-3) as u32;
                min = min.min(size / WORD_SIZE);
            }
        }
        (full as f32 / n as f32, near as f32 / n as f32, min)
    }

    pub fn set_theme(&mut self, dark: bool) {
        self.pal = if dark { &DARK } else { &LIGHT };
    }

    pub fn set_zoom(&mut self, z: f32) {
        self.zoom = if z.is_finite() { z.clamp(ZOOM_MIN, ZOOM_MAX) } else { 1.0 };
    }

    /// Card-local units → CSS px, for a card's resting place.
    pub fn card_m(&self, c: &Card) -> M {
        let r = M::r(turn(c.rot)).then(M::s(self.card_zoom()));
        match c.loc {
            Loc::Table { x, y } => {
                let (px, py) = self.tray_px(x, y);
                M::t(px, py).then(M::s(self.scale())).then(r)
            }
            Loc::Slot(s) => {
                let (x, y) = slot_pos(s as usize);
                self.board_m().then(M::t(x, y)).then(r)
            }
            Loc::Float { x, y } => self.board_m().then(M::t(x, y)).then(r),
        }
    }

    fn pose_m(&self, p: &Pose) -> M {
        if p.board {
            self.board_m().then(M::t(p.x, p.y)).then(M::r(p.a)).then(M::s(self.card_zoom() * p.s))
        } else {
            let (px, py) = self.tray_px(p.x, p.y);
            M::t(px, py).then(M::s(self.scale())).then(M::r(p.a)).then(M::s(self.card_zoom() * p.s))
        }
    }

    fn target_pose(c: &Card) -> Pose {
        let a = turn(c.rot);
        match c.loc {
            Loc::Table { x, y } => Pose { board: false, x, y, a, s: 1.0, wait: 0.0 },
            Loc::Slot(s) => {
                let (x, y) = slot_pos(s as usize);
                Pose { board: true, x, y, a, s: 1.0, wait: 0.0 }
            }
            Loc::Float { x, y } => Pose { board: true, x, y, a, s: 1.0, wait: 0.0 },
        }
    }

    /// Re-express a pose in the other frame without moving it on screen.
    fn reframe(&self, p: Pose, board: bool) -> Pose {
        if p.board == board {
            return p;
        }
        let (px, py) = self.pose_m(&Pose { a: 0.0, ..p }).apply(0.0, 0.0);
        if board {
            let (x, y) = self.board_m().inv().apply(px, py);
            Pose { board, x, y, a: p.a - self.angle, ..p }
        } else {
            let (x, y) = self.px_tray(px, py);
            Pose { board, x, y, a: p.a + self.angle, ..p }
        }
    }

    fn dragging_card(&self) -> Option<usize> {
        self.press.as_ref().filter(|p| p.moved).and_then(|p| match p.target {
            Target::Card(i) => Some(i),
            _ => None,
        })
    }

    /// Cards other players are dragging right now (card → their seat), from
    /// their live pointers: a hold ends when they let go, go quiet or leave.
    pub fn holds(&self) -> BTreeMap<usize, u32> {
        if !matches!(self.view_phase(), Phase::Guessing(_)) { return BTreeMap::new(); }
        self.cursors.values().filter(|c| c.seat != 0).filter_map(|c| c.card.map(|i| (i as usize, c.seat))).collect()
    }

    /// Back-to-front draw order: like a real table, the card touched last
    /// is on top; cards being held or dragged float above everything.
    fn draw_order(&self, cards: &[Card]) -> Vec<usize> {
        let dragging = self.dragging_card();
        let holds = self.holds();
        let rank = |i: usize| -> u8 {
            if Some(i) == dragging {
                2
            } else {
                holds.contains_key(&i) as u8
            }
        };
        let mut v: Vec<usize> = (0..cards.len()).collect();
        v.sort_by_key(|&i| (rank(i), cards[i].stamp));
        v
    }

    fn leaf_m_at(&self, d: u8, angle: f32) -> M {
        let l = self.layout();
        M::t(l.bc.0, l.bc.1).then(M::s(l.k)).then(M::r(angle + turn(d)))
    }

    fn leaf_mid() -> f32 {
        -(LEAF_IN + LEAF_OUT) / 2.0
    }

    /// Whether a press at this point would pick up a card (so the shell
    /// knows a one-finger drag elsewhere can pan its zoom instead).
    /// Which card a touch at (px, py) would pick up (tests).
    pub fn card_at_point(&self, px: f32, py: f32) -> Option<usize> {
        match self.hit(px, py) {
            Target::Card(i) => Some(i),
            _ => None,
        }
    }

    pub fn on_card(&self, px: f32, py: f32) -> bool {
        matches!(self.hit(px, py), Target::Card(_))
    }

    fn hit(&self, px: f32, py: f32) -> Target {
        let (bx, by) = self.board_m().inv().apply(px, py);
        if bx.hypot(by) <= HUB_R + 6.0 {
            return Target::Hub;
        }
        let sc = self.scene();
        let card = self.draw_order(&sc.cards).into_iter().rev().find(|&i| {
            let (lx, ly) = self.card_m(&sc.cards[i]).inv().apply(px, py);
            lx.abs() <= HALF && ly.abs() <= HALF
        });
        if let Some(i) = card.filter(|&i| sc.movable[i]) {
            return Target::Card(i);
        }
        let r = bx.hypot(by);
        if r <= BOARD_HALF || card.is_some() {
            return Target::Board;
        }
        // Anywhere on a leaf (outside the centre) belongs to that leaf.
        if let Some(d) = (0..4).find(|&d| {
            let (x, y) = self.leaf_m_at(d, self.angle).inv().apply(px, py);
            -y >= x.abs() && -y <= EXTENT + 10.0
        }) {
            return Target::Petal(d);
        }
        Target::Empty
    }

    // ---- input --------------------------------------------------------------

    /// Another player's hand on card `i` (from their live pointer): its
    /// device and seat. With `below`, only a hand on a device numbered lower.
    fn hand_on(&self, i: usize, below: Option<u32>) -> Option<(u32, u32)> {
        if !matches!(self.view_phase(), Phase::Guessing(_)) { return None; }
        self.cursors
            .iter()
            .filter(|(d, c)| c.card == Some(i as u8) && c.seat != 0 && Some(c.seat) != self.seat && below.is_none_or(|b| **d < b))
            .map(|(d, c)| (*d, c.seat))
            .next()
    }

    /// The player on device `d`: this device's, or a connected one's.
    fn seat_of_device(&self, d: u32) -> Option<u32> {
        if d == self.device {
            return self.seat;
        }
        self.peers.get(&d).map(|(seat, _)| *seat).filter(|s| *s != 0)
    }

    /// A card is in one hand at a time: say whose, and pulse it in their colour.
    fn refuse(&mut self, i: usize, seat: u32) {
        self.refused = Some((i, seat, self.now));
        let who = self.name_of(seat);
        self.status(&format!("{who} is moving this card"));
    }

    pub fn pointer_down(&mut self, sx: f32, sy: f32) {
        let mut target = self.hit(sx, sy);
        // Someone else is holding it: the first hand keeps it.
        if let Target::Card(i) = target {
            if let Some((_, seat)) = self.hand_on(i, None) {
                self.refuse(i, seat);
                target = Target::Held(i);
            }
        }
        let (grab, origin) = match target {
            Target::Card(i) => {
                let c = self.scene().cards[i];
                let (cx, cy) = self.card_m(&c).apply(0.0, 0.0);
                ((cx - sx, cy - sy), Some(c))
            }
            _ => ((0.0, 0.0), None),
        };
        self.press = Some(Press { sx, sy, target, grab, origin, moved: false, spin: None, trail: Vec::new() });
    }

    pub fn pointer_move(&mut self, sx: f32, sy: f32) {
        if matches!(self.view_phase(), Phase::Guessing(_)) && self.seat.is_some() {
            let (bx, by) = self.board_m().inv().apply(sx, sy);
            let on_board = bx.abs() <= EXTENT && by.abs() <= EXTENT;
            self.my_cursor = if on_board {
                (2, bx, by)
            } else {
                let (u, v) = self.px_tray(sx, sy);
                (1, u, v)
            };
            self.cursor_dirty = true;
        }

        let Some(p) = self.press.as_ref() else { return };
        if !p.moved && (sx - p.sx).hypot(sy - p.sy) < DRAG_THRESHOLD {
            return;
        }
        // Someone else picked it up between my press and my first move.
        if let (Target::Card(i), false) = (p.target, p.moved) {
            if let Some((_, seat)) = self.hand_on(i, None) {
                self.refuse(i, seat);
                self.press.as_mut().unwrap().target = Target::Held(i);
            }
        }
        let Some(p) = self.press.as_mut() else { return };
        p.moved = true;
        match p.target {
            Target::Card(i) => {
                let (cx, cy) = (sx + p.grab.0, sy + p.grab.1);
                self.drag_card(i, cx, cy);
            }
            Target::Held(_) => {}
            _ => {
                // Dragging anywhere else spins my view of the board.
                let (psx, psy, spin) = (p.sx, p.sy, p.spin);
                let bc = self.layout().bc;
                let ang = |x: f32, y: f32| (y - bc.1).atan2(x - bc.0);
                let start = spin.unwrap_or((ang(psx, psy), self.angle));
                self.angle = start.1 + wrap(ang(sx, sy) - start.0);
                let (a, now) = (self.angle, self.now);
                if let Some(p) = self.press.as_mut() {
                    p.spin = Some((ang(sx, sy), a));
                    p.trail.push((now, a));
                    if p.trail.len() > 12 {
                        p.trail.remove(0);
                    }
                }
            }
        }
    }

    fn drag_card(&mut self, i: usize, cx: f32, cy: f32) {
        let sc = self.scene();
        let (Some(w), Some(_)) = (sc.writer, self.seat) else { return };
        let cur = sc.cards[i];
        let in_board_frame = !matches!(cur.loc, Loc::Table { .. });
        let (bx, by) = self.board_m().inv().apply(cx, cy);
        // Only the tray takes cards off the board: a drop anywhere else (even
        // off the board's edge, e.g. at the top of the screen) stays on it.
        let to_board =
            !self.layout().tray.is_some_and(|(x, y, tw, th, _)| cx >= x && cx <= x + tw && cy >= y && cy <= y + th);
        // Keep the card's on-screen orientation when it changes frame.
        let rot = match (in_board_frame, to_board) {
            (false, true) => quarter(cur.rot as i32 - self.q),
            (true, false) => quarter(cur.rot as i32 + self.q),
            _ => cur.rot,
        };
        let loc = if to_board {
            Loc::Float { x: bx.clamp(HALF - FIT_EXTENT, FIT_EXTENT - HALF), y: by.clamp(HALF - FIT_EXTENT, FIT_EXTENT - HALF) }
        } else {
            let (u, v) = self.px_tray(cx, cy);
            Loc::Table { x: u, y: v }
        };
        self.set_card(w, i, loc, rot);
        // My own drag follows the finger exactly.
        let pose = Self::target_pose(&self.shared.cards[&(w, i as u8)]);
        self.poses.insert((w, i as u8), pose);
    }

    /// The gesture was taken over (a second finger: the browser pinch-zooms).
    /// Nothing happens: a dragged card goes back where it was, a spin snaps
    /// back to the nearest quarter, and there's no tap.
    pub fn pointer_cancel(&mut self) {
        let Some(p) = self.press.take() else { return };
        // Letting go of a card ends its hold on other screens.
        self.cursor_dirty |= p.moved && matches!(p.target, Target::Card(_));
        if p.spin.is_some() {
            self.q = (self.angle / FRAC_PI_2).round() as i32;
        }
        if let (Target::Card(i), true, Some(o)) = (p.target, p.moved, p.origin) {
            if let Some(w) = self.scene().writer {
                self.set_card(w, i, o.loc, o.rot);
            }
        }
    }

    pub fn pointer_up(&mut self, sx: f32, sy: f32) {
        self.pointer_move(sx, sy);
        let Some(p) = self.press.take() else { return };
        self.cursor_dirty |= p.moved && matches!(p.target, Target::Card(_));
        if p.spin.is_some() {
            self.q = self.fling_quarter(&p.trail);
            return;
        }
        match (p.target, p.moved) {
            (Target::Card(i), true) => self.drop_card(i),
            (Target::Card(i), false) => {
                let sc = self.scene();
                let c = sc.cards[i];
                self.set_card(sc.writer.unwrap(), i, c.loc, (c.rot + 1) % 4);
            }
            // Tapping a leaf never turns the board: it only edits (when writing).
            (Target::Petal(d), false) => {
                if self.seated_writing().is_some() {
                    self.edit_clue(d);
                }
            }
            (Target::Hub | Target::Board, false) => self.q += 1,
            _ => {}
        }
        self.flush_cards();
    }

    /// Where a released spin comes to rest. It carries on at the speed it had
    /// over its last ~120 ms for FLING_S, as if coasting, then settles on the
    /// nearest quarter: a short, quick flick turns the board a quarter, and a
    /// slow drag settles where it was let go. At most one extra quarter.
    fn fling_quarter(&self, trail: &[(f64, f32)]) -> i32 {
        let here = self.angle / FRAC_PI_2;
        let mut coast = 0.0;
        if let Some(&(t1, a1)) = trail.last() {
            let early = trail.iter().find(|&&(t, _)| t1 - t <= FLING_WINDOW_MS && t < t1);
            // A finger that stopped before lifting has no fling left.
            if let (Some(&(t0, a0)), true) = (early, self.now - t1 <= FLING_WINDOW_MS) {
                let speed = (a1 - a0) / ((t1 - t0) as f32 / 1000.0);
                coast = (speed * FLING_S / FRAC_PI_2).clamp(-1.0, 1.0);
            }
        }
        (here + coast).round() as i32
    }

    /// Cards stay exactly where they're dropped, like cards on a real table
    /// (so players can leave "maybe" placements, even on top of each other),
    /// except very close to a spot, where they snap. Nothing else moves:
    /// cards sharing a spot are fine, and the one put there last counts.
    fn drop_card(&mut self, i: usize) {
        let sc = self.scene();
        let Some(w) = sc.writer else { return };
        let c = sc.cards[i];
        let Loc::Float { x, y } = c.loc else {
            return self.set_card(w, i, c.loc, c.rot);
        };
        let spot = (0..NSLOTS).filter(|&s| sc.locked & (1 << s) == 0).find(|&s| {
            let (sx, sy) = slot_pos(s);
            (x - sx).hypot(y - sy) <= SNAP_R
        });
        let Some(s) = spot else {
            return self.set_card(w, i, c.loc, c.rot);
        };
        self.set_card(w, i, Loc::Slot(s as u8), c.rot);
    }

    /// Turn my view so leaf `d` sits at the top, reading upright.
    pub fn face_petal(&mut self, d: u8) {
        let delta = quarter(-(d as i32) - self.q) as i32;
        self.q += if delta == 3 { -1 } else { delta };
    }

    /// Open the editor over leaf `d`, lying along the leaf exactly like its
    /// clue is drawn (vertical for side leaves). The board doesn't turn.
    pub fn edit_clue(&mut self, d: u8) {
        self.editing = Some(d);
        let f = self.leaf_m_at(d, self.q as f32 * FRAC_PI_2).then(M::t(0.0, Self::leaf_mid()));
        let (x, y) = f.apply(0.0, 0.0);
        let angle = f.angle();
        let k = self.scale();
        let vertical = angle.sin().abs() > 0.7;
        let room = if vertical { self.h } else { self.w } - 16.0;
        let w = (2.0 * LEAF_HALF_W * k * 0.95).clamp(180.0_f32.min(room), room);
        let h = ((LEAF_OUT - LEAF_IN) * k * 0.6).max(48.0);
        let mut p = vec![d];
        for v in [x, y, w, h, angle] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p.extend_from_slice(self.my_sub().clues[d as usize].as_bytes());
        self.event(EV_EDIT_CLUE, &p);
    }

    fn put_sub(&mut self, f: impl FnOnce(&mut Sub)) {
        let Some(me) = self.seated_writing() else { return };
        let mut sub = self.my_sub();
        f(&mut sub);
        sub.stamp = self.stamp();
        self.commit(Body::Sub(me, sub));
    }

    /// Save a clue (shared, so another device can take over); with
    /// `advance`, open the next empty leaf. Edits after submitting keep you
    /// submitted (until guessing starts), unless a clue is left empty.
    pub fn set_clue(&mut self, d: u8, text: &str, advance: bool) {
        if d as usize >= NSLOTS || self.seated_writing().is_none() {
            return;
        }
        let text = clamp_text(text, MAX_TEXT);
        self.editing = None;
        if self.my_sub().clues[d as usize] != text {
            self.put_sub(|s| {
                s.ready &= !text.is_empty();
                s.clues[d as usize] = text;
            });
        }
        let clues = self.my_sub().clues;
        if advance && !clues[d as usize].is_empty() {
            if let Some(n) = (1..4).map(|k| (d + k) % 4).find(|&n| clues[n as usize].is_empty()) {
                self.edit_clue(n);
            }
        }
    }

    pub fn set_decoys(&mut self, n: u8) {
        let n = n.min(MAX_DECOYS);
        if self.my_sub().decoys != n {
            self.put_sub(|s| s.decoys = n);
        }
    }

    pub fn rotate_view(&mut self, dir: i32) {
        self.q += dir.signum();
    }

    pub fn can_ready(&self) -> bool {
        self.seated_writing().is_some() && self.my_sub().clues.iter().all(|c| !c.is_empty())
    }

    /// What a button does: see the `ACT_` codes.
    pub fn action(&mut self, code: u32) {
        let phase = self.view_phase();
        match code {
            ACT_WRITE | ACT_JOIN_GUESSING if self.shared.start.is_some() && self.can_write() => {
                self.pointer_cancel();
                self.editing = None;
                self.guessing_with_group = code == ACT_JOIN_GUESSING;
                self.my_cursor = (0, 0.0, 0.0);
                self.cursor_dirty = true;
                self.poses.clear();
            }
            ACT_DEAL if matches!(phase, Phase::Lobby | Phase::End) => self.new_round(),
            ACT_REDEAL => self.new_round(),
            ACT_READY if self.can_ready() => {
                let r = !self.my_sub().ready;
                self.put_sub(|s| s.ready = r);
            }
            ACT_CHECK => self.check(),
            ACT_NEXT => {
                if let Phase::Guessing(k) = phase {
                    let w = self.shared.puzzles()[k];
                    if self.shared.check(w).done() {
                        let s = self.stamp();
                        let w = self.shared.puzzles().get(self.shared.next_unsolved(k)).copied().unwrap_or(0);
                        self.commit(Body::Focus(w, s));
                    }
                }
            }
            ACT_PROCEED if self.shared.phase() == Phase::Writing && self.seat.is_some() && self.ready_count() > 0 => {
                self.start_guessing();
            }
            ACT_NEW_GAME => self.new_game(),
            _ => {}
        }
    }

    /// Who a new round would deal in: seated players at the table now.
    pub fn dealable(&self) -> Vec<u32> {
        let here = self.present();
        self.shared.players().into_iter().filter(|p| here.contains(p)).collect()
    }

    fn ready_count(&self) -> usize {
        self.shared.round_players().iter().filter(|p| self.shared.is_ready(**p)).count()
    }

    fn new_round(&mut self) {
        let players = self.dealable();
        if players.is_empty() {
            return self.status("Join the table first.");
        }
        // A room with an imported clover opens by guessing it.
        let customs = Custom::all();
        if self.custom_room && self.shared.round.n == 0 && !customs.is_empty() {
            return self.start_custom(customs, players);
        }
        let prev = self.shared.round;
        // Draw down the deck: this round deals after the cards the last one used.
        let deck = if prev.n == 0 { 0 } else { prev.deck + (prev.nseats as usize * MAX_CARDS) as u32 };
        let (seed, n, stamp) = (self.rng.next() as u32, prev.n + 1, self.stamp());
        let round = Round { epoch: prev.epoch, seed, n, stamp, deck, ..self.lineage(stamp) }.with_players(&players);
        self.deal(round);
    }

    /// Round 1 of a room with imported clovers: straight to guessing them.
    fn start_custom(&mut self, customs: Vec<&'static Custom>, players: Vec<u32>) {
        let mut seats: Vec<u32> = customs.iter().map(|c| c.writer()).collect();
        seats.extend(players);
        let (epoch, seed, stamp) = (self.shared.round.epoch, self.rng.next() as u32, self.stamp());
        let round = Round { epoch, seed, n: 1, stamp, custom: 1, ..self.lineage(stamp) }.with_players(&seats);
        self.deal(round);
        for c in customs {
            let w = c.writer();
            // Their authors aren't at the table: an inactive seat carries the name.
            let stamp = self.stamp();
            self.commit(Body::Seat(w, Seat { name: c.name.clone(), active: false, stamp }));
            let stamp = self.stamp();
            let sub = Sub { clues: c.clues.clone(), decoys: (c.cards.len() - NSLOTS) as u8, ready: true, stamp };
            self.commit(Body::Sub(w, sub));
        }
        self.start_guessing();
    }

    /// Where a round dealt here now comes from: this table (or, for the
    /// table's first deal, a new table named by this deal), the current
    /// round as its parent, and this seat as its dealer.
    fn lineage(&self, stamp: Stamp) -> Round {
        let cur = self.shared.round;
        let table = if cur.table == Stamp::default() { stamp } else { cur.table };
        Round { table, parent: cur.stamp, dealer: self.seat.unwrap_or(0), ..Round::default() }
    }

    /// Make `round` current here and announce it.
    fn deal(&mut self, round: Round) {
        let op = Op { round, body: Body::None };
        self.begin_change();
        self.shared.apply(&op);
        self.finish_change(true);
        self.send(&[Msg::Op(op)]);
    }

    /// This round's clovers selected by `mask`, each as a scrambled,
    /// shareable board (see `model::scramble`), separated by blank lines.
    pub fn export_boards(&mut self, mask: u32) -> String {
        let boards: Vec<String> = self
            .shared
            .puzzles()
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .filter_map(|(_, &w)| self.shared.export_board(w, &self.name_of(w)))
            .collect();
        boards.iter().map(|b| scramble(b, &mut self.rng)).collect::<Vec<_>>().join("\n\n")
    }

    /// This round's clovers for the device's game log: each in answer order
    /// (the share-link text format, unscrambled) with how many guesses it
    /// took and whether it was solved.
    pub fn review_json(&self) -> String {
        self.boards_json(&self.shared.puzzles())
    }

    fn boards_json(&self, writers: &[u32]) -> String {
        boards_json(&self.shared, writers)
    }

    /// Scramble boards given in answer order (e.g. typed in from a screenshot).
    pub fn scramble_boards(&mut self, text: &str) -> String {
        text.split("\n\n").map(|b| scramble(b.trim(), &mut self.rng)).collect::<Vec<_>>().join("\n\n")
    }

    /// Start over with whoever is here: the next epoch's lobby (it outranks
    /// every round of this game), and players who aren't here leave the table.
    fn new_game(&mut self) {
        let stamp = self.stamp();
        self.deal(Round { epoch: self.shared.round.epoch + 1, stamp, ..self.lineage(stamp) });
        let here = self.present();
        let gone: Vec<(u32, Seat)> =
            self.shared.seats.iter().filter(|(id, s)| s.active && !here.contains(id)).map(|(id, s)| (*id, s.clone())).collect();
        for (id, seat) in gone {
            let stamp = self.stamp();
            self.commit(Body::Seat(id, Seat { active: false, stamp, ..seat }));
        }
    }

    pub fn can_check(&self) -> bool {
        let sc = self.scene();
        match (self.view_phase(), sc.writer, self.seat) {
            (Phase::Guessing(_), Some(w), Some(me)) => {
                w != me && !self.shared.check(w).done() && sc.claim.iter().all(|o| o.is_some())
            }
            _ => false,
        }
    }

    /// Guess: wrong cards are booted back to the tray, right ones stay and
    /// lock. No scoring and no limit: keep guessing until all four are right.
    fn check(&mut self) {
        if !self.can_check() {
            return;
        }
        let sc = self.scene();
        let w = sc.writer.unwrap();
        let d = self.shared.deal(w);
        let prev = self.shared.check(w);
        let mut mask = prev.mask;
        for s in 0..NSLOTS {
            let k = sc.claim[s].unwrap();
            // Orientation is judged relative to the board.
            if k == s && sc.cards[k].rot == d.sol_rot[s] {
                mask |= 1 << s;
            }
        }
        // Remember each placement this check rejects, so nobody tries it again
        // without noticing.
        let mut tried = prev.tried;
        for s in 0..NSLOTS {
            if mask & (1 << s) == 0 {
                let k = sc.claim[s].unwrap();
                tried |= Check::bit(k, s, sc.cards[k].rot);
            }
        }
        let stamp = self.stamp();
        let check = Check { attempts: prev.attempts.saturating_add(1), mask, stamp, tried };
        let n = sc.cards.len();
        for s in 0..NSLOTS {
            if mask & (1 << s) == 0 {
                let k = sc.claim[s].unwrap();
                let h = self.shared.home_card(&d, n, k);
                self.set_card(w, k, h.loc, h.rot);
            }
        }
        // The boots and the check travel in one packet, cards first, so every
        // screen sees the guess and the booted cards in the same frame (and
        // shakes them on their way back).
        let op = self.op(Body::Check(w, check));
        self.shared.apply(&op);
        let mut msgs = self.card_msgs();
        msgs.push(Msg::Op(op));
        self.send(&msgs);
    }

    // ---- frame --------------------------------------------------------------

    pub fn frame(&mut self, now: f64) {
        self.dt = ((now - self.now) / 1000.0).clamp(0.0, 0.1) as f32;
        self.now = now;
        let spinning = self.press.as_ref().is_some_and(|p| p.spin.is_some());
        if !spinning {
            let target = self.q as f32 * FRAC_PI_2;
            let diff = target - self.angle;
            self.angle = if diff.abs() < 1e-3 { target } else { self.angle + diff * (1.0 - (-self.dt * 12.0).exp()) };
        }
        if now - self.last_hello_ms >= HELLO_EVERY_MS {
            self.hello();
        }
        self.keep_my_seat();
        self.maybe_start_guessing();
        // Pin legacy selections and a newly reopened round to a writer
        // before another late submission can change the displayed index.
        let focus = match (self.shared.next.0, self.shared.phase()) {
            (Selection::Index(_), Phase::Guessing(k)) | (Selection::Writer(0), Phase::Guessing(k)) => Some(self.shared.puzzles()[k]),
            (Selection::Index(_), Phase::End) => Some(0),
            _ => None,
        };
        if let Some(w) = focus {
            let s = self.stamp();
            self.commit(Body::Focus(w, s));
        }
        let phase = self.view_phase();
        // A clover just got finished: let the leaves glow briefly.
        let done_now = match phase {
            Phase::Guessing(k) => {
                let w = self.shared.puzzles()[k];
                Some((w, self.shared.check(w).done()))
            }
            _ => None,
        };
        // The list can grow ahead of this clover. Its changing index must
        // not rotate the board or restart the cards' animations.
        let phase_changed = if matches!((phase, self.last_phase), (Phase::Guessing(_), Phase::Guessing(_))) {
            done_now.map(|(w, _)| w) != self.last_done.map(|(w, _)| w)
        } else {
            phase != self.last_phase
        };
        if let (Some((w, true)), Some((pw, false))) = (done_now, self.last_done) {
            if w == pw {
                self.glow = 1.0;
                self.solved_at = now;
            }
        }
        // A guess was made (here or by anyone): cards that just left the
        // board for the tray were booted, so give them a shake on the way.
        if let Phase::Guessing(k) = phase {
            let w = self.shared.puzzles()[k];
            let g = self.shared.check(w).attempts;
            if self.last_guess.is_some_and(|(pw, pg)| pw == w && g > pg) {
                let sc = self.scene();
                for (i, c) in sc.cards.iter().enumerate() {
                    let was_on_board = self.poses.get(&(w, i as u8)).is_some_and(|p| p.board);
                    if matches!(c.loc, Loc::Table { .. }) && was_on_board {
                        self.shakes.insert((w, i as u8), now);
                    }
                }
            }
            self.last_guess = Some((w, g));
        }
        self.shakes.retain(|_, t| now - *t < 600.0);
        self.last_done = done_now;
        self.glow = (self.glow - self.dt / 1.8).max(0.0);
        self.last_phase = phase;
        if phase_changed {
            // A round that ends is kept in Past games (and again if it's
            // replaced later; the same key updates the same entry).
            if phase == Phase::End {
                if let Some(json) = keep_json(&self.shared) {
                    self.event(EV_KEEP, json.as_bytes());
                }
            }
            // New phase or new clover: spin back upright the short way.
            self.q = (self.q as f32 / 4.0).round() as i32 * 4;
            self.poses.clear();
        }
        // Two hands took the same card before either heard of the other:
        // the hand on the lower-numbered device keeps it, and this one lets go
        // (every device applies the same rule, so nothing more is sent).
        if let Some(i) = self.dragging_card() {
            if let Some((_, seat)) = self.hand_on(i, Some(self.device)) {
                if let Some(p) = self.press.as_mut() {
                    p.target = Target::Held(i);
                }
                self.cursor_dirty = true;
                self.refuse(i, seat);
            }
        }
        self.flush_cards();
        // A card held still is still held: say so every second, so other
        // screens' holds only time out when this device really goes quiet.
        if self.dragging_card().is_some() && now - self.last_cursor_ms >= 1000.0 {
            self.cursor_dirty = true;
        }
        if self.cursor_dirty && now - self.last_cursor_ms >= 33.0 {
            self.cursor_dirty = false;
            self.last_cursor_ms = now;
            let (kind, x, y) = self.my_cursor;
            let (device, seat, card) = (self.device, self.seat.unwrap_or(0), self.dragging_card().map(|i| i as u8));
            self.send(&[Msg::Cursor { device, seat, kind, x, y, card }]);
        }
        self.cursors.retain(|_, c| now - c.seen < CURSOR_TTL_MS);
        self.render();
        // Compared as bits: colours ride in the buffer as f32s, and some are NaN.
        let same = |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits());
        self.unchanged = same(&self.cmds, &self.last_cmds) && self.strs == self.last_strs;
        if !self.unchanged {
            self.last_cmds.clone_from(&self.cmds);
            self.last_strs.clone_from(&self.strs);
        }
    }

    /// Guessing starts on its own only once everyone dealt into the round
    /// has submitted, online or not. This must not depend on who *this*
    /// device can see: in a mesh, a device that hasn't connected to someone
    /// yet would start without them. Anyone can override ("Proceed with 3 of
    /// 5").
    fn maybe_start_guessing(&mut self) {
        if self.shared.phase() != Phase::Writing {
            return;
        }
        let players = self.shared.round_players();
        if !players.is_empty() && players.iter().all(|p| self.shared.is_ready(*p)) {
            self.start_guessing();
        }
    }

    fn start_guessing(&mut self) {
        let w = self.shared.round.players().iter().copied().find(|w| self.shared.is_ready(*w)).unwrap_or(0);
        let s = self.stamp();
        self.commit(Body::Start(s));
        self.commit(Body::Focus(w, s));
    }

    /// Skip animations (tests / harness).
    pub fn settle(&mut self) {
        self.angle = self.q as f32 * FRAC_PI_2;
        self.poses.clear();
        let sc = self.scene();
        if let Some(w) = sc.writer {
            for (i, c) in sc.cards.iter().enumerate() {
                self.poses.insert((w, i as u8), Self::target_pose(c));
            }
        }
    }

    // ---- rendering ----------------------------------------------------------

    fn xform(&mut self, m: M) {
        let d = M::s(self.dpr).then(m);
        self.cmds.extend_from_slice(&[C_XFORM, d.a, d.b, d.c, d.d, d.e, d.f]);
    }

    #[allow(clippy::too_many_arguments)]
    fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: u32, stroke: u32, lw: f32) {
        self.cmds.extend_from_slice(&[C_RRECT, x, y, w, h, r, f32::from_bits(fill), f32::from_bits(stroke), lw]);
    }

    fn circle(&mut self, x: f32, y: f32, r: f32, fill: u32, stroke: u32, lw: f32) {
        self.cmds.extend_from_slice(&[C_CIRCLE, x, y, r, f32::from_bits(fill), f32::from_bits(stroke), lw]);
    }

    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, x: f32, y: f32, size: f32, color: u32, font: u8, s: &str, maxw: f32) {
        let off = self.strs.len() as f32;
        self.strs.extend_from_slice(s.as_bytes());
        self.cmds.extend_from_slice(&[
            C_TEXT,
            x,
            y,
            size,
            f32::from_bits(color),
            font as f32,
            off,
            s.len() as f32,
            maxw,
        ]);
    }

    /// Ease displayed card poses toward where the cards really are. Cards
    /// that appear (a new clover) are dealt in from the centre, staggered.
    fn animate(&mut self, sc: &Scene) {
        let Some(w) = sc.writer else { return };
        let k = 1.0 - (-self.dt * 14.0).exp();
        let dragging = self.dragging_card();
        for (i, c) in sc.cards.iter().enumerate() {
            let key = (w, i as u8);
            let target = Self::target_pose(c);
            let pose = match self.poses.get(&key) {
                Some(p) if Some(i) != dragging => {
                    let p = self.reframe(*p, target.board);
                    if p.wait > 0.0 {
                        Pose { wait: p.wait - self.dt, ..p }
                    } else {
                        let close = (p.x - target.x).abs() + (p.y - target.y).abs() + (p.s - 1.0).abs() < 1e-3
                            && wrap(target.a - p.a).abs() < 1e-3;
                        if close {
                            target
                        } else {
                            Pose {
                                board: target.board,
                                x: p.x + (target.x - p.x) * k,
                                y: p.y + (target.y - p.y) * k,
                                a: p.a + wrap(target.a - p.a) * k,
                                s: p.s + (1.0 - p.s) * k,
                                wait: 0.0,
                            }
                        }
                    }
                }
                None if self.motion && Some(i) != dragging => {
                    Pose { board: true, x: 0.0, y: 0.0, a: target.a - 0.6, s: 0.35, wait: i as f32 * 0.06 }
                }
                _ => target,
            };
            self.poses.insert(key, pose);
        }
    }

    /// The two words leaf `d` must link, for the writer: the words on the
    /// cards either side of it that face it.
    fn keywords(&self, sc: &Scene, d: u8) -> Option<String> {
        if self.view_phase() != Phase::Writing || sc.writer.is_none() || sc.cards.len() < NSLOTS {
            return None;
        }
        let word = |s: usize| {
            let side = (d as i32 - sc.cards[s].rot as i32).rem_euclid(4) as usize;
            sc.words[s][side]
        };
        Some(format!("{} & {}", word(d as usize), word((d as usize + 1) % NSLOTS)))
    }

    /// The current clover described for an automated player (the agent
    /// test harness; `_i` is the internal index, for the harness only): whose it is, the four clues (top, right, bottom, left),
    /// each card's location and its words as they currently face (top, right,
    /// bottom, left; board frame, which is the screen's while the view is
    /// upright), which spots are solved, and, while writing, the two words
    /// each leaf links.
    pub fn agent_view_json(&self) -> String {
        let sc = self.scene();
        // Cards are numbered by their shuffled tray order, never by internal
        // index: the first internal indices are the answer, in spot order.
        let n = sc.cards.len();
        let rank: Vec<usize> = match sc.writer {
            Some(w) => {
                let d = self.shared.deal(w);
                (0..n).map(|i| (0..n).filter(|&j| d.home[j] < d.home[i]).count()).collect()
            }
            None => (0..n).collect(),
        };
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| rank[i]);
        let cards: Vec<String> = order
            .iter()
            .map(|&i| (i, &sc.cards[i]))
            .map(|(i, c)| {
                let at = match c.loc {
                    Loc::Table { .. } => "\"tray\"".to_string(),
                    Loc::Slot(s) => format!("\"slot {s}\""),
                    Loc::Float { .. } => "\"board\"".to_string(),
                };
                let words = turn_words(sc.words[i], c.rot).map(json_str).join(",");
                format!(
                    "{{\"card\":{},\"_i\":{i},\"at\":{at},\"words\":[{words}],\"movable\":{},\"rejectedHere\":{},\"heldBy\":{},\"movedBy\":{}}}",
                    rank[i],
                    sc.movable.get(i).copied().unwrap_or(false),
                    self.rejected_in(&sc, i).is_some(),
                    self.hand_on(i, None).map_or("null".into(), |(_, seat)| json_str(&self.name_of(seat))),
                    self.seat_of_device(sc.cards[i].stamp.peer).map_or("null".into(), |seat| json_str(&self.name_of(seat)))
                )
            })
            .collect();
        let leaves: Vec<String> = (0..4u8).map(|d| self.keywords(&sc, d).map_or("null".into(), |k| json_str(&k))).collect();
        format!(
            "{{\"writer\":{},\"clues\":[{}],\"solvedSlots\":[{}],\"cards\":[{}],\"leafWords\":[{}],\"viewQuarter\":{}}}",
            sc.writer.map_or("null".into(), |w| json_str(&self.name_of(w))),
            sc.clues.iter().map(|c| json_str(c)).collect::<Vec<_>>().join(","),
            // Writers' own cards are locked too; only guessing has solved spots.
            (0..NSLOTS)
                .filter(|&s| sc.locked & (1 << s) != 0 && matches!(self.view_phase(), Phase::Guessing(_)))
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(","),
            cards.join(","),
            leaves.join(","),
            self.q.rem_euclid(4)
        )
    }

    /// The spot where card `i` lies now, if a check already rejected it
    /// there turned as it is now (it would count for that spot at a check).
    pub fn rejected_at(&self, i: usize) -> Option<usize> {
        self.rejected_in(&self.scene(), i)
    }

    fn rejected_in(&self, sc: &Scene, i: usize) -> Option<usize> {
        let w = sc.writer.filter(|_| matches!(self.view_phase(), Phase::Guessing(_)))?;
        let check = self.shared.check(w);
        let s = (0..NSLOTS).find(|&s| sc.claim[s] == Some(i) && sc.locked & (1 << s) == 0).filter(|_| !check.done())?;
        check.rejected(i, s, sc.cards[i].rot).then_some(s)
    }

    pub fn render(&mut self) {
        self.cmds.clear();
        self.strs.clear();
        self.cmds.extend_from_slice(&[C_CLEAR, f32::from_bits(self.pal.bg)]);
        let sc = self.scene();
        let phase = self.view_phase();
        let l = self.layout();
        self.animate(&sc);

        if let Some((x, y, w, h, _)) = l.tray {
            self.xform(M::ID);
            self.rrect(x, y, w, h, 18.0, self.pal.tray, self.pal.tray_edge, 1.5);
        }

        // The clover: four heart-shaped leaves (one clue each) round a centre.
        let board = self.board_m();
        let glow = if self.motion { self.glow * (0.6 + 0.4 * (self.now as f32 / 180.0).sin()) } else { 0.0 };
        let mid = Self::leaf_mid();
        for d in 0..4u8 {
            self.xform(self.leaf_m_at(d, self.angle));
            let fill = if self.editing == Some(d) { self.pal.leaf_active } else { mix(self.pal.leaf, self.pal.leaf_glow, glow) };
            self.cmds.extend_from_slice(&[C_LEAF, 290.0, 430.0, f32::from_bits(fill), f32::from_bits(self.pal.leaf_edge), 4.0]);
        }
        self.xform(board);
        self.circle(0.0, 0.0, BOARD_HALF, self.pal.centre, self.pal.centre_edge, 4.0);
        for d in 0..4u8 {
            self.xform(self.leaf_m_at(d, self.angle));
            let clue = sc.clues[d as usize].clone();
            let keys = self.keywords(&sc, d);
            let maxw = 2.0 * LEAF_HALF_W - 30.0;
            // The writer sees "a & b" on an empty leaf as a prompt; once a clue
            // is written the two words are right there on the cards beside it.
            match (clue.is_empty(), keys) {
                (false, _) => self.text(0.0, mid, CLUE_SIZE, self.pal.clue_ink, F_SERIF, &clue, maxw),
                (true, Some(k)) => self.text(0.0, mid, 46.0, self.pal.keyword_ink, F_SANS_BOLD, &k, maxw),
                (true, None) => {}
            }
        }
        self.xform(board);
        for s in 0..NSLOTS {
            let (x, y) = slot_pos(s);
            let solved = sc.locked & (1 << s) != 0 && phase != Phase::Writing;
            let claimed = sc.claim[s].is_some() && matches!(phase, Phase::Guessing(_));
            // The card here, turned as it is, was already rejected here.
            let repeat = sc.claim[s].and_then(|k| self.rejected_in(&sc, k)) == Some(s);
            let (fill, stroke, lw) = match (solved, claimed) {
                (true, _) => (0x9be7a8aa, 0xffffff60, 3.0),
                (false, true) if repeat => (0x0000002e, self.pal.tried, 6.0),
                (false, true) => (0x0000002e, self.pal.gold, 3.0),
                _ => (0x0000002e, 0xffffff38, 3.0),
            };
            self.rrect(x - HALF - 6.0, y - HALF - 6.0, CARD + 12.0, CARD + 12.0, 18.0, fill, stroke, lw);
        }

        // Cards.
        let dragging = self.dragging_card();
        let holds = self.holds();
        let w = sc.writer.unwrap_or(0);
        for i in self.draw_order(&sc.cards) {
            let c = sc.cards[i];
            let pose = self.poses.get(&(w, i as u8)).copied().unwrap_or_else(|| Self::target_pose(&c));
            let mut f = self.pose_m(&pose);
            if self.motion {
                // Booted: a quick, decaying shake as it heads back.
                if let Some(t0) = self.shakes.get(&(w, i as u8)) {
                    let t = ((self.now - t0) / 1000.0) as f32;
                    let dx = (t * 48.0).sin() * 9.0 * (1.0 - t / 0.6).max(0.0);
                    f = M::t(dx, 0.0).then(f);
                }
                // Solved: one gentle pulse.
                let t = ((self.now - self.solved_at) / 1000.0) as f32;
                if (0.0..0.6).contains(&t) && matches!(c.loc, Loc::Slot(_)) {
                    let k = 1.0 + 0.07 * (t / 0.6 * PI).sin();
                    let (cx, cy) = f.apply(0.0, 0.0);
                    f = M::t(cx, cy).then(M::s(k)).then(M::t(-cx, -cy)).then(f);
                }
            }
            let held = holds.get(&i).copied();
            if held.is_some() || Some(i) == dragging {
                self.xform(M::t(5.0, 8.0).then(f));
                self.rrect(-HALF, -HALF, CARD, CARD, 18.0, 0x00000055, 0, 0.0);
            }
            self.xform(f);
            let solved = matches!(c.loc, Loc::Slot(s) if sc.locked & (1 << s) != 0) && phase != Phase::Writing;
            // A card I couldn't take: a pulse in the holder's colour.
            let refused = self.refused.filter(|r| r.0 == i && self.now - r.2 < 700.0);
            let (stroke, lw) = match held {
                _ if Some(i) == dragging => (0xffffffff, 4.0),
                _ if refused.is_some() => {
                    let (_, seat, t) = refused.unwrap();
                    (self.color_of(seat), 7.0 + 9.0 * (1.0 - ((self.now - t) / 700.0) as f32))
                }
                Some(h) => (self.color_of(h), 7.0),
                None if solved => (0x2fa84fff, 6.0),
                None => (self.pal.paper_edge, 2.0),
            };
            self.rrect(-HALF, -HALF, CARD, CARD, 18.0, self.pal.paper, stroke, lw);
            // Already rejected here, turned this way: a ✗ in the middle (where
            // no word goes), upright whatever the board's turn.
            if self.rejected_in(&sc, i).is_some() {
                let (cx, cy) = f.apply(0.0, 0.0);
                let (ex, ey) = f.apply(HALF, 0.0);
                self.xform(M::t(cx, cy).then(M::s((ex - cx).hypot(ey - cy) / HALF)));
                self.circle(0.0, 0.0, 30.0, self.pal.tried, self.pal.paper, 4.0);
                self.text(0.0, 2.0, 38.0, self.pal.paper, F_SANS_BOLD, "✗", 40.0);
            } else {
                self.circle(0.0, 0.0, 11.0, 0xeee4c8ff, 0, 0.0);
            }
            let layout = self.card_word_layout(&sc.words[i]);
            for s in 0..4u8 {
                self.xform(f.then(M::r(turn(s))));
                // Letters' tops sit WORD_EDGE from the edge at any size.
                let (size, off) = layout[s as usize];
                let y = -(HALF - WORD_EDGE - WORD_ASC * size);
                self.text(off, y, size, self.pal.ink, F_SANS_BOLD, sc.words[i][s as usize], 2.0 * WORD_REACH);
            }
        }

        // Hub: tap to turn the board.
        self.xform(board);
        self.circle(0.0, 0.0, HUB_R, self.pal.hub, self.pal.paper, 3.0);
        self.xform(M::t(l.bc.0, l.bc.1).then(M::s(l.k)));
        self.text(0.0, 1.0, 32.0, self.pal.paper, F_SANS_BOLD, "⟳", 44.0);

        // Other guessers' pointers.
        if matches!(phase, Phase::Guessing(_)) {
            let cursors: Vec<(u32, u8, f32, f32)> = self.cursors.values().map(|c| (c.seat, c.kind, c.x, c.y)).collect();
            for (seat, kind, x, y) in cursors {
                if seat == 0 {
                    continue;
                }
                let m = match kind {
                    1 => {
                        let (px, py) = self.tray_px(x, y);
                        M::t(px, py)
                    }
                    2 => self.board_m().then(M::t(x, y)).then(M::s(1.0 / l.k)),
                    _ => continue,
                };
                let col = self.color_of(seat);
                self.xform(m);
                self.circle(0.0, 0.0, 7.0, col, 0xffffffff, 2.5);
            }
        }
    }

    // ---- UI state for the HTML shell -------------------------------------------

    pub fn ui_json(&self) -> String {
        let s = &self.shared;
        let phase = self.view_phase();
        let sc = self.scene();
        let present = self.present();
        let writer = sc.writer.filter(|_| matches!(phase, Phase::Guessing(_)));
        let players = s.players();
        let rows: Vec<String> = players
            .iter()
            .map(|&p| {
                let status = match phase {
                    Phase::Writing if !s.round.players().contains(&p) => "next",
                    Phase::Writing if s.is_ready(p) => "ready",
                    Phase::Writing => "writing",
                    _ if s.round.custom == 0 && s.round.players().contains(&p) && !s.is_ready(p) => "writing",
                    Phase::Guessing(_) if writer == Some(p) => "writer",
                    _ => "",
                };
                format!(
                    "{{\"id\":{p},\"name\":{},\"color\":\"#{:06x}\",\"you\":{},\"away\":{},\"status\":\"{status}\"}}",
                    json_str(&self.name_of(p)),
                    self.color_of(p) >> 8,
                    self.seat == Some(p),
                    !present.contains(&p),
                )
            })
            .collect();
        let (pname, k, n) = match phase {
            Phase::Lobby => ("lobby", 0, 0),
            Phase::Writing => ("writing", 0, 0),
            Phase::Guessing(k) => ("guessing", k, s.puzzles().len()),
            Phase::End => ("end", 0, s.puzzles().len()),
        };
        // The round's progress: one entry per dealt-in player.
        let progress: Vec<String> = s
            .round_players()
            .iter()
            .map(|&p| {
                format!(
                    "{{\"name\":{},\"color\":\"#{:06x}\",\"ready\":{},\"away\":{}}}",
                    json_str(&self.name_of(p)),
                    self.color_of(p) >> 8,
                    s.is_ready(p),
                    !present.contains(&p)
                )
            })
            .collect();
        let boards: Vec<String> = s
            .puzzles()
            .iter()
            .map(|&w| format!("{{\"name\":{},\"solved\":{}}}", json_str(&self.name_of(w)), s.check(w).done()))
            .collect();
        let deal_names: Vec<String> = self.dealable().iter().map(|p| json_str(&self.name_of(*p))).collect();
        let sub = self.my_sub();
        let check = writer.map(|w| s.check(w)).unwrap_or_default();
        let seated = self.seat.is_some_and(|me| players.contains(&me));
        let in_round = self.seat.is_some_and(|me| s.round.players().contains(&me));
        format!(
            "{{\"phase\":\"{pname}\",\"seat\":{},\"seated\":{seated},\"name\":{},\"players\":[{}],\"watchers\":{},\
             \"inRound\":{in_round},\"ready\":{},\"canReady\":{},\"filled\":{},\"decoys\":{},\"progress\":[{}],\"readyCount\":{},\"dealable\":[{}],\
             \"k\":{k},\"n\":{n},\"writerName\":{},\"writerDecoys\":{},\"isWriter\":{},\"attempts\":{},\"done\":{},\
             \"canCheck\":{},\"claimed\":{},\"round\":{},\"boards\":[{}],\"strip\":{},\"menu\":{}}}",
            self.seat.map_or("null".into(), |x| x.to_string()),
            self.seat.map_or("null".into(), |x| json_str(&self.name_of(x))),
            rows.join(","),
            self.watchers(),
            sub.ready,
            self.can_ready(),
            sub.clues.iter().filter(|c| !c.is_empty()).count(),
            sub.decoys,
            progress.join(","),
            self.ready_count(),
            deal_names.join(","),
            writer.map_or("null".into(), |w| json_str(&self.name_of(w))),
            writer.map_or(0, |w| s.ncards(w) - NSLOTS),
            writer.is_some() && writer == self.seat,
            check.attempts,
            check.done(),
            self.can_check(),
            sc.claim.iter().filter(|c| c.is_some()).count(),
            s.round.n,
            boards.join(","),
            self.strip_json(),
            self.menu_json(),
        )
    }

    /// Text/background colour pairs actually used, for contrast checks.
    pub fn palette_json(&self) -> String {
        let pairs = [
            ("card words", self.pal.ink, self.pal.paper),
            ("clue on leaf", self.pal.clue_ink, self.pal.leaf),
            ("clue on glowing leaf", self.pal.clue_ink, self.pal.leaf_glow),
            ("clue on active leaf", self.pal.clue_ink, self.pal.leaf_active),
            ("keywords on leaf", self.pal.keyword_ink, self.pal.leaf),
            ("keywords on active leaf", self.pal.keyword_ink, self.pal.leaf_active),
            ("hub glyph", self.pal.paper, self.pal.hub),
            ("already-tried mark", self.pal.paper, self.pal.tried),
        ];
        let rows: Vec<String> =
            pairs.iter().map(|(n, f, b)| format!("[\"{n}\",\"#{:06x}\",\"#{:06x}\"]", f >> 8, b >> 8)).collect();
        format!("[{}]", rows.join(","))
    }

    // ---- harness probes ------------------------------------------------------

    /// Screen centre (CSS px) and on-screen angle of scene card `i`, at rest.
    pub fn card_screen(&self, i: usize) -> (f32, f32, f32) {
        let sc = self.scene();
        let Some(c) = sc.cards.get(i) else { return (-1.0, -1.0, 0.0) };
        let f = self.card_m(c);
        let (x, y) = f.apply(0.0, 0.0);
        (x, y, f.angle())
    }

    pub fn slot_screen(&self, s: usize) -> (f32, f32) {
        let (x, y) = slot_pos(s);
        self.board_m().apply(x, y)
    }

    pub fn board_screen(&self, x: f32, y: f32) -> (f32, f32) {
        self.board_m().apply(x, y)
    }

    pub fn petal_screen(&self, d: u8) -> (f32, f32) {
        self.leaf_m_at(d, self.angle).apply(0.0, Self::leaf_mid())
    }

    pub fn hub_screen(&self) -> (f32, f32) {
        self.layout().bc
    }

    pub fn shaking(&self) -> usize {
        self.shakes.len()
    }

    /// Displayed (animated) on-screen angle of card `i`.
    pub fn shown_angle(&self, i: usize) -> f32 {
        let sc = self.scene();
        let w = sc.writer.unwrap_or(0);
        self.poses.get(&(w, i as u8)).map_or(f32::NAN, |p| self.pose_m(p).angle())
    }
}

/// Minimise Σ(1 − x_j)² over x ∈ R⁴ subject to rows·x ≤ bound, exactly: the
/// optimum is the unique KKT point, found by trying every set of up to four
/// tight constraints (a few hundred 8×8 solves at most). None if infeasible.
fn min_squared_shrink(rows: &[([f32; 4], f32)]) -> Option<[f32; 4]> {
    let n = rows.len();
    let feasible = |x: &[f64; 4]| rows.iter().all(|(r, b)| (0..4).map(|j| r[j] as f64 * x[j]).sum::<f64>() <= *b as f64 + 1e-6);
    let mut best: Option<([f64; 4], f64)> = None;
    for mask in 0u32..(1 << n) {
        let act: Vec<usize> = (0..n).filter(|&i| mask & (1 << i) != 0).collect();
        if act.len() > 4 {
            continue;
        }
        // KKT: 2(x − 1) + Σ λ_i r_i = 0, r_i·x = b_i for tight i.
        let k = act.len();
        let size = 4 + k;
        let mut mtx = vec![vec![0.0f64; size + 1]; size];
        for j in 0..4 {
            mtx[j][j] = 2.0;
            for (t, &i) in act.iter().enumerate() {
                mtx[j][4 + t] = rows[i].0[j] as f64;
            }
            mtx[j][size] = 2.0;
        }
        for (t, &i) in act.iter().enumerate() {
            for j in 0..4 {
                mtx[4 + t][j] = rows[i].0[j] as f64;
            }
            mtx[4 + t][size] = rows[i].1 as f64;
        }
        let Some(sol) = gauss(mtx) else { continue };
        let x = [sol[0], sol[1], sol[2], sol[3]];
        if sol[4..].iter().any(|&l| l < -1e-9) || !feasible(&x) {
            continue;
        }
        let f: f64 = x.iter().map(|v| (1.0 - v).powi(2)).sum();
        if best.as_ref().is_none_or(|b| f < b.1 - 1e-12) {
            best = Some((x, f));
        }
    }
    best.map(|(x, _)| x.map(|v| v as f32))
}

/// Solve a square linear system given as an augmented matrix (partial
/// pivoting); None if singular.
fn gauss(mut m: Vec<Vec<f64>>) -> Option<Vec<f64>> {
    let n = m.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[piv][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, piv);
        for r in 0..n {
            if r != col {
                let f = m[r][col] / m[col][col];
                if f != 0.0 {
                    for c in col..=n {
                        m[r][c] -= f * m[col][c];
                    }
                }
            }
        }
    }
    Some((0..n).map(|r| m[r][n] / m[r][r]).collect())
}

/// The whole of a table as one packet (see `App::snapshot`).
fn snapshot_of(sh: &Shared) -> Vec<u8> {
    let mut b = Vec::new();
    for op in sh.all_ops() {
        encode(&Msg::Op(op), &mut b);
    }
    b
}

/// Clovers of `writers` in answer order (the share-link text), with guesses.
fn boards_json(sh: &Shared, writers: &[u32]) -> String {
    let name = |w: u32| sh.seats.get(&w).map(|s| s.name.clone()).unwrap_or_else(|| "someone".into());
    let rows: Vec<String> = writers
        .iter()
        .filter_map(|&w| {
            let text = sh.export_board(w, &name(w))?;
            let c = sh.check(w);
            Some(format!("{{\"text\":{},\"guesses\":{},\"solved\":{}}}", json_str(&text), c.attempts, c.done()))
        })
        .collect();
    format!("[{}]", rows.join(","))
}

/// A round for Past games, if anything was written in it: its key (table and
/// round), number, whether it finished, who was in it, and its clovers
/// (those being guessed, or while writing, whoever had written anything).
pub fn keep_json(sh: &Shared) -> Option<String> {
    let r = sh.round;
    let written = |w: &u32| sh.latest(*w).is_some_and(|x| x.clues.iter().any(|c| !c.is_empty()));
    if r.n == 0 || !(sh.start.is_some() || r.players().iter().any(written)) {
        return None;
    }
    let writers: Vec<u32> = if sh.start.is_some() { sh.puzzles() } else { r.players().iter().copied().filter(written).collect() };
    let name = |w: &u32| json_str(&sh.seats.get(w).map(|s| s.name.clone()).unwrap_or_else(|| "someone".into()));
    Some(format!(
        "{{\"key\":\"{}.{}/{}.{}\",\"n\":{},\"finished\":{},\"players\":[{}],\"boards\":{}}}",
        r.table.clock,
        r.table.peer,
        r.stamp.clock,
        r.stamp.peer,
        r.n,
        sh.phase() == Phase::End,
        r.players().iter().map(name).collect::<Vec<_>>().join(","),
        boards_json(sh, &writers)
    ))
}
