//! One link, one initial deal, including people who have not arrived yet.
//! This is a versioned bootstrap, not another sync protocol. Loading it
//! produces ordinary round/seat ops; subsequent edits use the existing merge.

use crate::{json::Json, model::*, pages::norm_room};
use std::collections::BTreeSet;

pub const INVITE_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct Invite {
    pub room: String,
    pub names: Vec<String>,
    pub round: Round,
}

fn names(raw: &[Json]) -> Result<Vec<String>, String> {
    if !(2..=MAX_PLAYERS).contains(&raw.len()) {
        return Err(format!("Add between 2 and {MAX_PLAYERS} players, including yourself."));
    }
    let mut ids = BTreeSet::new();
    raw.iter().map(|v| {
        let name = v.str().unwrap_or("").trim();
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err("Give every player a name.".into());
        }
        if clamp_text(name, 24) != name {
            return Err(format!("“{name}” is too long. Use a shorter name."));
        }
        if !ids.insert(seat_id(name)) {
            return Err("Use a different name for each player. Add an initial if two people share a name.".into());
        }
        Ok(name.to_string())
    }).collect()
}

impl Invite {
    pub fn create(input: &str, stamp: Stamp, seed: u32) -> Result<Self, String> {
        let v = Json::parse(input).ok_or("Couldn't read the player list.")?;
        let mut raw = vec![v.get("name").clone()];
        raw.extend(v.get("friends").str().unwrap_or("").lines().filter(|n| !n.trim().is_empty()).map(Json::str_of));
        let names = names(&raw)?;
        let base = norm_room(v.get("room").str().unwrap_or(""));
        let base = if base.is_empty() { "table" } else { &base };
        // An invitation always makes a new room, even if its friendly name
        // was used before. Never restore an unrelated table under this key.
        let room = format!("{}-{seed:08x}", base.chars().take(23).collect::<String>().trim_end_matches('-'));
        Ok(Self::new(room, names, stamp, seed))
    }

    fn new(room: String, names: Vec<String>, stamp: Stamp, seed: u32) -> Self {
        let seats: Vec<u32> = names.iter().map(|n| seat_id(n)).collect();
        let round = Round { n: 1, seed, stamp, table: stamp, dealer: seats[0], ..Round::default() }.with_players(&seats);
        Self { room, names, round }
    }

    pub fn json(&self) -> String {
        format!("{{\"v\":{INVITE_VERSION},\"protocol\":{PROTOCOL},\"room\":{},\"clock\":{},\"peer\":{},\"seed\":{},\"names\":[{}]}}",
            json_str(&self.room), json_str(&self.round.stamp.clock.to_string()), self.round.stamp.peer,
            self.round.seed, self.names.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(","))
    }

    pub fn parse(text: &str, room: &str) -> Result<Self, String> {
        if text.len() > 4096 { return Err("This invite is too long. Ask for the original link.".into()); }
        let v = Json::parse(text).ok_or("This invite is damaged. Ask for the original link.")?;
        if v.get("v").num() != Some(INVITE_VERSION as f64)
            || !v.get("protocol").num().is_some_and(|p| p.fract() == 0.0 && (MIN_PROTOCOL as f64..=PROTOCOL as f64).contains(&p)) {
            return Err("This invite needs a different version of Shamrock. Reload the app or ask for a new invite.".into());
        }
        if room.is_empty() || norm_room(room) != room || v.get("room").str() != Some(room) {
            return Err("This invite belongs to a different table. Open the complete original link.".into());
        }
        let number = |key| v.get(key).num().filter(|n| n.is_finite() && *n >= 0.0 && *n <= u32::MAX as f64 && n.fract() == 0.0).map(|n| n as u32);
        let seed = number("seed").ok_or("This invite's deal is damaged.")?;
        let peer = number("peer").filter(|p| *p != 0).ok_or("This invite's identity is damaged.")?;
        let clock = v.get("clock").str().and_then(|n| n.parse::<u64>().ok()).filter(|n| *n > 0 && *n < u64::MAX / 2).ok_or("This invite's identity is damaged.")?;
        Ok(Self::new(room.into(), names(v.get("names").arr())?, Stamp { clock, peer }, seed))
    }

    pub fn snapshot(&self) -> Vec<u8> {
        let mut packet = Vec::new();
        encode(&Msg::Op(Op { round: self.round, body: Body::None }), &mut packet);
        for name in &self.names {
            let seat = Seat { name: name.clone(), active: true, stamp: self.round.stamp };
            encode(&Msg::Op(Op { round: self.round, body: Body::Seat(seat_id(name), seat) }), &mut packet);
        }
        packet
    }
}
