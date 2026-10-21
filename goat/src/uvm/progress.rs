//! Game progress through the original scripts: GGProgression (quests, secrets, stats,
//! collectibles) on the local player, saved as JSON instead of the Steam cloud file;
//! GGGameInfo.InitGameComponents -> GGQuestManager (quest pool for the map, progress, HUD quest);
//! GGPlayerControllerGame.mAchievementHandler registered for game events like GGGameInfo.Login
//! does, with UnlockAchievement recorded locally (no Steam).
//!
//! Files: save/progression.json (every plain GGProgression property), save/achievements.json
//! (unlocked achievement names with the time).

use super::bridge::{Host, HostEvent};
use super::value::{StructVal, Value};
use super::{ObjId, Ty, Vm};
use std::path::PathBuf;

pub fn save_dir() -> PathBuf {
    if let Ok(d) = std::env::var("GOAT_SAVE_DIR") {
        return PathBuf::from(d);
    }
    crate::mods::mods_dir().parent().map(|p| p.join("save")).unwrap_or_else(|| PathBuf::from("save"))
}

fn to_json(vm: &Vm, v: &Value) -> Option<serde_json::Value> {
    use serde_json::Value as J;
    Some(match v {
        Value::Byte(b) => J::from(*b),
        Value::Int(i) => J::from(*i),
        Value::Bool(b) => J::from(*b),
        Value::Float(f) => J::from(*f as f64),
        Value::Name(n) => serde_json::json!({ "name": vm.name_of(*n) }),
        Value::Str(s) => J::from(s.clone()),
        Value::Array(a) => J::Array(a.iter().filter_map(|x| to_json(vm, x)).collect()),
        Value::Struct(s) => {
            let mut m = serde_json::Map::new();
            m.insert("__struct".into(), J::from(vm.name_of(s.name).to_string()));
            for (k, x) in &s.fields {
                if let Some(j) = to_json(vm, x) {
                    m.insert(vm.name_of(*k).to_string(), j);
                }
            }
            J::Object(m)
        }
        Value::Obj(_) | Value::Delegate(..) => return None,
    })
}

/// JSON back into a value shaped like `template` (the property's current/default value).
fn from_json(vm: &mut Vm, j: &serde_json::Value, template: &Value, elem_ty: Option<&Ty>) -> Value {
    use serde_json::Value as J;
    match (template, j) {
        (Value::Byte(_), J::Number(n)) => Value::Byte(n.as_u64().unwrap_or(0) as u8),
        (Value::Int(_), J::Number(n)) => Value::Int(n.as_i64().unwrap_or(0) as i32),
        (Value::Float(_), J::Number(n)) => Value::Float(n.as_f64().unwrap_or(0.0) as f32),
        (Value::Bool(_), J::Bool(b)) => Value::Bool(*b),
        (Value::Str(_), J::String(s)) => Value::Str(s.clone()),
        (Value::Name(_), J::Object(o)) => Value::Name(vm.sym(o.get("name").and_then(|x| x.as_str()).unwrap_or("None"))),
        (Value::Array(_), J::Array(items)) => {
            let zero = elem_ty.map(|t| vm.zero(t, 1)).unwrap_or(Value::Int(0));
            let inner = match elem_ty {
                Some(Ty::Array(t)) => Some((**t).clone()),
                _ => None,
            };
            Value::Array(items.iter().map(|x| from_json(vm, x, &zero, inner.as_ref())).collect())
        }
        (Value::Struct(s), J::Object(o)) => {
            let mut out: StructVal = (**s).clone();
            let names: Vec<(super::Sym, String)> = out.fields.iter().map(|(k, _)| (*k, vm.name_of(*k).to_string())).collect();
            for (k, name) in names {
                if let Some(x) = o.get(&name) {
                    let cur = out.fields.iter().find(|(f, _)| *f == k).map(|(_, v)| v.clone()).unwrap_or(Value::Int(0));
                    let v = from_json(vm, x, &cur, None);
                    *out.field_mut(k) = v;
                }
            }
            Value::Struct(Box::new(out))
        }
        _ => template.clone(),
    }
}

/// The local player's GGProgression.
fn progression(vm: &mut Vm) -> Option<ObjId> {
    let engine = *vm.globals.get("engine")?;
    let lp = match vm.get(engine, "GamePlayers") {
        Value::Array(a) => a.first()?.obj()?,
        _ => return None,
    };
    vm.get(lp, "mSaveProgression").obj()
}

/// Write GGProgression to save/progression.json (GGEngine.SaveProgression).
pub fn save_progression(vm: &mut Vm) -> bool {
    let Some(p) = progression(vm) else { return false };
    let mut m = serde_json::Map::new();
    let mut keys: Vec<_> = vm.obj(p).props.keys().copied().collect();
    keys.sort_by_key(|k| vm.name_of(*k).to_string());
    for k in keys {
        if let Some(j) = to_json(vm, &vm.obj(p).props[&k]) {
            m.insert(vm.name_of(k).to_string(), j);
        }
    }
    let dir = save_dir();
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("progression.json"), serde_json::to_string_pretty(&serde_json::Value::Object(m)).unwrap_or_default()).is_ok()
}

fn load_progression(vm: &mut Vm, p: ObjId) {
    let Ok(text) = std::fs::read_to_string(save_dir().join("progression.json")) else { return };
    let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(&text) else { return };
    let class = vm.obj(p).class;
    for (name, j) in m {
        let k = vm.sym(&name);
        let ty = vm.find_prop(class, k).map(|d| d.ty.clone());
        let cur = vm.get(p, &name);
        let elem = match &ty {
            Some(Ty::Array(t)) => Some((**t).clone()),
            _ => None,
        };
        let v = from_json(vm, &j, &cur, elem.as_ref());
        vm.set(p, &name, v);
    }
}

/// Achievements unlocked here (save/achievements.json: name -> unix time).
fn local_achievements() -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(save_dir().join("achievements.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|j| j.as_object().cloned())
        .unwrap_or_default()
}

/// Unlocked achievements: this game's and the player's Steam ones (read-only, from Steam's
/// local stats cache) - mutator unlocks follow the player's real account like the original.
pub fn achievements() -> serde_json::Map<String, serde_json::Value> {
    let mut all = local_achievements();
    if std::env::var_os("GOAT_NO_STEAM_ACHIEVEMENTS").is_none() && std::env::var_os("GOAT_SAVE_DIR").is_none() {
        for (name, time) in steam_achievements() {
            all.entry(name).or_insert(serde_json::Value::from(time));
        }
    }
    all
}

/// Steam binary KeyValues (appcache/stats/*.bin): 0 = subtree, 1 = string, 2 = int32, 3 = float,
/// 7 = uint64, 8 = end.
#[derive(Debug, Clone)]
enum Kv {
    Tree(Vec<(String, Kv)>),
    Str(String),
    Int(i64),
}

impl Kv {
    fn get(&self, k: &str) -> Option<&Kv> {
        match self {
            Kv::Tree(v) => v.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, x)| x),
            _ => None,
        }
    }
    fn entries(&self) -> &[(String, Kv)] {
        match self {
            Kv::Tree(v) => v,
            _ => &[],
        }
    }
}

fn parse_kv(b: &[u8], i: &mut usize, depth: u32) -> Option<Kv> {
    let mut out = vec![];
    while *i < b.len() {
        let t = b[*i];
        *i += 1;
        if t == 8 {
            return Some(Kv::Tree(out));
        }
        let e = *i + b[*i..].iter().position(|&c| c == 0)?;
        let k = String::from_utf8_lossy(&b[*i..e]).into_owned();
        *i = e + 1;
        let v = match t {
            0 if depth < 16 => parse_kv(b, i, depth + 1)?,
            1 => {
                let e = *i + b[*i..].iter().position(|&c| c == 0)?;
                let s = String::from_utf8_lossy(&b[*i..e]).into_owned();
                *i = e + 1;
                Kv::Str(s)
            }
            2 | 3 => {
                let v = i32::from_le_bytes(b.get(*i..*i + 4)?.try_into().ok()?) as i64;
                *i += 4;
                Kv::Int(v)
            }
            7 => {
                let v = u64::from_le_bytes(b.get(*i..*i + 8)?.try_into().ok()?) as i64;
                *i += 8;
                Kv::Int(v)
            }
            _ => return None,
        };
        out.push((k, v));
    }
    Some(Kv::Tree(out))
}

/// Achievement API names unlocked on the player's Steam account (with their unlock time).
pub fn steam_achievements() -> Vec<(String, i64)> {
    // <Steam>/steamapps/common/GoatSimulator -> <Steam>/appcache/stats
    let Some(steam) = upk::game_dir().parent().and_then(|p| p.parent()).and_then(|p| p.parent()).map(|p| p.to_path_buf()) else { return vec![] };
    let dir = steam.join("appcache").join("stats");
    let load = |p: &std::path::Path| std::fs::read(p).ok().and_then(|b| parse_kv(&b, &mut 0, 0));
    let Some(schema) = load(&dir.join("UserGameStatsSchema_265930.bin")) else { return vec![] };
    // the most recently written stats file of any account on this PC
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with("UserGameStats_") && n.ends_with("_265930.bin")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort();
    let Some(user) = files.last().and_then(|(_, p)| load(p)) else { return vec![] };
    let stats_schema = schema.entries().first().and_then(|(_, t)| t.get("stats")).cloned();
    let cache = user.get("cache").or_else(|| user.entries().first().map(|(_, t)| t)).and_then(|c| if c.get("crc").is_some() { Some(c) } else { c.get("cache") });
    let (Some(stats_schema), Some(cache)) = (stats_schema, cache) else { return vec![] };
    let mut out = vec![];
    for (id, st) in stats_schema.entries() {
        let Some(bits) = st.get("bits") else { continue };
        let Some(Kv::Int(data)) = cache.get(id).and_then(|c| c.get("data")) else { continue };
        let times = cache.get(id).and_then(|c| c.get("AchievementTimes"));
        for (bit, b) in bits.entries() {
            let Ok(n) = bit.parse::<u32>() else { continue };
            if n < 64 && (data >> n) & 1 == 1 {
                if let Some(Kv::Str(name)) = b.get("name") {
                    let t = match times.and_then(|t| t.get(bit)) {
                        Some(Kv::Int(v)) => *v,
                        _ => 0,
                    };
                    out.push((name.clone(), t));
                }
            }
        }
    }
    out
}

/// "ACH_PUKE_MACHINE" -> "Puke Machine"
pub fn achievement_title(id: &str) -> String {
    id.trim_start_matches("ACH_")
        .split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn register(vm: &mut Vm) {
    // GGMutator.IsUnlocked: mAchGrantsUnlock against the local achievements (Steam's list in
    // the original); GOAT_UNLOCK_ALL=1 unlocks every mutator. Subclasses with their own rule
    // (secrets) keep their script.
    vm.overrides.insert("ggmutator.isunlocked".into(), |vm, c| {
        if std::env::var_os("GOAT_UNLOCK_ALL").is_some() {
            return Value::Bool(true);
        }
        let Some(cdo) = c.this else { return Value::Bool(true) };
        let ach = vm.get(cdo, "mAchGrantsUnlock");
        let n = match ach {
            Value::Byte(b) => b as usize,
            _ => return Value::Bool(true),
        };
        let class = vm.obj(cdo).class;
        let y = vm.sym("mAchGrantsUnlock");
        let name = match vm.find_prop(class, y).map(|d| d.ty.clone()) {
            Some(Ty::Byte(Some(e))) => vm.enums.get(e).and_then(|d| d.names.get(n)).cloned(),
            _ => None,
        };
        match name {
            Some(id) if !id.ends_with("_MAX") => Value::Bool(achievements().contains_key(&id)),
            _ => Value::Bool(true),
        }
    });
    vm.overrides.insert("ggengine.saveprogression".into(), |vm, _| Value::Bool(save_progression(vm)));
    // GGPitchingMachineAbstract.Pitch: the ball is a Rust physics body (pitch.rs)
    vm.overrides.insert("ggpitchingmachineabstract.pitch".into(), |vm, c| {
        if let Some(t) = c.this {
            crate::pitch::vm_pitch(vm.obj(t).name.clone());
        }
        Value::Obj(None)
    });
    // GGAchievementHandler.UnlockAchievement: recorded locally, shown on screen once
    vm.overrides.insert("ggachievementhandler.unlockachievement".into(), |vm, c| {
        let n = match c.arg(0) {
            Value::Byte(b) => *b as usize,
            Value::Int(i) => *i as usize,
            _ => return Value::Int(0),
        };
        let id = match vm.funcs[c.func].params.first().map(|p| &p.ty) {
            Some(Ty::Byte(Some(e))) => vm.enums.get(*e).and_then(|d| d.names.get(n)).cloned(),
            _ => None,
        };
        // out of range ("None" defaults) and the ACH_MAX sentinels unlock nothing
        let Some(id) = id.filter(|i| !i.ends_with("_MAX")) else { return Value::Int(0) };
        if achievements().contains_key(&id) {
            return Value::Int(0);
        }
        let mut all = local_achievements();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        all.insert(id.clone(), serde_json::Value::from(now));
        let dir = save_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("achievements.json"), serde_json::to_string_pretty(&serde_json::Value::Object(all)).unwrap_or_default());
        vm.say(format!("achievement unlocked: {id}"));
        vm.events.push(HostEvent::Message(format!("Achievement unlocked: {}", achievement_title(&id))));
        vm.events.push(HostEvent::Achievement(id));
        Value::Int(0)
    });
}

impl Host {
    /// GGLocalPlayer.mSaveProgression (loaded), the achievement handler as a notification
    /// listener (GGGameInfo.Login) and GGGameInfo.InitGameComponents (the quest manager).
    pub fn progress_start(&mut self) {
        let Some(game) = self.game else { return };
        if std::env::var_os("GOAT_NO_QUESTS").is_some() {
            return;
        }
        register(&mut self.vm);
        let Some(pc_class) = self.vm.class_by_name("GGProgression") else { return };
        let engine = self.vm.globals["engine"];
        let lp = match self.vm.get(engine, "GamePlayers") {
            Value::Array(a) => a.first().and_then(|v| v.obj()),
            _ => None,
        };
        let Some(lp) = lp else { return };
        let prog = self.vm.spawn_object(pc_class, Some("GGProgression_0".into()));
        load_progression(&mut self.vm, prog);
        self.vm.set(lp, "mSaveProgression", Value::Obj(Some(prog)));
        // achievements
        if let (Some(goat), Some(ah_class)) = (self.goat, self.vm.class_by_name("GGAchievementHandler")) {
            if let Some(pc) = self.vm.get(goat, "Controller").obj() {
                let ah = match self.vm.get(pc, "mAchievementHandler").obj() {
                    Some(a) => a,
                    None => {
                        let a = self.vm.spawn_object(ah_class, None);
                        self.vm.set(pc, "mAchievementHandler", Value::Obj(Some(a)));
                        a
                    }
                };
                self.vm.call_named(game, "AddNotificationListener", vec![Value::Obj(Some(ah))]);
            }
        }
        // collectibles register their IDs first (GGQuestCollectible counts them in InitQuest)
        self.collectibles_start();
        self.vm.call_named(game, "InitGameComponents", vec![]);
        // GGMusicManager was spawned before BeginPlay events ran: PostBeginPlay -> InitMusic now
        if let Some(mm) = self.vm.get(game, "mMusicManager").obj() {
            self.vm.globals.insert("musicmanager", mm);
            if self.vm.get(mm, "mAutoInitMusic").truthy() {
                self.vm.call_named(mm, "InitMusic", vec![]);
            }
            if std::env::var_os("UVM_MUSIC_TRACE").is_some() {
                let was = self.vm.trace_calls;
                self.vm.trace_calls = true;
                self.vm.call_named(mm, "StartMusic", vec![]);
                self.vm.trace_calls = was;
            }
        }
        self.quests = self.vm.get(game, "mQuestManager").obj();
        if let Some(q) = self.quests {
            let n = quest_list(&mut self.vm, q).len();
            self.vm.say(format!("== quest manager: {n} quests on this map"));
        }
    }

    /// GGGameInfo.Tick's part for quests.
    pub fn progress_tick(&mut self, dt: f32) {
        // GGMusicManager.Tick: track queue (beats, sorting, cross-fades)
        if let Some(&mm) = self.vm.globals.get("musicmanager") {
            self.vm.call_named(mm, "Tick", vec![Value::Float(dt)]);
        }
        if std::env::var_os("GOAT_NO_QUESTTICK").is_some() {
            return;
        }
        // QuestTick only reports progress to the HUD and completes quests (KnockDistance sums the
        // time): 15 times a second with the time since the last call is plenty
        self.quest_dt += dt;
        if self.quest_dt < 1.0 / 15.0 {
            return;
        }
        let dt = std::mem::take(&mut self.quest_dt);
        if let Some(q) = self.quests {
            self.vm.call_named(q, "QuestTick", vec![Value::Float(dt)]);
        }
    }

    /// Quests for the UI/tests: (identifier, class, completed, progress, completion progress).
    pub fn quests(&mut self) -> Vec<QuestInfo> {
        match self.quests {
            Some(q) => quest_list(&mut self.vm, q),
            None => vec![],
        }
    }
}

#[derive(Debug, Clone)]
pub struct QuestInfo {
    pub id: u8,
    pub class: String,
    pub completed: bool,
    pub active: bool,
    pub progress: f32,
    pub goal: f32,
}

fn quest_list(vm: &mut Vm, q: ObjId) -> Vec<QuestInfo> {
    let mut out = vec![];
    let mut cur = vm.get(q, "mQuestPoolItr").obj();
    while let Some(o) = cur {
        if out.len() > 500 {
            break;
        }
        let mut num = |k: &str| match vm.get(o, k) {
            Value::Int(i) => i as f32,
            Value::Float(f) => f,
            Value::Byte(b) => b as f32,
            _ => 0.0,
        };
        let (id, progress, goal) = (num("mQuestIdentifier") as u8, num("mCurrentProgress"), num("mCompletionProgress"));
        let completed = vm.get(o, "mIsCompleted").truthy();
        let active = vm.get(o, "mIsActive").truthy();
        out.push(QuestInfo { id, class: vm.class_name(vm.obj(o).class).to_string(), completed, active, progress, goal });
        cur = vm.get(o, "mNextQuest").obj();
    }
    out
}
