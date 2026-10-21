//! Race leaderboards offline. The original reads a friends leaderboard from Steam stats
//! (GGGameInfoRace.ReadRaceLeaderboard -> ReadOnlineStatsForFriends) and the race-ended popup
//! (GGUIViewPopupRaceEnded.SetBody) lists the player's best and the friends' times. Here the
//! player's best per leaderboard is kept in save/race_best.json and the popup lists it the way
//! the original does with no friends on the board: "1. <Steam name> - <best time>".

use super::*;
use std::collections::BTreeMap;

fn path() -> std::path::PathBuf {
    super::progress::save_dir().join("race_best.json")
}

/// Leaderboard id (ELeaderBoardID) -> best time in ms.
pub fn load() -> BTreeMap<String, i32> {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(b: &BTreeMap<String, i32>) {
    let p = path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(s) = serde_json::to_string_pretty(b) {
        let _ = std::fs::write(p, s);
    }
}

/// The signed-in Steam user's display name, from Steam's own local config (MostRecent user in
/// config/loginusers.vdf); "Goat" when there is none.
pub fn player_name() -> String {
    let vdf = upk::game_dir().join("..").join("..").join("..").join("config").join("loginusers.vdf");
    let text = std::fs::read_to_string(&vdf).unwrap_or_default();
    // blocks of "key" "value" lines per user; take the one marked MostRecent
    let mut best: Option<String> = None;
    let mut cur_name: Option<String> = None;
    for line in text.lines() {
        let parts: Vec<&str> = line.split('"').filter(|p| !p.trim().is_empty()).collect();
        match parts.as_slice() {
            [k, v] if k.eq_ignore_ascii_case("PersonaName") => {
                cur_name = Some(v.to_string());
                if best.is_none() {
                    best = cur_name.clone();
                }
            }
            [k, v] if k.eq_ignore_ascii_case("MostRecent") && *v == "1" => best = cur_name.clone(),
            _ => {}
        }
    }
    best.filter(|n| !n.is_empty()).unwrap_or_else(|| "Goat".into())
}

/// UnrealScript's float to string (6 decimals, trailing zeros trimmed to 2).
fn secs(ms: i32) -> String {
    format!("{:.2}", ms as f32 * 0.001)
}

pub fn register(vm: &mut Vm) {
    // OnlineStatsRead.GetIntStatValueForPlayer: the saved best for this read's leaderboard
    vm.overrides.insert("onlinestatsread.getintstatvalueforplayer".into(), |vm, c| {
        let Some(t) = c.this else { return Value::Bool(false) };
        let id = vm.get(t, "ViewId").int();
        let best = load().get(&id.to_string()).copied().unwrap_or(0);
        if c.args.len() > 2 {
            c.args[2] = Value::Int(best);
        }
        Value::Bool(best > 0)
    });
    // GGUIViewPopupRaceEnded.SetBody (one player): the time, then the board with the new best
    vm.overrides.insert("gguiviewpopupraceended.setbody".into(), |vm, c| {
        let Some(t) = c.this else { return Value::Int(0) };
        let body = c.str(0);
        let time = vm.get(t, "mRaceTimeMS").int();
        let board = vm.get(t, "mLeaderboardRead").obj().map(|r| vm.get(r, "ViewId").int());
        let mut out = format!("{body} {}<br><br>", secs(time));
        if let Some(id) = board {
            let mut all = load();
            let key = id.to_string();
            let prev = all.get(&key).copied().unwrap_or(0);
            let best = if prev == 0 { time } else if time > 0 { prev.min(time) } else { prev };
            if best != prev && best > 0 {
                all.insert(key, best);
                save(&all);
            }
            let best = if best == 0 { 666 } else { best };
            out.push_str(&format!("1. {} - {}<br>", player_name(), secs(best)));
        }
        if std::env::var_os("GOAT_TRACE").is_some() {
            println!("    race popup body: {out:?}");
        }
        // GGUIViewPopup.SetBody
        let Some(popup) = vm.class_by_name("GGUIViewPopup") else { return Value::Int(0) };
        let y = vm.sym("SetBody");
        if let Some(f) = vm.find_func(popup, y, None) {
            vm.invoke(Some(t), f, vec![Value::Str(out)], vec![true]);
        }
        Value::Int(0)
    });
}
