//! Native harness: properties the game must satisfy, independent of browsers.

use cloverleaf::app::{App, BOLD_OFF, BOLD_ON, EV_STATUS, SHEET_CLOVERS, SHEET_JOIN, SHEET_PLAYERS, SHEET_RECAP, EV_EDIT_CLUE, EV_KEEP, EV_MOVED, EV_NET, EV_NOTICE, EV_OUTDATED, ZOOM_MAX, ZOOM_MIN};
use cloverleaf::model::*;

const TAU: f32 = std::f32::consts::TAU;

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.05
}

#[test]
fn a_failed_connection_is_not_reported_as_nobody_being_here() {
    let mut a = App::new(100);
    a.join("Ana");
    assert_eq!(a.strip().hint, vec!["Waiting for others"]);
    a.connection_failed = true;
    assert!(a.strip().hint[0].contains("Couldn't connect"));
    assert!(!a.strip().pulse);
    a.links = 1;
    assert!(!a.strip().hint[0].contains("Couldn't connect"));
    a.links = 0;
    a.solo = true;
    assert!(!a.strip().hint[0].contains("Couldn't connect"));
}

fn app(seed: u64) -> App {
    let mut a = App::new(seed);
    (a.w, a.h, a.dpr) = (1000.0, 800.0, 1.0);
    a
}

#[test]
fn recap_rendering_preserves_the_live_board_and_all_answer_words() {
    let mut a = app(7);
    a.join("Ana");
    a.action(1);
    a.q = 1;
    a.angle = std::f32::consts::FRAC_PI_2;
    a.dpr = 3.0;
    a.render();
    let before = (a.shared.digest(), a.ui_json(), a.cmds.iter().map(|f| f.to_bits()).collect::<Vec<_>>(), a.strs.clone(), a.outbox.clone());
    let board = cloverleaf::json::Json::parse(r#"{"text":"Carrie\nfix\nwinged\nhatch\nmodel\ncat|one|two|body\nrepair|victory|three|four\nfive|museum|roof|six\nseven|eight|handle|talent","solved":true,"guesses":1}"#).unwrap();
    let view = cloverleaf::pages::board_view(&board);
    let (commands, words) = a.clover_preview(&view);
    assert!(!commands.is_empty());
    let words = String::from_utf8(words).unwrap();
    for text in ["fix", "winged", "hatch", "model", "cat", "one", "two", "body", "repair", "victory", "three", "four", "five", "museum", "roof", "six", "seven", "eight", "handle", "talent"] {
        assert!(words.contains(text), "preview includes {text}, including inward card words");
    }
    assert_eq!(before, (a.shared.digest(), a.ui_json(), a.cmds.iter().map(|f| f.to_bits()).collect::<Vec<_>>(), a.strs.clone(), a.outbox.clone()));
    assert_eq!((a.q, a.angle, a.dpr), (1, std::f32::consts::FRAC_PI_2, 3.0));
}

/// Split an outbox into (net packets, other events).
fn drain(a: &mut App) -> (Vec<Vec<u8>>, Vec<(u8, Vec<u8>)>) {
    let (mut net, mut ev) = (Vec::new(), Vec::new());
    let b = std::mem::take(&mut a.outbox);
    let mut p = 0;
    while p < b.len() {
        let kind = b[p];
        let len = u32::from_le_bytes(b[p + 1..p + 5].try_into().unwrap()) as usize;
        let body = b[p + 5..p + 5 + len].to_vec();
        if kind == EV_NET { net.push(body) } else { ev.push((kind, body)) }
        p += 5 + len;
    }
    (net, ev)
}

/// Devices on a perfect network.
struct Table {
    apps: Vec<App>,
    t: f64,
}

impl Table {
    /// `n` devices, each seated under its own name.
    fn new(n: usize) -> Table {
        let mut t = Table { apps: (0..n).map(|k| app(1000 + k as u64)).collect(), t: 0.0 };
        for k in 0..n {
            t.apps[k].join(&format!("Player {k}"));
            t.pump();
        }
        t
    }
    fn add(&mut self, a: App) -> usize {
        self.apps.push(a);
        self.apps[0].on_peer_join();
        self.pump();
        self.apps.len() - 1
    }
    fn pump(&mut self) {
        for _ in 0..8 {
            self.t += 16.0;
            let mut sent = Vec::new();
            for (k, a) in self.apps.iter_mut().enumerate() {
                a.frame(self.t);
                for p in drain(a).0 {
                    sent.push((k, p));
                }
            }
            if sent.is_empty() {
                break;
            }
            for (from, p) in sent {
                for (k, a) in self.apps.iter_mut().enumerate() {
                    if k != from {
                        a.on_packet(&p, from as u32 + 1);
                    }
                }
            }
        }
    }
    fn digests_equal(&self) -> bool {
        let d = self.apps[0].shared.digest();
        self.apps.iter().all(|a| a.shared.digest() == d)
    }
    fn idx_of_seat(&self, seat: u32) -> usize {
        self.apps.iter().position(|a| a.seat == Some(seat)).unwrap()
    }
    fn guesser_for(&self, w: u32) -> usize {
        self.apps.iter().position(|a| a.seat.is_some() && a.seat != Some(w)).unwrap()
    }
    fn writer_now(&self) -> u32 {
        match self.apps[0].shared.phase() {
            Phase::Guessing(k) => self.apps[0].shared.puzzles()[k],
            p => panic!("not guessing: {p:?}"),
        }
    }
}

fn tap(a: &mut App, at: (f32, f32)) {
    a.pointer_down(at.0, at.1);
    a.pointer_up(at.0, at.1);
}

fn drag(a: &mut App, from: (f32, f32), to: (f32, f32)) {
    a.pointer_down(from.0, from.1);
    for k in 1..=10 {
        let t = k as f32 / 10.0;
        a.pointer_move(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
    }
    a.pointer_up(to.0, to.1);
}

fn card_at(a: &App, i: usize) -> (f32, f32) {
    let (x, y, _) = a.card_screen(i);
    (x, y)
}

fn write(a: &mut App, clues: [&str; 4]) {
    for (d, c) in clues.iter().enumerate() {
        a.set_clue(d as u8, c, false);
    }
}

fn write_and_ready(a: &mut App) {
    write(a, ["sea", "sky", "fire", "dog"]);
    a.action(2);
}

/// Everyone seated writes and taps Ready; pump until guessing.
fn table_in_guessing(n: usize) -> Table {
    let mut t = Table::new(n);
    t.apps[0].action(1);
    t.pump();
    for k in 0..n {
        write_and_ready(&mut t.apps[k]);
        t.pump();
    }
    t
}

/// Place the current clover correctly from device `k`'s screen.
fn solve(t: &mut Table, k: usize, decoy_in_spot0: bool) {
    let a = &mut t.apps[k];
    a.settle();
    let w = a.scene().writer.unwrap();
    let d = a.shared.deal(w);
    for s in 0..NSLOTS {
        if a.scene().locked & (1 << s) != 0 {
            continue;
        }
        let card = if decoy_in_spot0 && s == 0 { NSLOTS } else { s };
        let (from, to) = (card_at(a, card), a.slot_screen(s));
        drag(a, from, to);
        let want = if card == s { d.sol_rot[s] } else { a.scene().cards[card].rot };
        while a.scene().cards[card].rot != want {
            let at = card_at(a, card);
            tap(a, at);
        }
    }
}

#[test]
fn wire_roundtrip() {
    let s = Stamp { clock: 99, peer: 7 };
    let st = |c, p| Stamp { clock: c, peer: p };
    let round = Round { epoch: 2, seed: 42, n: 3, stamp: st(3, 1), deck: 16, table: st(1, 5), parent: st(2, 6), dealer: 9, ..Round::default() }
        .with_players(&[9, 4, 7]);
    let op = |body| Msg::Op(Op { round, body });
    let msgs = vec![
        op(Body::None),
        op(Body::Seat(9, Seat { name: "Ana ☘".into(), active: true, stamp: s })),
        op(Body::Sub(9, Sub { clues: ["a".into(), "fruit".into(), "".into(), "d".into()], decoys: 2, ready: true, stamp: s })),
        op(Body::Start(s)),
        op(Body::Card(9, 5, Card { loc: Loc::Table { x: 0.25, y: 1.0 }, rot: 3, stamp: s })),
        op(Body::Card(9, 0, Card { loc: Loc::Slot(2), rot: 1, stamp: s })),
        op(Body::Card(9, 4, Card { loc: Loc::Float { x: -3.0, y: 4.0 }, rot: 0, stamp: s })),
        op(Body::Check(9, Check { attempts: 1, mask: 0b0101, stamp: s, tried: 1 << 77 | 5 })),
        op(Body::Next(3, s)),
        op(Body::Focus(9, s)),
        op(Body::Move(st(1, 5), Moved { room: "quiet-heron-dell".into(), by: 9, stamp: s })),
        Msg::Hello { device: 5, seat: 9, sum: 0xdead_beef, ver: PROTOCOL },
        Msg::Cursor { device: 5, seat: 9, kind: 2, x: 10.0, y: -10.0, card: Some(3) },
        Msg::Cursor { device: 5, seat: 9, kind: 1, x: 0.5, y: 0.5, card: None },
    ];
    let mut b = Vec::new();
    for m in &msgs {
        encode(m, &mut b);
    }
    assert_eq!(decode(&b), msgs);
}

#[test]
fn decode_never_panics_on_garbage() {
    let mut r = Rng::new(42);
    for _ in 0..20_000 {
        let n = r.below(120);
        let b: Vec<u8> = (0..n).map(|_| r.next() as u8).collect();
        let _ = decode(&b);
    }
}

#[test]
fn full_game_two_rounds_three_players() {
    let mut t = table_in_guessing(3);
    assert!(t.digests_equal());
    assert_eq!(t.apps[0].shared.phase(), Phase::Guessing(0), "guessing starts once everyone is ready");
    assert_eq!(t.apps[0].shared.puzzles().len(), 3);
    for k in 0..3 {
        let w = t.writer_now();
        let (g, wi) = (t.guesser_for(w), t.idx_of_seat(w));
        // The writer can't move their own cards.
        let before = t.apps[wi].shared.digest();
        let (from, to) = (card_at(&t.apps[wi], 0), t.apps[wi].slot_screen(0));
        drag(&mut t.apps[wi], from, to);
        assert_eq!(t.apps[wi].shared.digest(), before);
        assert!(!t.apps[wi].can_check());

        solve(&mut t, g, k == 1);
        t.pump();
        assert!(t.apps[g].can_check());
        t.apps[g].action(3);
        t.pump();
        let c = t.apps[0].shared.check(w);
        if k == 1 {
            assert_eq!((c.attempts, c.mask, c.done()), (1, 0b1110, false));
            let (cards, claim) = t.apps[0].shared.layout(w);
            assert!(claim[0].is_none());
            assert!(matches!(cards[NSLOTS].loc, Loc::Table { .. }), "wrong card went back to the tray");
            solve(&mut t, g, false);
            t.pump();
            t.apps[g].action(3);
            t.pump();
            let c = t.apps[0].shared.check(w);
            assert_eq!((c.attempts, c.done()), (2, true), "keep guessing until solved");
        } else {
            assert_eq!((c.attempts, c.done()), (1, true));
        }
        assert!(t.digests_equal());
        t.apps[wi].action(4);
        t.pump();
    }
    assert_eq!(t.apps[1].shared.phase(), Phase::End);
    // Next round keeps the table.
    t.apps[2].action(1);
    t.pump();
    let s = &t.apps[0].shared;
    assert_eq!((s.phase(), s.round.n, s.players().len()), (Phase::Writing, 2, 3));
}

#[test]
fn guessing_waits_for_an_explicit_ready() {
    let mut t = Table::new(2);
    t.apps[0].action(1);
    t.pump();
    write(&mut t.apps[0], ["a", "b", "c", "d"]);
    write_and_ready(&mut t.apps[1]);
    t.pump();
    assert_eq!(t.apps[0].shared.phase(), Phase::Writing, "all clues written isn't enough");
    t.apps[0].action(2);
    t.apps[0].action(2); // un-ready
    t.pump();
    assert_eq!(t.apps[0].shared.phase(), Phase::Writing);
    t.apps[0].action(2);
    t.pump();
    assert_eq!(t.apps[0].shared.phase(), Phase::Guessing(0));
}

#[test]
fn submitted_players_can_keep_editing_until_guessing_starts() {
    let mut t = Table::new(2);
    t.apps[0].action(1);
    t.pump();
    let me = t.apps[0].seat.unwrap();
    write_and_ready(&mut t.apps[0]);
    t.pump();
    t.apps[0].set_clue(2, "changed", false);
    t.apps[0].set_decoys(3);
    t.pump();
    let s = &t.apps[1].shared;
    assert!(s.is_ready(me), "editing keeps you submitted");
    assert_eq!(s.latest(me).unwrap().clues[2], "changed");
    // Clearing a clue does un-submit (it can't be guessed half-empty).
    t.apps[0].set_clue(1, "", false);
    t.pump();
    assert!(!t.apps[1].shared.is_ready(me));
    t.apps[0].set_clue(1, "back", false);
    t.apps[0].action(2);
    write_and_ready(&mut t.apps[1]);
    t.pump();
    let s = &t.apps[1].shared;
    assert_eq!(s.phase(), Phase::Guessing(0));
    assert_eq!(s.effective(me).unwrap().clues, ["sea".to_string(), "back".into(), "changed".into(), "dog".into()]);
    assert_eq!(s.ncards(me), 7);
}

#[test]
fn an_edit_racing_proceed_resolves_the_same_everywhere() {
    let mut t = Table::new(3);
    t.apps[0].action(1);
    t.pump();
    let me = t.apps[0].seat.unwrap();
    write_and_ready(&mut t.apps[0]);
    write_and_ready(&mut t.apps[1]);
    t.pump();
    // Concurrently: player 0 edits while player 1 proceeds without player 2.
    t.apps[0].set_clue(0, "late edit", false);
    t.apps[1].action(5);
    let (e, p) = (drain(&mut t.apps[0]).0, drain(&mut t.apps[1]).0);
    // Deliver in opposite orders to the third device and back.
    for x in e.iter().chain(p.iter()) {
        t.apps[2].on_packet(x, 9);
    }
    for x in p.iter() {
        t.apps[0].on_packet(x, 2);
    }
    for x in e.iter() {
        t.apps[1].on_packet(x, 1);
    }
    t.pump();
    assert!(t.digests_equal());
    let c = t.apps[0].shared.effective(me).unwrap().clues[0].clone();
    assert!(c == "sea" || c == "late edit");
    assert_eq!(t.apps[2].shared.puzzles().len(), 2);
}

#[test]
fn starting_fixes_the_roster_late_joiners_wait_for_next_round() {
    let mut t = Table::new(2);
    t.apps[0].action(1);
    t.pump();
    let k = t.add(app(42));
    t.apps[k].join("Latecomer");
    t.pump();
    let late = t.apps[k].seat.unwrap();
    assert!(!t.apps[0].shared.round.players().contains(&late));
    assert!(t.apps[k].scene().cards.is_empty(), "no clover this round");
    assert!(t.apps[k].ui_json().contains("\"inRound\":false"));
    write_and_ready(&mut t.apps[0]);
    write_and_ready(&mut t.apps[1]);
    t.pump();
    assert_eq!(t.apps[k].shared.phase(), Phase::Guessing(0), "the latecomer doesn't block");
    // They can guess, and they're dealt in next round.
    let w = t.writer_now();
    assert!(t.apps[k].scene().movable.iter().any(|m| *m) || w == late);
    for _ in 0..2 {
        let w = t.writer_now();
        let g = t.guesser_for(w);
        solve(&mut t, g, false);
        t.apps[g].action(3);
        t.pump();
        t.apps[g].action(4);
        t.pump();
    }
    t.apps[k].action(1);
    t.pump();
    assert!(t.apps[0].shared.round.players().contains(&late));
}

#[test]
fn auto_proceed_ignores_who_is_online() {
    let mut t = Table::new(3);
    t.apps[0].action(1);
    t.pump();
    // Player 2 submits, then drops off entirely.
    write_and_ready(&mut t.apps[2]);
    t.pump();
    let _gone = t.apps.pop().unwrap();
    for a in t.apps.iter_mut() {
        a.on_link_lost(3);
    }
    write_and_ready(&mut t.apps[0]);
    write_and_ready(&mut t.apps[1]);
    t.pump();
    assert_eq!(t.apps[0].shared.phase(), Phase::Guessing(0), "all three submitted, offline or not");
    assert_eq!(t.apps[0].shared.puzzles().len(), 3);
}

#[test]
fn decoy_bid_sets_cards_in_play() {
    let mut t = Table::new(2);
    t.apps[0].action(1);
    t.pump();
    t.apps[0].set_decoys(3);
    write_and_ready(&mut t.apps[0]);
    t.apps[1].set_decoys(0);
    write_and_ready(&mut t.apps[1]);
    t.pump();
    let s = &t.apps[1].shared;
    assert_eq!(s.ncards(t.apps[0].seat.unwrap()), 7);
    assert_eq!(s.ncards(t.apps[1].seat.unwrap()), 4);
}

#[test]
fn away_players_block_until_someone_starts_without_them() {
    let mut t = Table::new(3);
    t.apps[1].action(1);
    t.pump();
    write_and_ready(&mut t.apps[0]);
    write_and_ready(&mut t.apps[1]);
    t.pump();
    // Going quiet (a backgrounded tab) is not leaving.
    for _ in 0..60 {
        t.t += 200.0;
        for k in 0..2 {
            t.apps[k].frame(t.t);
            drain(&mut t.apps[k]);
        }
    }
    assert_eq!(t.apps[0].shared.phase(), Phase::Writing, "a silent player still blocks");
    // Even a dropped connection doesn't auto-skip anyone (another device
    // may simply not have connected to them yet)...
    let gone = t.apps.pop().unwrap();
    for a in t.apps.iter_mut() {
        a.on_link_lost(3);
    }
    t.pump();
    assert_eq!(t.apps[0].shared.phase(), Phase::Writing);
    assert!(t.apps[0].ui_json().contains("\"ready\":false,\"away\":true"), "but they are shown as away");
    // ...a ready player decides.
    t.apps[0].action(5);
    t.pump();
    assert_eq!(t.apps[1].shared.phase(), Phase::Guessing(0));
    assert!(!t.apps[0].shared.puzzles().contains(&gone.seat.unwrap()));
}

#[test]
fn a_device_that_cant_see_everyone_never_starts_early() {
    let mut t = Table::new(3);
    t.apps[0].action(1);
    t.pump();
    // A watcher who has only heard from two of the three players.
    let mut w = app(321);
    w.watch();
    t.apps[0].on_peer_join();
    for p in drain(&mut t.apps[0]).0 {
        w.on_packet(&p, 1);
    }
    write_and_ready(&mut t.apps[0]);
    write_and_ready(&mut t.apps[1]);
    t.pump();
    for p in [0, 1] {
        t.apps[p].on_peer_join();
        for pk in drain(&mut t.apps[p]).0 {
            w.on_packet(&pk, p as u32 + 1);
        }
    }
    w.frame(t.t + 5.0);
    assert_eq!(w.shared.phase(), Phase::Writing);
}

#[test]
fn choosing_an_existing_name_takes_over_the_seat() {
    let mut t = Table::new(2);
    t.apps[0].action(1);
    t.pump();
    write(&mut t.apps[0], ["one", "two", "", ""]);
    t.pump();
    let mut phone = app(99);
    phone.on_packet(&{
        t.apps[0].on_peer_join();
        drain(&mut t.apps[0]).0.concat()
    }, 0);
    let k = t.add(phone);
    t.apps[k].join("player 0"); // names are case-insensitive identities
    t.pump();
    assert_eq!(t.apps[k].seat, t.apps[0].seat);
    assert_eq!(t.apps[0].shared.players().len(), 2, "no new seat created");
    let top = t.apps[k].petal_screen(0);
    tap(&mut t.apps[k], top);
    let (_, ev) = drain(&mut t.apps[k]);
    let edit = ev.iter().find(|(kind, _)| *kind == EV_EDIT_CLUE).expect("editor opens");
    assert_eq!(&edit.1[21..], b"one", "drafts follow the seat");
}

#[test]
fn watchers_cant_move_cards_and_dont_block() {
    let mut t = Table::new(2);
    let mut w = app(55);
    w.watch();
    let wi = t.add(w);
    t.apps[0].action(1);
    t.pump();
    assert!(t.apps[wi].scene().cards.is_empty(), "watchers don't get a clover");
    write_and_ready(&mut t.apps[0]);
    write_and_ready(&mut t.apps[1]);
    t.pump();
    assert_eq!(t.apps[0].shared.phase(), Phase::Guessing(0));
    let before = t.apps[wi].shared.digest();
    t.apps[wi].settle();
    let (from, to) = (card_at(&t.apps[wi], 0), t.apps[wi].slot_screen(0));
    drag(&mut t.apps[wi], from, to);
    assert_eq!(t.apps[wi].shared.digest(), before);
}

#[test]
fn new_game_starts_over_with_whoever_is_here() {
    let mut t = table_in_guessing(3);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    solve(&mut t, g, false);
    t.apps[g].action(3);
    t.pump();
    assert!(t.apps[0].shared.check(w).done());
    let gone = t.apps.pop().unwrap();
    for a in t.apps.iter_mut() {
        a.on_link_lost(3);
    }
    t.pump();
    t.apps[0].action(7);
    t.pump();
    let s = &t.apps[1].shared;
    assert_eq!((s.phase(), s.round.n), (Phase::Lobby, 0));
    assert_eq!(s.players().len(), 2);
    assert!(!s.players().contains(&gone.seat.unwrap()));
    assert!(t.digests_equal());
}

#[test]
fn cards_stay_where_dropped_unless_near_a_spot() {
    let mut t = table_in_guessing(2);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    let a = &mut t.apps[g];
    a.settle();
    let (sx, sy) = slot_pos(0);
    // Tentative: near spot 0 but not on it.
    let to = a.board_screen(sx + 45.0, sy + 40.0);
    let from = card_at(a, 0);
    drag(a, from, to);
    let c = a.scene().cards[0];
    match c.loc {
        Loc::Float { x, y } => assert!(approx(x, sx + 45.0) && approx(y, sy + 40.0)),
        l => panic!("expected free placement, got {l:?}"),
    }
    assert_eq!(a.scene().claim[0], Some(0), "close enough to count for a check");
    // Far from any spot: placed, not claimed.
    let to = a.board_screen(0.0, -230.0);
    let from = card_at(a, 1);
    drag(a, from, to);
    assert!(matches!(a.scene().cards[1].loc, Loc::Float { .. }));
    assert!(a.scene().claim.iter().all(|c| *c != Some(1)));
    // Very close: snaps.
    let (x2, y2) = slot_pos(2);
    let to = a.board_screen(x2 + 15.0, y2 - 10.0);
    let from = card_at(a, 2);
    drag(a, from, to);
    assert_eq!(a.scene().cards[2].loc, Loc::Slot(2));
    // Two cards may share a spot loosely while people think; nearest claims it.
    let to = a.board_screen(x2 + 60.0, y2 + 20.0);
    let from = card_at(a, 3);
    drag(a, from, to);
    assert_eq!(a.scene().claim[2], Some(2));
    // Dropped squarely on an occupied spot, a card lands on top, like a real
    // card: nothing else moves, and the one put there last counts.
    let from = card_at(a, 3);
    let to = a.slot_screen(2);
    drag(a, from, to);
    assert_eq!(a.scene().cards[3].loc, Loc::Slot(2));
    assert_eq!(a.scene().cards[2].loc, Loc::Slot(2), "the card underneath stays put");
    assert_eq!(a.scene().claim[2], Some(3), "the top card counts");
    // Take the top one away: the one left behind counts again.
    let from = card_at(a, 3);
    let to = a.board_screen(x2 + 180.0, y2 + 180.0);
    drag(a, from, to);
    assert_eq!(a.scene().claim[2], Some(2));
}

#[test]
fn turning_the_board_is_local_and_keeps_tray_cards_still() {
    let mut t = table_in_guessing(2);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    {
        let a = &mut t.apps[g];
        a.settle();
        let (from, to) = (card_at(a, 0), a.slot_screen(0));
        drag(a, from, to);
        let to = a.board_screen(-40.0, 200.0);
        let from = card_at(a, 1);
        drag(a, from, to);
    }
    t.pump();
    let a = &mut t.apps[g];
    let digest = a.shared.digest();
    let n = a.scene().cards.len();
    let before: Vec<_> = (0..n).map(|i| a.card_screen(i)).collect();
    let (cx, cy) = a.hub_screen();
    tap(a, (cx, cy));
    assert_eq!(a.q, 1, "tapping the hub turns the board");
    a.settle();
    a.frame(t.t + 1.0);
    assert!(drain(a).0.iter().all(|p| decode(p).iter().all(|m| !matches!(m, Msg::Op(_)))), "no shared ops");
    assert_eq!(a.shared.digest(), digest);
    for (i, b) in before.iter().enumerate() {
        let (x, y, ang) = a.card_screen(i);
        if board_pos(&a.scene().cards[i]).is_some() {
            let (ex, ey) = (cx - (b.1 - cy), cy + (b.0 - cx));
            assert!(approx(x, ex) && approx(y, ey), "board card {i} turns with the board");
            let d = (ang - b.2 - std::f32::consts::FRAC_PI_2).rem_euclid(TAU);
            assert!(d < 1e-3 || d > TAU - 1e-3);
        } else {
            assert!(approx(x, b.0) && approx(y, b.1) && approx(ang, b.2), "tray card {i} stays put");
        }
    }
}

#[test]
fn card_turns_and_board_turns_animate() {
    let mut t = table_in_guessing(2);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    let a = &mut t.apps[g];
    a.settle();
    let mut now = t.t;
    a.frame(now);
    let start = a.shown_angle(0);
    let at = card_at(a, 0);
    tap(a, at);
    now += 16.0;
    a.frame(now);
    let target = a.card_screen(0).2;
    let mid = a.shown_angle(0);
    let gap = |x: f32| (x - target + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI;
    assert!(gap(mid).abs() > 0.2 && gap(mid).abs() < gap(start).abs(), "card is mid-turn after one frame");
    for _ in 0..60 {
        now += 16.0;
        a.frame(now);
    }
    assert!(gap(a.shown_angle(0)).abs() < 1e-2, "card settles");
    a.rotate_view(1);
    now += 16.0;
    a.frame(now);
    assert!(a.angle > 0.1 && a.angle < 1.4, "board is mid-turn after one frame");
}

#[test]
fn tapping_leaves_edits_without_turning_the_board() {
    let mut t = Table::new(1);
    let a = &mut t.apps[0];
    a.action(1);
    a.frame(1000.0);
    drain(a);
    a.settle();
    // Tap the side and the outer edge of each leaf, not just its middle.
    for d in 0..4u8 {
        for (dx, dy) in [(0.0, -370.0), (-150.0, -330.0), (120.0, -410.0)] {
            let (x, y) = {
                let (s, c) = (d as f32 * std::f32::consts::FRAC_PI_2).sin_cos();
                (dx * c - dy * s, dx * s + dy * c)
            };
            let at = a.board_screen(x, y);
            tap(a, at);
            let (_, ev) = drain(a);
            let e = ev.iter().find(|(k, _)| *k == EV_EDIT_CLUE).expect("leaf tap opens the editor");
            assert_eq!(e.1[0], d, "tap at {dx},{dy} on leaf {d}");
            assert_eq!(a.q, 0, "tapping a leaf never turns the board");
        }
    }
    a.set_clue(0, "ocean", true);
    let (_, ev) = drain(a);
    let next = ev.iter().find(|(k, _)| *k == EV_EDIT_CLUE).expect("Enter opens the next leaf");
    assert_eq!(next.1[0], 1);
    assert_eq!(a.q, 0, "and doesn't turn the board either");
    // The editor lies along its leaf: leaf 1 is the right-hand one, so vertical.
    let angle = f32::from_le_bytes(next.1[17..21].try_into().unwrap());
    assert!((angle - std::f32::consts::FRAC_PI_2).abs() < 1e-3, "side-leaf editor runs vertically ({angle})");
}

#[test]
fn writers_see_the_two_keywords_each_leaf_links() {
    let mut t = Table::new(1);
    let a = &mut t.apps[0];
    a.action(1);
    a.frame(1000.0);
    let me = a.seat.unwrap();
    let d = a.shared.deal(me);
    // Leaf 0 (top) links the top-facing words of the cards in spots 0 and 1.
    let top = |s: usize| d.words[s][(4 - d.sol_rot[s] as usize) % 4];
    let label = format!("{} & {}", top(0), top(1));
    let strs = String::from_utf8_lossy(&a.strs).into_owned();
    assert!(strs.contains(&label), "{label} not drawn");
    assert!(!strs.contains("tap to write"));
    // Once the leaf has a clue, the reminder goes: the words are on the cards.
    write(a, ["alpha", "beta", "gamma", "delta"]);
    a.frame(2000.0);
    let strs = String::from_utf8_lossy(&a.strs).into_owned();
    assert!(strs.contains("alpha") && !strs.contains(&label), "{label} still drawn under a written clue");
}

#[test]
fn palette_meets_wcag_aa() {
    fn lum(hex: &str) -> f64 {
        let v = u32::from_str_radix(&hex[1..], 16).unwrap();
        let ch = |s: u32| {
            let c = ((v >> s) & 255) as f64 / 255.0;
            if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
    }
    let mut t = Table::new(1);
    for dark in [false, true] {
        t.apps[0].set_theme(dark);
        let json = t.apps[0].palette_json();
        for row in json.trim_matches(|c| c == '[' || c == ']').split("],[") {
            let f: Vec<&str> = row.split(',').map(|x| x.trim_matches(|c| c == '"' || c == '[' || c == ']')).collect();
            let (a, b) = (lum(f[1]), lum(f[2]));
            let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
            assert!(ratio >= 4.5, "{} ({}): contrast {ratio:.2}", f[0], if dark { "dark" } else { "light" });
        }
    }
}

#[test]
fn dragging_around_the_board_spins_it_to_the_nearest_quarter() {
    let mut t = Table::new(1);
    let a = &mut t.apps[0];
    a.action(1);
    let (cx, cy) = a.hub_screen();
    let r = 330.0;
    a.pointer_down(cx + r, cy);
    for k in 1..=20 {
        let th = (k as f32 / 20.0) * 1.45;
        a.pointer_move(cx + r * th.cos(), cy + r * th.sin());
    }
    a.pointer_up(cx + r * 1.45f32.cos(), cy + r * 1.45f32.sin());
    assert_eq!(a.q, 1);
}

/// Spins a quarter of `turn` (in quarters) over `ms`, then lets go after
/// `hold` ms without moving. Returns the quarter the board settles on.
fn spin(turn: f32, ms: f64, hold: f64) -> i32 {
    let mut t = Table::new(1);
    let a = &mut t.apps[0];
    a.action(1);
    let (cx, cy) = a.hub_screen();
    let r = 330.0;
    let mut now = 1000.0;
    a.frame(now);
    a.pointer_down(cx + r, cy);
    let end = turn * std::f32::consts::FRAC_PI_2;
    for k in 1..=10 {
        now += ms / 10.0;
        a.frame(now);
        let th = end * k as f32 / 10.0;
        a.pointer_move(cx + r * th.cos(), cy + r * th.sin());
    }
    a.frame(now + hold);
    a.pointer_up(cx + r * end.cos(), cy + r * end.sin());
    a.q
}

#[test]
fn a_quick_flick_spins_the_board_a_quarter_but_a_slow_nudge_does_not() {
    // A third of a quarter in 80 ms carries on to the next quarter, either way.
    assert_eq!(spin(0.35, 80.0, 0.0), 1);
    assert_eq!(spin(-0.35, 80.0, 0.0), -1);
    // The same distance slowly, or held still before letting go, springs back.
    assert_eq!(spin(0.35, 800.0, 0.0), 0);
    assert_eq!(spin(0.35, 80.0, 300.0), 0);
    // A fast long spin coasts at most one quarter past where it was let go.
    assert!(spin(1.0, 60.0, 0.0) <= 2);
}

#[test]
fn writer_taps_on_locked_cards_turn_the_board() {
    let mut t = Table::new(1);
    let a = &mut t.apps[0];
    a.action(1);
    a.settle();
    let before = a.scene().cards.iter().map(|c| c.rot).collect::<Vec<_>>();
    let at = card_at(a, 2);
    tap(a, at);
    assert_eq!(a.q, 1);
    assert_eq!(a.scene().cards.iter().map(|c| c.rot).collect::<Vec<_>>(), before);
}

#[test]
fn drop_in_a_turned_view_lands_in_the_right_spot() {
    let mut t = table_in_guessing(2);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    let a = &mut t.apps[g];
    a.rotate_view(1);
    a.settle();
    let top_left = (0..4)
        .min_by(|&s, &u| {
            let (sx, sy) = a.slot_screen(s);
            let (ux, uy) = a.slot_screen(u);
            (sx + sy).partial_cmp(&(ux + uy)).unwrap()
        })
        .unwrap();
    assert_eq!(top_left, 3);
    let rot_in_tray = a.scene().cards[2].rot;
    let (from, to) = (card_at(a, 2), a.slot_screen(3));
    drag(a, from, to);
    let c = a.scene().cards[2];
    assert_eq!(c.loc, Loc::Slot(3));
    assert_eq!(c.rot, (rot_in_tray + 3) % 4, "on-screen orientation is kept");
}

#[test]
fn concurrent_drops_on_one_spot_resolve_identically() {
    let mut t = table_in_guessing(3);
    let w = t.writer_now();
    let gs: Vec<usize> = (0..3).filter(|&k| t.apps[k].seat != Some(w)).collect();
    for (n, &k) in gs.iter().enumerate() {
        let a = &mut t.apps[k];
        a.settle();
        let (from, to) = (card_at(a, n), a.slot_screen(0));
        drag(a, from, to);
    }
    t.pump();
    assert!(t.digests_equal());
    let (_, claim) = t.apps[0].shared.layout(w);
    assert!(claim[0] == Some(0) || claim[0] == Some(1));
}

#[test]
fn late_joiner_receives_full_state() {
    let mut t = table_in_guessing(2);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    let (from, to) = (card_at(&t.apps[g], 1), t.apps[g].slot_screen(2));
    drag(&mut t.apps[g], from, to);
    t.pump();
    let mut late = app(77);
    t.apps[0].on_peer_join();
    for p in drain(&mut t.apps[0]).0 {
        late.on_packet(&p, 0);
    }
    assert_eq!(late.shared.digest(), t.apps[0].shared.digest());
}

#[test]
fn newer_rounds_reset_round_state_but_never_seats() {
    let mut s = Shared::default();
    let st = |c| Stamp { clock: c, peer: 1 };
    let r1 = Round { seed: 1, n: 1, stamp: st(2), ..Round::default() }.with_players(&[5, 6]);
    s.apply(&Op { round: r1, body: Body::Seat(5, Seat { name: "a".into(), active: true, stamp: st(3) }) });
    s.apply(&Op { round: r1, body: Body::Sub(5, Sub { ready: true, stamp: st(4), ..Sub::default() }) });
    let r2 = Round { seed: 2, n: 2, stamp: st(6), ..Round::default() }.with_players(&[5, 6]);
    s.apply(&Op { round: r2, body: Body::None });
    assert!(s.subs.is_empty());
    assert_eq!(s.players(), vec![5]);
    // A late op from round 1 doesn't resurrect anything.
    s.apply(&Op { round: r1, body: Body::Sub(5, Sub { stamp: st(9), ..Sub::default() }) });
    assert!(s.subs.is_empty());
    // A lone device's fresh deal (round 1, one player, even if newer) can't
    // replace a table further along; a seat it brings still joins the table.
    let lone = Round { seed: 3, n: 1, stamp: st(20), ..Round::default() }.with_players(&[7]);
    s.apply(&Op { round: lone, body: Body::Seat(7, Seat { name: "c".into(), active: true, stamp: st(21) }) });
    assert_eq!((s.round, s.players()), (r2, vec![5, 7]));
    // Nor can a deal with fewer players in, at the same round.
    s.apply(&Op { round: Round { seed: 4, n: 2, stamp: st(1), ..Round::default() }.with_players(&[7]), body: Body::None });
    assert_eq!(s.round, r2);
    // A New game (the next epoch) outranks every round of the last one, and
    // the table keeps its seats.
    let fresh = Round { epoch: 1, stamp: st(30), ..Round::default() };
    s.apply(&Op { round: fresh, body: Body::None });
    assert_eq!((s.round, s.phase(), s.players()), (fresh, Phase::Lobby, vec![5, 7]));
}

/// Random play by 3 devices, delivered out of order with duplicates.
#[test]
fn replicas_converge_under_reordering_and_duplication() {
    let (mut guessed, mut moved) = (0, 0);
    for seed in 0..120u64 {
        let mut r = Rng::new(seed);
        let mut reps: Vec<App> = (0..3).map(|k| app(seed * 10 + k + 1)).collect();
        let mut pending: Vec<(usize, Vec<u8>)> = Vec::new();
        let mut now = 0.0;
        for step in 0..90 {
            now += 50.0;
            let k = r.below(3);
            let a = &mut reps[k];
            a.frame(now);
            a.settle();
            match if step < 3 { 99 } else { r.below(12) } {
                99 | 0 => a.join(["Ann", "Bo", "Cy"][r.below(3)]),
                1 => a.action(1),
                2 => write_and_ready(a),
                3 => a.action(3),
                4 => a.action(4),
                5 => a.action(if r.below(8) == 0 { 7 } else { 5 }),
                6 => a.set_decoys(r.below(5) as u8),
                7 => a.rotate_view(if r.below(2) == 0 { 1 } else { -1 }),
                _ => {
                    let n = a.scene().cards.len();
                    if n > 0 {
                        let from = card_at(a, r.below(n));
                        let to = match r.below(3) {
                            0 => (r.below(1000) as f32, r.below(800) as f32),
                            1 => a.board_screen(r.below(600) as f32 - 300.0, r.below(600) as f32 - 300.0),
                            _ => a.slot_screen(r.below(4)),
                        };
                        drag(a, from, to);
                    }
                }
            }
            a.frame(now + 1.0);
            for p in drain(a).0 {
                for dst in 0..3 {
                    if dst != k {
                        pending.push((dst, p.clone()));
                    }
                }
            }
            let mut i = 0;
            while i < pending.len() {
                if r.below(3) == 0 {
                    let (dst, p) = pending.swap_remove(i);
                    reps[dst].on_packet(&p, 0);
                    if r.below(5) == 0 {
                        reps[dst].on_packet(&p, 0);
                    }
                } else {
                    i += 1;
                }
            }
        }
        for (dst, p) in pending.drain(..).rev() {
            reps[dst].on_packet(&p, 0);
        }
        let d0 = reps[0].shared.digest();
        for a in &reps[1..] {
            assert_eq!(a.shared.digest(), d0, "seed {seed}");
        }
        guessed += (!reps[0].shared.puzzles().is_empty()) as u32;
        moved += (!reps[0].shared.cards.is_empty()) as u32;
    }
    assert!(guessed >= 20 && moved >= 20, "coverage: {guessed} guessed, {moved} with card moves");
}

#[test]
fn deals_are_whole_cards_from_a_draw_down_deck() {
    let mut t = Table::new(3);
    t.apps[0].action(1);
    t.pump();
    let s = &t.apps[0].shared;
    let mut seen = std::collections::HashSet::new();
    for &w in s.round.players() {
        let d = s.deal(w);
        for c in d.words {
            assert!(cloverleaf::words::CARDS.contains(&c), "a real Shamrock card");
            assert!(seen.insert(c), "no card dealt twice in a round");
        }
        assert_eq!(s.deal(w).words, d.words, "deterministic");
    }
    // The next round deals from further down the deck: no repeats.
    for _ in 0..3 {
        let w = t.apps[0].shared.round.players()[0];
        let _ = w;
        t.apps[0].action(6);
        t.pump();
        let s = &t.apps[1].shared;
        for &w in s.round.players() {
            for c in s.deal(w).words {
                assert!(seen.insert(c), "card repeated before the deck ran out");
            }
        }
    }
    assert_eq!(seen.len(), 4 * 3 * MAX_CARDS);
    // Every position maps to a card, and a full pass uses each card once.
    let g = t.apps[0].shared.round.deck_seed();
    let pass: std::collections::HashSet<usize> = (0..220).map(|p| deck_card(g, p)).collect();
    assert_eq!(pass.len(), 220);
}

#[test]
fn board_fills_the_screen_and_everything_fits() {
    for (w, h) in [(390.0, 700.0), (1280.0, 760.0), (768.0, 1000.0)] {
        let mut t = table_in_guessing(2);
        let wr = t.writer_now();
        let g = t.guesser_for(wr);
        let a = &mut t.apps[g];
        (a.w, a.h) = (w, h);
        let board_px = 2.0 * EXTENT * a.scale();
        assert!(board_px >= 0.6 * w.min(h), "{w}x{h}: board is {board_px}px");
        for q in 0..4 {
            a.q = q;
            a.settle();
            for i in 0..a.scene().cards.len() {
                let (x, y, _) = a.card_screen(i);
                assert!(x > 0.0 && x < w && y > 0.0 && y < h, "{w}x{h}: card {i} off screen at q={q}");
            }
        }
    }
}

#[test]
fn card_size_is_local_and_hit_testing_follows_it() {
    let mut t = table_in_guessing(2);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    let (o, a) = {
        let (head, tail) = t.apps.split_at_mut(g.max(1));
        if g == 0 { (&mut tail[0], &mut head[0]) } else { (&mut head[0], &mut tail[0]) }
    };
    for a in [&mut *a, &mut *o] {
        (a.w, a.h) = (390.0, 700.0);
        a.settle();
    }
    let digest = a.shared.digest();
    a.set_zoom(9.0);
    assert!((a.zoom - ZOOM_MAX).abs() < 1e-6, "clamped");
    assert_eq!(a.shared.digest(), digest, "zoom is not shared state");
    // The phone default (1.25×) leaves neighbouring spots clear.
    assert!(1.25 <= cloverleaf::app::ZOOM_TOUCH);
    // A tap near the zoomed card's corner (outside its unzoomed size) still hits it.
    let (from, to) = (card_at(a, 0), a.slot_screen(1));
    drag(a, from, to);
    let (cx, cy) = a.slot_screen(1);
    // (In the tray, the drawn zoom is capped so three cards fit across.)
    assert!(a.card_zoom() > 1.0 && a.card_zoom() <= ZOOM_MAX);
    let corner = HALF * a.scale() * (1.0 + a.card_zoom()) / 2.0;
    let rot = a.scene().cards[0].rot;
    tap(a, (cx + corner, cy + corner));
    assert_eq!(a.scene().cards[0].rot, (rot + 1) % 4);
    // Tray cards never overlap, even at the largest size.
    let tray: Vec<(f32, f32)> = (0..a.scene().cards.len())
        .filter(|&i| matches!(a.scene().cards[i].loc, Loc::Table { .. }))
        .map(|i| card_at(a, i))
        .collect();
    let size = CARD * a.scale() * a.card_zoom();
    for (i, p) in tray.iter().enumerate() {
        for q in &tray[i + 1..] {
            assert!((p.0 - q.0).abs() >= size - 0.5 || (p.1 - q.1).abs() >= size - 0.5, "tray cards overlap: {p:?} {q:?}");
        }
    }
    // Every card stays on screen at the largest size.
    for i in 0..a.scene().cards.len() {
        let (x, y, _) = a.card_screen(i);
        assert!(x > 0.0 && x < a.w && y > 0.0 && y < a.h, "card {i} off screen");
    }
    // Another player at a different size sees the same positions on the board.
    o.set_zoom(ZOOM_MIN);
    t.pump();
    assert!(t.digests_equal());
}

#[test]
fn guessing_boots_wrong_cards_silently_and_never_runs_out() {
    let mut t = table_in_guessing(2);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    for round in 0..3 {
        solve(&mut t, g, true); // a decoy in spot 0 every time
        t.pump();
        drain(&mut t.apps[g]);
        t.apps[g].action(3);
        let (net, ev) = drain(&mut t.apps[g]);
        assert!(ev.iter().all(|(k, _)| *k != cloverleaf::app::EV_STATUS), "no commentary");
        for p in &net {
            for k in (0..t.apps.len()).filter(|&k| k != g) {
                t.apps[k].on_packet(p, g as u32 + 1);
            }
        }
        t.pump();
        let c = t.apps[0].shared.check(w);
        assert_eq!((c.attempts, c.mask, c.done()), (round + 1, 0b1110, false));
        assert!(matches!(t.apps[0].shared.layout(w).0[NSLOTS].loc, Loc::Table { .. }), "booted");
    }
    solve(&mut t, g, false);
    t.apps[g].action(3);
    t.pump();
    assert!(t.apps[1].shared.check(w).done());
}

/// A miss shakes the booted cards on every screen, however the guess's
/// packets are spread across frames on arrival.
#[test]
fn a_miss_shakes_booted_cards_on_other_screens() {
    let mut t = table_in_guessing(3);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    let k = (0..t.apps.len()).find(|&k| k != g && k != w as usize % t.apps.len()).unwrap_or((g + 1) % t.apps.len());
    solve(&mut t, g, true);
    t.pump();
    drain(&mut t.apps[g]);
    t.apps[g].action(3);
    let (net, _) = drain(&mut t.apps[g]);
    // One packet per frame, as a slow link would deliver them.
    let mut shook = 0;
    for p in &net {
        t.t += 16.0;
        t.apps[k].on_packet(p, g as u32 + 1);
        t.apps[k].frame(t.t);
        shook = shook.max(t.apps[k].shaking());
    }
    assert!(matches!(t.apps[k].shared.layout(w).0[NSLOTS].loc, Loc::Table { .. }), "booted");
    assert!(shook > 0, "the booted card shakes on another screen ({} packets)", net.len());
}

/// Dragging a card above the board (top of the screen) keeps it on the
/// board at the edge; only the tray takes cards off.
#[test]
fn a_card_dropped_above_the_board_stays_on_the_board() {
    let mut t = table_in_guessing(2);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    let a = &mut t.apps[g];
    let from = card_at(a, NSLOTS);
    assert!(matches!(a.shared.layout(w).0[NSLOTS].loc, Loc::Table { .. }));
    drag(a, from, (a.w / 2.0, 1.0));
    match a.shared.layout(w).0[NSLOTS].loc {
        Loc::Float { y, .. } => assert!(y < -EXTENT / 2.0, "at the board's top edge, got y {y}"),
        l => panic!("left the board: {l:?}"),
    }
}

/// A second finger (the browser's pinch zoom) cancels a drag: the card goes
/// back where it was, and nothing is tapped or turned.
#[test]
fn a_cancelled_gesture_puts_the_card_back_and_acts_on_nothing() {
    let mut t = table_in_guessing(2);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    let a = &mut t.apps[g];
    let before = a.shared.layout(w).0[NSLOTS];
    let from = card_at(a, NSLOTS);
    let to = a.slot_screen(0);
    a.pointer_down(from.0, from.1);
    for k in 1..=5 {
        let f = k as f32 / 10.0;
        a.pointer_move(from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f);
    }
    assert_ne!(a.shared.layout(w).0[NSLOTS].loc, before.loc, "mid-drag");
    a.pointer_cancel();
    let after = a.shared.layout(w).0[NSLOTS];
    assert_eq!((after.loc, after.rot), (before.loc, before.rot));
    // A cancelled tap doesn't turn the card either.
    a.pointer_down(from.0, from.1);
    a.pointer_cancel();
    assert_eq!(a.shared.layout(w).0[NSLOTS].rot, before.rot);
}

/// The game log gets each clover as multi-line text, so JSON strings must
/// escape control characters rather than drop them.
#[test]
fn review_keeps_board_lines_intact() {
    assert_eq!(json_str("a\nb\u{1}\"c"), "\"a\\nb\\u0001\\\"c\"");
    let t = table_in_guessing(2);
    let json = t.apps[0].review_json();
    let w = t.writer_now();
    let text = t.apps[0].shared.export_board(w, "x").unwrap();
    assert!(text.lines().count() >= 9);
    assert!(json.contains("\\n") && !json.contains('\n'), "newlines escaped, not dropped: {json}");
}

/// Anyone can jump to any clover while guessing; everyone follows, Next goes
/// to the next unsolved one (wrapping), and the round ends when all are solved.
#[test]
fn guessers_choose_the_order_of_clovers() {
    let mut t = table_in_guessing(3);
    let n = t.apps[0].shared.puzzles().len();
    assert_eq!(n, 3);
    t.apps[1].jump(2);
    t.pump();
    for a in &t.apps {
        assert_eq!(a.shared.phase(), Phase::Guessing(2));
    }
    // Solve clover 2; Next wraps round to clover 0 (unsolved), not "end".
    let w = t.writer_now();
    let g = t.guesser_for(w);
    solve(&mut t, g, false);
    t.apps[g].action(3);
    t.pump();
    t.apps[g].action(4);
    t.pump();
    assert_eq!(t.apps[0].shared.phase(), Phase::Guessing(0));
    // Two people jump at once: everyone ends up on the same clover.
    t.apps[0].jump(1);
    t.apps[2].jump(2);
    t.pump();
    let p = t.apps[0].shared.phase();
    assert!(t.apps.iter().all(|a| a.shared.phase() == p), "converged");
    // Solve the rest in whatever order; the round ends only when all are solved.
    for _ in 0..n {
        if !matches!(t.apps[0].shared.phase(), Phase::Guessing(_)) {
            break;
        }
        let w = t.writer_now();
        if !t.apps[0].shared.check(w).done() {
            let g = t.guesser_for(w);
            solve(&mut t, g, false);
            t.apps[g].action(3);
            t.pump();
        }
        t.apps[0].action(4);
        t.pump();
    }
    assert_eq!(t.apps[1].shared.phase(), Phase::End);
}

/// Card words stay centred. A long word between short ones keeps full size
/// (it may reach into both corners); two long neighbours collide and the
/// bigger shrinks step by step until neither reaches into the other's letters.
#[test]
fn long_card_words_use_free_corners() {
    use cloverleaf::app::{WORD_SAFE_W, WORD_SIZE};
    let mut a = App::new(1);
    let long = (WORD_SAFE_W * 1.25) * 100.0 / WORD_SIZE;
    a.set_word_widths(&format!("long\t{long}\nlong2\t{long}\nshort\t100"));
    let l = a.card_word_layout(&["long", "short", "short", "short"]);
    assert!(l.iter().all(|x| x.0 == WORD_SIZE && x.1 == 0.0), "{l:?}");
    let l = a.card_word_layout(&["long", "long2", "short", "short"]);
    assert!(l.iter().all(|x| x.1 == 0.0), "centred: {l:?}");
    assert!(l[0].0 < WORD_SIZE || l[1].0 < WORD_SIZE, "neighbouring long words collide: {l:?}");
    assert!(l[2].0 == WORD_SIZE && l[3].0 == WORD_SIZE, "short words untouched: {l:?}");
    // Resolved: at most one of the two reaches past the other's letters.
    let reach = |j: usize| long * l[j].0 / 100.0 / 2.0;
    let clear = |k: usize| 105.0 - 3.0 - 0.96 * l[k].0 - 6.0;
    assert!(reach(0) <= clear(1) + 1e-3 || reach(1) <= clear(0) + 1e-3, "{l:?}");
}

#[test]
fn imported_and_shared_boards_are_guessed_first_and_check_against_their_layout() {
    // A board read off a screenshot.
    assert!(Custom::load(
        "Josh's old phone\nsnake\nlink\nhorse\ngrappling\n\
         tube|berry|throne|spy\nanimal|map|waiter|fool\nbasket|weak|knight|toy\npond|firefight|sheep|climbing\n\
         tattoo|doe|clothing|banana"
    ));
    // Boards shared from a finished round of a real game.
    let mut src = table_in_guessing(3);
    let n = src.apps[0].shared.puzzles().len();
    let shared = src.apps[0].export_boards(0b101);
    let boards: Vec<&str> = shared.split("\n\n").collect();
    assert_eq!(boards.len(), 2, "two of {n} selected");
    for b in &boards {
        assert!(Custom::load(b));
    }
    let picked: Vec<u32> = [0, 2].iter().map(|&i| src.apps[0].shared.puzzles()[i]).collect();
    let _ = &mut src;

    let mut t = Table::new(2);
    for a in t.apps.iter_mut() {
        a.custom_room = true; // as if both opened the same link
    }
    t.apps[0].action(1);
    t.pump();
    let s = &t.apps[1].shared;
    assert_eq!(s.phase(), Phase::Guessing(0), "straight to guessing");
    let ws = s.puzzles();
    assert_eq!(ws.len(), 3);
    let w = ws[0];
    assert_eq!(s.seats[&w].name, "Josh's old phone");
    assert_eq!(s.effective(w).unwrap().clues[3], "grappling");
    assert_eq!(s.ncards(w), 5);
    assert_eq!(s.deal(w).words[2], ["basket", "weak", "knight", "toy"]);
    // The shared boards keep their writers' names, clues and decoy counts.
    for (k, &orig) in picked.iter().enumerate() {
        let (a, b) = (&src.apps[0].shared, &t.apps[0].shared);
        let w2 = ws[k + 1];
        assert_eq!(b.seats[&w2].name, src.apps[0].name_of(orig));
        assert_eq!(b.effective(w2).unwrap().clues, a.effective(orig).unwrap().clues);
        assert_eq!(b.ncards(w2), a.ncards(orig));
    }
    // Each is solved on the first guess by placing it as it was laid out.
    for k in 0..3 {
        let w = t.writer_now();
        solve(&mut t, 1, false);
        t.apps[1].action(3);
        t.pump();
        assert_eq!(t.apps[0].shared.check(w).attempts, 1);
        assert!(t.apps[0].shared.check(w).done(), "board {k}");
        assert!(t.digests_equal());
        t.apps[0].action(4);
        t.pump();
    }
    // The next round is an ordinary deal.
    t.apps[0].action(1);
    t.pump();
    assert_eq!(t.apps[0].shared.phase(), Phase::Writing);
    assert_eq!(t.apps[0].shared.round.custom, 0);
}

#[test]
fn scrambled_boards_hide_the_layout_and_round_trip_exactly() {
    let board = "Josh\nsnake\nlink\nhorse\ngrappling\n\
                 tube|berry|throne|spy\nanimal|map|waiter|fool\nbasket|weak|knight|toy\npond|firefight|sheep|climbing\n\
                 tattoo|doe|clothing|banana";
    let mut rng = Rng::new(7);
    let mut differs = 0;
    for _ in 0..50 {
        let s = scramble(board, &mut rng);
        assert!(s.lines().last().unwrap().starts_with('@'), "answer kept apart");
        // The answer comes back exactly; decoys keep a random turn (their
        // orientation means nothing, and turning every card hides which are decoys).
        let back = unscramble(&s).unwrap();
        let (b, o): (Vec<&str>, Vec<&str>) = (back.lines().collect(), board.lines().collect());
        assert_eq!(b[..9], o[..9], "answer round trips exactly");
        let decoy: [&str; 4] = b[9].split('|').collect::<Vec<_>>().try_into().unwrap();
        assert!((0..4).any(|r| turn_words(decoy, r).join("|") == o[9]), "decoy is the same card");
        // The listed cards (as words) aren't simply the answer in order.
        let listed: Vec<&str> = s.lines().skip(5).take(5).collect();
        let answer: Vec<&str> = board.lines().skip(5).collect();
        differs += (listed != answer) as u32;
    }
    assert!(differs >= 48, "scrambling actually scrambles ({differs}/50)");
    // Malformed answer lines are rejected, not trusted.
    assert!(unscramble("x\na\nb\nc\nd\n1|2|3|4\n1|2|3|4\n1|2|3|4\n1|2|3|4\n@ 0 0 0 0 1 0 2 0").is_none(), "card used twice");
    assert!(unscramble("x\na\nb\nc\nd\n1|2|3|4\n1|2|3|4\n1|2|3|4\n1|2|3|4\n@ 9 0 1 0 2 0 3 0").is_none(), "out of range");
    assert_eq!(turn_words(["t", "r", "b", "l"], 1), ["l", "t", "r", "b"], "a quarter turn moves the top word right");
}

/// The tray order must not give the answer away: across many deals, the four
/// answer cards should land in spot order only about 1 time in 24.
#[test]
fn tray_order_does_not_follow_the_answer() {
    let (mut ordered, mut total) = (0, 0);
    let mut t = Table::new(3);
    t.apps[0].action(1);
    t.pump();
    for _ in 0..60 {
        let s = &t.apps[0].shared;
        for &w in s.round.players() {
            let d = s.deal(w);
            let n = s.ncards(w);
            let rank = |i: usize| (0..n).filter(|&j| d.home[j] < d.home[i]).count();
            total += 1;
            if (0..3).all(|i| rank(i) < rank(i + 1)) {
                ordered += 1;
            }
        }
        t.apps[0].action(6); // deal new cards: a fresh round, fresh seed
        t.pump();
    }
    let share = ordered as f64 / total as f64;
    eprintln!("answer cards in spot order: {ordered} of {total} deals ({:.1}%)", share * 100.0);
    assert!(share < 0.12, "answer cards in spot order in {ordered} of {total} deals");
}

#[test]
fn players_who_join_before_meeting_both_stay_seated() {
    // Each sits down before hearing from the other, so each starts a game of
    // its own; when they meet, the later game wins and the other's seat is
    // taken back under the same name.
    let (mut a, mut b) = (app(71), app(72));
    a.join("Ana");
    b.join("Ben");
    drain(&mut a);
    drain(&mut b);
    let mut t = Table { apps: vec![a, b], t: 0.0 };
    t.apps[0].on_peer_join();
    t.apps[1].on_peer_join();
    for _ in 0..4 {
        t.pump();
    }
    for a in &t.apps {
        let names: Vec<_> = a.shared.seats.values().filter(|s| s.active).map(|s| s.name.clone()).collect();
        assert!(names.contains(&"Ana".to_string()) && names.contains(&"Ben".to_string()), "{names:?}");
    }
    assert!(t.digests_equal());
}

#[test]
fn a_dragged_card_shows_as_held_elsewhere_until_let_go() {
    let mut t = table_in_guessing(3);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    let other = (0..3).find(|&k| k != g).unwrap();
    let a = &mut t.apps[g];
    a.settle();
    let from = card_at(a, 0);
    a.pointer_down(from.0, from.1);
    a.pointer_move(from.0 + 40.0, from.1 + 40.0);
    t.pump();
    let seat = t.apps[g].seat.unwrap();
    assert_eq!(t.apps[other].holds().get(&0), Some(&seat), "held while dragging");
    let a = &mut t.apps[g];
    a.pointer_up(from.0 + 40.0, from.1 + 40.0);
    t.t += 100.0; // past the pointer's send throttle
    t.pump();
    assert!(t.apps[other].holds().is_empty(), "let go");
    assert!(t.digests_equal());
}

/// Like `Table::pump`, keeping the events each device raised.
fn pump_events(t: &mut Table) -> Vec<(usize, u8, String)> {
    let mut events = Vec::new();
    for _ in 0..8 {
        t.t += 16.0;
        let mut sent = Vec::new();
        for (k, a) in t.apps.iter_mut().enumerate() {
            a.frame(t.t);
            let (net, ev) = drain(a);
            sent.extend(net.into_iter().map(|p| (k, p)));
            events.extend(ev.into_iter().map(|(kind, b)| (k, kind, String::from_utf8_lossy(&b).into_owned())));
        }
        if sent.is_empty() {
            break;
        }
        for (from, p) in sent {
            for (k, a) in t.apps.iter_mut().enumerate() {
                if k != from {
                    a.on_packet(&p, from as u32 + 1);
                }
            }
        }
    }
    events
}

/// Events of `kind` per device, as (device, JSON).
fn of(events: &[(usize, u8, String)], kind: u8) -> Vec<(usize, String)> {
    events.iter().filter(|e| e.1 == kind).map(|e| (e.0, e.2.clone())).collect()
}

/// Two devices that sat down apart, with a round dealt (one table).
fn pair(names: [&str; 2], seed: u64) -> Table {
    let mut t = Table { apps: vec![app(seed), app(seed + 1)], t: 0.0 };
    for (k, n) in names.iter().enumerate() {
        t.apps[k].join(n);
        t.pump();
    }
    t.apps[0].action(1);
    t.pump();
    t
}

/// Merge two tables' devices onto one network: everyone connects.
fn meet(a: Table, b: Table) -> (Table, Vec<(usize, u8, String)>) {
    let mut t = Table { apps: a.apps.into_iter().chain(b.apps).collect(), t: a.t.max(b.t) + 1000.0 };
    for a in t.apps.iter_mut() {
        a.on_peer_join();
    }
    let ev = pump_events(&mut t);
    (t, ev)
}

#[test]
fn every_round_change_is_explained_once_by_what_it_was() {
    let mut t = Table::new(3);
    t.apps[0].action(1);
    pump_events(&mut t);
    // Dealt by someone else: "next", naming the dealer; the dealer hears nothing.
    t.apps[1].action(6);
    let ev = pump_events(&mut t);
    let n = of(&ev, EV_NOTICE);
    assert_eq!(n.len(), 2, "{n:?}");
    assert!(n.iter().all(|(k, j)| *k != 1 && j.contains("\"kind\":\"next\"") && j.contains("\"by\":\"Player 1\"")), "{n:?}");
    // A New game.
    t.apps[2].action(7);
    let n = of(&pump_events(&mut t), EV_NOTICE);
    assert!(n.len() == 2 && n.iter().all(|(_, j)| j.contains("\"kind\":\"newGame\"") && j.contains("Player 2")), "{n:?}");
    // Two deals cross while cut off from each other: the loser hears "crossed".
    t.apps[0].action(1);
    pump_events(&mut t);
    t.apps[2].action(6);
    t.apps[1].action(6); // both before any delivery: neither has seen the other's
    let n = of(&pump_events(&mut t), EV_NOTICE);
    assert!(n.iter().any(|(_, j)| j.contains("\"kind\":\"crossed\"")), "{n:?}");
    // Rounds missed while away: a device that slept through two deals.
    let mut away = t.apps.pop().unwrap();
    for k in 0..2 {
        t.apps[k].on_link_lost(3);
    }
    t.apps[0].action(6);
    t.pump();
    t.apps[1].action(6);
    t.pump();
    drain(&mut away);
    t.apps.push(away);
    for a in t.apps.iter_mut() {
        a.on_peer_join();
    }
    let n = of(&pump_events(&mut t), EV_NOTICE);
    assert!(n.len() == 1 && n[0].0 == 2 && n[0].1.contains("\"kind\":\"missed\""), "{n:?}");
    assert!(t.digests_equal());
}

#[test]
fn two_groups_meeting_in_one_room_merge_and_the_displaced_round_is_kept() {
    // Ana and Ben are writing their first round; Cy and Dee are a round ahead.
    let mut ab = pair(["Ana", "Ben"], 81);
    write(&mut ab.apps[0], ["moon", "", "", ""]);
    ab.pump();
    let mut cd = pair(["Cy", "Dee"], 91);
    cd.apps[1].action(6);
    cd.pump();
    let (mut t, events) = meet(ab, cd);
    assert!(t.digests_equal());
    assert_eq!((t.apps[0].shared.round.n, t.apps[0].shared.players().len()), (2, 4), "one table: the round further along, everyone seated");
    let notices = of(&events, EV_NOTICE);
    let ana = notices.iter().find(|e| e.0 == 0).expect("Ana is told");
    assert!(ana.1.contains("\"kind\":\"otherTable\"") && ana.1.contains("\"Cy\"") && ana.1.contains("\"Dee\"") && !ana.1.contains("\"Ben\""), "{}", ana.1);
    assert!(notices.iter().all(|e| e.0 < 2), "Cy and Dee lost nothing and are told nothing: {notices:?}");
    let kept = of(&events, EV_KEEP);
    assert!(kept.iter().any(|(k, j)| *k == 0 && j.contains("moon") && j.contains("\"finished\":false")), "Ana keeps her round: {kept:?}");
    // A late edit to the replaced round (it crossed the merge) still lands there.
    let r1 = t.apps[1].aside.as_ref().unwrap().round;
    let late = Op { round: r1, body: Body::Sub(seat_id("Ben"), Sub { clues: ["late".into(), "".into(), "".into(), "".into()], stamp: Stamp { clock: 999, peer: 3 }, ..Sub::default() }) };
    let mut b = Vec::new();
    encode(&Msg::Op(late), &mut b);
    t.apps[0].on_packet(&b, 2);
    let (_, ev) = drain(&mut t.apps[0]);
    assert!(ev.iter().any(|(k, j)| *k == EV_KEEP && String::from_utf8_lossy(j).contains("late")), "the kept round is updated");
    // Next round deals everyone in.
    t.apps[2].action(6);
    t.pump();
    assert_eq!(t.apps[0].shared.round.players().len(), 4);
}

#[test]
fn a_moved_group_is_offered_the_move_and_nobody_else_is() {
    let mut ab = pair(["Ana", "Ben"], 61);
    write(&mut ab.apps[1], ["kept", "", "", ""]);
    ab.pump();
    let mut cd = pair(["Cy", "Dee"], 71);
    cd.apps[0].action(6);
    cd.pump();
    let (mut t, _) = meet(ab, cd);
    // Cy and Dee came from the winning table: there's nothing to move.
    assert!(t.apps[2].record_move("nope").is_none());
    // Ana moves her group.
    let seed = t.apps[0].record_move("quiet-heron-dell").expect("Ana came from another table");
    let ev = pump_events(&mut t);
    let moved = of(&ev, EV_MOVED);
    assert_eq!(moved.len(), 1, "only Ben is offered it (Ana did it): {moved:?}");
    assert!(moved[0].0 == 1 && moved[0].1.contains("quiet-heron-dell") && moved[0].1.contains("\"by\":\"Ana\""), "{moved:?}");
    // The new room, seeded from Ana's copy and Ben's, is their table again,
    // with Ben's draft; the old room carries on as it was.
    let mut new_room = Table { apps: vec![app(500), app(501)], t: 0.0 };
    new_room.apps[0].on_packet(&seed, 0);
    let bens = t.apps[1].origin_snapshot().unwrap();
    new_room.apps[1].on_packet(&bens, 0);
    new_room.apps[0].join("Ana");
    new_room.apps[1].join("Ben");
    for a in new_room.apps.iter_mut() {
        a.on_peer_join();
    }
    new_room.pump();
    assert!(new_room.digests_equal());
    let s = &new_room.apps[0].shared;
    assert_eq!((s.round.n, s.players().len()), (1, 2));
    assert_eq!(s.latest(seat_id("Ben")).map(|x| x.clues[0].clone()), Some("kept".into()));
    assert_eq!(t.apps[2].shared.round.n, 2, "the old room is untouched");
}

#[test]
fn a_page_that_hears_a_newer_protocol_asks_to_reload_and_stops_sending_changes() {
    let mut t = Table::new(2);
    let mut b = Vec::new();
    encode(&Msg::Hello { device: 77, seat: 0, sum: 0, ver: PROTOCOL + 1 }, &mut b);
    t.apps[0].on_packet(&b, 9);
    t.apps[0].action(1);
    let (net, ev) = drain(&mut t.apps[0]);
    assert!(ev.iter().any(|(k, _)| *k == EV_OUTDATED));
    assert!(net.iter().all(|p| decode(p).iter().all(|m| !matches!(m, Msg::Op(_)))), "no table changes go out");
}

#[test]
fn a_round_that_ends_is_kept_once_finished() {
    let mut t = table_in_guessing(2);
    let mut events = Vec::new();
    for _ in 0..2 {
        let w = t.writer_now();
        let g = t.guesser_for(w);
        solve(&mut t, g, false);
        t.apps[g].action(3);
        events.extend(pump_events(&mut t));
        t.apps[g].action(4);
        events.extend(pump_events(&mut t));
    }
    assert_eq!(t.apps[0].shared.phase(), Phase::End);
    let kept = of(&events, EV_KEEP);
    assert!((0..2).all(|k| kept.iter().any(|(d, j)| *d == k && j.contains("\"finished\":true"))), "{kept:?}");
}

#[test]
fn a_new_game_mid_round_keeps_the_round_it_replaced() {
    let mut t = Table::new(2);
    t.apps[0].action(1);
    t.pump();
    write(&mut t.apps[1], ["kept", "", "", ""]);
    t.pump();
    t.apps[0].action(7);
    let events = pump_events(&mut t);
    let kept = of(&events, EV_KEEP);
    assert!(kept.iter().any(|(k, j)| *k == 0 && j.contains("kept")) && kept.iter().any(|(k, j)| *k == 1 && j.contains("kept")), "both keep it: {kept:?}");
    let n = of(&events, EV_NOTICE);
    assert!(n.len() == 1 && n[0].0 == 1 && n[0].1.contains("newGame"), "only the other device is told: {n:?}");
}

/// The status strip's first line, with names in bold shown as *name*.
fn status(a: &App) -> String {
    a.strip().hint.first().cloned().unwrap_or_default().replace(BOLD_ON, "*").replace(BOLD_OFF, "*")
}

#[test]
fn the_strip_calls_players_by_first_name_unless_two_share_it() {
    let mut t = Table { apps: vec![app(1), app(2), app(3), app(4)], t: 0.0 };
    for (k, n) in ["Bartholomew Wigglesworth", "Ben Smith", "Ben Jones", "WWWWWWWWWWWWWWWWWWWWWWWW"].iter().enumerate() {
        t.apps[k].join(n);
        t.pump();
    }
    let a = &t.apps[0];
    let short: Vec<String> = a.shared.players().iter().map(|&p| a.short_name(p)).collect();
    for want in ["Bartholomew", "Ben Smith", "Ben Jones", "WWWWWWWWWWW…"] {
        assert!(short.iter().any(|s| s == want), "{want} in {short:?}");
    }
}

#[test]
fn the_strip_says_who_is_missing_and_offers_to_start_anyway() {
    let mut t = Table::new(3);
    for a in t.apps.iter_mut() {
        a.links = 2;
    }
    assert_eq!(status(&t.apps[0]), "3 players here");
    assert_eq!(t.apps[0].strip().go.map(|b| (b.label, b.act)), Some(("Start".into(), "deal")));
    t.apps[0].action(1);
    t.pump();
    assert_eq!(status(&t.apps[0]), "0 of 4 clues written");
    write_and_ready(&mut t.apps[0]);
    t.pump();
    // Everyone is a "Player": full names, and the override needn't repeat them.
    let st = t.apps[0].strip();
    assert_eq!(status(&t.apps[0]), "Waiting for *Player 1* and *Player 2*");
    assert_eq!(st.hint.last().map(String::as_str), Some("Waiting for 2"), "shorter versions for narrow screens: {:?}", st.hint);
    assert_eq!(st.alt.map(|b| (b.label, b.act)), Some(("Start anyway".into(), "proceed")));
    assert!(st.go.is_none());
    // Cut off: say so rather than look normal.
    t.apps[0].links = 0;
    let st = t.apps[0].strip();
    assert!(st.pulse && status(&t.apps[0]) == "Reconnecting to the table…" && st.invite);
}

#[test]
fn a_watcher_and_the_writer_see_who_is_being_guessed() {
    let mut t = table_in_guessing(3);
    for a in t.apps.iter_mut() {
        a.links = 2;
    }
    let w = t.writer_now();
    let writer = t.idx_of_seat(w);
    let guesser = t.guesser_for(w);
    assert_eq!(status(&t.apps[writer]), "They're guessing your clover");
    assert_eq!(status(&t.apps[guesser]), format!("*Player {writer}'s* clover, first of 3"));
    assert_eq!(t.apps[guesser].strip().tap, Some("clovers"));
}

#[test]
fn a_notice_says_what_happened_in_a_sentence() {
    let mut t = Table::new(2);
    t.apps[0].action(1);
    pump_events(&mut t);
    write(&mut t.apps[0], ["moon", "", "", ""]);
    t.pump();
    t.apps[1].action(6);
    let n = of(&pump_events(&mut t), EV_NOTICE);
    assert!(n.iter().any(|(k, j)| *k == 0 && j.contains("\"text\":\"Player 1 dealt round 2. Your round is in Past games.\"") && j.contains("\"action\":null")), "{n:?}");
}

#[test]
fn a_saved_table_is_restored_from_this_version_without_an_age_limit() {
    let mut t = Table::new(2);
    t.apps[0].action(1);
    t.pump();
    let saved = t.apps[0].snapshot();
    let fresh = || app(77);
    assert!(!fresh().restore(&saved, PROTOCOL as u32 + 1), "another version's save");
    let mut b = fresh();
    assert!(b.restore(&saved, PROTOCOL as u32));
    assert_eq!(b.shared.digest(), t.apps[0].shared.digest());
}

#[test]
fn sheets_say_who_is_doing_what_and_how_the_round_went() {
    let mut t = table_in_guessing(3);
    for a in t.apps.iter_mut() {
        a.links = 2;
    }
    let w = t.writer_now();
    let g = t.guesser_for(w);
    let players = cloverleaf::json::Json::parse(&t.apps[g].sheet_json(SHEET_PLAYERS)).unwrap();
    let doing: Vec<(String, String)> = players.get("rows").arr().iter().map(|r| (format!("{}{}", r.get("name").str().unwrap(), r.get("you").str().unwrap()), r.get("doing").str().unwrap().to_string())).collect();
    assert!(doing.contains(&(format!("Player {}", t.idx_of_seat(w)), "their clover is up".into())), "{doing:?}");
    assert!(doing.contains(&(format!("Player {g} (you)"), "here".into())), "{doing:?}");
    let join = cloverleaf::json::Json::parse(&t.apps[g].sheet_json(SHEET_JOIN)).unwrap();
    assert_eq!((join.get("heading").str(), join.get("rows").arr().len()), (Some("Or continue as:"), 2), "everyone but you");
    let clovers = cloverleaf::json::Json::parse(&t.apps[g].sheet_json(SHEET_CLOVERS)).unwrap();
    assert_eq!(clovers.get("rows").arr()[0].get("note").str(), Some("now"));
    // Solve every clover first try: the recap says so.
    for _ in 0..3 {
        let w = t.writer_now();
        let k = t.guesser_for(w);
        solve(&mut t, k, false);
        t.pump();
        t.apps[k].action(3);
        t.pump();
        t.apps[k].action(4);
        t.pump();
    }
    let recap = cloverleaf::json::Json::parse(&t.apps[0].sheet_json(SHEET_RECAP)).unwrap();
    assert_eq!(recap.get("stats").to_string(), r#"[["3/3","solved"],["3","guesses"],["3","first try"]]"#);
    assert_eq!(recap.get("boards").arr()[0].get("badge").str(), Some("☘ first try"));
    assert_eq!((recap.get("head").str(), recap.get("next").bool()), (Some("Round 1 done ☘"), Some(true)));
}

#[test]
fn a_rejected_placement_is_marked_when_it_comes_back() {
    let mut t = table_in_guessing(3);
    let w = t.writer_now();
    let g = t.guesser_for(w);
    let other = (0..3).find(|&k| k != g).unwrap();
    // Everything right except card 0, turned a quarter wrong.
    solve(&mut t, g, false);
    let at = card_at(&t.apps[g], 0);
    tap(&mut t.apps[g], at);
    t.pump();
    let wrong = t.apps[g].scene().cards[0].rot;
    assert_eq!(t.apps[g].rejected_at(0), None, "nothing rejected yet");
    t.apps[g].action(3);
    t.pump();
    assert!(t.apps[g].shared.check(w).rejected(0, 0, wrong));
    // Put it back in spot 0, turned the same wrong way: every screen marks it.
    let a = &mut t.apps[g];
    a.settle();
    let (from, to) = (card_at(a, 0), a.slot_screen(0));
    drag(a, from, to);
    while t.apps[g].scene().cards[0].rot != wrong {
        let at = card_at(&t.apps[g], 0);
        tap(&mut t.apps[g], at);
    }
    t.pump();
    assert_eq!(t.apps[g].rejected_at(0), Some(0));
    assert_eq!(t.apps[other].rejected_at(0), Some(0), "the mark is shared");
    // Turned once more: a turn not yet tried here, so no mark.
    let at = card_at(&t.apps[g], 0);
    tap(&mut t.apps[g], at);
    t.pump();
    assert_eq!(t.apps[g].rejected_at(0), None);
    // Right at last: no mark, and checking solves it.
    let a = &mut t.apps[g];
    let want = a.shared.deal(w).sol_rot[0];
    while a.scene().cards[0].rot != want {
        let at = card_at(a, 0);
        tap(a, at);
    }
    assert_eq!(t.apps[g].rejected_at(0), None);
    t.apps[g].action(3);
    t.pump();
    assert!(t.apps[g].shared.check(w).done() && t.apps[g].rejected_at(0).is_none());
}

/// Start dragging card `i` on device `k` toward `to` (screen px), a few
/// steps, without letting go.
fn grab_toward(a: &mut App, i: usize, to: (f32, f32)) {
    let from = card_at(a, i);
    a.pointer_down(from.0, from.1);
    for k in 1..=4 {
        let t = k as f32 / 4.0;
        a.pointer_move(from.0 + (to.0 - from.0) * t * 0.5, from.1 + (to.1 - from.1) * t * 0.5);
    }
}

fn statuses(a: &mut App) -> Vec<String> {
    drain(a).1.into_iter().filter(|(k, _)| *k == EV_STATUS).map(|(_, b)| String::from_utf8_lossy(&b).into_owned()).collect()
}

#[test]
fn a_card_is_in_one_hand_at_a_time() {
    let mut t = table_in_guessing(3);
    let w = t.writer_now();
    let gs: Vec<usize> = (0..3).filter(|&k| t.apps[k].seat != Some(w)).collect();
    let (a, b) = (gs[0], gs[1]);
    for x in t.apps.iter_mut() {
        x.settle();
    }
    // A picks up card 1; once B's screen knows, B can't take it.
    let to = t.apps[a].slot_screen(0);
    grab_toward(&mut t.apps[a], 1, to);
    t.pump();
    // Held still, finger down, for longer than a silent hand's time-out:
    // still held.
    for _ in 0..40 {
        t.t += 150.0;
        let now = t.t;
        for x in t.apps.iter_mut() {
            x.frame(now);
        }
        t.pump();
    }
    let before = t.apps[b].scene().cards[1];
    let from = card_at(&t.apps[b], 1);
    let away = t.apps[b].slot_screen(3);
    drag(&mut t.apps[b], from, away);
    assert_eq!(t.apps[b].scene().cards[1].loc, before.loc, "B's drag did nothing");
    assert!(statuses(&mut t.apps[b]).iter().any(|s| s.contains("is moving this card")), "B is told who has it");
    // Nor turn it with a tap.
    let at = card_at(&t.apps[b], 1);
    tap(&mut t.apps[b], at);
    assert_eq!(t.apps[b].scene().cards[1].rot, before.rot, "B's tap didn't turn it");
    // A lets go on spot 0: everyone agrees it's there, and now B can take it.
    let to = t.apps[a].slot_screen(0);
    t.apps[a].pointer_up(to.0, to.1);
    t.pump();
    assert!(t.digests_equal());
    assert_eq!(t.apps[b].scene().cards[1].loc, Loc::Slot(0));
    let from = card_at(&t.apps[b], 1);
    drag(&mut t.apps[b], from, away);
    assert_eq!(t.apps[b].scene().cards[1].loc, Loc::Slot(3), "free again once A let go");
}

#[test]
fn two_hands_grabbing_at_once_leave_one_holding_it() {
    let mut t = table_in_guessing(3);
    let w = t.writer_now();
    let gs: Vec<usize> = (0..3).filter(|&k| t.apps[k].seat != Some(w)).collect();
    let (a, b) = (gs[0], gs[1]);
    let watcher = (0..3).find(|&k| k != a && k != b).unwrap();
    for x in t.apps.iter_mut() {
        x.settle();
    }
    // Both grab card 2 before either hears of the other.
    let (ta, tb) = (t.apps[a].slot_screen(1), t.apps[b].slot_screen(3));
    grab_toward(&mut t.apps[a], 2, ta);
    grab_toward(&mut t.apps[b], 2, tb);
    t.pump();
    let (keeper, _loser, drop) = if t.apps[a].device < t.apps[b].device { (a, b, 1u8) } else { (b, a, 3u8) };
    // Both keep moving; only the keeper's moves count, and the watcher sees one hand.
    for _ in 0..3 {
        for &k in &[a, b] {
            let to = if k == a { ta } else { tb };
            t.apps[k].pointer_move(to.0 - 5.0, to.1 - 5.0);
        }
        t.pump();
    }
    let keeper_seat = t.apps[keeper].seat.unwrap();
    assert_eq!(t.apps[watcher].holds().get(&2), Some(&keeper_seat), "the watcher sees only the keeper's hand");
    assert!(!t.apps[keeper].holds().contains_key(&2), "the loser's hand let go");
    // Both let go over their own spots: the card goes where the keeper put it.
    t.apps[a].pointer_up(ta.0, ta.1);
    t.apps[b].pointer_up(tb.0, tb.1);
    t.pump();
    assert!(t.digests_equal());
    assert_eq!(t.apps[watcher].scene().cards[2].loc, Loc::Slot(drop));
}

#[test]
fn a_hand_that_goes_silent_lets_go_of_its_card() {
    let mut t = table_in_guessing(3);
    let w = t.writer_now();
    let gs: Vec<usize> = (0..3).filter(|&k| t.apps[k].seat != Some(w)).collect();
    let (a, b) = (gs[0], gs[1]);
    for x in t.apps.iter_mut() {
        x.settle();
    }
    let to = t.apps[a].slot_screen(0);
    grab_toward(&mut t.apps[a], 1, to);
    t.pump();
    assert!(t.apps[b].holds().contains_key(&1));
    // A's phone dies mid-drag: nothing more from it. 4 s later, B can take the card.
    let later = t.t + 4100.0;
    t.apps[b].frame(later);
    assert!(!t.apps[b].holds().contains_key(&1));
    let from = card_at(&t.apps[b], 1);
    let away = t.apps[b].slot_screen(3);
    drag(&mut t.apps[b], from, away);
    assert_eq!(t.apps[b].scene().cards[1].loc, Loc::Slot(3));
}

#[test]
fn on_a_wide_screen_guess_sits_by_the_clover_clear_of_cards_and_tray() {
    let mut t = table_in_guessing(2);
    let g = (0..2).find(|&k| t.apps[k].strip().go.is_some_and(|b| b.act == "check")).expect("a guesser");
    let w = 1 - g;
    for (sw, sh, near) in [(1920.0, 1030.0, true), (1280.0, 670.0, true), (1440.0, 850.0, true), (844.0, 340.0, false), (390.0, 790.0, false), (1000.0, 1400.0, false)] {
        let a = &mut t.apps[g];
        (a.w, a.h) = (sw, sh);
        let st = a.strip();
        assert_eq!(st.go_at.is_some(), near, "{sw}x{sh}: {:?}", st.go_at);
        let Some((x, y, size)) = st.go_at else { continue };
        let l = a.layout();
        // The corner square: past every card spot, before the tray, on screen.
        let spots = (SLOT_OFF + HALF) * l.k;
        let (tx, ..) = l.tray.unwrap();
        let (x0, y0, x1, y1) = (x - size / 2.0, y - size / 2.0, x + size / 2.0, y + size / 2.0);
        assert!(x0 >= l.bc.0 + spots - 0.5 && y0 >= l.bc.1 + spots - 0.5, "{sw}x{sh}: overlaps the card spots");
        assert!(x1 <= tx && y1 <= sh && size >= 130.0, "{sw}x{sh}: runs into the tray or off screen");
        // The writer has no button to place.
        let wa = &mut t.apps[w];
        (wa.w, wa.h) = (sw, sh);
        assert_eq!(wa.strip().go_at, None);
    }
}
