//! What the page shows from this device's own records: Past games, its
//! tables, what happened here, and a finished clover read back. The page
//! keeps these in the browser's storage and hands them over as stored
//! (JSON); everything here decides what's kept and what's said, and the page
//! only lays it out (and formats dates, which need the browser's locale).
//! Also table names: how a typed one becomes a link, and suggestions.

use crate::json::Json;
use crate::model::NSLOTS;

/// How many rounds Past games keeps, and how many lines "What happened".
pub const LOG_MAX: usize = 60;
pub const HAPPENED_MAX: usize = 10;

fn obj(kv: Vec<(&str, Json)>) -> Json {
    Json::Obj(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}
fn s(x: &str) -> Json {
    Json::str_of(x)
}

pub fn guesses(n: u32) -> String {
    format!("{n} guess{}", if n == 1 { "" } else { "es" })
}

/// A finished (or half-written) clover as read back in the recap and Past
/// games, from its board text (see `Shared::export_board`) and how the
/// guessing went: each clue on its leaf with the two words it linked, the
/// four complete answer cards, the decoys, a badge,
/// and whether it can be played again (all four clues written).
pub fn board_view(b: &Json) -> Json {
    let text = b.get("text").str().unwrap_or("");
    let solved = b.get("solved").bool().unwrap_or(false);
    let tries = b.get("guesses").num().unwrap_or(0.0) as u32;
    let mut lines = text.split('\n');
    let name = lines.next().unwrap_or("").to_string();
    let rest: Vec<&str> = lines.collect();
    let clues: Vec<String> = (0..NSLOTS).map(|d| rest.get(d).map_or("", |c| c.trim()).to_string()).collect();
    let cards: Vec<Vec<String>> = rest.iter().skip(NSLOTS).filter(|l| !l.trim().is_empty()).map(|l| l.split('|').map(|w| w.trim().to_string()).collect()).collect();
    let word = |spot: usize, edge: usize| cards.get(spot).and_then(|c| c.get(edge)).cloned().unwrap_or_default();
    // Leaf d links spot d and the next spot clockwise, both on side d.
    let leaves = (0..NSLOTS).map(|d| obj(vec![("clue", s(&clues[d])), ("words", s(&format!("{} + {}", word(d, d), word((d + 1) % NSLOTS, d))))])).collect();
    let answer = (0..NSLOTS).map(|sp| Json::Arr((0..4).map(|e| s(&word(sp, e))).collect())).collect();
    let decoys = cards.iter().skip(NSLOTS).map(|c| c.join(" · ")).collect::<Vec<_>>();
    let decoys = if decoys.is_empty() { Json::Null } else { s(&format!("Decoy{}: {}", if decoys.len() > 1 { "s" } else { "" }, decoys.join("; "))) };
    let (badge, first) = match (solved, tries) {
        (false, 0) => ("not guessed".to_string(), false),
        (false, _) => ("not solved".to_string(), false),
        (true, 1) => ("☘ first try".to_string(), true),
        (true, n) => (guesses(n), false),
    };
    obj(vec![
        ("title", s(&format!("{name}'s clover"))),
        ("badge", s(&badge)),
        ("first", Json::Bool(first)),
        ("solved", Json::Bool(solved)),
        ("leaves", Json::Arr(leaves)),
        ("cards", Json::Arr(answer)),
        ("decoys", decoys),
        ("playable", Json::Bool(clues.iter().all(|c| !c.is_empty()))),
        ("text", s(text)),
    ])
}

/// Past games with a round kept (or updated, by its key) from `entry` (an
/// EV_KEEP payload) at table `room`: newest first, at most `LOG_MAX`. An
/// update keeps the round's first date. Nothing changes for a round with no
/// clovers.
pub fn keep_into(log: &str, entry: &str, room: &str, now: f64) -> String {
    let mut log = Json::parse(log).filter(|l| matches!(l, Json::Arr(_))).unwrap_or(Json::Arr(Vec::new()));
    let Some(e) = Json::parse(entry) else { return log.to_string() };
    if e.get("boards").arr().is_empty() {
        return log.to_string();
    }
    let key = e.get("key").str().unwrap_or("").to_string();
    let Json::Arr(rows) = &mut log else { unreachable!() };
    let at = rows.iter().find(|r| r.get("id").str() == Some(&key)).and_then(|r| r.get("at").num()).unwrap_or(now);
    rows.retain(|r| r.get("id").str() != Some(&key));
    rows.insert(
        0,
        obj(vec![
            ("id", s(&key)),
            ("room", s(room)),
            ("round", e.get("n").clone()),
            ("at", Json::Num(at)),
            ("players", e.get("players").clone()),
            ("boards", e.get("boards").clone()),
            ("finished", e.get("finished").clone()),
        ]),
    );
    rows.truncate(LOG_MAX);
    log.to_string()
}

/// Past games as the page lists them: per round, a line about it (the page
/// puts the date in front), who played and how many clovers, each clover,
/// and the button to play the finished ones again (none if none can be).
pub fn past_games_view(log: &str) -> String {
    let log = Json::parse(log).unwrap_or(Json::Arr(Vec::new()));
    let rows: Vec<Json> = log
        .arr()
        .iter()
        .map(|e| {
            let boards: Vec<Json> = e.get("boards").arr().iter().map(board_view).collect();
            let playable: Vec<Json> = boards.iter().filter(|b| b.get("playable").bool() == Some(true)).map(|b| b.get("text").clone()).collect();
            let n = boards.len();
            let players: Vec<&str> = e.get("players").arr().iter().filter_map(Json::str).collect();
            let replay = match playable.len() {
                0 => Json::Null,
                k if k == n => s("Play these again"),
                _ => s("Play the finished clovers again"),
            };
            obj(vec![
                ("id", e.get("id").clone()),
                ("at", e.get("at").clone()),
                (
                    "line",
                    s(&format!(
                        "round {} at #{}{}",
                        e.get("round").num().unwrap_or(0.0),
                        e.get("room").str().unwrap_or("?"),
                        if e.get("finished").bool() == Some(false) { " · unfinished" } else { "" }
                    )),
                ),
                ("who", s(&format!("{} · {n} clover{}", players.join(", "), if n == 1 { "" } else { "s" }))),
                ("boards", Json::Arr(boards)),
                ("replay", replay),
                ("replayTexts", Json::Arr(playable)),
            ])
        })
        .collect();
    let count = rows.len();
    obj(vec![
        ("rows", Json::Arr(rows)),
        ("homeButton", if count > 0 { s(&format!("Past games ({count})")) } else { Json::Null }),
    ])
    .to_string()
}

/// "What happened" with one more line (a notice: its text and button).
pub fn happened_add(list: &str, notice: &str, now: f64) -> String {
    let mut list = Json::parse(list).filter(|l| matches!(l, Json::Arr(_))).unwrap_or(Json::Arr(Vec::new()));
    let n = Json::parse(notice).unwrap_or(Json::Null);
    if let Json::Arr(rows) = &mut list {
        rows.insert(0, obj(vec![("at", Json::Num(now)), ("text", n.get("text").clone()), ("action", n.get("action").clone())]));
        rows.truncate(HAPPENED_MAX);
    }
    list.to_string()
}

/// This device's tables (room → {name, at}, as kept, plus rooms that only
/// left a saved name), most recent first, with the name to offer by
/// default: the one used most recently.
pub fn tables_view(tables: &str) -> String {
    let t = Json::parse(tables).unwrap_or(Json::Null);
    let mut rows: Vec<(String, String, f64)> = match &t {
        Json::Obj(kv) => kv.iter().map(|(room, v)| (room.clone(), v.get("name").str().unwrap_or("").to_string(), v.get("at").num().unwrap_or(0.0))).collect(),
        _ => Vec::new(),
    };
    rows.sort_by(|a, b| b.2.total_cmp(&a.2));
    let name = rows.iter().find(|r| !r.1.is_empty()).map_or(Json::Null, |r| s(&r.1));
    let rows = rows
        .into_iter()
        .map(|(room, name, at)| {
            obj(vec![
                ("room", s(&room)),
                ("title", s(&format!("#{room}"))),
                ("by", s(&format!("as {}", if name.is_empty() { "?" } else { &name }))),
                ("at", Json::Num(at)),
            ])
        })
        .collect();
    obj(vec![("rows", Json::Arr(rows)), ("name", name)]).to_string()
}

/// A typed table name as a link-safe room ("Family Night" → family-night):
/// lowercase letters, digits and single dashes, at most 32 characters.
pub fn norm_room(typed: &str) -> String {
    let mut out = String::new();
    for c in typed.trim().to_lowercase().chars() {
        let c = if c.is_whitespace() || c == '_' || c == '-' { '-' } else { c };
        if c == '-' {
            if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        }
    }
    let mut out: String = out.trim_end_matches('-').chars().take(32).collect();
    while out.ends_with('-') {
        out.pop();
    }
    out
}

// Table names are the link (and the only key to the table): suggestions are
// easy to type and unlikely to collide: adjective-animal-place, e.g.
// merry-otter-cove, about 1.4M names.
const ADJ: &str = "amber brave bright calm clever cosy daring eager fancy gentle golden happy jolly kind lively lucky merry mellow noble plucky proud quick quiet rapid shiny silly sleepy snappy spry sunny swift tidy witty zesty bold breezy chirpy dapper fuzzy giddy hearty nimble perky rosy sturdy cheery crisp dandy dreamy fluffy frisky gleeful grand humble jaunty keen leafy loyal lucid misty modest nifty peppy polite prime quaint quirky ready regal rustic sage savvy serene sharp snug sparky spunky steady sweet tender trusty upbeat vivid warm wise wild zany able agile airy brisk bubbly chummy comfy dainty dusky elated fair feisty fond frank fresh gallant glad hardy jazzy jovial lofty lush mighty mild nippy plush pure";
const ANIMAL: &str = "otter badger falcon heron koala lemur marten panda puffin raven robin seal tiger walrus wombat beaver bison camel crane dingo eagle ferret gecko goose hare ibis jackal lark llama moose newt oriole owl parrot quail rabbit shrew sloth swan toad viper wren yak zebra alpaca bear bobcat canary cobra condor cougar coyote crow deer dove duck egret elk finch fox frog gibbon gopher hawk hedgehog hippo hyena iguana impala jaguar kiwi kudu lion lynx magpie mink mole monkey moth mule ocelot okapi oryx osprey ox pelican penguin pigeon pony possum puma rhino salmon shark sheep skunk sparrow squid stork tapir tern trout turtle vole weasel whale wolf";
const PLACE: &str = "cove bay brook cliff creek dale dell dune field fjord forest glade glen grove harbor heath hill hollow island lagoon lake marsh meadow mesa moor nook oasis orchard park pass peak pier plain pond prairie quay reef ridge river rock shore slope spring summit thicket trail tundra vale valley wharf wood acre arbor atoll basin beach bluff bridge burrow butte canyon cape castle cavern cellar chapel court crater delta den depot docks downs estate farm fen ferry fort garden gate gorge grotto hamlet haven inlet isle jetty knoll lane ledge lodge loft manor market mill mound oaks outpost palace pasture patio pavilion perch plaza porch pueblo ranch rapids retreat ridgeway roost square steppe tower village vista";

/// A suggested table name, from a random number the page draws.
pub fn suggest_room(random: u32) -> String {
    let mut x = random as u64 | 1;
    let mut pick = |list: &str| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let words: Vec<&str> = list.split(' ').collect();
        words[(x % words.len() as u64) as usize].to_string()
    };
    format!("{}-{}-{}", pick(ADJ), pick(ANIMAL), pick(PLACE))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = r#"{"text":"Ana\nsea\nsky\nfire\n\na|b|c|d\ne|f|g|h\ni|j|k|l\nm|n|o|p\nq|r|s|t","guesses":1,"solved":true}"#;

    #[test]
    fn a_clover_reads_back_with_its_links_and_badge() {
        let v = board_view(&Json::parse(BOARD).unwrap());
        assert_eq!(v.get("title").str(), Some("Ana's clover"));
        assert_eq!(v.get("badge").str(), Some("☘ first try"));
        assert_eq!(v.get("leaves").arr()[0].get("words").str(), Some("a + e"), "top leaf: top words of spots 0 and 1");
        assert_eq!(v.get("leaves").arr()[3].get("words").str(), Some("p + d"), "left leaf: left words of spots 3 and 0");
        assert_eq!(v.get("cards").arr()[0].to_string(), r#"["a","b","c","d"]"#, "all four words retain their answer orientation");
        assert_eq!(v.get("decoys").str(), Some("Decoy: q · r · s · t"));
        assert_eq!(v.get("playable").bool(), Some(false), "the left clue is empty");
    }

    #[test]
    fn past_games_keep_a_round_once_and_its_first_date() {
        let entry = format!(r#"{{"key":"1.1/2.2","n":3,"finished":false,"players":["Ana","Ben"],"boards":[{BOARD}]}}"#);
        let log = keep_into("", &entry, "merry-otter-cove", 100.0);
        let log = keep_into(&log, &entry.replace("false", "true"), "merry-otter-cove", 200.0);
        let rows = Json::parse(&log).unwrap();
        assert_eq!(rows.arr().len(), 1);
        assert_eq!((rows.arr()[0].get("at").num(), rows.arr()[0].get("finished").bool()), (Some(100.0), Some(true)));
        let view = Json::parse(&past_games_view(&log)).unwrap();
        let row = &view.get("rows").arr()[0];
        assert_eq!(row.get("line").str(), Some("round 3 at #merry-otter-cove"));
        assert_eq!(row.get("who").str(), Some("Ana, Ben · 1 clover"));
        assert_eq!(row.get("replay"), &Json::Null, "a half-written clover can't be played again");
        assert_eq!(view.get("homeButton").str(), Some("Past games (1)"));
        assert_eq!(keep_into(&log, r#"{"key":"x","boards":[]}"#, "r", 1.0), log, "nothing kept for a round with no clovers");
    }

    #[test]
    fn table_names_become_links_and_suggestions_vary() {
        assert_eq!(norm_room("  Family Night__2! "), "family-night-2");
        assert_eq!(norm_room("--a  -  b--"), "a-b");
        assert_eq!(norm_room(&"x".repeat(40)).len(), 32);
        assert_ne!(suggest_room(1), suggest_room(2));
        assert_eq!(suggest_room(7).split('-').count(), 3);
        let t = tables_view(r#"{"old":{"name":"Ann","at":5},"new":{"name":"","at":9}}"#);
        let t = Json::parse(&t).unwrap();
        assert_eq!(t.get("rows").arr()[0].get("title").str(), Some("#new"));
        assert_eq!(t.get("name").str(), Some("Ann"), "the most recent name used");
        let h = happened_add("[]", r#"{"text":"Ben dealt round 2.","action":null}"#, 1.0);
        assert_eq!(Json::parse(&h).unwrap().arr()[0].get("text").str(), Some("Ben dealt round 2."));
    }
}
