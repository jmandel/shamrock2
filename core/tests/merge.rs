//! The merge rules of the shared table, checked directly on `Shared::apply`
//! (see "How each part merges" in docs/sync-model.html). Every change
//! carries its round, and every field merges by a rule that ignores
//! arrival order, so any set of changes, delivered in any order, any number
//! of times, gives one table. The network simulator checks the same end to
//! end; this checks the rule itself, for every kind of change, with
//! deliveries no network would happen to produce.

use cloverleaf::model::*;

/// xorshift64*: small, seeded, no dependencies.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn coin(&mut self) -> bool {
        self.below(2) == 0
    }
}

/// Random changes as several devices would make them: every stamp unique
/// (a device's clock and its id), spread over a few competing rounds,
/// including two rounds that tie on everything but their stamp.
fn changes(r: &mut Rng) -> Vec<Op> {
    let mut clock = 0u64;
    let mut stamp = |r: &mut Rng| {
        clock += 1 + r.below(3);
        Stamp { clock, peer: 1 + r.below(4) as u32 }
    };
    let mut rounds = Vec::new();
    for _ in 0..4 {
        let s = stamp(r);
        let players: Vec<u32> = (0..2 + r.below(3)).map(|p| 10 + p as u32).collect();
        rounds.push(Round { epoch: r.below(2) as u16, n: 1 + r.below(2) as u16, seed: r.next() as u32, stamp: s, ..Round::default() }.with_players(&players));
    }
    // Two rounds alike but for their stamp: the stamp breaks the tie.
    let twin = Round { stamp: stamp(r), ..rounds[0] };
    rounds.push(twin);
    let mut ops = Vec::new();
    for _ in 0..10 + r.below(40) {
        let round = rounds[r.below(rounds.len() as u64) as usize];
        let s = stamp(r);
        let w = 10 + r.below(3) as u32;
        let body = match r.below(11) {
            0 => Body::None,
            1 => Body::Seat(10 + r.below(5) as u32, Seat { name: format!("p{}", r.below(9)), active: r.coin(), stamp: s }),
            2 => Body::Move(Stamp { clock: r.below(3), peer: 1 }, Moved { room: format!("room-{}", r.below(9)), by: w, stamp: s }),
            3 => {
                let clue = |r: &mut Rng| if r.coin() { String::new() } else { format!("clue{}", r.below(5)) };
                Body::Sub(w, Sub { clues: [clue(r), clue(r), clue(r), clue(r)], decoys: r.below(MAX_DECOYS as u64 + 1) as u8, ready: r.coin(), stamp: s })
            }
            4 => Body::Start(s),
            5 => {
                let loc = match r.below(3) {
                    0 => Loc::Table { x: r.below(100) as f32 / 100.0, y: r.below(100) as f32 / 100.0 },
                    1 => Loc::Slot(r.below(NSLOTS as u64) as u8),
                    _ => Loc::Float { x: r.below(400) as f32 - 200.0, y: r.below(400) as f32 - 200.0 },
                };
                Body::Card(w, r.below(MAX_CARDS as u64) as u8, Card { loc, rot: r.below(4) as u8, stamp: s })
            }
            6 => {
                let tried = (0..r.below(4)).fold(0u128, |t, _| t | Check::bit(r.below(MAX_CARDS as u64) as usize, r.below(4) as usize, r.below(4) as u8));
                Body::Check(w, Check { attempts: 1 + r.below(3) as u8, mask: r.below(16) as u8, stamp: s, tried })
            }
            7 => Body::Next(r.below(3) as u16, s),
            8 => Body::Focus(if r.coin() { w } else { 0 }, s),
            9 => Body::SitOut(w),
            _ => Body::Seat(w, Seat { name: format!("p{w}"), active: true, stamp: s }),
        };
        ops.push(Op { round, body });
    }
    ops
}

fn shuffled(ops: &[Op], r: &mut Rng) -> Vec<Op> {
    let mut v = ops.to_vec();
    for i in (1..v.len()).rev() {
        v.swap(i, r.below(i as u64 + 1) as usize);
    }
    v
}

/// Apply every change in turn, checking each one reports a change exactly
/// when it made one (callers rely on that to know when to redraw and
/// resync).
fn apply_all(ops: &[Op]) -> Shared {
    let mut sh = Shared::default();
    for op in ops {
        let before = sh.clone();
        let changed = sh.apply(op);
        assert_eq!(changed, sh != before, "apply said {changed} for {op:?}");
    }
    sh
}

#[test]
fn any_order_any_repeats_gives_one_table() {
    let mut r = Rng(0x5eed_cafe_f00d);
    for run in 0..3000 {
        let ops = changes(&mut r);
        let one = apply_all(&ops);
        // Any order.
        for _ in 0..3 {
            assert_eq!(apply_all(&shuffled(&ops, &mut r)), one, "run {run}: order changed the table");
        }
        // Any repeats: every change delivered twice, the copies anywhere.
        let twice: Vec<Op> = ops.iter().chain(ops.iter()).cloned().collect();
        assert_eq!(apply_all(&shuffled(&twice, &mut r)), one, "run {run}: repeats changed the table");
        // Delivered again on top: nothing changes.
        let mut sh = one.clone();
        for op in &ops {
            assert!(!sh.apply(op), "run {run}: a change applied twice changed the table: {op:?}");
        }
        // In two halves that meet: each side's full state, merged both ways.
        let cut = r.below(ops.len() as u64 + 1) as usize;
        let (a, b) = (apply_all(&ops[..cut]), apply_all(&ops[cut..]));
        let mut ab = a.clone();
        for op in b.all_ops() {
            ab.apply(&op);
        }
        let mut ba = b.clone();
        for op in a.all_ops() {
            ba.apply(&op);
        }
        assert_eq!(ab, ba, "run {run}: two halves merged differently each way");
        assert_eq!(ab, one, "run {run}: two halves merged to something else");
    }
}

#[test]
fn the_highest_round_wins_and_brings_only_its_own_changes() {
    let mut r = Rng(0xfeed_beef);
    for _ in 0..500 {
        let ops = changes(&mut r);
        let sh = apply_all(&ops);
        let top = ops.iter().map(|o| o.round).max_by_key(|x| x.rank()).unwrap();
        assert_eq!(sh.round, top, "the table is on the highest-ranked round");
        // Exactly that round's changes, applied alone, give the same round state.
        let own: Vec<Op> = ops.iter().filter(|o| o.round.stamp == top.stamp).cloned().collect();
        let mut alone = apply_all(&own);
        alone.seats.clone_from(&sh.seats);
        alone.moves.clone_from(&sh.moves);
        assert_eq!(alone, sh, "a lower round's changes leaked into the current one");
        // Rejections only accumulate: every placement any check of this
        // round rejected is still known, whichever check won.
        for (w, c) in &sh.checks {
            let all = own.iter().fold(0u128, |t, o| match &o.body {
                Body::Check(x, k) if x == w => t | k.tried,
                _ => t,
            });
            assert_eq!(c.tried, all, "a check's rejections were lost when another check won");
        }
    }
}
