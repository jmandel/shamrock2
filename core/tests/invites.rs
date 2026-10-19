use cloverleaf::{app::{App, ACT_NEW_GAME, ACT_READY, ACT_REDEAL, ACT_PROCEED, ACT_NEXT, ACT_WRITE, ACT_JOIN_GUESSING, SHEET_JOIN}, invite::Invite, json::Json, model::*};
use std::collections::BTreeSet;

fn invite(n: usize) -> Invite {
    let friends = (1..n).map(|k| format!("Player {k}")).collect::<Vec<_>>().join("\n");
    let input = format!("{{\"room\":\"Friday Night\",\"name\":\"Player 0\",\"friends\":{}}}", json_str(&friends));
    let text = App::new(71).create_invite(&input);
    let room = Json::parse(&text).unwrap().get("room").str().unwrap().to_string();
    Invite::parse(&text, &room).unwrap()
}

fn open(i: &Invite, k: usize) -> App {
    let mut a = App::new(900 + k as u64);
    a.load_invite(&i.room, &i.json()).unwrap();
    a.join(&i.names[k]);
    a
}

fn submit(a: &mut App, text: &str) {
    for d in 0..4 { a.set_clue(d, text, false); }
    a.action(ACT_READY);
}

#[test]
fn invited_writers_see_connection_failures_but_can_keep_writing() {
    let i = invite(2);
    let mut a = open(&i, 0);
    assert!(!a.strip().hint[0].contains("Couldn't connect"));
    a.connection_failed = true;
    assert!(a.strip().hint[0].contains("Couldn't connect"));
    submit(&mut a, "clue");
    assert!(a.shared.is_ready(a.seat.unwrap()));
}

fn sync(a: &mut App, b: &mut App) {
    for lap in 0..3 {
        let (sa, sb) = (a.snapshot(), b.snapshot());
        a.on_packet(&sb, 2);
        b.on_packet(&sa, 1);
        a.frame(100.0 + lap as f64);
        b.frame(100.0 + lap as f64);
    }
    assert_eq!(a.shared, b.shared);
}

#[test]
fn late_writers_and_offline_submissions_join_without_moving_the_current_board() {
    for wrote_offline in [false, true] {
        let i = invite(4);
        let (mut a, mut b, mut late) = (open(&i, 1), open(&i, 2), open(&i, 0));
        if wrote_offline { submit(&mut late, "offline"); }
        submit(&mut a, "a");
        submit(&mut b, "b");
        sync(&mut a, &mut b);
        assert_eq!(a.strip().alt.unwrap().act, "proceed");
        a.action(ACT_PROCEED);
        sync(&mut a, &mut b);
        let writer = a.scene().writer.unwrap();
        let stamp = Stamp { clock: 100, peer: a.device };
        a.shared.apply(&Op { round: i.round, body: Body::Check(writer, Check { attempts: 2, mask: 1, tried: 1 << 9, stamp }) });
        a.shared.apply(&Op { round: i.round, body: Body::Card(writer, 2, Card { loc: Loc::Slot(3), rot: 1, stamp }) });
        sync(&mut a, &mut b);
        let (cards, checks) = (a.shared.cards.clone(), a.shared.checks.clone());
        a.rotate_view(1);
        a.settle();
        sync(&mut a, &mut late);
        if !wrote_offline {
            assert_eq!(late.view_phase(), Phase::Writing);
            assert_eq!(late.scene().writer, late.seat);
            assert!(late.agent_view_json().contains("leafWords\":[\""), "late writers see their own word pairs");
            assert_eq!(late.strip().go.unwrap().label, "Add my clover");
            assert!(late.layout().tray.is_none());
            late.action(ACT_JOIN_GUESSING);
            assert!(matches!(late.view_phase(), Phase::Guessing(_)));
            assert_eq!(late.scene().writer, Some(writer));
            assert_eq!(late.strip().alt.unwrap().label, "Write my clover");
            late.action(ACT_WRITE);
            submit(&mut late, "late");
            assert!(matches!(late.view_phase(), Phase::Guessing(_)));
            let saved = late.snapshot();
            late.set_clue(0, "cannot edit a live puzzle", false);
            late.set_decoys(4);
            late.action(ACT_READY);
            assert_eq!(late.snapshot(), saved, "submitted late clues are frozen");
        }
        sync(&mut a, &mut late);
        sync(&mut a, &mut b);
        assert_eq!(a.shared.puzzles().len(), 3);
        assert_eq!(a.shared.puzzles()[0], late.seat.unwrap(), "a late clover can precede the current one in deal order");
        assert_eq!(a.scene().writer, Some(writer));
        assert_eq!(a.q, 1, "the board must not spin when a clover is added ahead of it");
        assert_eq!(a.shared.cards, cards);
        assert_eq!(a.shared.checks, checks);
        assert_eq!(a.shared.round, i.round);
        // Deliver all the same records backwards, twice, to a new replica.
        let ops = a.shared.all_ops();
        let mut replay = Shared::default();
        for op in ops.iter().rev().chain(ops.iter().rev()) { replay.apply(op); }
        assert_eq!(replay, a.shared);
    }
}

#[test]
fn a_late_clover_reopens_a_finished_round_and_is_included_in_the_recap() {
    let i = invite(3);
    let (mut a, mut late, mut other) = (open(&i, 1), open(&i, 2), open(&i, 0));
    submit(&mut a, "first");
    a.action(ACT_PROCEED);
    let writer = a.scene().writer.unwrap();
    a.shared.apply(&Op { round: i.round, body: Body::Check(writer, Check { mask: 15, attempts: 1, stamp: Stamp { clock: 50, peer: 1 }, ..Check::default() }) });
    a.action(ACT_NEXT);
    assert_eq!(a.shared.phase(), Phase::End);
    sync(&mut a, &mut late);
    assert_eq!(late.view_phase(), Phase::Writing);
    submit(&mut late, "after end");
    sync(&mut a, &mut late);
    assert_eq!(a.scene().writer, late.seat);
    assert!(a.shared.check(writer).done());
    assert_eq!(Json::parse(&a.review_json()).unwrap().arr().len(), 2);
    // A second arrival cannot replace the clover that just reopened play.
    sync(&mut a, &mut other);
    submit(&mut other, "second arrival");
    sync(&mut a, &mut other);
    assert_eq!(a.scene().writer, late.seat);
    assert_eq!(a.shared.puzzles().len(), 3);
}

#[test]
fn version_three_invites_and_saves_remain_readable() {
    let i = invite(3);
    let mut json = Json::parse(&i.json()).unwrap();
    json.set("protocol", Json::Num(3.0));
    let old = Invite::parse(&json.to_string(), &i.room).unwrap();
    let mut a = open(&old, 1);
    submit(&mut a, "saved");
    a.action(ACT_PROCEED);
    a.shared.next.0 = Selection::Index(0); // the old on-wire selection
    let mut b = open(&old, 1);
    assert!(b.restore(&a.snapshot(), 3));
    b.frame(100.0);
    assert_eq!(b.scene().writer, a.seat);
    assert_eq!(b.shared.next.0, Selection::Writer(a.seat.unwrap()));
    assert!(b.shared.is_ready(a.seat.unwrap()));
}

#[test]
fn two_to_twelve_people_can_write_apart_reload_and_merge_the_same_round() {
    for n in 2..=MAX_PLAYERS {
        let i = invite(n);
        let mut apps = Vec::new();
        let mut cards = BTreeSet::new();
        // Reverse arrival order, no links between any of the writers.
        for k in (0..n).rev() {
            let mut a = open(&i, k);
            assert_eq!(a.shared.round, i.round);
            assert_eq!(a.present().len(), 1, "reserving a seat does not invent presence");
            let w = seat_id(&i.names[k]);
            for card in a.shared.deal(w).words { assert!(cards.insert(card), "deals must not overlap"); }
            for d in 0..NSLOTS { a.set_clue(d as u8, &format!("player{k}-leaf{d}"), false); }
            a.action(ACT_READY);
            a.frame(100.0);
            assert_eq!(a.shared.phase(), Phase::Writing, "wait for all reserved players");
            // Return on a new device instance with the original link and save.
            let save = a.snapshot();
            let mut restored = App::new(10000 + k as u64);
            restored.load_invite(&i.room, &i.json()).unwrap();
            assert!(restored.restore(&save, PROTOCOL as u32));
            restored.join(&i.names[k]);
            assert!(restored.shared.is_ready(w));
            apps.push(restored);
        }
        // Full syncs arrive in different orders, with repeats. Auto-proceed
        // may race on several devices; its stamp must converge too.
        for lap in 0..4 {
            let snapshots: Vec<_> = apps.iter().map(App::snapshot).collect();
            for (k, a) in apps.iter_mut().enumerate() {
                for j in 0..n { a.on_packet(&snapshots[(k + j + lap) % n], j as u32 + 1); }
                a.frame(200.0 + lap as f64 * 100.0);
            }
        }
        let expected = apps[0].shared.digest();
        for a in &mut apps {
            assert_eq!(a.shared.digest(), expected);
            assert_eq!(a.shared.round, i.round);
            assert_eq!(a.shared.phase(), Phase::Guessing(0));
            assert_eq!(a.shared.puzzles().len(), n);
            for k in 0..n {
                let s = a.shared.effective(seat_id(&i.names[k])).unwrap();
                for d in 0..NSLOTS { assert_eq!(s.clues[d], format!("player{k}-leaf{d}")); }
            }
            a.load_invite(&i.room, &i.json()).unwrap();
            assert_eq!(a.shared.digest(), expected, "replaying the invite cannot undo clues or guessing");
        }
    }
}

#[test]
fn original_invites_and_old_saves_cannot_reset_a_later_round_or_new_game() {
    let i = invite(4);
    let initial = i.snapshot();
    let mut a = open(&i, 0);
    for action in [ACT_REDEAL, ACT_NEW_GAME] {
        a.action(action);
        let later = a.shared.round;
        let snapshot = a.snapshot();
        let mut b = open(&i, 1);
        assert!(b.restore(&snapshot, PROTOCOL as u32));
        b.load_invite(&i.room, &i.json()).unwrap();
        assert!(b.restore(&initial, PROTOCOL as u32));
        assert_eq!(b.shared.round, later);
    }
    // A separately created table has a different identity, even if names match.
    let other = Invite::create(r#"{"room":"Friday Night","name":"A","friends":"B\nC"}"#, Stamp { clock: 99, peer: 44 }, 42).unwrap();
    assert!(!a.restore(&other.snapshot(), PROTOCOL as u32));
}

#[test]
fn the_picker_uses_full_roster_names_and_only_confirms_live_seat_takeover() {
    let i = invite(4);
    let mut a = App::new(99);
    a.load_invite(&i.room, &i.json()).unwrap();
    a.peers.insert(123, (seat_id("Player 1"), 7));
    let j = Json::parse(&a.sheet_json(SHEET_JOIN)).unwrap();
    assert_eq!(j.get("heading").str(), Some("Who are you?"));
    assert_eq!(j.get("invited").bool(), Some(true));
    assert_eq!(j.get("rows").arr().len(), 4);
    for (k, row) in j.get("rows").arr().iter().enumerate() {
        assert_eq!(row.get("name").str(), Some(i.names[k].as_str()));
        assert_eq!(row.get("label").str(), Some(format!("I'm Player {k}").as_str()));
        assert_eq!(row.get("confirm").str().is_some(), k == 1);
    }
    a.join("Player 1");
    let j = Json::parse(&a.sheet_json(SHEET_JOIN)).unwrap();
    assert!(j.get("rows").arr()[1].get("confirm").str().is_none());
}

#[test]
fn bad_invites_are_rejected_before_changing_the_table() {
    let i = invite(3);
    let mut a = open(&i, 0);
    let before = a.shared.digest();
    assert!(a.load_invite("another-room", &i.json()).is_err());
    for (field, value) in [
        ("v", Json::Num(99.0)), ("protocol", Json::Num(0.0)), ("seed", Json::Num(-1.0)),
        ("peer", Json::Num(0.0)), ("clock", Json::str_of("18446744073709551615")),
        ("names", Json::Arr(vec![Json::str_of("Ana"), Json::str_of(" ana ")])),
        ("names", Json::Arr(vec![Json::str_of("Ana")])),
    ] {
        let mut j = Json::parse(&i.json()).unwrap();
        j.set(field, value);
        assert!(a.load_invite(&i.room, &j.to_string()).is_err(), "{field}");
        assert_eq!(a.shared.digest(), before);
    }
    for friends in ["", "player 0", "name that is much too long to fit", "A\nB\nC\nD\nE\nF\nG\nH\nI\nJ\nK\nL"] {
        let input = format!("{{\"room\":\"test\",\"name\":\"Player 0\",\"friends\":{}}}", json_str(friends));
        assert!(Json::parse(&a.create_invite(&input)).unwrap().get("error").str().is_some());
        assert_eq!(a.shared.digest(), before);
    }
}
