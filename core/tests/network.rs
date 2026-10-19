//! The game on an unreliable network: devices on a mesh of links with
//! latency and jitter, links that drop and reconnect, partitions that heal,
//! devices that go offline, reload (and rejoin under their saved name 1.5 s
//! later, as the page does) or close for good, all while players act at
//! random. After the chaos the network heals and goes quiet, and every run
//! must satisfy the properties at the bottom: one state everywhere, nobody
//! connected loses their seat, nothing is wiped without a New game, no card
//! stays "held" by a hand that has gone, and the round can still be played
//! to its end. Throughout, every device's status strip must tell the truth.
//!
//! The model follows the real transport (Trystero over WebRTC): each pair of
//! devices has at most one connection; a packet is broadcast on every open
//! connection, delivered in order on each, and lost if that connection closes
//! first; when a connection opens both ends call `on_peer_join` (a full state
//! sync), and when it closes both call `on_link_lost`.
//!
//! `SEED=123 cargo test --release --test network -- --nocapture` replays one
//! run with its event log.

use cloverleaf::app::{keep_json, App, BOLD_OFF, BOLD_ON, EV_KEEP, EV_MOVED, EV_NET, EV_NOTICE};
use cloverleaf::model::*;
use std::collections::{BTreeMap, BTreeSet};

const TICK: f64 = 20.0;
const NAMES: [&str; 5] = ["Ann", "Bo", "Cy", "Dee", "Eve"];

fn app(seed: u64) -> App {
    let mut a = App::new(seed);
    (a.w, a.h, a.dpr) = (1000.0, 800.0, 1.0);
    a
}

/// An outbox split into network packets and the other events.
fn packets(a: &mut App) -> (Vec<Vec<u8>>, Vec<(u8, String)>) {
    let b = std::mem::take(&mut a.outbox);
    let (mut out, mut ev, mut p) = (Vec::new(), Vec::new(), 0);
    while p < b.len() {
        let len = u32::from_le_bytes(b[p + 1..p + 5].try_into().unwrap()) as usize;
        let body = &b[p + 5..p + 5 + len];
        if b[p] == EV_NET {
            out.push(body.to_vec());
        } else {
            ev.push((b[p], String::from_utf8_lossy(body).into_owned()));
        }
        p += 5 + len;
    }
    (out, ev)
}

/// A round's Past games key, as `keep_json` writes it.
fn round_key(r: &Round) -> String {
    format!("{}.{}/{}.{}", r.table.clock, r.table.peer, r.stamp.clock, r.stamp.peer)
}

struct Dev {
    app: Option<App>,
    /// The name this device's page has saved (it rejoins under it on reload).
    name: Option<String>,
    join_at: Option<f64>,
    online: bool,
    /// Mid-drag: the finger is down and moving.
    dragging: bool,
    /// What the page last saved to localStorage (every 2 s), restored on reload.
    saved: Vec<u8>,
    /// The furthest this device's table has got (epoch, round): what a
    /// merge must never take away from it.
    best: (u16, u16),
    /// Rounds it has kept in Past games (EV_KEEP), and the first round that
    /// had something in it when it was replaced but wasn't kept.
    kept: BTreeSet<String>,
    lost: Option<String>,
    /// Notices raised (EV_NOTICE), and the first round change that didn't
    /// raise exactly one.
    notices: u32,
    notice_bad: Option<String>,
    /// Move prompts raised (EV_MOVED).
    moved: u32,
    /// Outdated as of the last flush, and whether a table change went out
    /// after that (it mustn't).
    was_outdated: bool,
    leaked: bool,
    /// The first time this device's status strip said something untrue.
    strip_bad: Option<String>,
}

struct Packet {
    at: f64,
    to: usize,
    conn: u32,
    bytes: Vec<u8>,
}

/// How rough the network is.
#[derive(Clone, Copy)]
struct Rough {
    /// One-way latency range, ms.
    latency: (f64, f64),
    /// Chance per tick that some fault happens.
    faults: u32, // out of 1000
    /// Chance per tick that some player acts.
    acts: u32, // out of 1000
}

struct Sim {
    devs: Vec<Dev>,
    /// Open connections: (i, j) with i < j → connection id.
    links: BTreeMap<(usize, usize), u32>,
    /// Pairs that can't reach each other (a partition).
    blocked: BTreeSet<(usize, usize)>,
    inflight: Vec<Packet>,
    /// Last scheduled delivery per direction, to keep each connection in order.
    last_at: BTreeMap<(usize, usize), f64>,
    next_conn: u32,
    now: f64,
    r: Rng,
    rough: Rough,
    seed: u64,
    log: Vec<String>,
    /// Notices raised in this run, by kind (coverage).
    kinds: BTreeMap<String, u32>,
    loads: u64,
}

fn pair(i: usize, j: usize) -> (usize, usize) {
    (i.min(j), i.max(j))
}

impl Sim {
    fn new(seed: u64, n: usize, rough: Rough) -> Sim {
        let devs = (0..n)
            .map(|k| Dev { app: Some(app(seed * 100 + k as u64 + 1)), name: None, join_at: None, online: true, dragging: false, saved: Vec::new(), best: (0, 0), kept: BTreeSet::new(), lost: None, notices: 0, notice_bad: None, moved: 0, was_outdated: false, leaked: false, strip_bad: None })
            .collect();
        Sim {
            devs,
            links: BTreeMap::new(),
            blocked: BTreeSet::new(),
            inflight: Vec::new(),
            last_at: BTreeMap::new(),
            next_conn: 1,
            now: 1000.0,
            r: Rng::new(seed ^ 0x5eed),
            rough,
            seed,
            log: Vec::new(),
            kinds: BTreeMap::new(),
            loads: 0,
        }
    }

    fn note(&mut self, s: String) {
        if std::env::var("SEED").is_ok() {
            println!("{:>7.0} {s}", self.now);
        }
        self.log.push(format!("{:.0} {s}", self.now));
    }

    fn live(&self, k: usize) -> bool {
        self.devs[k].app.is_some() && self.devs[k].online
    }

    /// Send everything device `k` has queued on each of its open connections.
    fn flush(&mut self, k: usize) {
        let Some(a) = self.devs[k].app.as_mut() else { return };
        let (out, events) = packets(a);
        for (kind, json) in events {
            match kind {
                EV_NOTICE => {
                    self.devs[k].notices += 1;
                    let kind = json.split("\"kind\":\"").nth(1).and_then(|t| t.split('"').next()).unwrap_or("?");
                    *self.kinds.entry(kind.to_string()).or_default() += 1;
                }
                EV_MOVED => self.devs[k].moved += 1,
                EV_KEEP => {
                    let key = json.split("\"key\":\"").nth(1).and_then(|t| t.split('"').next()).unwrap_or_default();
                    self.devs[k].kept.insert(key.to_string());
                }
                _ => {}
            }
        }
        let outdated = self.devs[k].app.as_ref().is_some_and(|a| a.outdated);
        if self.devs[k].was_outdated && out.iter().any(|p| decode(p).iter().any(|m| matches!(m, Msg::Op(_)))) {
            self.devs[k].leaked = true;
        }
        self.devs[k].was_outdated = outdated;
        if out.is_empty() {
            return;
        }
        let peers: Vec<(usize, u32)> = self
            .links
            .iter()
            .filter_map(|(&(i, j), &c)| if i == k { Some((j, c)) } else if j == k { Some((i, c)) } else { None })
            .collect();
        for bytes in out {
            for &(to, conn) in &peers {
                let (lo, hi) = self.rough.latency;
                let lat = lo + (hi - lo) * (self.r.below(1000) as f64 / 1000.0);
                let at = (self.now + lat).max(*self.last_at.get(&(k, to)).unwrap_or(&0.0));
                self.last_at.insert((k, to), at);
                self.inflight.push(Packet { at, to, conn, bytes: bytes.clone() });
            }
        }
    }

    /// A round with anything in it that just stopped being current must be
    /// in the device's Past games.
    fn check_kept(&mut self, k: usize, before: Round, after: Round, had: bool) {
        let d = &mut self.devs[k];
        if had && before.stamp != after.stamp && !d.kept.contains(&round_key(&before)) && d.lost.is_none() {
            d.lost = Some(round_key(&before));
        }
    }

    fn connect(&mut self, i: usize, j: usize) {
        let p = pair(i, j);
        if i == j || self.links.contains_key(&p) || !self.live(i) || !self.live(j) || self.blocked.contains(&p) {
            return;
        }
        let conn = self.next_conn;
        self.next_conn += 1;
        self.links.insert(p, conn);
        self.note(format!("link {i}-{j} up (#{conn})"));
        for k in [i, j] {
            self.devs[k].app.as_mut().unwrap().on_peer_join();
            self.flush(k);
        }
    }

    fn disconnect(&mut self, i: usize, j: usize) {
        if let Some(conn) = self.links.remove(&pair(i, j)) {
            self.note(format!("link {i}-{j} down (#{conn})"));
            for k in [i, j] {
                if let Some(a) = self.devs[k].app.as_mut() {
                    a.on_link_lost(conn);
                }
            }
        }
    }

    fn drop_all_links(&mut self, k: usize) {
        for j in 0..self.devs.len() {
            self.disconnect(k, j);
        }
    }

    fn tick(&mut self) {
        self.now += TICK;
        for k in 0..self.devs.len() {
            if self.devs[k].join_at.is_some_and(|t| t <= self.now) && self.live(k) {
                self.devs[k].join_at = None;
                let name = self.devs[k].name.clone().unwrap();
                self.note(format!("dev {k} rejoins as {name}"));
                self.devs[k].app.as_mut().unwrap().join(&name);
            }
            if self.live(k) {
                let now = self.now;
                self.devs[k].app.as_mut().unwrap().frame(now);
                let r = self.devs[k].app.as_ref().unwrap().shared.round;
                self.devs[k].best = self.devs[k].best.max((r.epoch, r.n));

                if (now / TICK) as u64 % 100 == k as u64 {
                    self.devs[k].saved = self.devs[k].app.as_ref().unwrap().snapshot();
                }
            }
        }
        // Deliver what has arrived, in time order, on connections still open.
        self.inflight.sort_by(|a, b| a.at.total_cmp(&b.at));
        let due = self.inflight.iter().take_while(|p| p.at <= self.now).count();
        let open: BTreeSet<u32> = self.links.values().copied().collect();
        for p in self.inflight.drain(..due).collect::<Vec<_>>() {
            if open.contains(&p.conn) {
                if let Some(a) = self.devs[p.to].app.as_mut() {
                    // Exactly one notice per change of round from outside
                    // (none when arriving: no table yet), none otherwise.
                    let before = a.shared.round;
                    let had = keep_json(&a.shared).is_some();
                    a.on_packet(&p.bytes, p.conn);
                    let after = a.shared.round;
                    let n0 = self.devs[p.to].notices;
                    self.flush(p.to);
                    self.check_kept(p.to, before, after, had);
                    let want = (before.stamp != after.stamp && before.table != Stamp::default()) as u32;
                    let got = self.devs[p.to].notices - n0;
                    if got != want && self.devs[p.to].notice_bad.is_none() {
                        self.devs[p.to].notice_bad = Some(format!("{got} notices for a change {:?} → {:?}", (before.epoch, before.n), (after.epoch, after.n)));
                    }
                }
            }
        }
        for k in 0..self.devs.len() {
            self.flush(k);
        }
        // What each page shows: told how many connections it has (as the
        // page tells it), its status strip must be true to its own table.
        for k in 0..self.devs.len() {
            let links = self.links.keys().filter(|(i, j)| *i == k || *j == k).count() as u32;
            let Some(a) = self.devs[k].app.as_mut() else { continue };
            a.links = links;
            if let Some(why) = strip_lie(a).filter(|_| self.devs[k].strip_bad.is_none()) {
                self.devs[k].strip_bad = Some(format!("at {:.0} ms: {why}", self.now));
            }
        }
        if std::env::var("SEED").is_ok() {
            let games: Vec<String> = self
                .devs
                .iter()
                .map(|d| d.app.as_ref().map_or("-".into(), |a| format!("e{}r{}/{}", a.shared.round.epoch, a.shared.round.n, a.shared.round.nseats)))
                .collect();
            let g = games.join(" ");
            if self.log.last().is_none_or(|l| !l.ends_with(&g)) {
                self.note(format!("games {g}"));
            }
        }
    }

    /// Connections form between reachable online devices, after a while.
    fn reconnect_some(&mut self, chance: u32) {
        let n = self.devs.len();
        for i in 0..n {
            for j in i + 1..n {
                if self.r.below(1000) < chance as usize {
                    self.connect(i, j);
                }
            }
        }
    }

    /// Player `k` does something; a round it replaces locally (a deal, a New
    /// game) must be kept too.
    fn act(&mut self, k: usize) {
        let a = self.devs[k].app.as_ref().unwrap();
        let (before, had) = (a.shared.round, keep_json(&a.shared).is_some());
        self.act_inner(k);
        if let Some(a) = self.devs[k].app.as_ref() {
            let after = a.shared.round;
            self.flush(k);
            self.check_kept(k, before, after, had);
        }
    }

    fn act_inner(&mut self, k: usize) {
        let r = self.r.below(100);
        let dev = &mut self.devs[k];
        let a = dev.app.as_mut().unwrap();
        if dev.dragging {
            // Finish (or abandon) the drag first.
            if r < 70 {
                let (x, y) = (self.r.below(1000) as f32, self.r.below(800) as f32);
                a.pointer_up(x, y);
            } else {
                a.pointer_cancel();
            }
            dev.dragging = false;
            return;
        }
        let what = match r {
            0..=7 => {
                let name = NAMES[self.r.below(NAMES.len())].to_string();
                a.join(&name);
                dev.name = Some(name.clone());
                format!("join as {name}")
            }
            8..=13 => {
                a.action(1);
                "start / next round".into()
            }
            14..=27 => {
                for d in 0..4u8 {
                    a.set_clue(d, ["sea", "sky", "fire", "dog", "ice"][self.r.below(5)], false);
                }
                a.action(2);
                "write + toggle ready".into()
            }
            28..=31 => {
                a.set_clue(self.r.below(4) as u8, ["", "moon", "tea"][self.r.below(3)], false);
                "edit a clue".into()
            }
            32..=35 => {
                a.action(5);
                "proceed".into()
            }
            36..=44 => {
                a.action(3);
                "guess".into()
            }
            45..=50 => {
                a.action(4);
                "next clover".into()
            }
            51..=53 => {
                a.jump(self.r.below(4));
                "jump".into()
            }
            54..=55 => {
                a.set_decoys(self.r.below(5) as u8);
                "decoys".into()
            }
            56 => {
                if self.r.below(3) == 0 {
                    a.action(7);
                    "NEW GAME".into()
                } else {
                    a.action(6);
                    "deal new cards".into()
                }
            }
            57..=64 => {
                // Pick up a card and start dragging (finished on a later turn).
                let n = a.scene().cards.len();
                if n == 0 {
                    return;
                }
                a.settle();
                let (x, y, _) = a.card_screen(self.r.below(n));
                a.pointer_down(x, y);
                a.pointer_move(x + 30.0, y + 30.0);
                dev.dragging = true;
                "drag starts".into()
            }
            _ => {
                // A whole drag, often onto a spot, sometimes a tap (turn).
                let n = a.scene().cards.len();
                if n == 0 {
                    return;
                }
                a.settle();
                let i = self.r.below(n);
                let (x, y, _) = a.card_screen(i);
                let to = if self.r.below(4) == 0 { (x, y) } else { a.slot_screen(self.r.below(4)) };
                a.pointer_down(x, y);
                for s in 1..=6 {
                    let t = s as f32 / 6.0;
                    a.pointer_move(x + (to.0 - x) * t, y + (to.1 - y) * t);
                }
                a.pointer_up(to.0, to.1);
                "drag / tap a card".into()
            }
        };
        self.note(format!("dev {k}: {what}"));
    }

    fn fault(&mut self) {
        let n = self.devs.len();
        let k = self.r.below(n);
        match self.r.below(10) {
            0 | 1 => {
                let j = self.r.below(n);
                self.disconnect(k, j);
            }
            2 => {
                // Partition: a random side can't reach the rest.
                let side: Vec<bool> = (0..n).map(|_| self.r.below(2) == 0).collect();
                self.note(format!("partition {side:?}"));
                for i in 0..n {
                    for j in i + 1..n {
                        if side[i] != side[j] {
                            self.blocked.insert((i, j));
                            self.disconnect(i, j);
                        }
                    }
                }
            }
            3 | 4 => {
                self.note("partition heals".into());
                self.blocked.clear();
            }
            5 if self.devs[k].app.is_some() && self.devs[k].online => {
                self.note(format!("dev {k} offline"));
                self.drop_all_links(k);
                self.devs[k].online = false;
            }
            6 if self.devs[k].app.is_some() && !self.devs[k].online => {
                self.note(format!("dev {k} back online"));
                self.devs[k].online = true;
            }
            7 | 8 if self.devs[k].app.is_some() => self.reload(k),
            9 if self.devs.iter().filter(|d| d.app.is_some()).count() > 2 && self.r.below(4) == 0 => {
                self.note(format!("dev {k} closes for good"));
                self.drop_all_links(k);
                self.devs[k].app = None;
            }
            _ => {}
        }
    }

    /// Reload: a fresh page; a saved name rejoins 1.5 s later.
    fn reload(&mut self, k: usize) {
        if self.devs[k].app.is_none() {
            return;
        }
        self.note(format!("dev {k} reloads"));
        self.drop_all_links(k);
        // Every page load gets its own device id, as crypto random does.
        self.loads += 1;
        let seed = self.seed * 100_000 + 1000 + self.loads;
        // The page saves as it unloads, so the reload restores it all.
        self.devs[k].saved = self.devs[k].app.as_ref().unwrap().snapshot();
        let invitation = self.devs[k].app.as_ref().unwrap().invitation.clone();
        let d = &mut self.devs[k];
        let mut fresh = app(seed);
        if let Some(i) = invitation { fresh.load_invite(&i.room, &i.json()).unwrap(); }
        assert!(fresh.restore(&d.saved, PROTOCOL as u32));
        packets(&mut fresh); // restoring a save raises nothing worth keeping
        d.app = Some(fresh);
        d.online = true;
        d.dragging = false;
        d.was_outdated = false;
        d.join_at = d.name.as_ref().map(|_| self.now + 1500.0);
    }

    /// A new device at this table: `a` (already seeded), sitting as `name`.
    fn add(&mut self, mut a: App, name: &str) -> usize {
        a.join(name);
        self.devs.push(Dev { app: Some(a), name: Some(name.into()), join_at: None, online: true, dragging: false, saved: Vec::new(), best: (0, 0), kept: BTreeSet::new(), lost: None, notices: 0, notice_bad: None, moved: 0, was_outdated: false, leaked: false, strip_bad: None });
        self.devs.len() - 1
    }

    /// Time passes with links forming between online devices (the away stay away).
    fn run_for(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.reconnect_some(1000);
            self.tick();
        }
    }

    /// The first `cut` devices can't reach the rest.
    fn split(&mut self, cut: usize) {
        let n = self.devs.len();
        self.note(format!("split at {cut}"));
        for i in 0..cut {
            for j in cut..n {
                self.blocked.insert((i, j));
                self.disconnect(i, j);
            }
        }
    }

    /// Device `k` does `f` to its app, if it's here and online.
    fn with(&mut self, k: usize, what: &str, f: impl FnOnce(&mut App)) {
        if !self.live(k) {
            return;
        }
        let a = self.devs[k].app.as_ref().unwrap();
        let (before, had) = (a.shared.round, keep_json(&a.shared).is_some());
        f(self.devs[k].app.as_mut().unwrap());
        let after = self.devs[k].app.as_ref().unwrap().shared.round;
        self.flush(k);
        self.check_kept(k, before, after, had);
        self.note(format!("dev {k}: {what}"));
    }

    fn apps(&self) -> impl Iterator<Item = (usize, &App)> {
        self.devs.iter().enumerate().filter_map(|(k, d)| d.app.as_ref().filter(|_| d.online).map(|a| (k, a)))
    }

    /// Everything heals: no partition, every online device connected, and
    /// 2 s pass with nobody acting.
    fn heal(&mut self) {
        self.heal_for(100);
    }

    fn heal_for(&mut self, ticks: u32) {
        self.blocked.clear();
        for d in &mut self.devs {
            if d.app.is_some() {
                d.online = true;
            }
        }
        for _ in 0..ticks {
            self.reconnect_some(1000);
            self.tick();
        }
    }

    fn fail(&self, what: &str) -> ! {
        let tail: Vec<&String> = self.log.iter().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
        panic!(
            "seed {}: {what}\n  last events:\n    {}\n  replay: SEED={} cargo test --release --test network -- --nocapture",
            self.seed,
            tail.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n    "),
            self.seed
        );
    }

    fn digest_of(&self, k: usize) -> String {
        let sh = &self.devs[k].app.as_ref().unwrap().shared;
        format!("{:?} {}", sh.round.stamp, sh.digest())
    }
}

/// What happened across all runs (to check the chaos really got exercised).
#[derive(Default, Debug)]
struct Coverage {
    runs: u32,
    guessing_reached: u32,
    reloads: u32,
    partitions: u32,
    finished: u32,
    /// Unfinished runs with fewer than two distinct players left. A
    /// reserved writer can submit late, but cannot guess their own clover.
    alone: u32,
    /// The largest table seen, in bytes.
    biggest: usize,
    /// Notices by kind, and in how many runs each kind occurred.
    kinds: BTreeMap<String, (u32, u32)>,
    /// Late changes merged into replaced rounds, and in how many runs.
    late: (u32, u32),
    /// The most seats a table had, and the most rounds one got through.
    most_seats: usize,
    most_rounds: u16,
    /// Runs where a device heard a newer protocol.
    outdated_runs: u32,
}

impl Coverage {
    /// In how many runs notices of `kind` occurred.
    fn runs_with(&self, kind: &str) -> u32 {
        self.kinds.get(kind).map_or(0, |k| k.1)
    }
}

/// A story family: each run of the simulator follows one, steering faults
/// and actions towards a kind of situation (with the usual random noise on
/// top), and each family's test requires that situation to have occurred.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Story {
    /// Random play and faults, nothing steered.
    Everyday,
    /// The table splits in two for a long while; both sides deal, write and
    /// press Proceed; then it heals.
    SplitBrain,
    /// Two groups sit down and deal apart (two tables), then meet.
    TwoTables,
    /// Every device reloads at once, three times.
    MassReload,
    /// The table splits; one side starts a New game, the other deals.
    NewGameInSplit,
    /// Someone keeps dealing while everyone keeps editing, on a slow
    /// network: edits cross round changes.
    CrossingEdits,
    /// Many names come and go; absent ones are removed.
    Churn,
    /// The table plays round after round, cooperatively, through the noise.
    LongSession,
    /// One device hears a newer protocol mid-game, and later reloads.
    VersionSkew,
}

/// One run: set up a table, play through chaos, heal, check, then play the
/// round out cooperatively and check it ends.
fn run(seed: u64, rough: Rough, story: Story, cov: &mut Coverage) {
    let n = 3 + (seed % 3) as usize;
    let mut s = Sim::new(seed, n, rough);
    let half = (n / 2).max(1);

    // Setup on a good network: everyone connects, sits down, a round is
    // dealt. Two tables: two groups that can't reach each other each sit
    // down and deal (one group sometimes a round further).
    if story == Story::TwoTables {
        s.split(half);
    }
    s.reconnect_some(1000);
    for _ in 0..20 {
        s.tick();
    }
    for k in 0..n {
        s.devs[k].name = Some(NAMES[k].into());
        s.devs[k].app.as_mut().unwrap().join(NAMES[k]);
        s.tick();
    }
    for _ in 0..20 {
        s.tick();
    }
    s.devs[0].app.as_mut().unwrap().action(1);
    if story == Story::TwoTables {
        s.devs[half].app.as_mut().unwrap().action(1);
        if s.r.below(2) == 0 {
            for _ in 0..10 {
                s.tick();
            }
            s.devs[half].app.as_mut().unwrap().action(6);
        }
    }
    for _ in 0..20 {
        s.tick();
    }

    // Chaos, steered by the story.
    let mut guessed = false;
    for step in 0..600 {
        steer(&mut s, story, step, half);
        if s.r.below(1000) < s.rough.faults as usize {
            s.fault();
        }
        s.reconnect_some(30);
        if s.r.below(1000) < s.rough.acts as usize {
            let k = s.r.below(n);
            if s.live(k) {
                s.act(k);
            }
        }
        s.tick();
        guessed |= s.apps().any(|(_, a)| matches!(a.shared.phase(), Phase::Guessing(_)));
    }
    cov.guessing_reached += guessed as u32;
    let late: u32 = s.devs.iter().filter_map(|d| d.app.as_ref()).map(|a| a.late_merges).sum();
    cov.late = (cov.late.0 + late, cov.late.1 + (late > 0) as u32);
    cov.outdated_runs += s.log.iter().any(|l| l.contains("hears a newer protocol")) as u32;
    for d in s.devs.iter().filter(|d| d.leaked) {
        let _ = d;
        s.fail("a page that heard a newer protocol still sent table changes");
    }
    cov.reloads += s.log.iter().filter(|l| l.contains("reloads")).count() as u32;
    for (k, n) in &s.kinds {
        let e = cov.kinds.entry(k.clone()).or_default();
        e.0 += n;
        e.1 += 1;
    }
    cov.partitions += s.log.iter().filter(|l| l.contains("partition [")).count() as u32;

    // Every hand lets go; then heal and go quiet.
    for d in &mut s.devs {
        if let (Some(a), true) = (d.app.as_mut(), d.dragging) {
            a.pointer_cancel();
            d.dragging = false;
        }
    }
    // Long enough for drifted devices to notice (two pings) and resync.
    s.heal_for(600);

    // 1. One state everywhere.
    let live: Vec<usize> = s.apps().map(|(k, _)| k).collect();
    let d0 = s.digest_of(live[0]);
    for &k in &live[1..] {
        if s.digest_of(k) != d0 {
            let cards = |k: usize| format!("{:?}", s.devs[k].app.as_ref().unwrap().shared.cards);
            s.fail(&format!("devices {} and {k} disagree after healing:\n  {}\n  {}\n  {}\n  {}", live[0], d0, s.digest_of(k), cards(live[0]), cards(k)));
        }
    }
    // 2. Nobody connected has lost their seat (or their name).
    for &k in &live {
        let a = s.devs[k].app.as_ref().unwrap();
        if let Some(id) = a.seat {
            match a.shared.seats.get(&id) {
                Some(seat) if seat.active => {}
                _ => s.fail(&format!("device {k} thinks it is seated but the table has no seat for it")),
            }
        }
        if s.devs[k].name.is_some() && s.devs[k].join_at.is_none() && a.seat.is_none() {
            s.fail(&format!("device {k} saved a name but isn't seated"));
        }
    }
    // 3. Nothing wiped: no device's table ever went back to less progress
    // (a New game counts as progress).
    for &k in &live {
        let r = s.devs[k].app.as_ref().unwrap().shared.round;
        if (r.epoch, r.n) < s.devs[k].best {
            s.fail(&format!("device {k}'s table went back from {:?} to {:?}", s.devs[k].best, (r.epoch, r.n)));
        }
    }
    // 6. Every round a device had anything written in is in its Past games
    // once it stopped being current; every change from outside was
    // explained exactly once; and the table stays small.
    for &k in &live {
        let d = &s.devs[k];
        let a = d.app.as_ref().unwrap();
        if let Some(lost) = &d.lost {
            s.fail(&format!("device {k} never kept round {lost} in Past games"));
        }
        if let Some(why) = &d.notice_bad {
            s.fail(&format!("device {k}: {why}"));
        }
        if let Some(why) = &d.strip_bad {
            s.fail(&format!("device {k}'s status strip: {why}"));
        }
        let size = a.snapshot().len();
        cov.biggest = cov.biggest.max(size);
        cov.most_seats = cov.most_seats.max(a.shared.seats.len());
        cov.most_rounds = cov.most_rounds.max(a.shared.round.n);
        if size > 50_000 {
            s.fail(&format!("device {k}'s table is {size} bytes, over the 50 KB budget"));
        }
    }
    // 4. No card is still drawn as held once every hand has let go.
    for &k in &live {
        if let Some((i, _)) = s.devs[k].app.as_ref().unwrap().holds().into_iter().next() {
            s.fail(&format!("device {k} still shows card {i} as held after everyone let go"));
        }
    }

    // 5. The round can be played to its end: whoever is seated writes and
    // submits; if it still waits (on someone gone), Proceed; if nobody dealt
    // in is left to submit, deal again. Then solve every clover, moving on.
    let a0 = live[0];
    for _ in 0..3 {
        match s.devs[a0].app.as_ref().unwrap().shared.phase() {
            Phase::Lobby => s.devs[a0].app.as_mut().unwrap().action(1),
            Phase::Writing => {}
            _ => break,
        }
        s.heal();
        for &k in &live {
            let a = s.devs[k].app.as_mut().unwrap();
            if a.shared.phase() == Phase::Writing && a.seat.is_some_and(|me| a.shared.round_players().contains(&me)) {
                for d in 0..4u8 {
                    a.set_clue(d, "clue", false);
                }
                if !a.shared.is_ready(a.seat.unwrap()) {
                    a.action(2);
                }
            }
        }
        s.heal();
        let seated = live.iter().copied().find(|&k| s.devs[k].app.as_ref().unwrap().seat.is_some()).unwrap_or(a0);
        if s.devs[seated].app.as_ref().unwrap().shared.phase() == Phase::Writing {
            s.devs[seated].app.as_mut().unwrap().action(5);
            s.heal();
        }
        if s.devs[seated].app.as_ref().unwrap().shared.phase() == Phase::Writing {
            // Nobody dealt in is here to submit. The new-round checklist
            // now preserves away players, so explicitly select those here.
            let a = s.devs[seated].app.as_mut().unwrap();
            let players: Vec<u32> = a.shared.players().into_iter().filter(|p| a.present().contains(p)).collect();
            let input = format!(r#"{{"round":"{}","roster":{:?},"players":{:?},"replace":true}}"#, a.round_token(), a.shared.players(), players);
            if let Err(e) = a.deal_selected(&input) { s.fail(&format!("could not select the remaining players: {e}")); }
            s.heal();
        }
    }
    // Reserved writers may still be finishing while the table guesses.
    // Add their clovers when another person remains to guess them; a lone
    // writer needn't reopen an otherwise finished round with their own.
    let remaining: BTreeSet<u32> = live.iter().filter_map(|&k| s.devs[k].app.as_ref().unwrap().seat).collect();
    if remaining.len() > 1 {
        for &k in &live {
            let a = s.devs[k].app.as_mut().unwrap();
            if a.can_write() {
                a.action(cloverleaf::app::ACT_WRITE);
                for d in 0..4 { a.set_clue(d, "clue", false); }
                if !a.shared.is_ready(a.seat.unwrap()) { a.action(2); }
            }
        }
    }
    s.heal();
    for _ in 0..12 {
        let phase = s.devs[a0].app.as_ref().unwrap().shared.phase();
        let Phase::Guessing(_) = phase else { break };
        let w = s.devs[a0].app.as_ref().unwrap().scene().writer.unwrap();
        let Some(g) = live.iter().copied().find(|&k| {
            let a = s.devs[k].app.as_ref().unwrap();
            a.seat.is_some() && a.seat != Some(w)
        }) else {
            break; // nobody left to guess (only the writer is seated)
        };
        let a = s.devs[g].app.as_mut().unwrap();
        solve(a);
        a.action(3);
        s.heal();
        let a = s.devs[g].app.as_mut().unwrap();
        a.action(4);
        s.heal();
    }
    let phase = s.devs[a0].app.as_ref().unwrap().shared.phase();
    let seats: BTreeSet<u32> = live.iter().filter_map(|&k| s.devs[k].app.as_ref().unwrap().seat).collect();
    let only_writer = seats.len() < 2;
    if phase != Phase::End && !only_writer {
        let a = s.devs[a0].app.as_ref().unwrap();
        let w = a.scene().writer;
        let detail = w.map(|w| {
            let c = a.shared.check(w);
            let sc = a.scene();
            let seats: Vec<_> = live.iter().map(|&k| (k, s.devs[k].app.as_ref().unwrap().seat)).collect();
            format!("writer {w} mask {:04b} attempts {} cards {:?} seats {seats:?} puzzles {:?}", c.mask, c.attempts,
                sc.cards.iter().map(|c| (c.loc, c.rot)).collect::<Vec<_>>(), a.shared.puzzles())
        });
        let sh = &a.shared;
        let writing = format!("round players {:?} ready {:?} seats {:?} present {:?}", sh.round_players(),
            sh.round_players().iter().map(|p| sh.is_ready(*p)).collect::<Vec<_>>(), seats, a.present());
        s.fail(&format!("the round could not be played to its end (stuck in {phase:?}): {detail:?} {writing}"));
    }
    cov.finished += (phase == Phase::End) as u32;
    cov.alone += (phase != Phase::End && only_writer) as u32;
    let d0 = s.digest_of(live[0]);
    if live.iter().any(|&k| s.digest_of(k) != d0) {
        s.fail("devices disagree at the end of the round");
    }
    cov.runs += 1;
}

/// 7. What the status strip says is true of this device's own table: who
/// it's waiting for is exactly who hasn't submitted, the clover it names is
/// the one on the board, and a seated, connected player always has
/// something to press unless waiting is the point (submitted, sitting this
/// round out, or watching others guess their own clover).
fn strip_lie(a: &App) -> Option<String> {
    let st = a.strip();
    let sh = &a.shared;
    let first = st.hint.first().cloned().unwrap_or_default();
    let bold: Vec<String> = first.split(BOLD_ON).skip(1).filter_map(|t| t.split(BOLD_OFF).next()).map(String::from).collect();
    let seated = a.seat.is_some_and(|me| sh.players().contains(&me));
    let in_round = a.seat.is_some_and(|me| sh.round.players().contains(&me));
    let me_ready = a.seat.is_some_and(|me| sh.is_ready(me));
    if first.contains("aiting for") && !st.pulse {
        let mut want: Vec<String> = sh.round_players().iter().filter(|p| !sh.is_ready(**p)).map(|&p| a.short_name(p)).collect();
        let mut got = bold.clone();
        want.sort();
        got.sort();
        if got != want {
            return Some(format!("{first:?} but still writing: {want:?}"));
        }
    }
    if st.alt.as_ref().is_some_and(|b| b.act == "proceed") && bold.is_empty() && !st.pulse {
        return Some(format!("offers {:?} without saying who is missing ({first:?})", st.alt));
    }
    if let Phase::Guessing(k) = a.view_phase() {
        let w = sh.puzzles()[k];
        let named = format!("{}'s", a.short_name(w));
        if !st.pulse && Some(w) != a.seat && first.contains("clover") && bold.first() != Some(&named) {
            return Some(format!("{first:?} but the clover on the board is {named}"));
        }
    }
    let waiting = match a.view_phase() {
        Phase::Writing => me_ready || !in_round,
        Phase::Guessing(k) => a.seat == Some(sh.puzzles()[k]) && !sh.check(sh.puzzles()[k]).done(),
        _ => false,
    };
    if seated && a.links > 0 && st.go.is_none() && st.alt.is_none() && !waiting {
        return Some(format!("nothing to press in {:?} ({first:?})", sh.phase()));
    }
    None
}

/// A point where a touch picks up card `i` (not one lying over it).
fn grab_point(a: &App, i: usize) -> Option<(f32, f32)> {
    let (x, y, _) = a.card_screen(i);
    let offs = [0.0, -20.0, 20.0, -40.0, 40.0, -55.0, 55.0];
    offs.iter().flat_map(|&dx| offs.iter().map(move |&dy| (x + dx, y + dy))).find(|&(px, py)| a.card_at_point(px, py) == Some(i))
}

fn drag_card(a: &mut App, from: (f32, f32), to: (f32, f32)) {
    a.pointer_down(from.0, from.1);
    for k in 1..=6 {
        let t = k as f32 / 6.0;
        a.pointer_move(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
    }
    a.pointer_up(to.0, to.1);
    a.settle();
}

/// Put the right cards in every open spot, turned right (from this screen).
fn solve(a: &mut App) {
    a.settle();
    let Some(w) = a.scene().writer else { return };
    let d = a.shared.deal(w);
    for s in 0..NSLOTS {
        if a.scene().locked & (1 << s) != 0 {
            continue;
        }
        // Clear whatever lies over the card, out to the board's edge.
        for corner in 0..6 {
            if grab_point(a, s).is_some() {
                break;
            }
            let (x, y, _) = a.card_screen(s);
            let Some(top) = a.card_at_point(x, y) else { break };
            let Some(p) = grab_point(a, top) else { break };
            let to = a.board_screen(if corner % 2 == 0 { -260.0 } else { 260.0 }, if corner / 2 % 2 == 0 { -260.0 } else { 260.0 });
            drag_card(a, p, to);
        }
        let Some(from) = grab_point(a, s) else { continue };
        let to = a.slot_screen(s);
        drag_card(a, from, to);
        for _ in 0..4 {
            if a.scene().cards[s].rot == d.sol_rot[s] {
                break;
            }
            let Some(p) = grab_point(a, s) else { break };
            a.pointer_down(p.0, p.1);
            a.pointer_up(p.0, p.1);
            a.settle();
        }
    }
}

const WORDS: [&str; 8] = ["sea", "sky", "fire", "dog", "ice", "moon", "tea", ""];

/// A clue edit, as a player makes: one leaf, some word (or cleared).
fn edit(a: &mut App, d: u8, w: &str) {
    a.set_clue(d, w, false);
}

/// One useful step of playing the round on, from this device: deal, write
/// and submit, Proceed, solve and guess, or move to the next clover.
fn play_on(a: &mut App) {
    let me = a.seat;
    match a.view_phase() {
        Phase::Lobby | Phase::End => a.action(1),
        Phase::Writing => match me {
            Some(me) if a.shared.round_players().contains(&me) && !a.shared.is_ready(me) => {
                for d in 0..4u8 {
                    a.set_clue(d, "clue", false);
                }
                a.action(2);
            }
            _ => a.action(5),
        },
        Phase::Guessing(_) => {
            let w = a.scene().writer;
            if w.is_some_and(|w| a.shared.check(w).done()) {
                a.action(4);
            } else if me.is_some() && w != me {
                solve(a);
                a.action(3);
            }
        }
    }
}

/// What a story does at `step` of the chaos, on top of the random noise.
fn steer(s: &mut Sim, story: Story, step: u32, half: usize) {
    let n = s.devs.len();
    let pick = |s: &mut Sim, from: usize, to: usize| from + s.r.below(to.saturating_sub(from).max(1));
    match story {
        Story::Everyday => {}
        Story::SplitBrain => {
            if step == 100 {
                s.split(half);
            }
            if (100..400).contains(&step) && step % 25 == 0 {
                for (from, to) in [(0, half), (half, n)] {
                    let k = pick(s, from, to);
                    let w = WORDS[s.r.below(6)];
                    match s.r.below(3) {
                        0 => s.with(k, "deals again", |a| a.action(6)),
                        1 => s.with(k, "writes and submits", |a| {
                            for d in 0..4u8 {
                                edit(a, d, w);
                            }
                            a.action(2);
                        }),
                        _ => s.with(k, "proceeds", |a| a.action(5)),
                    }
                }
            }
            if step == 400 {
                s.note("split heals".into());
                s.blocked.clear();
            }
        }
        Story::TwoTables => {
            if step == 250 {
                s.note("the two tables meet".into());
                s.blocked.clear();
            }
        }
        Story::MassReload => {
            if [150, 300, 450].contains(&step) {
                s.note("everyone reloads".into());
                for k in 0..n {
                    s.reload(k);
                }
            }
        }
        Story::NewGameInSplit => match step {
            100 => s.split(half),
            150 => {
                let k = pick(s, 0, half);
                s.with(k, "starts a NEW GAME", |a| a.action(7));
            }
            160 => {
                let k = pick(s, half, n);
                s.with(k, "deals again", |a| a.action(6));
            }
            170 => {
                let k = pick(s, half, n);
                s.with(k, "writes", |a| edit(a, 0, "split"));
            }
            350 => {
                s.note("split heals".into());
                s.blocked.clear();
            }
            _ => {}
        },
        Story::CrossingEdits => {
            if step % 40 == 0 {
                let k = s.r.below(n);
                s.with(k, "deals again", |a| a.action(6));
            }
            if step % 2 == 0 {
                let (k, d, w) = (s.r.below(n), s.r.below(4) as u8, WORDS[s.r.below(WORDS.len())]);
                s.with(k, "edits", |a| edit(a, d, w));
            }
        }
        Story::Churn => {
            if step % 8 == 0 {
                let k = s.r.below(n);
                let name = format!("P{}", s.r.below(40));
                if s.live(k) {
                    s.devs[k].name = Some(name.clone());
                }
                s.with(k, "joins under a new name", move |a| a.join(&name));
            }
            if step % 30 == 0 {
                let k = s.r.below(n);
                s.with(k, "removes someone away", |a| {
                    let here = a.present();
                    if let Some(id) = a.shared.seats.iter().find(|(id, x)| x.active && !here.contains(id)).map(|(id, _)| *id) {
                        a.remove_seat(id);
                    }
                });
            }
        }
        Story::LongSession => {
            if step % 3 == 0 {
                let k = s.r.below(n);
                s.with(k, "plays on", play_on);
            }
        }
        Story::VersionSkew => match step {
            200 if s.live(1) => {
                let mut b = Vec::new();
                encode(&Msg::Hello { device: 4242, seat: 0, sum: 0, ver: PROTOCOL + 1 }, &mut b);
                s.devs[1].app.as_mut().unwrap().on_packet(&b, 999);
                s.note("dev 1 hears a newer protocol".into());
            }
            450 => s.reload(1),
            _ => {}
        },
    }
}

const CALM: Rough = Rough { latency: (5.0, 40.0), faults: 15, acts: 120 };
const STORMY: Rough = Rough { latency: (20.0, 400.0), faults: 60, acts: 200 };

/// Seeds `from..from + count` ($SEEDS times as many, for a long soak), or
/// just $SEED (to replay a failure) if it's in range.
fn seeds(from: u64, count: u64) -> Vec<u64> {
    let n = count * std::env::var("SEEDS").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
    match std::env::var("SEED") {
        Ok(s) => Some(s.parse().unwrap()).filter(|s| (from..from + n).contains(s)).into_iter().collect(),
        Err(_) => (from..from + n).collect(),
    }
}

/// Run a story family over its seeds, then check its coverage rule.
fn family(story: Story, rough: Rough, from: u64, count: u64, rule: impl Fn(&Coverage) -> Result<(), String>) {
    let mut cov = Coverage::default();
    for seed in seeds(from, count) {
        run(seed, rough, story, &mut cov);
    }
    println!("{story:?}: {cov:?}");
    if std::env::var("SEED").is_err() {
        if let Err(why) = rule(&cov) {
            panic!("{story:?} didn't cover what it's for: {why}\n  {cov:?}");
        }
    }
}

/// `ok` or the reason.
fn need(ok: bool, why: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(why.into()) }
}

#[test]
fn the_table_survives_a_calm_but_flaky_network() {
    // Some runs end with only one distinct player. Require broad finishing
    // coverage, and account for every run; each playable table is also a
    // hard per-run assertion in run(), independent of this coverage gate.
    family(Story::Everyday, CALM, 0, 150, |c| {
        need(c.reloads > 150 && c.partitions > 80 && c.guessing_reached > 100
            && c.finished >= c.runs * 9 / 10 && c.finished + c.alone == c.runs, "everyday play under faults; every playable table finishes")
    });
}

#[test]
fn the_table_survives_a_stormy_network() {
    family(Story::Everyday, STORMY, 10_000, 150, |c| {
        need(c.reloads > 600 && c.partitions > 300 && c.guessing_reached > 100
            && c.finished >= c.runs * 9 / 10 && c.finished + c.alone == c.runs, "everyday play under heavy faults; every playable table finishes")
    });
}

#[test]
fn story_split_brain() {
    family(Story::SplitBrain, CALM, 20_000, 40, |c| {
        need(c.runs_with("missed") >= c.runs / 2 && c.runs_with("crossed") >= c.runs / 10, "both sides dealing: rounds missed and deals crossing")
    });
}

#[test]
fn story_two_tables_meet() {
    family(Story::TwoTables, CALM, 30_000, 40, |c| {
        need(c.runs_with("otherTable") >= c.runs * 4 / 5, "two tables meeting in one room")
    });
}

#[test]
fn story_mass_reload() {
    family(Story::MassReload, CALM, 40_000, 40, |c| {
        need(c.reloads >= c.runs * 9 && c.finished >= c.runs * 4 / 5, "everyone reloading, and still finishing rounds")
    });
}

#[test]
fn story_new_game_in_a_split() {
    family(Story::NewGameInSplit, CALM, 50_000, 40, |c| {
        need(c.runs_with("newGame") >= c.runs * 3 / 4, "a New game meeting a deal")
    });
}

#[test]
fn story_edits_crossing_round_changes() {
    family(Story::CrossingEdits, STORMY, 60_000, 40, |c| {
        need(c.late.1 >= c.runs / 3, "edits landing in a round already replaced")
    });
}

#[test]
fn story_churn() {
    family(Story::Churn, CALM, 70_000, 40, |c| need(c.most_seats >= 30, "a table with many names"));
}

#[test]
fn story_long_session() {
    family(Story::LongSession, CALM, 80_000, 40, |c| need(c.most_rounds >= 15, "a long session of many rounds"));
}

#[test]
fn story_version_skew() {
    family(Story::VersionSkew, CALM, 90_000, 40, |c| need(c.outdated_runs >= c.runs * 3 / 4, "a page hearing a newer protocol"));
}

/// Group invites: nobody overlaps at first; then subsets meet, reload and
/// split before the full group meets. Coverage requires every run to have
/// independent submissions, reloads, a partition, and every clover preserved.
#[test]
fn story_invited_group_arrives_separately() {
    let (mut runs, mut alone, mut reloads, mut partitioned, mut finished) = (0, 0, 0, 0, 0);
    for seed in seeds(110_000, 40) {
        let n = 3 + (seed % 3) as usize;
        let mut s = Sim::new(seed, n, STORMY);
        let input = format!("{{\"room\":\"meet-later\",\"name\":\"Ann\",\"friends\":{}}}", json_str(&NAMES[1..n].join("\n")));
        let i = cloverleaf::invite::Invite::create(&input, Stamp { clock: 1, peer: seed as u32 }, seed as u32).unwrap();
        for x in 0..n { for y in x + 1..n { s.blocked.insert(pair(x, y)); } }
        // Different arrival orders, with each person's own complete draft.
        for offset in 0..n {
            let k = (offset + seed as usize) % n;
            s.devs[k].name = Some(NAMES[k].into());
            let a = s.devs[k].app.as_mut().unwrap();
            a.load_invite(&i.room, &i.json()).unwrap();
            a.join(NAMES[k]);
            for d in 0..4 { a.set_clue(d, &format!("{seed}-{k}-{d}"), false); }
            if k != n - 1 { a.action(2); alone += 1; }
            s.tick();
            assert_eq!(s.devs[k].app.as_ref().unwrap().shared.phase(), Phase::Writing);
            s.reload(k);
            reloads += 1;
        }
        s.run_for(100); // re-take saved identities, with every link still blocked
        s.blocked.clear();
        s.split(n / 2);
        s.run_for(100);
        partitioned += 1;
        // Peers meet in groups through jitter and lost connections; no one
        // can guess before the final writer submits and the groups meet.
        for _ in 0..100 {
            if s.r.below(8) == 0 { let k = s.r.below(n); s.drop_all_links(k); }
            s.reconnect_some(120);
            s.tick();
        }
        for (_, a) in s.apps() { assert_eq!(a.shared.phase(), Phase::Writing); }
        s.devs[n - 1].app.as_mut().unwrap().action(2);
        s.heal_for(600);
        let expected = s.digest_of(0);
        for k in 0..n {
            let a = s.devs[k].app.as_ref().unwrap();
            assert_eq!(s.digest_of(k), expected);
            assert_eq!(a.shared.round, i.round);
            assert_eq!(a.shared.puzzles().len(), n);
            for w in 0..n { for d in 0..4 {
                assert_eq!(a.shared.effective(seat_id(NAMES[w])).unwrap().clues[d], format!("{seed}-{w}-{d}"));
            } }
            if let Some(why) = &s.devs[k].strip_bad { s.fail(why); }
        }
        for _ in 0..n {
            let writer = s.devs[0].app.as_ref().unwrap().scene().writer.unwrap();
            let g = (0..n).find(|k| seat_id(NAMES[*k]) != writer).unwrap();
            solve(s.devs[g].app.as_mut().unwrap());
            s.devs[g].app.as_mut().unwrap().action(3);
            s.heal();
            s.devs[g].app.as_mut().unwrap().action(4);
            s.heal();
        }
        assert!(s.apps().all(|(_, a)| a.shared.phase() == Phase::End));
        finished += 1;
        runs += 1;
    }
    if std::env::var("SEED").is_err() {
        assert!(runs >= 40 && alone >= 2 * runs && reloads >= 3 * runs && partitioned == runs && finished == runs,
            "invites: {runs} runs, {alone} independent submissions, {reloads} reloads, {partitioned} partitions, {finished} completed");
    }
}

/// A subset starts and solves a clover before two reserved players arrive:
/// one with an offline submission, one who writes after seeing guessing.
/// Reloads and jitter must preserve the current board and completed checks.
#[test]
fn story_invited_subset_guesses_before_late_writers_arrive() {
    let (mut runs, mut resumed, mut late_submitted, mut finished) = (0, 0, 0, 0);
    for seed in seeds(120_000, 40) {
        let mut s = Sim::new(seed, 4, STORMY);
        let input = format!("{{\"room\":\"partial-group\",\"name\":\"Ann\",\"friends\":{}}}", json_str(&NAMES[1..4].join("\n")));
        let i = cloverleaf::invite::Invite::create(&input, Stamp { clock: 1, peer: seed as u32 }, seed as u32).unwrap();
        for x in 0..4 { for y in x + 1..4 { if (x, y) != (1, 2) { s.blocked.insert((x, y)); } } }
        for k in 0..4 {
            s.devs[k].name = Some(NAMES[k].into());
            let a = s.devs[k].app.as_mut().unwrap();
            a.load_invite(&i.room, &i.json()).unwrap();
            a.join(NAMES[k]);
            if k > 0 {
                for d in 0..4 { a.set_clue(d, &format!("{seed}-{k}"), false); }
                a.action(2);
            }
        }
        for _ in 0..150 { s.reconnect_some(1000); s.tick(); }
        assert_eq!(s.devs[1].app.as_ref().unwrap().shared.phase(), Phase::Writing);
        s.devs[1].app.as_mut().unwrap().action(5);
        s.run_for(150);
        assert_eq!(s.devs[2].app.as_ref().unwrap().scene().writer, Some(seat_id(NAMES[1])));
        solve(s.devs[2].app.as_mut().unwrap());
        s.devs[2].app.as_mut().unwrap().action(3);
        s.run_for(150);
        assert!(s.devs[1].app.as_ref().unwrap().shared.check(seat_id(NAMES[1])).done());
        s.devs[1].app.as_mut().unwrap().action(4);
        s.run_for(150);
        let current = s.devs[1].app.as_ref().unwrap().scene().writer;
        assert_eq!(current, Some(seat_id(NAMES[2])));
        s.reload(1);
        s.heal_for(600);
        resumed += 1;
        assert_eq!(s.devs[1].app.as_ref().unwrap().scene().writer, current);
        assert_eq!(s.devs[0].app.as_ref().unwrap().view_phase(), Phase::Writing);
        assert_eq!(s.devs[3].app.as_ref().unwrap().scene().writer, current);
        for d in 0..2 { s.devs[0].app.as_mut().unwrap().set_clue(d, "late draft", false); }
        s.reload(0);
        s.heal_for(200);
        for d in 2..4 { s.devs[0].app.as_mut().unwrap().set_clue(d, "late draft", false); }
        s.devs[0].app.as_mut().unwrap().action(2);
        s.heal_for(400);
        late_submitted += 1;
        let expected = s.digest_of(0);
        for (k, a) in s.apps() {
            assert_eq!(s.digest_of(k), expected);
            assert_eq!(a.scene().writer, current);
            assert_eq!(a.shared.round, i.round);
            assert_eq!(a.shared.puzzles().len(), 4);
            assert!(a.shared.check(seat_id(NAMES[1])).done());
            assert_eq!(a.shared.effective(seat_id(NAMES[0])).unwrap().clues, ["late draft".to_string(), "late draft".into(), "late draft".into(), "late draft".into()]);
            if let Some(why) = &s.devs[k].strip_bad { s.fail(why); }
        }
        for _ in 0..3 {
            let writer = s.devs[0].app.as_ref().unwrap().scene().writer.unwrap();
            let g = (0..4).find(|k| seat_id(NAMES[*k]) != writer).unwrap();
            solve(s.devs[g].app.as_mut().unwrap());
            s.devs[g].app.as_mut().unwrap().action(3);
            s.heal();
            s.devs[g].app.as_mut().unwrap().action(4);
            s.heal();
        }
        assert!(s.apps().all(|(_, a)| a.shared.phase() == Phase::End));
        finished += 1;
        runs += 1;
    }
    if std::env::var("SEED").is_err() {
        assert!(runs >= 40 && resumed == runs && late_submitted == runs && finished == runs,
            "late arrivals: {runs} runs, {resumed} resumes, {late_submitted} late submissions, {finished} finished");
    }
}

// ---- the bugs the random runs found, as fixed scenarios ------------------------

/// Three devices on a good network, seated, with a round dealt.
fn table3() -> Sim {
    let mut s = Sim::new(7, 3, CALM);
    s.reconnect_some(1000);
    for k in 0..3 {
        s.devs[k].name = Some(NAMES[k].into());
        s.devs[k].app.as_mut().unwrap().join(NAMES[k]);
        s.tick();
    }
    s.heal();
    s.devs[0].app.as_mut().unwrap().action(1);
    s.heal();
    s
}

#[test]
fn a_change_that_missed_one_peer_still_reaches_it() {
    // Ann's link to Cy is down when she writes a clue; then Ann's tab closes.
    // Bo and Cy stayed connected throughout, so no full sync happens between
    // them: they must notice the difference and resync on their own.
    let mut s = table3();
    s.disconnect(0, 2);
    s.blocked.insert((0, 2));
    s.devs[0].app.as_mut().unwrap().set_clue(0, "only bo saw this", false);
    for _ in 0..10 {
        s.tick();
    }
    s.drop_all_links(0);
    s.devs[0].app = None;
    s.heal_for(400);
    assert_eq!(s.digest_of(1), s.digest_of(2));
}

#[test]
fn a_player_who_reloads_and_deals_alone_cannot_wipe_the_table() {
    // Cy reloads while cut off, sees an empty table, sits down and deals.
    // When the network heals, the table in play (further along) must win.
    let mut s = table3();
    for k in 0..3 {
        let a = s.devs[k].app.as_mut().unwrap();
        a.set_clue(0, "kept", false);
    }
    s.heal();
    let round = s.devs[0].app.as_ref().unwrap().shared.round;
    s.drop_all_links(2);
    s.blocked.extend([(0, 2), (1, 2)]);
    s.devs[2].saved = Vec::new(); // no save on this device
    s.devs[2].app = Some(app(999));
    s.devs[2].app.as_mut().unwrap().join("Cy");
    s.devs[2].app.as_mut().unwrap().action(1);
    s.heal();
    let a = s.devs[2].app.as_ref().unwrap();
    assert_eq!(a.shared.round, round, "Cy's lone deal replaced the table");
    assert!(a.seat.is_some_and(|me| a.shared.seats.get(&me).is_some_and(|x| x.active)), "Cy sits back down at the table");
    assert_eq!(s.digest_of(0), s.digest_of(2));
}

#[test]
fn a_table_survives_every_device_reloading() {
    let mut s = table3();
    s.devs[1].app.as_mut().unwrap().set_clue(2, "remember me", false);
    s.heal();
    let before = s.digest_of(0);
    // Every page reloads at once; each restores its own save.
    for k in 0..3 {
        s.drop_all_links(k);
        let saved = s.devs[k].app.as_ref().unwrap().snapshot();
        let mut fresh = app(500 + k as u64);
        fresh.on_packet(&saved, 0);
        s.devs[k].app = Some(fresh);
        let name = s.devs[k].name.clone().unwrap();
        s.devs[k].app.as_mut().unwrap().join(&name);
    }
    s.heal();
    assert_eq!(s.digest_of(0), before);
    assert_eq!(s.digest_of(1), before);
}

#[test]
fn a_card_held_by_a_phone_that_vanished_is_let_go() {
    let mut s = table3();
    // Into guessing: everyone submits.
    for k in 0..3 {
        let a = s.devs[k].app.as_mut().unwrap();
        for d in 0..4u8 {
            a.set_clue(d, "clue", false);
        }
        a.action(2);
        s.heal();
    }
    let w = s.devs[0].app.as_ref().unwrap().scene().writer.unwrap();
    let g = (0..3).find(|&k| s.devs[k].app.as_ref().unwrap().seat != Some(w)).unwrap();
    // The guesser picks up a card and their phone dies mid-drag.
    let a = s.devs[g].app.as_mut().unwrap();
    a.settle();
    let (x, y, _) = a.card_screen(0);
    a.pointer_down(x, y);
    a.pointer_move(x + 40.0, y + 40.0);
    s.tick();
    s.drop_all_links(g);
    s.devs[g].app = None;
    s.heal_for(300);
    for k in (0..3).filter(|&k| k != g) {
        assert!(s.devs[k].app.as_ref().unwrap().holds().is_empty(), "device {k} still shows a held card");
    }
}

// ---- moving a group to a new room ------------------------------------------------

#[derive(Default, Debug)]
struct MoveCoverage {
    runs: u32,
    /// Group members who followed, and how many of those were away when the
    /// move was made (back later, some after a reload).
    followers: u32,
    late: u32,
    after_reload: u32,
}

/// Two groups sit down apart (group A smaller and a round behind), then meet;
/// one of A moves the group to a new room, and the rest of A follow when
/// they're told: some there at the time, some away, back later (some after
/// reloading). The new room must hold every follower's work, nobody in B may
/// be offered the move, and the old room must stay in agreement.
fn run_move(seed: u64, mc: &mut MoveCoverage) {
    let a_size = 2 + (seed % 2) as usize;
    let n = a_size + 2;
    let mut s = Sim::new(seed, n, CALM);
    s.split(a_size);
    s.run_for(20);
    for k in 0..n {
        s.devs[k].name = Some(NAMES[k].into());
        s.devs[k].app.as_mut().unwrap().join(NAMES[k]);
    }
    s.run_for(20);
    s.with(0, "deals for group A", |a| a.action(1));
    s.with(a_size, "deals for group B", |a| a.action(1));
    s.run_for(20);
    s.with(a_size, "deals B's round 2", |a| a.action(6));
    for k in 0..a_size {
        let clue = format!("a{k}");
        s.with(k, "writes", move |a| edit(a, 0, &clue));
    }
    s.run_for(40);
    let table_a = s.devs[0].app.as_ref().unwrap().shared.round.table;
    // Some of A are away when the tables meet.
    let mut away: Vec<(usize, bool)> = Vec::new();
    for k in 1..a_size {
        if s.r.below(2) == 0 {
            let reload = s.r.below(2) == 0;
            away.push((k, reload));
        }
    }
    for &(k, _) in &away {
        s.drop_all_links(k);
        s.devs[k].online = false;
        s.note(format!("dev {k} away"));
    }
    s.blocked.clear();
    s.run_for(200);
    // Ana moves group A.
    let snap = s.devs[0].app.as_mut().unwrap().record_move("room-two").unwrap_or_else(|| s.fail("group A has no table to move"));
    s.flush(0);
    s.run_for(100);
    let mut two = Sim::new(seed ^ 0x2222, 0, CALM);
    let arrive = |two: &mut Sim, snap: &[u8], name: &str, seed: u64| {
        let mut a = app(seed);
        a.on_packet(snap, 0);
        two.add(a, name);
    };
    arrive(&mut two, &snap, NAMES[0], seed * 7 + 1);
    let follow = |s: &mut Sim, two: &mut Sim, k: usize| {
        if s.devs[k].moved == 0 {
            s.fail(&format!("dev {k} of the moved group was never told"));
        }
        let snap = s.devs[k].app.as_ref().unwrap().origin_snapshot().unwrap_or_else(|| s.fail("a follower has no table to bring"));
        arrive(two, &snap, NAMES[k], seed * 7 + 2 + k as u64);
    };
    for k in 1..a_size {
        if !away.iter().any(|(j, _)| *j == k) {
            follow(&mut s, &mut two, k);
            mc.followers += 1;
        }
    }
    // The away come back, some reloading first (from their save).
    for &(k, reload) in &away {
        s.devs[k].online = true;
        if reload {
            s.reload(k);
            mc.after_reload += 1;
        }
    }
    s.run_for(300);
    for &(k, _) in &away {
        follow(&mut s, &mut two, k);
        mc.followers += 1;
        mc.late += 1;
    }
    two.run_for(300);
    // The new room: one table, group A's, with every member's clue.
    let d0 = two.digest_of(0);
    if (1..two.devs.len()).any(|k| two.digest_of(k) != d0) {
        two.fail("the new room doesn't agree");
    }
    let sh = &two.devs[0].app.as_ref().unwrap().shared;
    if sh.round.table != table_a {
        two.fail("the new room isn't group A's table");
    }
    for k in 0..a_size {
        let mine = sh.latest(seat_id(NAMES[k])).map(|x| x.clues[0].clone());
        if mine.as_deref() != Some(format!("a{k}").as_str()) {
            two.fail(&format!("{}'s clue didn't make it to the new room: {mine:?}", NAMES[k]));
        }
    }
    // Nobody in B was offered it, and the old room agrees.
    for k in a_size..n {
        if s.devs[k].moved > 0 {
            s.fail(&format!("dev {k} of the other group was offered the move"));
        }
    }
    let live: Vec<usize> = s.apps().map(|(k, _)| k).collect();
    if live.iter().any(|&k| s.digest_of(k) != s.digest_of(live[0])) {
        s.fail("the old room doesn't agree after the move");
    }
    mc.runs += 1;
}

#[test]
fn story_a_group_moves_and_follows() {
    let mut mc = MoveCoverage::default();
    for seed in seeds(100_000, 40) {
        run_move(seed, &mut mc);
    }
    println!("Move: {mc:?}");
    if std::env::var("SEED").is_err() {
        assert!(mc.followers >= 40 && mc.late >= 8 && mc.after_reload >= 4, "moves and follows, some late, some after a reload: {mc:?}");
    }
}
