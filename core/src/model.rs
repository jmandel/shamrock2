//! Shared, replicated game state.
//!
//! Seats belong to the table and are only ever merged (last writer wins).
//! Everything else belongs to a round, and every op carries its round: the
//! higher-ranked round wins and resets round state (see `Round::rank`), and
//! a New game is just a round in the next epoch. Within a round each value
//! is a register merged by a monotone rule (LWW by Lamport stamp, or max for
//! counters). `apply` is commutative, associative and idempotent, so
//! replicas converge regardless of delivery order or duplication, and two
//! tables that meet (devices that started apart) merge rather than one
//! wiping the other.

use crate::words::CARDS;
use std::collections::{BTreeMap, BTreeSet};

pub const NSLOTS: usize = 4;
pub const MAX_DECOYS: u8 = 4;
pub const MAX_CARDS: usize = NSLOTS + MAX_DECOYS as usize;

// Board geometry, in board units (cards are 150 units).
pub const CARD: f32 = 210.0;
pub const HALF: f32 = CARD / 2.0;
pub const SLOT_OFF: f32 = 140.0;
/// Radius of the clover's round centre, where the four spots are.
pub const BOARD_HALF: f32 = 300.0;
/// Clue leaves sit between these distances from the centre.
pub const LEAF_IN: f32 = 290.0;
pub const LEAF_OUT: f32 = 400.0;
pub const LEAF_HALF_W: f32 = 205.0;
/// Half-extent of everything on the board, leaves included.
pub const EXTENT: f32 = 432.0;
/// Half-size the screen layout fits: just past the clue text on each leaf
/// (LEAF_OUT), so only the decorative lobe tips may touch the screen edge.
pub const FIT_EXTENT: f32 = LEAF_OUT + 10.0;
pub const HUB_R: f32 = 30.0;
/// Dropping within this distance of a free spot snaps into it.
pub const SNAP_R: f32 = 40.0;
/// A card within this distance of a spot counts as "in" it for a check.
pub const CLAIM_R: f32 = 100.0;

pub const MAX_TEXT: usize = 60;
/// The message format's version. Bump it whenever the wire format changes:
/// pages hearing a newer one ask their player to reload.
pub const PROTOCOL: u16 = 5;
/// Version 3 and 4 snapshots and invite deals still use readable records.
pub const MIN_PROTOCOL: u16 = 3;
pub const MAX_PLAYERS: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Hash)]
pub struct Stamp {
    pub clock: u64,
    pub peer: u32,
}

/// `Table` is a spot in the tray, normalised to 0..=1 along the tray's long
/// (x) and short (y) axes, so it fits every screen; it never turns with
/// anyone's view. `Slot` and `Float` are in board units and turn with the
/// board.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Loc {
    Table { x: f32, y: f32 },
    Slot(u8),
    Float { x: f32, y: f32 },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Card {
    pub loc: Loc,
    /// Clockwise quarter turns, relative to the frame `loc` lives in.
    pub rot: u8,
    pub stamp: Stamp,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Round {
    /// How many times the table has started a New game.
    pub epoch: u16,
    pub seed: u32,
    /// 1-based round number within the game (0 = no round yet).
    pub n: u16,
    pub stamp: Stamp,
    /// Seats dealt into this round (fixed when the round starts).
    pub seats: [u32; MAX_PLAYERS],
    pub nseats: u8,
    /// Where this round starts dealing in the game's deck (each dealt-in
    /// player takes `MAX_CARDS` cards).
    pub deck: u32,
    /// The table this round belongs to: the stamp of the table's very first
    /// deal, inherited by every round after it, across New games. Default
    /// until the first deal.
    pub table: Stamp,
    /// The round this one followed (default for a table's first deal).
    pub parent: Stamp,
    /// The seat that dealt it (0 = unknown).
    pub dealer: u32,
    /// 1 = this round guesses the imported clover (see `Custom`).
    pub custom: u8,
}

impl Round {
    /// Which of two rounds wins when they meet: a later epoch (New game)
    /// always; then the game further along; then the deal with more players
    /// in it; then the older. So a lone device that dealt before hearing
    /// from its table can't overwrite a table in play.
    pub fn rank(&self) -> (u16, u16, u8, core::cmp::Reverse<Stamp>) {
        (self.epoch, self.n, self.nseats, core::cmp::Reverse(self.stamp))
    }
    pub fn players(&self) -> &[u32] {
        &self.seats[..(self.nseats as usize).min(MAX_PLAYERS)]
    }
    /// The game's deck shuffle: one per table and epoch.
    pub fn deck_seed(&self) -> u32 {
        let k = self.table.clock ^ ((self.table.peer as u64) << 32) ^ (self.epoch as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        (k ^ (k >> 32)) as u32
    }
    pub fn with_players(mut self, players: &[u32]) -> Round {
        let n = players.len().min(MAX_PLAYERS);
        self.seats = [0; MAX_PLAYERS];
        self.seats[..n].copy_from_slice(&players[..n]);
        self.nseats = n as u8;
        self
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Seat {
    pub name: String,
    pub active: bool,
    pub stamp: Stamp,
}

/// A writer's clues (drafts until `ready`) and decoy bid.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Sub {
    pub clues: [String; NSLOTS],
    pub decoys: u8,
    pub ready: bool,
    pub stamp: Stamp,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Check {
    pub attempts: u8,
    /// Bit s set = spot s was right at the last check (and is now locked).
    pub mask: u8,
    pub stamp: Stamp,
    /// Every placement a check has rejected: card, spot and turn (relative
    /// to the board), one bit each (see `Check::bit`). It only grows, and
    /// merges as a union, so two checks made at once both count.
    pub tried: u128,
}

impl Check {
    /// Solved: guesses keep going until every spot is right (no scoring).
    pub fn done(&self) -> bool {
        self.mask == 0b1111
    }
    /// The bit for card `card` in spot `spot` turned `rot`.
    pub fn bit(card: usize, spot: usize, rot: u8) -> u128 {
        1u128 << ((card * NSLOTS + spot) * 4 + (rot as usize & 3))
    }
    /// Whether that placement was already rejected.
    pub fn rejected(&self, card: usize, spot: usize, rot: u8) -> bool {
        card < MAX_CARDS && spot < NSLOTS && self.tried & Self::bit(card, spot, rot) != 0
    }
}

/// Where a group that played at this room went: its table moved to `room`.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Moved {
    pub room: String,
    /// The seat that moved it.
    pub by: u32,
    pub stamp: Stamp,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Body {
    /// Just announces the round.
    None,
    Seat(u32, Seat),
    /// The group whose table id this is moved to another room.
    Move(Stamp, Moved),
    Sub(u32, Sub),
    /// Guessing begins; unfinished writers may still submit their clovers.
    Start(Stamp),
    Card(u32, u8, Card),
    Check(u32, Check),
    /// Index of the clover being guessed (last writer wins, so anyone can
    /// jump to any clover; past the last one means the round is over).
    Next(u16, Stamp),
    /// Clover identified by its writer, or 0 once all available clovers
    /// are done. Late submissions cannot change what this points at.
    Focus(u32, Stamp),
    /// Left unchecked when this round was dealt. Copied into the next deal
    /// until explicitly selected again; independent of connection presence.
    SitOut(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Selection {
    /// Kept for reading version 3 saves.
    Index(u16),
    Writer(u32),
}

impl Default for Selection {
    fn default() -> Self { Self::Index(0) }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Op {
    pub round: Round,
    pub body: Body,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Lobby,
    Writing,
    /// Index into `puzzles()`.
    Guessing(usize),
    End,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Shared {
    pub seats: BTreeMap<u32, Seat>,
    /// Groups that moved out of this room: their table id → where to.
    pub moves: BTreeMap<Stamp, Moved>,
    pub round: Round,
    /// Every version of every writer's clues, keyed (writer, stamp). Keeping
    /// versions defines the submission frozen at Start, or the first ready
    /// version after Start for a late writer, even when messages race.
    pub subs: BTreeMap<(u32, Stamp), Sub>,
    pub start: Option<Stamp>,
    pub cards: BTreeMap<(u32, u8), Card>,
    pub checks: BTreeMap<u32, Check>,
    pub next: (Selection, Stamp),
    pub sitting_out: BTreeSet<u32>,
}

/// A clover brought in from elsewhere (e.g. read off a screenshot of an
/// unfinished game), guessed as an ordinary clover. Every device in the room
/// loads the same file, so it behaves like shared state without being sent.
#[derive(Debug)]
pub struct Custom {
    /// Who wrote it, shown as the clover's author.
    pub name: String,
    /// Top, right, bottom, left.
    pub clues: [String; NSLOTS],
    /// Cards 0..4 are the answer (card s in spot s, words clockwise from the
    /// top as they sit there); the rest are decoys.
    pub cards: Vec<[&'static str; 4]>,
    writer: u32,
}

static CUSTOMS: std::sync::Mutex<Vec<&'static Custom>> = std::sync::Mutex::new(Vec::new());

/// Every word on an imported clover's cards (for measuring).
pub fn custom_words() -> Vec<&'static str> {
    CUSTOMS.lock().map(|c| c.iter().flat_map(|x| x.cards.iter().flatten().copied()).collect()).unwrap_or_default()
}

impl Custom {
    /// Each imported clover's author gets a seat id no typed name can
    /// produce (and two imports by the same name stay distinct).
    pub fn writer(&self) -> u32 {
        self.writer
    }

    /// Format: name, then four clues, then one card per line as `a|b|c|d`
    /// (at least four cards). Several clovers may be loaded; they're guessed
    /// in load order. Returns false (and keeps nothing) if malformed.
    pub fn load(text: &str) -> bool {
        let Some(text) = unscramble(text) else { return false };
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let Some(name) = lines.next() else { return false };
        let clues: Vec<String> = lines.by_ref().take(NSLOTS).map(|c| clamp_text(c, MAX_TEXT)).collect();
        let cards: Vec<[&'static str; 4]> = lines
            .take(MAX_CARDS)
            .filter_map(|l| {
                let w: Vec<&'static str> = l.split('|').map(|x| &*Box::leak(x.trim().to_string().into_boxed_str())).collect();
                w.try_into().ok()
            })
            .collect();
        if clues.len() < NSLOTS || cards.len() < NSLOTS {
            return false;
        }
        let mut all = CUSTOMS.lock().unwrap();
        let name = clamp_text(name, 40);
        let writer = seat_id(&format!("\u{1}{}#{}", name, all.len()));
        all.push(Box::leak(Box::new(Custom { name, clues: core::array::from_fn(|i| clues[i].clone()), cards, writer })));
        true
    }

    pub fn all() -> Vec<&'static Custom> {
        CUSTOMS.lock().unwrap().clone()
    }

    pub fn by_writer(w: u32) -> Option<&'static Custom> {
        Self::all().into_iter().find(|c| c.writer == w)
    }
}

/// A card's words (clockwise from the top) after `r` clockwise quarter turns.
pub fn turn_words<T: Copy>(w: [T; 4], r: u8) -> [T; 4] {
    core::array::from_fn(|d| w[(d + 4 - r as usize % 4) % 4])
}

/// Board text in answer order: name, four clues (top, right, bottom, left),
/// the answer cards for spots 0..4 as they sit, then decoys.
pub fn board_text(name: &str, clues: &[String; NSLOTS], cards: &[[&str; 4]]) -> String {
    let mut lines = vec![name.to_string()];
    lines.extend(clues.iter().cloned());
    lines.extend(cards.iter().map(|c| c.join("|")));
    lines.join("\n")
}

/// Make board text safe to pass around: cards shuffled and each randomly
/// turned, with the answer kept on a separate last line, `@ i r i r i r i r`
/// (for spots 0..4: which listed card, and how far it was turned).
pub fn scramble(text: &str, rng: &mut Rng) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 1 + NSLOTS + NSLOTS {
        return text.to_string();
    }
    let cards: Vec<[&str; 4]> =
        lines[1 + NSLOTS..].iter().filter_map(|l| l.split('|').collect::<Vec<_>>().try_into().ok()).collect();
    let mut order: Vec<usize> = (0..cards.len()).collect();
    for k in (1..order.len()).rev() {
        order.swap(k, rng.below(k + 1));
    }
    let turns: Vec<u8> = cards.iter().map(|_| rng.below(4) as u8).collect();
    let mut out: Vec<String> = lines[..1 + NSLOTS].iter().map(|l| l.to_string()).collect();
    out.extend(order.iter().map(|&i| turn_words(cards[i], turns[i]).join("|")));
    let answer: Vec<String> = (0..NSLOTS)
        .map(|spot| format!("{} {}", order.iter().position(|&i| i == spot).unwrap(), turns[spot]))
        .collect();
    out.push(format!("@ {}", answer.join(" ")));
    out.join("\n")
}

/// Undo `scramble` (text without an `@` line is already in answer order).
pub fn unscramble(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let Some(last) = lines.last().and_then(|l| l.strip_prefix('@')) else { return Some(text.to_string()) };
    let nums: Vec<usize> = last.split_whitespace().map(|x| x.parse().ok()).collect::<Option<_>>()?;
    if nums.len() != 2 * NSLOTS || lines.len() < 2 + NSLOTS + NSLOTS {
        return None;
    }
    let cards: Vec<[&str; 4]> = lines[1 + NSLOTS..lines.len() - 1]
        .iter()
        .map(|l| l.split('|').collect::<Vec<_>>().try_into().ok())
        .collect::<Option<_>>()?;
    let mut used = vec![false; cards.len()];
    let mut solved = Vec::new();
    for s in 0..NSLOTS {
        let (i, r) = (nums[2 * s], nums[2 * s + 1] as u8);
        if *used.get(i)? || r > 3 {
            return None;
        }
        used[i] = true;
        solved.push(turn_words(cards[i], (4 - r) % 4));
    }
    solved.extend(cards.iter().zip(&used).filter(|(_, u)| !**u).map(|(c, _)| *c));
    let clues: [String; NSLOTS] = core::array::from_fn(|i| lines[1 + i].to_string());
    Some(board_text(lines[0], &clues, &solved))
}

/// Seat id for a display name: names are identities, so choosing an
/// existing name on another device takes that seat over.
pub fn seat_id(name: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.trim().to_lowercase().bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    h.max(1)
}

/// Everything about one writer's clover that is derived from the seed.
pub struct Deal {
    pub words: [[&'static str; 4]; MAX_CARDS],
    /// Rotation of card s in spot s (the answer). Card s belongs in spot s.
    pub sol_rot: [u8; NSLOTS],
    /// Shuffle order for tray spots, and starting rotations.
    pub home: [u8; MAX_CARDS],
    pub home_rot: [u8; MAX_CARDS],
}

/// The game's draw-down deck: card at deck position `pos`. Each pass
/// through the deck is a fresh shuffle seeded by the game, so cards don't
/// repeat until the deck runs out.
pub fn deck_card(seed: u32, pos: u32) -> usize {
    let n = CARDS.len();
    let (pass, at) = (pos as usize / n, pos as usize % n);
    let mut rng = Rng::new(seed as u64 ^ (pass as u64).wrapping_mul(0xa076_1d64_78bd_642f));
    let mut perm: Vec<usize> = (0..n).collect();
    for k in (1..n).rev() {
        perm.swap(k, rng.below(k + 1));
    }
    perm[at]
}

/// Loose cards per tray row (portrait) or column (landscape).
pub const TRAY_PER_LINE: usize = 3;

/// Tray spot `k` of `n`: rows of up to three along the tray's long axis,
/// so loose cards don't cover each other's words even on a phone.
pub fn tray_spot(k: usize, n: usize) -> (f32, f32) {
    const PER_ROW: usize = TRAY_PER_LINE;
    let rows = n.div_ceil(PER_ROW).max(1);
    let (row, col) = (k / PER_ROW, k % PER_ROW);
    let in_row = (n - row * PER_ROW).min(PER_ROW);
    let x = 0.5 + (col as f32 - (in_row as f32 - 1.0) / 2.0) / (PER_ROW - 1) as f32;
    let y = if rows == 1 { 0.5 } else { row as f32 / (rows - 1) as f32 };
    (x, y)
}

pub fn slot_pos(s: usize) -> (f32, f32) {
    const P: [(f32, f32); NSLOTS] =
        [(-SLOT_OFF, -SLOT_OFF), (SLOT_OFF, -SLOT_OFF), (SLOT_OFF, SLOT_OFF), (-SLOT_OFF, SLOT_OFF)];
    P[s]
}

/// Board position of a card, if it is on the board.
pub fn board_pos(c: &Card) -> Option<(f32, f32)> {
    match c.loc {
        Loc::Slot(s) => Some(slot_pos(s as usize)),
        Loc::Float { x, y } => Some((x, y)),
        Loc::Table { .. } => None,
    }
}

impl Shared {
    /// Merge one op. Returns true if anything changed.
    pub fn apply(&mut self, op: &Op) -> bool {
        let mut changed = false;
        // Seats belong to the table: merged whatever round the op is from.
        if let Body::Seat(id, seat) = &op.body {
            if self.seats.get(id).is_none_or(|c| seat.stamp > c.stamp) {
                self.seats.insert(*id, seat.clone());
                changed = true;
            }
        }
        if let Body::Move(t, m) = &op.body {
            if self.moves.get(t).is_none_or(|c| m.stamp > c.stamp) {
                self.moves.insert(*t, m.clone());
                changed = true;
            }
        }
        if op.round.rank() > self.round.rank() {
            self.round = op.round;
            self.subs.clear();
            self.start = None;
            self.cards.clear();
            self.checks.clear();
            self.next = Default::default();
            self.sitting_out.clear();
            changed = true;
        }
        if op.round.stamp != self.round.stamp {
            return changed;
        }
        let newer = match &op.body {
            Body::SitOut(p) => self.sitting_out.insert(*p),
            Body::Sub(w, s) => {
                self.subs.insert((*w, s.stamp), s.clone()).is_none()
            }
            Body::Start(s) => {
                let n = self.start.is_none_or(|c| *s > c);
                if n {
                    self.start = Some(*s);
                }
                n
            }
            Body::Card(w, i, c) => {
                let n = self.cards.get(&(*w, *i)).is_none_or(|o| c.stamp > o.stamp);
                if n {
                    self.cards.insert((*w, *i), *c);
                }
                n
            }
            Body::Check(w, c) => {
                // The newest check wins; what was rejected is kept from both.
                let old = self.checks.get(w).copied();
                let newer = old.is_none_or(|o| (c.attempts, c.stamp) > (o.attempts, o.stamp));
                let tried = c.tried | old.map_or(0, |o| o.tried);
                let next = if newer { Check { tried, ..*c } } else { Check { tried, ..old.unwrap() } };
                let n = old != Some(next);
                self.checks.insert(*w, next);
                n
            }
            Body::Next(_, s) | Body::Focus(_, s) => {
                let selection = match op.body {
                    Body::Next(k, _) => Selection::Index(k),
                    Body::Focus(w, _) => Selection::Writer(w),
                    _ => unreachable!(),
                };
                let n = (*s, selection) > (self.next.1, self.next.0);
                if n {
                    self.next = (selection, *s);
                }
                n
            }
            _ => false,
        };
        changed || newer
    }

    /// Every stored register, for full-state sync with a new peer.
    pub fn all_ops(&self) -> Vec<Op> {
        let r = self.round;
        let op = |body| Op { round: r, body };
        let mut v = vec![op(Body::None)];
        v.extend(self.seats.iter().map(|(id, s)| op(Body::Seat(*id, s.clone()))));
        v.extend(self.moves.iter().map(|(t, m)| op(Body::Move(*t, m.clone()))));
        v.extend(self.subs.iter().map(|((w, _), s)| op(Body::Sub(*w, s.clone()))));
        v.extend(self.start.map(|s| op(Body::Start(s))));
        v.extend(self.cards.iter().map(|((w, i), c)| op(Body::Card(*w, *i, *c))));
        v.extend(self.checks.iter().map(|(w, c)| op(Body::Check(*w, *c))));
        v.extend(self.sitting_out.iter().map(|p| op(Body::SitOut(*p))));
        if self.next.1 != Stamp::default() {
            v.push(op(match self.next.0 {
                Selection::Index(k) => Body::Next(k, self.next.1),
                Selection::Writer(w) => Body::Focus(w, self.next.1),
            }));
        }
        v
    }

    /// Active seats, by id.
    pub fn players(&self) -> Vec<u32> {
        self.seats.iter().filter(|(_, s)| s.active).map(|(id, _)| *id).collect()
    }

    /// What `writer` was dealt this round: whole cards from the deck.
    pub fn deal(&self, writer: u32) -> Deal {
        let seat = self.round.players().iter().position(|p| *p == writer).unwrap_or(writer as usize % MAX_PLAYERS);
        let base = self.round.deck + (seat * MAX_CARDS) as u32;
        let cards: [usize; MAX_CARDS] = core::array::from_fn(|i| deck_card(self.round.deck_seed(), base + i as u32));
        let mut rng = Rng::new(((self.round.seed as u64) << 32 | writer as u64) ^ 0x9e37_79b9_7f4a_7c15);
        let mut home: [u8; MAX_CARDS] = core::array::from_fn(|i| i as u8);
        for k in (1..MAX_CARDS).rev() {
            home.swap(k, rng.below(k + 1));
        }
        if let Some(c) = Custom::by_writer(writer).filter(|_| self.round.custom == 1) {
            // The imported answer is exactly as it was laid out: no rotation.
            return Deal {
                words: core::array::from_fn(|i| c.cards.get(i).copied().unwrap_or(CARDS[cards[i]])),
                sol_rot: [0; NSLOTS],
                home,
                home_rot: core::array::from_fn(|_| rng.below(4) as u8),
            };
        }
        Deal {
            words: core::array::from_fn(|c| CARDS[cards[c]]),
            sol_rot: core::array::from_fn(|_| rng.below(4) as u8),
            home,
            home_rot: core::array::from_fn(|_| rng.below(4) as u8),
        }
    }

    /// The clover `w` as it sits solved: clues, then the answer cards for
    /// spots 0..4 with words clockwise from the top as they sit, then decoys.
    /// This is exactly `Custom::load`'s format, so boards can be shared.
    pub fn export_board(&self, w: u32, name: &str) -> Option<String> {
        let sub = self.effective(w)?;
        let d = self.deal(w);
        let solved: Vec<[&str; 4]> = (0..self.ncards(w))
            .map(|i| if i < NSLOTS { turn_words(d.words[i], d.sol_rot[i]) } else { d.words[i] })
            .collect();
        Some(board_text(name, &sub.clues, &solved))
    }

    /// A writer's latest clues (while writing).
    pub fn latest(&self, w: u32) -> Option<&Sub> {
        self.subs.range((w, Stamp::default())..=(w, Stamp { clock: u64::MAX, peer: u32::MAX })).next_back().map(|(_, s)| s)
    }

    /// Freeze ready clues at Start. An unfinished writer can join later:
    /// their first ready version after Start freezes their clover instead.
    pub fn effective(&self, w: u32) -> Option<&Sub> {
        match self.start {
            Some(start) => {
                let before = self.subs.range((w, Stamp::default())..(w, start)).next_back().map(|(_, s)| s);
                if before.is_some_and(|s| s.ready) { return before; }
                self.subs.range((w, start)..=(w, Stamp { clock: u64::MAX, peer: u32::MAX }))
                    .find(|(_, s)| s.ready).map(|(_, s)| s).or_else(|| self.latest(w))
            }
            None => self.latest(w),
        }
    }

    pub fn is_ready(&self, w: u32) -> bool {
        self.effective(w).is_some_and(|s| s.ready)
    }

    /// Players in this round who are still seated.
    pub fn round_players(&self) -> Vec<u32> {
        self.round.players().iter().copied().filter(|p| self.seats.get(p).is_some_and(|s| s.active)).collect()
    }

    /// Writers whose clovers get guessed, in deal order: those who had
    /// submitted when guessing started or have submitted since.
    pub fn puzzles(&self) -> Vec<u32> {
        if self.start.is_none() {
            return Vec::new();
        }
        self.round.players().iter().copied().filter(|w| self.is_ready(*w)).collect()
    }

    pub fn phase(&self) -> Phase {
        if self.round.n == 0 {
            Phase::Lobby
        } else if self.start.is_none() {
            Phase::Writing
        } else {
            let puzzles = self.puzzles();
            let k = match self.next.0 {
                Selection::Index(k) => ((k as usize) < puzzles.len()).then_some(k as usize),
                Selection::Writer(0) => puzzles.iter().position(|w| !self.check(*w).done()),
                Selection::Writer(w) => puzzles.iter().position(|p| *p == w),
            };
            k.map_or(Phase::End, Phase::Guessing)
        }
    }

    /// The clover to guess after `k`: the next unsolved one, wrapping round;
    /// past the end once every clover is solved.
    pub fn next_unsolved(&self, k: usize) -> usize {
        let p = self.puzzles();
        (1..=p.len()).map(|d| (k + d) % p.len()).find(|&i| !self.check(p[i]).done()).unwrap_or(p.len())
    }

    pub fn check(&self, writer: u32) -> Check {
        self.checks.get(&writer).copied().unwrap_or_default()
    }

    pub fn ncards(&self, writer: u32) -> usize {
        NSLOTS + self.effective(writer).map_or(1, |s| s.decoys.min(MAX_DECOYS)) as usize
    }

    pub fn home_card(&self, d: &Deal, n: usize, i: usize) -> Card {
        // Rank of card i among the n cards in play gives its shuffled spot.
        let k = (0..n).filter(|&j| d.home[j] < d.home[i]).count();
        let (x, y) = tray_spot(k, n);
        Card { loc: Loc::Table { x, y }, rot: d.home_rot[i], stamp: Stamp::default() }
    }

    /// Effective cards of `writer`'s clover while it is being guessed (locked
    /// spots enforced, answer shown once done) and which card claims each
    /// spot for a check: the nearest within `CLAIM_R`.
    pub fn layout(&self, writer: u32) -> (Vec<Card>, [Option<usize>; NSLOTS]) {
        let d = self.deal(writer);
        let n = self.ncards(writer);
        let check = self.check(writer);
        let locked = |s: usize| check.done() || check.mask & (1 << s) != 0;
        let cards: Vec<Card> = (0..n)
            .map(|i| {
                if i < NSLOTS && locked(i) {
                    Card { loc: Loc::Slot(i as u8), rot: d.sol_rot[i], stamp: Stamp::default() }
                } else if check.done() {
                    // Once done, decoys go back to the tray to show the answer.
                    self.home_card(&d, n, i)
                } else {
                    self.cards.get(&(writer, i as u8)).copied().unwrap_or_else(|| self.home_card(&d, n, i))
                }
            })
            .collect();
        let mut claim = [None::<usize>; NSLOTS];
        for (s, cl) in claim.iter_mut().enumerate() {
            if locked(s) {
                *cl = Some(s);
                continue;
            }
            let (sx, sy) = slot_pos(s);
            *cl = (0..n)
                .filter(|&i| !(i < NSLOTS && locked(i)))
                .filter_map(|i| {
                    let (x, y) = board_pos(&cards[i])?;
                    let dist = (x - sx).hypot(y - sy);
                    (dist <= CLAIM_R).then_some((dist.to_bits(), core::cmp::Reverse(cards[i].stamp), i))
                })
                .min()
                .map(|t| t.2);
        }
        (cards, claim)
    }

    /// Canonical rendering of the shared state; equal digests == converged.
    pub fn digest(&self) -> String {
        let phase = match self.phase() {
            Phase::Lobby => "lobby".to_string(),
            Phase::Writing => "writing".to_string(),
            Phase::Guessing(k) => format!("guessing:{k}"),
            Phase::End => "end".to_string(),
        };
        let seats: Vec<String> =
            self.seats.iter().map(|(id, s)| format!("[{id},{},{}]", json_str(&s.name), s.active)).collect();
        let moves: Vec<String> = self.moves.iter().map(|(t, m)| format!("[\"{}.{}\",{}]", t.clock, t.peer, json_str(&m.room))).collect();
        let subs: Vec<String> = self
            .round
            .players()
            .iter()
            .filter_map(|w| self.effective(*w).map(|s| (w, s)))
            .map(|(w, s)| {
                let clues: Vec<String> = s.clues.iter().map(|c| json_str(c)).collect();
                format!("[{w},{},{},{}]", s.decoys, s.ready, clues.join(","))
            })
            .collect();
        let f = |v: f32| format!("{v:.2}");
        let puzzles: Vec<String> = self
            .puzzles()
            .iter()
            .map(|&w| {
                let (cards, claim) = self.layout(w);
                let c = self.check(w);
                let cards: Vec<String> = cards
                    .iter()
                    .map(|c| match c.loc {
                        Loc::Table { x, y } => format!("[\"table\",{},{},{}]", f(x), f(y), c.rot),
                        Loc::Slot(s) => format!("[\"slot\",{s},{}]", c.rot),
                        Loc::Float { x, y } => format!("[\"float\",{},{},{}]", f(x), f(y), c.rot),
                    })
                    .collect();
                let claim: Vec<String> = claim.iter().map(|o| o.map_or("null".into(), |i| i.to_string())).collect();
                format!(
                    "{{\"writer\":{w},\"attempts\":{},\"mask\":{},\"tried\":\"{:x}\",\"claim\":[{}],\"cards\":[{}]}}",
                    c.attempts,
                    c.mask,
                    c.tried,
                    claim.join(","),
                    cards.join(",")
                )
            })
            .collect();
        format!(
            "{{\"phase\":\"{phase}\",\"epoch\":{},\"round\":{},\"seed\":{},\"dealt\":{:?},\"sittingOut\":{:?},\"seats\":[{}],\"moves\":[{}],\"subs\":[{}],\"puzzles\":[{}]}}",
            self.round.epoch,
            self.round.n,
            self.round.seed,
            self.round.players(),
            self.sitting_out.iter().collect::<Vec<_>>(),
            seats.join(","),
            moves.join(","),
            subs.join(","),
            puzzles.join(",")
        )
    }
}

pub fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' | '\\' => {
                out.push('\\');
                out.push(ch)
            }
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    /// SplitMix64.
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

// ---------------------------------------------------------------------------
// Wire format: a packet is a concatenation of records, each `[tag u8] body`.
// All integers little-endian. Op records start with the round they belong to.

const T_NONE: u8 = 1;
const T_SUB: u8 = 2;
const T_START: u8 = 3;
const T_CARD: u8 = 4;
const T_CHECK: u8 = 5;
const T_NEXT: u8 = 6;
const T_SEAT: u8 = 7;
const T_MOVE: u8 = 8;
const T_FOCUS: u8 = 9;
const T_SIT_OUT: u8 = 10;
const T_HELLO: u8 = 20;
const T_CURSOR: u8 = 21;

#[derive(Clone, PartialEq, Debug)]
pub enum Msg {
    Op(Op),
    /// Presence heartbeat: a device and the seat it plays (0 = watching).
    /// `sum`: a hash of the sender's whole table, so devices that have
    /// drifted apart notice and resync (see `App::on_packet`).
    /// `ver`: the sender's `PROTOCOL`, so a page on an older one knows to reload.
    Hello { device: u32, seat: u32, sum: u32, ver: u16 },
    /// kind: 0 hidden, 1 tray coords, 2 board coords.
    /// `card`: the card (of the clover on screen) this pointer is dragging,
    /// if any. Holds live here, not in the table: they end when the device
    /// goes quiet or leaves.
    Cursor { device: u32, seat: u32, kind: u8, x: f32, y: f32, card: Option<u8> },
}

fn put_stamp(b: &mut Vec<u8>, s: Stamp) {
    b.extend_from_slice(&s.clock.to_le_bytes());
    b.extend_from_slice(&s.peer.to_le_bytes());
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    let s = clamp_text(s, MAX_TEXT);
    b.push(s.len() as u8);
    b.extend_from_slice(s.as_bytes());
}

fn put_f32s(b: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
}

pub fn encode(m: &Msg, b: &mut Vec<u8>) {
    match m {
        Msg::Op(Op { round, body }) => {
            b.push(match body {
                Body::None => T_NONE,
                Body::Sub(..) => T_SUB,
                Body::Start(..) => T_START,
                Body::Card(..) => T_CARD,
                Body::Check(..) => T_CHECK,
                Body::Next(..) => T_NEXT,
                Body::Focus(..) => T_FOCUS,
                Body::SitOut(..) => T_SIT_OUT,
                Body::Seat(..) => T_SEAT,
                Body::Move(..) => T_MOVE,
            });
            b.extend_from_slice(&round.epoch.to_le_bytes());
            put_stamp(b, round.stamp);
            b.extend_from_slice(&round.seed.to_le_bytes());
            b.extend_from_slice(&round.n.to_le_bytes());
            b.push(round.nseats);
            for p in round.players() {
                b.extend_from_slice(&p.to_le_bytes());
            }
            b.extend_from_slice(&round.deck.to_le_bytes());
            put_stamp(b, round.table);
            put_stamp(b, round.parent);
            b.extend_from_slice(&round.dealer.to_le_bytes());
            b.push(round.custom);
            match body {
                Body::None => {}
                Body::SitOut(p) => b.extend_from_slice(&p.to_le_bytes()),
                Body::Seat(id, s) => {
                    b.extend_from_slice(&id.to_le_bytes());
                    put_stamp(b, s.stamp);
                    b.push(s.active as u8);
                    put_str(b, &s.name);
                }
                Body::Move(t, m) => {
                    put_stamp(b, *t);
                    put_stamp(b, m.stamp);
                    b.extend_from_slice(&m.by.to_le_bytes());
                    put_str(b, &m.room);
                }
                Body::Sub(w, s) => {
                    b.extend_from_slice(&w.to_le_bytes());
                    put_stamp(b, s.stamp);
                    b.push(s.decoys);
                    b.push(s.ready as u8);
                    for c in &s.clues {
                        put_str(b, c);
                    }
                }
                Body::Start(s) => put_stamp(b, *s),
                Body::Card(w, i, c) => {
                    b.extend_from_slice(&w.to_le_bytes());
                    b.push(*i);
                    put_stamp(b, c.stamp);
                    let (k, x, y) = match c.loc {
                        Loc::Table { x, y } => (0u8, x, y),
                        Loc::Slot(s) => (1, s as f32, 0.0),
                        Loc::Float { x, y } => (2, x, y),
                    };
                    b.push(k);
                    put_f32s(b, &[x, y]);
                    b.push(c.rot);
                }
                Body::Check(w, c) => {
                    b.extend_from_slice(&w.to_le_bytes());
                    put_stamp(b, c.stamp);
                    b.push(c.attempts);
                    b.push(c.mask);
                    b.extend_from_slice(&c.tried.to_le_bytes());
                }
                Body::Next(k, s) => {
                    b.extend_from_slice(&k.to_le_bytes());
                    put_stamp(b, *s);
                }
                Body::Focus(w, s) => {
                    b.extend_from_slice(&w.to_le_bytes());
                    put_stamp(b, *s);
                }
            }
        }
        Msg::Hello { device, seat, sum, ver } => {
            b.push(T_HELLO);
            b.extend_from_slice(&device.to_le_bytes());
            b.extend_from_slice(&seat.to_le_bytes());
            b.extend_from_slice(&sum.to_le_bytes());
            b.extend_from_slice(&ver.to_le_bytes());
        }
        Msg::Cursor { device, seat, kind, x, y, card } => {
            b.push(T_CURSOR);
            b.extend_from_slice(&device.to_le_bytes());
            b.extend_from_slice(&seat.to_le_bytes());
            b.push(*kind);
            put_f32s(b, &[*x, *y]);
            b.push(card.unwrap_or(u8::MAX));
        }
    }
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        let v = f32::from_le_bytes(self.take(4)?.try_into().ok()?);
        (v.is_finite() && v.abs() < 1e5).then_some(v)
    }
    fn stamp(&mut self) -> Option<Stamp> {
        Some(Stamp { clock: self.u64()?, peer: self.u32()? })
    }
    fn lt(&mut self, max: u8) -> Option<u8> {
        self.u8().filter(|v| *v < max)
    }
    fn bool(&mut self) -> Option<bool> {
        Some(self.lt(2)? == 1)
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        if n > MAX_TEXT {
            return None;
        }
        Some(core::str::from_utf8(self.take(n)?).ok()?.to_string())
    }

    fn msg(&mut self) -> Option<Msg> {
        let tag = self.u8()?;
        if tag == T_HELLO {
            return Some(Msg::Hello { device: self.u32()?, seat: self.u32()?, sum: self.u32()?, ver: self.u16()? });
        }
        if tag == T_CURSOR {
            let (device, seat) = (self.u32()?, self.u32()?);
            let kind = self.lt(3)?;
            let (x, y) = (self.f32()?, self.f32()?);
            let card = Some(self.u8()?).filter(|i| (*i as usize) < MAX_CARDS);
            return Some(Msg::Cursor { device, seat, kind, x, y, card });
        }
        let epoch = self.u16()?;
        let (stamp, seed, n) = (self.stamp()?, self.u32()?, self.u16()?);
        let nseats = self.u8().filter(|n| (*n as usize) <= MAX_PLAYERS)?;
        let mut seats = [0u32; MAX_PLAYERS];
        for p in seats.iter_mut().take(nseats as usize) {
            *p = self.u32()?;
        }
        let deck = self.u32()?;
        let (table, parent, dealer) = (self.stamp()?, self.stamp()?, self.u32()?);
        let round = Round { epoch, stamp, seed, n, seats, nseats, deck, table, parent, dealer, custom: self.lt(2)? };
        let body = match tag {
            T_NONE => Body::None,
            T_SIT_OUT => Body::SitOut(self.u32()?),
            T_SEAT => {
                let id = self.u32()?;
                let stamp = self.stamp()?;
                let active = self.bool()?;
                Body::Seat(id, Seat { name: self.str()?, active, stamp })
            }
            T_MOVE => {
                let (t, stamp, by) = (self.stamp()?, self.stamp()?, self.u32()?);
                Body::Move(t, Moved { room: self.str()?, by, stamp })
            }
            T_SUB => {
                let w = self.u32()?;
                let stamp = self.stamp()?;
                let decoys = self.u8().filter(|d| *d <= MAX_DECOYS)?;
                let ready = self.bool()?;
                let mut clues: [String; NSLOTS] = Default::default();
                for c in clues.iter_mut() {
                    *c = self.str()?;
                }
                Body::Sub(w, Sub { clues, decoys, ready, stamp })
            }
            T_START => Body::Start(self.stamp()?),
            T_CARD => {
                let w = self.u32()?;
                let i = self.lt(MAX_CARDS as u8)?;
                let stamp = self.stamp()?;
                let k = self.u8()?;
                let (x, y) = (self.f32()?, self.f32()?);
                let loc = match k {
                    0 => Loc::Table { x: x.clamp(0.0, 1.0), y: y.clamp(0.0, 1.0) },
                    1 if (0.0..NSLOTS as f32).contains(&x) && x.fract() == 0.0 => Loc::Slot(x as u8),
                    2 => Loc::Float { x, y },
                    _ => return None,
                };
                let rot = self.lt(4)?;
                Body::Card(w, i, Card { loc, rot, stamp })
            }
            T_CHECK => {
                let w = self.u32()?;
                let stamp = self.stamp()?;
                let attempts = self.u8()?;
                let mask = self.lt(16)?;
                let tried = u128::from_le_bytes(self.take(16)?.try_into().ok()?);
                Body::Check(w, Check { attempts, mask, stamp, tried })
            }
            T_NEXT => {
                let k = self.u16()?;
                Body::Next(k, self.stamp()?)
            }
            T_FOCUS => Body::Focus(self.u32()?, self.stamp()?),
            _ => return None,
        };
        Some(Msg::Op(Op { round, body }))
    }
}

/// Decode a packet. Stops at the first malformed record (never panics).
pub fn decode(b: &[u8]) -> Vec<Msg> {
    let mut r = Reader { b, p: 0 };
    let mut out = Vec::new();
    while r.p < b.len() {
        match r.msg() {
            Some(m) => out.push(m),
            None => break,
        }
    }
    out
}

/// Trim and byte-truncate text on a char boundary.
pub fn clamp_text(s: &str, max: usize) -> String {
    let s = s.trim();
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}
