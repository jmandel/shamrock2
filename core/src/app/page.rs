//! What the page says about the table: the status strip (its words,
//! shorter versions for narrow screens, and which buttons show), the menu,
//! every sheet's content (seats to continue as, players, clovers to pick or
//! share, the recap), and the sentences for notices. The shell only lays
//! these out; every decision is made here, where native tests and the
//! network simulator can check it. (What the page says from this device's
//! own records is in `pages`.)

use super::*;
use crate::json::Json;
use crate::pages::board_view;

/// Sheets the shell asks for (`App::sheet_json`).
pub const SHEET_JOIN: u32 = 1;
pub const SHEET_PLAYERS: u32 = 2;
pub const SHEET_CLOVERS: u32 = 3;
pub const SHEET_SHARE: u32 = 4;
pub const SHEET_RECAP: u32 = 5;

/// `App::action` codes. The strip names them (`"deal"`, `"ready"`, …) for
/// the shell, which also has two of its own: `"join"` and `"ownTable"`.
pub const ACT_DEAL: u32 = 1;
pub const ACT_READY: u32 = 2;
pub const ACT_CHECK: u32 = 3;
pub const ACT_NEXT: u32 = 4;
pub const ACT_PROCEED: u32 = 5;
pub const ACT_REDEAL: u32 = 6;
pub const ACT_NEW_GAME: u32 = 7;
pub const ACT_WRITE: u32 = 8;
pub const ACT_JOIN_GUESSING: u32 = 9;

/// Marks around text the shell shows in bold (a name).
pub const BOLD_ON: char = '\u{1}';
pub const BOLD_OFF: char = '\u{2}';

/// A strip button: its label, whether it can be pressed, and what it does.
#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    pub label: String,
    pub enabled: bool,
    pub act: &'static str,
}

/// The status strip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Strip {
    /// The status in plain words, longest first: the shell shows the first
    /// that fits beside the buttons.
    pub hint: Vec<String>,
    /// Not connected: a pulsing dot before the words.
    pub pulse: bool,
    pub go: Option<Button>,
    pub alt: Option<Button>,
    /// The decoy stepper shows on the board.
    pub decoys: bool,
    /// Tapping the status opens `"clovers"` (pick which one) or `"recap"`.
    pub tap: Option<&'static str>,
    pub invite: bool,
    /// The how-to tip for now (`"write"` or `"guess"`); shown once per device.
    pub tip: Option<&'static str>,
    /// The submission progress bar shows.
    pub segs: bool,
    /// The decoy stepper's label.
    pub decoy_label: String,
    /// Where the main button sits instead of the strip, when the board is big
    /// enough that the strip's corner is a long way from the cards: centre
    /// and width (CSS px) of the empty corner below and right of the clover.
    pub go_at: Option<(f32, f32, f32)>,
}

/// The empty corner between the clover's right and bottom leaves (board
/// units): it starts past the card spots and ends at the board's edge.
const CORNER: (f32, f32) = (SLOT_OFF + HALF, FIT_EXTENT);
/// Narrowest corner (CSS px) the button moves into; phones keep it in the strip.
const CORNER_MIN: f32 = 130.0;

const ORDINAL: [&str; 10] = ["first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth", "tenth"];

fn bold(s: &str) -> String {
    format!("{BOLD_ON}{s}{BOLD_OFF}")
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// "a", "a and b", "a, b and c".
pub fn and_list(xs: &[String]) -> String {
    match xs {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn button(label: &str, enabled: bool, act: &'static str) -> Option<Button> {
    Some(Button { label: label.into(), enabled, act })
}

impl App {
    /// How the strip calls a player: their first name, or the whole name
    /// when another player shares that first name; at most 12 characters,
    /// so a long name can't push the buttons off.
    pub fn short_name(&self, seat: u32) -> String {
        let first = |n: &str| n.split_whitespace().next().unwrap_or("").to_string();
        let name = self.name_of(seat);
        let f = first(&name);
        let clash = self.shared.players().iter().any(|&p| p != seat && first(&self.name_of(p)) == f);
        let t = if clash { name } else { f };
        if t.chars().count() > 12 {
            format!("{}…", t.chars().take(11).collect::<String>())
        } else {
            t
        }
    }

    pub fn strip(&self) -> Strip {
        let s = &self.shared;
        let phase = self.view_phase();
        let n = s.puzzles().len();
        let seated = self.seat.is_some_and(|me| s.players().contains(&me));
        let (k, writer) = match phase {
            Phase::Guessing(k) => (k, s.puzzles().get(k).copied()),
            _ => (0, None),
        };
        let done = writer.is_some_and(|w| s.check(w).done());
        let is_writer = writer.is_some() && writer == self.seat;
        let whose = writer.map(|w| format!("{} clover", bold(&format!("{}'s", self.short_name(w))))).unwrap_or_default();
        let clover = vec![
            format!("{whose}, {} of {n}", ORDINAL.get(k).map_or(format!("number {}", k + 1), |o| o.to_string())),
            format!("{whose}, {} of {n}", k + 1),
            whose.clone(),
        ];
        let solved = writer.map(|w| format!("Solved {} clover!", bold(&format!("{}'s", self.short_name(w))))).unwrap_or_default();
        let dealable = !self.dealable().is_empty();
        let mut st = Strip { segs: phase == Phase::Writing, decoy_label: plural(self.my_sub().decoys as usize, "decoy"), ..Strip::default() };

        if self.solo {
            match phase {
                Phase::Guessing(_) if done => {
                    st.hint = vec![solved];
                    st.go = button(if k + 1 < n { "Next clover" } else { "Done" }, true, "next");
                }
                Phase::Guessing(_) => {
                    st.hint = clover;
                    st.go = button("Guess", self.can_check(), "check");
                }
                _ => {
                    st.hint = vec![if Custom::all().is_empty() { "No clovers in this link" } else { "All done" }.into()];
                    st.go = button("Start your own table", true, "ownTable");
                }
            }
        } else if !seated {
            st.hint = vec![match phase {
                _ if s.players().is_empty() => "No one's playing yet".into(),
                Phase::Guessing(_) => format!("Watching {whose}"),
                _ => "Watching".into(),
            }];
            st.go = button("Join", true, "join");
        } else {
            match phase {
                Phase::Lobby => {
                    let customs = Custom::all();
                    if self.custom_room && s.round.n == 0 && !customs.is_empty() {
                        let names: Vec<String> = customs.iter().map(|c| bold(&c.name)).collect();
                        st.hint = vec![format!("Opens with {}", and_list(&names))];
                        st.go = button("Start guessing", dealable, "deal");
                    } else {
                        let here = self.dealable().len();
                        st.hint = vec![if here < 2 { "Just you here".into() } else { format!("{here} players here") }];
                        st.go = button("Start", dealable, "deal");
                    }
                }
                Phase::Writing => {
                    let in_round = self.seat.is_some_and(|me| s.round.players().contains(&me));
                    let sub = self.my_sub();
                    let filled = sub.clues.iter().filter(|c| !c.is_empty()).count();
                    let round = s.round_players();
                    let left: Vec<String> = round.iter().filter(|p| !s.is_ready(**p)).map(|&p| bold(&self.short_name(p))).collect();
                    let ready = self.ready_count();
                    let who = if left.is_empty() { "the others".into() } else { and_list(&left) };
                    let mut waiting = vec![format!("Waiting for {who}")];
                    if left.len() > 1 {
                        waiting.push(format!("Waiting for {}", plural(left.len(), "player")));
                        waiting.push(format!("Waiting for {}", left.len()));
                    }
                    if !in_round {
                        st.hint = vec!["You're in the next round".into()];
                    } else if !sub.ready {
                        st.tip = Some("write");
                        st.hint = vec![if filled < 4 { format!("{filled} of 4 clues written") } else { format!("Clues done · {}", plural(sub.decoys as usize, "decoy")) }];
                        st.go = button("Submit", self.can_ready(), "ready");
                    } else {
                        st.hint = [vec![format!("Submitted · waiting for {who}")], waiting.clone()].concat();
                    }
                    st.decoys = in_round && filled >= 4;
                    if s.start.is_some() && self.can_write() {
                        st.hint = vec![format!("Add your clover · {filled} of 4 clues written"), format!("Your clover · {filled} of 4 clues")];
                        st.go = button("Add my clover", self.can_ready(), "ready");
                        st.alt = button("Guess with group", true, "joinGuessing");
                    } else if (sub.ready || !in_round) && ready > 0 && ready < round.len() {
                        // The status beside it names who is missing, so the button needn't.
                        st.alt = button("Start anyway", true, "proceed");
                        st.hint = if sub.ready { waiting } else { [vec![format!("You're in next round · waiting for {who}")], waiting].concat() };
                    }
                    // Everyone dealt in has gone and nobody submitted: nothing
                    // to start with, so offer a fresh deal for whoever is here.
                    let present = self.present();
                    if !in_round && ready == 0 && round.iter().all(|p| !present.contains(p)) && dealable {
                        st.hint = vec!["The writers have left".into()];
                        st.go = button("Deal again", true, "redeal");
                    }
                }
                Phase::Guessing(_) if done => {
                    st.hint = vec![solved];
                    st.go = button(if s.next_unsolved(k) < n { "Next clover" } else { "Finish round" }, true, "next");
                }
                Phase::Guessing(_) if is_writer => st.hint = vec!["They're guessing your clover".into()],
                Phase::Guessing(_) => {
                    st.hint = clover;
                    st.go = button("Guess", self.can_check(), "check");
                }
                Phase::End => {
                    st.hint = vec![format!("Round {} is over", s.round.n)];
                    st.go = button("Next round", dealable, "deal");
                }
            }
        }
        if s.start.is_some() && phase != Phase::Writing && self.can_write() {
            st.alt = button("Write my clover", true, "write");
        }
        if matches!(phase, Phase::Guessing(_)) && !(is_writer && !done) && (self.solo || seated) {
            st.tip = Some("guess");
        }
        // Not connected to anyone: say so rather than look normal while broken.
        if !self.solo && self.links == 0 && (self.connection_failed || !(self.invitation.is_some() && phase == Phase::Writing && seated)) {
            st.pulse = !self.connection_failed;
            let others = s.players().iter().any(|&p| Some(p) != self.seat);
            st.hint = if self.connection_failed {
                vec!["Couldn't connect to the table · try another network".into(), "Couldn't connect · try another network".into(), "Couldn't connect".into()]
            } else {
                vec![if others { "Reconnecting to the table…" } else { "Waiting for others" }.into()]
            };
        }
        // While writing from an invite, being alone is expected. Keep the
        // useful writing/waiting status, and keep the chosen identity visible.
        if self.invitation.is_some() && phase == Phase::Writing && seated {
            let name = self.short_name(self.seat.unwrap());
            st.hint = st.hint.iter().map(|h| format!("{name} · {h}")).collect();
        }
        st.tap = match phase {
            Phase::Guessing(_) if n > 1 => Some("clovers"),
            Phase::End if n > 0 => Some("recap"),
            _ => None,
        };
        st.invite = !self.solo && (phase == Phase::Lobby || self.links == 0 || (self.invitation.is_some() && phase == Phase::Writing && s.start.is_none()));
        st.tip = st.tip.filter(|_| seated || self.solo);
        // Guessing on a wide screen: Guess (and Next clover after it) sits by
        // the clover, a short reach from the cards.
        if st.go.as_ref().is_some_and(|b| matches!(b.act, "check" | "next")) {
            let l = self.layout();
            if l.tray.is_some_and(|t| !t.4) {
                let (near, far) = CORNER;
                let (mid, w) = ((near + far) / 2.0 * l.k, (far - near) * l.k);
                if w >= CORNER_MIN {
                    st.go_at = Some((l.bc.0 + mid, l.bc.1 + mid, w));
                }
            }
        }
        st
    }

    pub(super) fn strip_json(&self) -> String {
        let st = self.strip();
        let btn = |b: &Option<Button>| {
            b.as_ref().map_or("null".into(), |b| format!("{{\"label\":{},\"enabled\":{},\"act\":\"{}\"}}", json_str(&b.label), b.enabled, b.act))
        };
        let opt = |o: Option<&str>| o.map_or("null".into(), |x| format!("\"{x}\""));
        let hint: Vec<String> = st.hint.iter().map(|h| json_str(h)).collect();
        format!(
            "{{\"hint\":[{}],\"pulse\":{},\"go\":{},\"alt\":{},\"decoys\":{},\"tap\":{},\"invite\":{},\"tip\":{},\"segs\":{},\"decoyLabel\":{},\"goAt\":{}}}",
            hint.join(","),
            st.pulse,
            btn(&st.go),
            btn(&st.alt),
            st.decoys,
            opt(st.tap),
            st.invite,
            opt(st.tip),
            st.segs,
            json_str(&st.decoy_label),
            st.go_at.map_or("null".into(), |(x, y, w)| format!("[{x:.0},{y:.0},{w:.0}]"))
        )
    }

    /// Menu items that only make sense some of the time, and the recap's
    /// heading and buttons.
    pub(super) fn menu_json(&self) -> String {
        let s = &self.shared;
        let seated = self.seat.is_some_and(|me| s.players().contains(&me));
        let unsubmit = s.phase() == Phase::Writing && seated && self.my_sub().ready;
        format!("{{\"unsubmit\":{unsubmit},\"share\":{},\"watch\":{seated}}}", !s.puzzles().is_empty())
    }

    /// A sheet's content (see the `SHEET_` codes), as JSON for the shell to
    /// lay out.
    pub fn sheet_json(&self, kind: u32) -> String {
        let s = &self.shared;
        let me = self.seat;
        let seated = me.is_some_and(|m| s.players().contains(&m));
        let present = self.present();
        let color = |p: u32| s_(&format!("#{:06x}", self.color_of(p) >> 8));
        let v = match kind {
            SHEET_JOIN if self.invitation.is_some() => {
                let invite = self.invitation.as_ref().unwrap();
                let rows = invite.names.iter().map(|name| {
                    let p = seat_id(name);
                    let here = present.contains(&p) && Some(p) != me;
                    let note = if Some(p) == me { "You on this device" } else if here { "Already here · switch devices" }
                        else if s.is_ready(p) { "Clues submitted" } else { "Open your clover" };
                    obj(vec![
                        ("name", s_(name)), ("label", s_(&format!("I'm {name}"))), ("note", s_(note)),
                        ("confirm", if here { s_(&format!("{name} is already playing on another device. Only continue if you are {name}.")) } else { Json::Null }),
                        ("confirmLabel", s_(&format!("Yes, I'm {name}"))),
                    ])
                }).collect();
                obj(vec![
                    ("invited", Json::Bool(true)), ("heading", s_("Who are you?")),
                    ("intro", s_(&format!("Choose your name from the {} players below. Everyone has a different clover.", invite.names.len()))),
                    ("rows", Json::Arr(rows)),
                ])
            }
            // Joining: the players you could continue as (taking over a seat).
            SHEET_JOIN => {
                let rows: Vec<Json> = s
                    .players()
                    .into_iter()
                    .filter(|&p| Some(p) != me)
                    .map(|p| {
                        let away = !present.contains(&p);
                        obj(vec![("name", s_(&self.name_of(p))), ("color", color(p)), ("away", Json::Bool(away)), ("note", s_(if away { "away" } else { "" }))])
                    })
                    .collect();
                obj(vec![("heading", if rows.is_empty() { Json::Null } else { s_("Or continue as:") }), ("rows", Json::Arr(rows))])
            }
            // The menu's players: what each is up to, and who can be removed.
            SHEET_PLAYERS => {
                let phase = s.phase();
                let writer = match phase {
                    Phase::Guessing(k) => s.puzzles().get(k).copied(),
                    _ => None,
                };
                let rows: Vec<Json> = s
                    .players()
                    .into_iter()
                    .map(|p| {
                        let away = !present.contains(&p);
                        let doing = match phase {
                            _ if away => "away",
                            Phase::Writing if !s.round.players().contains(&p) => "in the next round",
                            Phase::Writing if s.is_ready(p) => "submitted",
                            Phase::Writing => "writing",
                            _ if s.round.custom == 0 && s.round.players().contains(&p) && !s.is_ready(p) => "still writing",
                            Phase::Guessing(_) if writer == Some(p) => "their clover is up",
                            _ => "here",
                        };
                        obj(vec![
                            ("id", Json::Num(p as f64)),
                            ("name", s_(&self.name_of(p))),
                            ("you", s_(if Some(p) == me { " (you)" } else { "" })),
                            ("doing", s_(doing)),
                            ("removable", Json::Bool(away && Some(p) != me)),
                        ])
                    })
                    .collect();
                let w = self.watchers();
                obj(vec![("rows", Json::Arr(rows)), ("watching", if w > 0 { s_(&format!("{w} watching")) } else { Json::Null })])
            }
            // Which clover everyone guesses: any, in any order.
            SHEET_CLOVERS => {
                let k = match s.phase() {
                    Phase::Guessing(k) => Some(k),
                    _ => None,
                };
                let rows: Vec<Json> = s
                    .puzzles()
                    .iter()
                    .enumerate()
                    .map(|(i, &w)| {
                        let note = if Some(i) == k { "now" } else if s.check(w).done() { "solved ✓" } else { "" };
                        obj(vec![("label", s_(&format!("{}. {} clover", i + 1, bold(&format!("{}'s", self.name_of(w)))))), ("note", s_(note)), ("now", Json::Bool(Some(i) == k))])
                    })
                    .collect();
                obj(vec![("rows", Json::Arr(rows))])
            }
            // Clovers to share: this round's.
            SHEET_SHARE => {
                let rows: Vec<Json> = s.puzzles().iter().map(|&w| s_(&format!("{}'s clover", self.name_of(w)))).collect();
                obj(vec![("rows", Json::Arr(rows))])
            }
            // The end of a round: totals, then every clover's answer.
            _ => {
                let boards: Vec<Json> = Json::parse(&self.review_json()).map(|b| b.arr().iter().map(board_view).collect()).unwrap_or_default();
                let n = boards.len();
                let solved = s.puzzles().iter().filter(|&&w| s.check(w).done()).count();
                let tries: u32 = s.puzzles().iter().map(|&w| s.check(w).attempts as u32).sum();
                let first = boards.iter().filter(|b| b.get("first").bool() == Some(true)).count();
                let stat = |v: String, k: &str| Json::Arr(vec![s_(&v), s_(k)]);
                obj(vec![
                    ("head", s_(&if self.solo { "All done ☘".to_string() } else { format!("Round {} done ☘", s.round.n) })),
                    (
                        "stats",
                        Json::Arr(vec![
                            stat(format!("{solved}/{n}"), "solved"),
                            stat(tries.to_string(), if tries == 1 { "guess" } else { "guesses" }),
                            stat(first.to_string(), "first try"),
                        ]),
                    ),
                    ("boards", Json::Arr(boards)),
                    ("share", Json::Bool(!self.solo)),
                    ("next", Json::Bool(!self.solo && seated)),
                    ("nextEnabled", Json::Bool(!self.dealable().is_empty())),
                ])
            }
        };
        v.to_string()
    }

    /// The sentence for a change of round someone else made (see
    /// `finish_change`), and the button offered with it, if any.
    pub(super) fn notice_text(&self, kind: &str, others: &[String], kept_unfinished: bool) -> (String, Option<(&'static str, &'static str)>) {
        let new = self.shared.round;
        let who = if new.dealer != 0 { self.name_of(new.dealer) } else { "Someone".into() };
        let dealt_in = self.seat.is_some_and(|me| new.players().contains(&me));
        let later = if dealt_in { "" } else { " You're in next round." };
        let n = new.n;
        let mut text = match kind {
            "next" => format!("{who} dealt round {n}.{later}"),
            "missed" => format!("{who} dealt round {n} while you were away.{later}"),
            "crossed" => format!("{who} dealt at the same moment as you; continuing {who}'s.{later}"),
            "newGame" => format!("{who} started a new game."),
            _ => format!(
                "{} {} also been playing at this table.{later}",
                if others.is_empty() { "Others".into() } else { and_list(others) },
                if others.len() == 1 { "has" } else { "have" }
            ),
        };
        if kept_unfinished {
            text.push_str(" Your round is in Past games.");
        }
        let action = (kind == "otherTable").then_some(("move", "Move us to a new table"));
        (text, action)
    }

    /// Saves never expire. Merge them like any other sync, so older saves
    /// cannot undo newer moves. A group invite also binds the save to its
    /// original table identity (including later rounds and New games).
    pub fn restore(&mut self, bytes: &[u8], version: u32) -> bool {
        if !(MIN_PROTOCOL as u32..=PROTOCOL as u32).contains(&version) {
            return false;
        }
        if let Some(i) = &self.invitation {
            if !decode(bytes).iter().any(|m| matches!(m, Msg::Op(op) if op.round.table == i.round.table)) {
                return false;
            }
        }
        self.on_packet(bytes, 0);
        true
    }
}

fn obj(kv: Vec<(&str, Json)>) -> Json {
    Json::Obj(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}
fn s_(x: &str) -> Json {
    Json::str_of(x)
}
