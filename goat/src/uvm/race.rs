//! Races: GGTriggerVolumeRace's popup restarts the map with
//! "?game=GoatGame.GGGameInfoRace?SkipIntro?raceID=<id>" (the options travel to the new process in
//! GOAT_GAME_OPTIONS). The host then spawns that GameInfo, runs InitGame (raceID, the 1 s
//! CountDownTimer), the checkpoints' PostBeginPlay (mesh holders, CheckpointSpawned ordering)
//! and PostLogin (RaceCountdown state, checkpoint owners, the bike for BikeRace). The script does
//! the rest: checkpoint Touch -> PlayerPassedCheckPoint, the HUD countdown / game timer, the
//! "Finished" popup and restart. `race_tick` runs the race parts of GGGameInfoRace.Tick
//! (FinalizeCheckpoints, the timer label) without GGGameInfo.Tick's quest tick.

use super::*;
use super::bridge::Host;

/// The URL options of this run ("?game=...?raceID=..."), if any.
pub fn game_options() -> Option<String> {
    std::env::var("GOAT_GAME_OPTIONS").ok().filter(|s| !s.is_empty())
}

/// GameInfo.ParseOption.
pub fn parse_option(options: &str, key: &str) -> Option<String> {
    options
        .split('?')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            k.eq_ignore_ascii_case(key).then(|| v.to_string())
        })
        .next()
}

/// The GameInfo class the URL asks for ("GGGameInfoRace"), without its package.
pub fn game_class_name() -> Option<String> {
    let g = parse_option(&game_options()?, "game")?;
    Some(g.rsplit('.').next()?.to_string())
}

pub fn race_id() -> Option<String> {
    parse_option(&game_options()?, "raceID")
}

static VIEW: std::sync::Mutex<Option<RaceView>> = std::sync::Mutex::new(None);

pub fn publish(v: RaceView) {
    if let Ok(mut g) = VIEW.lock() {
        *g = Some(v);
    }
}

/// The latest race state from the VM (None outside races).
pub fn view() -> Option<RaceView> {
    VIEW.lock().ok().and_then(|g| g.clone()).filter(|v| v.active)
}

/// What the Rust side shows of a running race.
#[derive(Clone, Debug, Default)]
pub struct RaceView {
    pub active: bool,
    /// The player is held in GGPlayerControllerGame state RaceCountdown.
    pub frozen: bool,
    /// Checkpoints in order: (mMeshToUse actor name, shown for player 0).
    pub checkpoints: Vec<(String, bool)>,
    /// Their volumes' locations (Bevy space) and whether player 0 passed them.
    pub locations: Vec<bevy::prelude::Vec3>,
    pub passed: Vec<bool>,
    /// mRaceTimes[0] (ms, 0 until finished) and mIsRaceActive.
    pub time_ms: i32,
    pub running: bool,
}

impl Host {
    pub fn race_game(&mut self) -> Option<ObjId> {
        let game = self.game?;
        let rc = self.vm.class_by_name("GGGameInfoRace")?;
        self.vm.is_child_of(self.vm.obj(game).class, rc).then_some(game)
    }

    pub fn race_start(&mut self) {
        let Some(game) = self.race_game() else { return };
        let opts = game_options().unwrap_or_default();
        self.vm.call_named(game, "InitGame", vec![Value::Str(opts.clone()), Value::Str(String::new())]);
        // one local player in the cross-map data (DeActivateCheckpointForGoat counts them)
        if let Some(&vp) = self.vm.globals.get("viewport") {
            let cmd = self.vm.call_named(vp, "GetCrossMapData", vec![]).and_then(|v| v.obj());
            if let Some(cmd) = cmd {
                let empty = matches!(self.vm.get(cmd, "mLoggedInControllerIDs"), Value::Array(ref a) if a.is_empty()) || matches!(self.vm.get(cmd, "mLoggedInControllerIDs"), Value::Obj(None));
                if empty {
                    self.vm.set(cmd, "mLoggedInControllerIDs", Value::Array(vec![Value::Int(0)]));
                }
            }
        }
        // the checkpoints' PostBeginPlay (mesh holders, CheckpointSpawned)
        if let (Some(mp), Some(base)) = (self.vm.map_pkg, self.vm.class_by_name("GGRaceCheckpointAbstract")) {
            let p = &self.vm.reg.packages[mp];
            let idx: Vec<i32> = (0..p.exports.len())
                .filter(|&i| p.class_name(i as i32 + 1).contains("RaceCheckpoint") && p.obj_path(i as i32 + 1).contains(".PersistentLevel."))
                .map(|i| i as i32 + 1)
                .collect();
            for i in idx {
                let Some(o) = self.vm.obj_for_ref(mp, i) else { continue };
                if self.vm.is_child_of(self.vm.obj(o).class, base) {
                    self.vm.call_named(o, "PostBeginPlay", vec![]);
                }
            }
        }
        if let Some(pc) = self.goat.and_then(|g| self.vm.get(g, "Controller").obj()) {
            self.vm.call_named(game, "PostLogin", vec![Value::Obj(Some(pc))]);
        }
        let n = match self.vm.get(game, "mOrderedCheckpoints") {
            Value::Array(a) => a.len(),
            _ => 0,
        };
        self.vm.say(format!("== race {opts}: {n} checkpoints"));
    }

    /// GGGameInfoRace.Tick without GGGameInfo.Tick.
    pub fn race_tick(&mut self) {
        let Some(game) = self.race_game() else { return };
        if self.vm.get(game, "mLastCheckpointIndex").int() != -1 && !self.vm.get(game, "mFinalizedCheckpoints").truthy() {
            self.vm.call_named(game, "FinalizeCheckpoints", vec![]);
        }
        if !self.vm.get(game, "mIsRaceActive").truthy() {
            return;
        }
        let Some(m) = self.hud_movie() else { return };
        let Some(label) = self.vm.get(m, "mGameTimer").obj() else { return };
        let finished = match self.vm.get(game, "mRaceTimes") {
            Value::Array(a) => a.first().map(|v| v.int()).unwrap_or(0),
            _ => 0,
        };
        let secs = if finished > 0 {
            finished as f32 / 1000.0
        } else {
            let start = self.vm.get(game, "mGameStartTime").float();
            self.vm.call_named(game, "RealTimeSince", vec![Value::Float(start)]).map(|v| v.float()).unwrap_or(0.0)
        };
        let text = self.vm.call_named(game, "FormatTime", vec![Value::Int(secs as i32)]).unwrap_or(Value::Str(String::new()));
        self.vm.call_named(label, "SetLabelText", vec![text]);
    }

    pub fn race_view(&mut self) -> RaceView {
        let Some(game) = self.race_game() else { return RaceView::default() };
        let mut v = RaceView { active: true, ..Default::default() };
        v.frozen = self
            .goat
            .and_then(|g| self.vm.get(g, "Controller").obj())
            .and_then(|pc| self.vm.obj(pc).state)
            .is_some_and(|s| self.vm.name_of(s).eq_ignore_ascii_case("RaceCountdown"));
        let cps: Vec<Option<ObjId>> = match self.vm.get(game, "mOrderedCheckpoints") {
            Value::Array(a) => a.iter().map(|x| x.obj()).collect(),
            _ => vec![],
        };
        for cp in cps.into_iter().flatten() {
            let mesh = self.vm.get(cp, "mMeshToUse").obj().map(|m| self.vm.obj(m).name.clone()).unwrap_or_default();
            let holder = match self.vm.get(cp, "mStaticMeshHolders") {
                Value::Array(a) => a.first().and_then(|x| x.obj()),
                _ => None,
            };
            let passed = holder.is_some_and(|h| self.vm.get(h, "mDeactivated").truthy());
            let shown = holder.is_some_and(|h| !self.vm.get(h, "bHidden").truthy()) && !passed;
            v.checkpoints.push((mesh, shown));
            v.passed.push(passed);
            // the gate mesh (the brush pivot can sit off the gate)
            let at = self.vm.get(cp, "mMeshToUse").obj().unwrap_or(cp);
            let l = self.vm.get(at, "Location");
            let l = if matches!(l, Value::Struct(_)) { self.vm.vec3(&l) } else { [0.0; 3] };
            v.locations.push(crate::convert::pos(l));
        }
        v.time_ms = match self.vm.get(game, "mRaceTimes") {
            Value::Array(a) => a.first().map(|x| x.int()).unwrap_or(0),
            _ => 0,
        };
        v.running = self.vm.get(game, "mIsRaceActive").truthy();
        v
    }
}

impl Host {
    /// GGWorkshop.mUGCDetails from the Workshop items downloaded with the mod menu, so the main
    /// menu's Workshop -> Downloaded lists them (subscribed, fully downloaded).
    pub fn workshop_start(&mut self) {
        let Some(&vp) = self.vm.globals.get("viewport") else { return };
        let Some(wc) = self.vm.class_by_name("GGWorkshop") else { return };
        // GGGameViewportClient.mWorkshop (instanced)
        let ws = match self.vm.get(vp, "mWorkshop").obj() {
            Some(w) => w,
            None => {
                let w = self.vm.spawn_object(wc, None);
                self.vm.set(vp, "mWorkshop", Value::Obj(Some(w)));
                w
            }
        };
        let y = self.vm.sym("mUGCDetails");
        let ty = self.vm.classes[wc].props.iter().find(|p| p.name == y).map(|p| p.ty.clone());
        let Some(Ty::Array(inner)) = ty else { return };
        let Ty::Struct(sd) = *inner else { return };
        let items = crate::mods::downloaded_workshop();
        let mut arr = vec![];
        for (id, title) in &items {
            let mut e = self.vm.struct_zero(sd);
            *e.field_mut(self.vm.sym("PublishHandle")) = Value::Str(id.clone());
            // no FileHandle: these are downloaded (UpdateDownloadProgress asks Steam only about
            // files with one)
            *e.field_mut(self.vm.sym("Title")) = Value::Str(title.clone());
            *e.field_mut(self.vm.sym("IsSubscribed")) = Value::Bool(true);
            *e.field_mut(self.vm.sym("DownloadedPct")) = Value::Int(100);
            arr.push(Value::Struct(Box::new(e)));
        }
        self.vm.set(ws, "mUGCDetails", Value::Array(arr));
        self.vm.say(format!("== workshop: {} downloaded items", items.len()));
    }
}
