//! Hosts Workshop mutators: builds the script world the mutators expect (engine, local
//! player, viewport, WorldInfo, GGGameInfo, player controller, the goat), adds mutators
//! through the game's own GameInfo.AddMutator and applies them with SetPlayerDefaults ->
//! BaseMutator.ModifyPlayer, then ticks them. The script goat's movement values are
//! published for the Rust goat (`goat_values()`).

use super::*;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

#[derive(Debug, Clone)]
pub enum HostEvent {
    SetMaterial { component: Option<ObjId>, index: i32, material: String },
    SetSkeletalMesh { component: Option<ObjId>, mesh: String },
    AttachToSocket { component: Option<ObjId>, mesh: Option<ObjId>, socket: String },
    Detach(ObjId),
    /// A MaterialInstanceConstant's parameters changed (tint whatever uses it).
    MaterialParam(ObjId),
    /// AddImpulse on an actor's component (Unreal units; velocity_change = ignore mass).
    Impulse { actor: ObjId, impulse: [f32; 3], velocity_change: bool },
    PlaySound { cue: String },
    /// Text for the player (Broadcast, ClientMessage).
    Message(String),
    /// An achievement was unlocked (its EAchievementsID name).
    Achievement(String),
    /// A console command the VM doesn't run itself (slomo, god...).
    Console(String),
    /// GGSeqAct_GiveScore (Kismet): a named trick for the combo.
    KismetScore { name: String, score: i32, special: bool },
    /// Particle effects (see fx.rs).
    Fx(super::fx::FxCmd),
    /// A map actor moved by matinee (Unreal units / rotator), with its initial placement.
    Mover { name: String, location: [f32; 3], rotation: [i32; 3], initial: ([f32; 3], [i32; 3]) },
}

/// Movement values read back from the script goat after mutators ran.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoatValues {
    pub walk_speed: f32,
    pub sprint_speed: f32,
    pub strafe_speed: f32,
    pub reverse_speed: f32,
    pub jump_z: f32,
    pub multi_jump_boost: f32,
    pub max_multi_jump: i32,
    pub air_control: f32,
    pub wall_run_speed: f32,
}

/// (values now, values before any mutator ran)
static VALUES: RwLock<Option<(GoatValues, GoatValues)>> = RwLock::new(None);

/// Script-goat values when Workshop mutators are running (None = untouched game defaults).
pub fn goat_values() -> Option<GoatValues> {
    VALUES.read().unwrap().map(|v| v.0)
}

/// What the Rust goat uses: the script goat's values while mutators run, else the
/// GGGoat defaults the port was built from.
#[derive(Debug, Clone, Copy)]
pub struct Movement {
    pub walk_speed: f32,
    pub sprint_speed: f32,
    pub strafe_speed: f32,
    pub reverse_speed: f32,
    pub jump_z: f32,
    pub multi_jump_boost: f32,
    pub air_control: f32,
    /// MaxMultiJump added by mutators (GGJumpyGoatComponent-style `MaxMultiJump += n`).
    pub extra_jumps: i32,
}

pub fn movement() -> Movement {
    use crate::goat::tune;
    match *VALUES.read().unwrap() {
        Some((v, base)) => Movement {
            walk_speed: v.walk_speed,
            sprint_speed: v.sprint_speed,
            strafe_speed: v.strafe_speed,
            reverse_speed: v.reverse_speed,
            jump_z: v.jump_z,
            multi_jump_boost: v.multi_jump_boost,
            air_control: v.air_control,
            extra_jumps: v.max_multi_jump.saturating_sub(base.max_multi_jump),
        },
        None => Movement {
            walk_speed: tune::WALK_SPEED,
            sprint_speed: tune::SPRINT_SPEED,
            strafe_speed: tune::STRAFE_SPEED,
            reverse_speed: tune::REVERSE_SPEED,
            jump_z: tune::JUMP_Z,
            multi_jump_boost: tune::MULTI_JUMP_BOOST,
            air_control: tune::AIR_CONTROL,
            extra_jumps: 0,
        },
    }
}

/// A mutator class listed in a Workshop item's GoatMod.ini.
#[derive(Debug, Clone)]
pub struct ModEntry {
    pub id: String,
    pub class_path: String,
    pub display: String,
    pub packages: Vec<PathBuf>,
}

/// Mutators of downloaded Workshop items (`+mAllMutators=(FileName="Pkg.Class",...)`).
pub fn workshop_mutators(dir: &Path) -> Vec<ModEntry> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    let mut items: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    items.sort();
    for item in items {
        let id = item.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ini = ["GoatMod.ini", "goatmod.ini"].iter().map(|n| item.join(n)).find(|p| p.exists());
        let Some(text) = ini.and_then(|p| std::fs::read(p).ok()) else { continue };
        let text = String::from_utf8_lossy(&text).into_owned();
        let packages: Vec<PathBuf> = std::fs::read_dir(item.join("Scripts"))
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("u"))).collect())
            .unwrap_or_default();
        for line in text.lines() {
            let l = line.trim();
            if !l.to_ascii_lowercase().starts_with("+mallmutators=") {
                continue;
            }
            let field = |k: &str| {
                let lk = l.to_ascii_lowercase();
                let at = lk.find(&format!("{}=\"", k.to_ascii_lowercase()))? + k.len() + 2;
                let end = l[at..].find('"')? + at;
                Some(l[at..end].to_string())
            };
            if let Some(fname) = field("FileName") {
                out.push(ModEntry { id: id.clone(), class_path: fname.clone(), display: field("DisplayName").unwrap_or(fname), packages: packages.clone() });
            }
        }
    }
    out
}

/// The base game's own mutators (Config/Common/DefaultDLC.ini mAllMutators: GoatGame.* and the
/// GoatVanilla classes cooked into Startup.upk); id "game", key "gm-<FileName>" in enabled.json.
pub fn game_mutators() -> Vec<ModEntry> {
    let path = upk::game_dir().join("GoatGame").join("Config").join("Common").join("DefaultDLC.ini");
    let Ok(b) = std::fs::read(&path) else { return vec![] };
    let text = String::from_utf8_lossy(&b).into_owned();
    let mut out = vec![];
    for l in text.lines() {
        let l = l.trim().trim_start_matches(['+', '.']);
        if !l.to_ascii_lowercase().starts_with("mallmutators=") {
            continue;
        }
        let field = |k: &str| {
            let lk = l.to_ascii_lowercase();
            let at = lk.find(&format!("{}=\"", k.to_ascii_lowercase()))? + k.len() + 2;
            let end = l[at..].find('"')? + at;
            Some(l[at..end].to_string())
        };
        let (Some(file), display) = (field("FileName"), field("DisplayName").unwrap_or_default()) else { continue };
        if display.starts_with("NOT USED") {
            continue;
        }
        let description = field("Description").unwrap_or_default();
        out.push(ModEntry { id: "game".into(), class_path: file, display: if description.is_empty() { display } else { format!("{display}: {description}") }, packages: vec![] });
    }
    out
}

/// Mutators for enabled keys: "ws-<workshop id>" items and "gm-<FileName>" game mutators.
pub fn mutators_for(keys: &[String]) -> Vec<ModEntry> {
    let dir = crate::mods::mods_dir().join("workshop");
    let mut out: Vec<ModEntry> = workshop_mutators(&dir).into_iter().filter(|e| keys.iter().any(|k| k.strip_prefix("ws-") == Some(e.id.as_str()))).collect();
    out.extend(game_mutators().into_iter().filter(|e| keys.iter().any(|k| k.strip_prefix("gm-").is_some_and(|f| f.eq_ignore_ascii_case(&e.class_path)))));
    out
}

/// Script packages the mutator host links against (from the game's CookedPC).
pub const BASE_PACKAGES: &[&str] = &["Core.u", "Engine.u", "GameFramework.u", "GFxUI.u", "IpDrv.u", "UDKBase.u", "GoatGame.u", "Startup.upk"];

pub struct Host {
    pub vm: Vm,
    pub goat: Option<ObjId>,
    pub game: Option<ObjId>,
    pub mutators: Vec<ObjId>,
    pub ticks: u64,
    base: Option<GoatValues>,
    /// Goat state written by the last sync_in (to spot script changes).
    pub written: Option<super::sync::GoatState>,
    pub prev_keys: Vec<Sym>,
    /// Keys that went down this frame (inventory.rs).
    pub downs: Vec<Sym>,
    pub helpers: HashMap<String, ObjId>,
    /// Orbital camera zoom last seen in the script world.
    pub last_zoom: Option<[f32; 3]>,
    pub kismet: super::kismet::Kismet,
    /// Map actors running their own script classes (see kismet::map_scripts_start).
    pub map_actors: Vec<ObjId>,
    pub map_ticks: bool,
    /// GGGameInfo.mQuestManager once InitGameComponents ran (see progress.rs).
    pub quests: Option<ObjId>,
    /// Map GGCollectibleActors (see collectibles.rs).
    pub collectibles: Vec<ObjId>,
    /// Open the main menu on the next menu frame (boot into the front end).
    pub pending_main_menu: bool,
    /// Time not yet given to QuestTick (it runs at 15 Hz).
    pub quest_dt: f32,
    /// Goat material set by script (kept across mesh swaps).
    pub goat_material: Option<bevy::prelude::Handle<bevy::prelude::StandardMaterial>>,
}

impl Vm {
    /// New actor: CDO values + WorldInfo/Owner (no BeginPlay events).
    pub fn spawn_actor(&mut self, class: usize, owner: Option<ObjId>) -> ObjId {
        let o = self.spawn_object(class, None);
        if let Some(&wi) = self.globals.get("worldinfo") {
            self.set(o, "WorldInfo", Value::Obj(Some(wi)));
        }
        if owner.is_some() {
            self.set(o, "Owner", Value::Obj(owner));
        }
        // the spawn native runs the actor's startup events like UE3's SpawnActor
        if self.globals.contains_key("ready") {
            let cn = self.class_name(class).to_string();
            let on = self.obj(o).name.clone();
            self.say(format!("spawned {cn} -> {on}"));
            self.call_named(o, "PreBeginPlay", vec![]);
            if !self.obj(o).destroyed {
                self.call_named(o, "PostBeginPlay", vec![]);
            }
        }
        o
    }
}

impl Host {
    /// Load the base packages, the given mod packages and optionally the map package (its actors
    /// and Kismet become script objects), and build the script world.
    pub fn with_map(mod_packages: &[PathBuf], map: Option<&std::path::Path>) -> anyhow::Result<Self> {
        let t = std::time::Instant::now();
        let mut pkgs = vec![];
        for p in BASE_PACKAGES {
            pkgs.push(upk::Package::open(upk::cooked_dir().join(p))?);
        }
        for p in mod_packages {
            match upk::Package::open(p) {
                Ok(pk) => pkgs.push(pk),
                Err(e) => eprintln!("[uvm] skipping {}: {e}", p.display()),
            }
        }
        let mut map_pkg = None;
        if let Some(mp) = map {
            match upk::Package::open(mp) {
                Ok(pk) => {
                    map_pkg = Some(pkgs.len());
                    pkgs.push(pk);
                }
                Err(e) => eprintln!("[uvm] map {}: {e}", mp.display()),
            }
        }
        let mut vm = Vm::new(upk::registry::Registry::new(pkgs));
        vm.map_pkg = map_pkg;
        vm.say(format!("loaded {} packages in {:.2} s", vm.reg.packages.len(), t.elapsed().as_secs_f32()));
        let mut h = Host { vm, goat: None, game: None, mutators: vec![], ticks: 0, base: None, written: None, prev_keys: vec![], downs: vec![], helpers: HashMap::new(), goat_material: None, last_zoom: None, kismet: Default::default(), map_actors: vec![], map_ticks: false, quests: None, collectibles: vec![], pending_main_menu: false, quest_dt: 0.0 };
        h.build_world()?;
        Ok(h)
    }

    fn class(&mut self, name: &str) -> anyhow::Result<usize> {
        self.vm.class_by_name(name).ok_or_else(|| anyhow::anyhow!("class {name} not found"))
    }

    fn build_world(&mut self) -> anyhow::Result<()> {
        let vm_trace = self.vm.trace_calls;
        self.vm.trace_calls = false;
        let wi_c = self.class("WorldInfo")?;
        let wi = self.vm.spawn_object(wi_c, Some("WorldInfo_0".into()));
        self.vm.globals.insert("worldinfo", wi);
        self.vm.set(wi, "WorldInfo", Value::Obj(Some(wi)));
        // GGEngine (the game's engine class: mSettings, GetGGEngine) when the packages have it
        let eng_c = self.class("GGEngine").or_else(|_| self.class("GameEngine"))?;
        let engine = self.vm.spawn_object(eng_c, Some("GameEngine_0".into()));
        self.vm.globals.insert("engine", engine);
        let lp_c = self.class("GGLocalPlayer")?;
        let lp = self.vm.spawn_object(lp_c, Some("GGLocalPlayer_0".into()));
        self.vm.set(lp, "mPlayerSlot", Value::Int(0));
        self.vm.set(engine, "GamePlayers", Value::Array(vec![Value::Obj(Some(lp))]));
        let vp_c = self.class("GGGameViewportClient")?;
        let vp = self.vm.spawn_object(vp_c, Some("GGGameViewportClient_0".into()));
        self.vm.globals.insert("viewport", vp);
        self.vm.set(engine, "GameViewport", Value::Obj(Some(vp)));
        self.vm.set(lp, "ViewportClient", Value::Obj(Some(vp)));
        self.vm.overrides.insert("gggameviewportclient.getviewportclient".into(), |vm, _| Value::Obj(vm.globals.get("viewport").copied()));
        // the URL's ?game= (GGGameInfoRace for races), else GGGameInfo
        let base_game = self.class("GGGameInfo")?;
        let game_c = super::race::game_class_name()
            .and_then(|n| self.vm.class_by_name(&n))
            .filter(|&c| self.vm.is_child_of(c, base_game))
            .unwrap_or(base_game);
        let game = self.vm.spawn_actor(game_c, None);
        self.vm.set(wi, "Game", Value::Obj(Some(game)));
        self.vm.globals.insert("game", game);
        // GGGameInfo.PostBeginPlay: mMusicManager = Spawn(mMusicManagerClass, self)
        if let Some(mc) = self.vm.get(game, "mMusicManagerClass").obj().and_then(|o| self.vm.as_class(&Value::Obj(Some(o)))) {
            let mm = self.vm.spawn_actor(mc, Some(game));
            self.vm.set(game, "mMusicManager", Value::Obj(Some(mm)));
        }
        let pc_c = self.class("GGPlayerControllerGame")?;
        let pc = self.vm.spawn_actor(pc_c, None);
        let pi_c = self.class("GGPlayerInputGame")?;
        let pi = self.vm.spawn_object(pi_c, None);
        self.vm.obj_mut(pi).outer = Some(pc);
        self.vm.set(pc, "PlayerInput", Value::Obj(Some(pi)));
        self.vm.set(pc, "Player", Value::Obj(Some(lp)));
        self.vm.set(lp, "Actor", Value::Obj(Some(pc)));
        let goat_c = self.class("GGGoat")?;
        let goat = self.vm.spawn_actor(goat_c, None);
        self.vm.set(goat, "Controller", Value::Obj(Some(pc)));
        // PlayerCamera (GGCamera; its CameraCache follows the game camera), myHUD, BroadcastHandler
        let cam_class = self.vm.get(pc, "CameraClass").obj().and_then(|o| self.vm.as_class(&Value::Obj(Some(o)))).or_else(|| self.vm.class_by_name("GGCamera"));
        if let Some(cc) = cam_class {
            let cam = self.vm.spawn_actor(cc, Some(pc));
            self.vm.set(cam, "PCOwner", Value::Obj(Some(pc)));
            self.vm.set(pc, "PlayerCamera", Value::Obj(Some(cam)));
            self.vm.globals.insert("camera", cam);
            // GGCamera.PostBeginPlay creates mCameraModes (orbital = CM_ORBIT 1)
            self.vm.call_named(cam, "PostBeginPlay", vec![]);
        }
        let hud_class = self.vm.get(game, "HUDType").obj().and_then(|o| self.vm.as_class(&Value::Obj(Some(o)))).or_else(|| self.vm.class_by_name("HUD"));
        if let Some(hc) = hud_class {
            let hud = self.vm.spawn_actor(hc, Some(pc));
            self.vm.set(hud, "PlayerOwner", Value::Obj(Some(pc)));
            self.vm.set(pc, "myHUD", Value::Obj(Some(hud)));
        }
        // WorldInfo.MyEmitterPool (particles are logged, not drawn)
        if let Some(pc_class) = self.vm.class_by_name("GGEmitterPool").or_else(|| self.vm.class_by_name("EmitterPool")) {
            let pool = self.vm.spawn_actor(pc_class, None);
            self.vm.set(wi, "MyEmitterPool", Value::Obj(Some(pool)));
        }
        let bh_class = self.vm.get(game, "BroadcastHandlerClass").obj().and_then(|o| self.vm.as_class(&Value::Obj(Some(o)))).or_else(|| self.vm.class_by_name("BroadcastHandler"));
        if let Some(bc) = bh_class {
            let bh = self.vm.spawn_actor(bc, Some(game));
            self.vm.set(game, "BroadcastHandler", Value::Obj(Some(bh)));
        }
        self.vm.set(pc, "bIsPlayer", Value::Bool(true));
        if let Some(pv_c) = self.vm.class_by_name("DefaultPhysicsVolume") {
            let pv = self.vm.spawn_actor(pv_c, None);
            self.vm.set(goat, "PhysicsVolume", Value::Obj(Some(pv)));
        }
        // GGGoat.PossessedBy caches the local player's slot
        self.vm.set(goat, "mCachedSlotNr", Value::Int(0));
        self.vm.set(pc, "Pawn", Value::Obj(Some(goat)));
        self.vm.globals.insert("goat", goat);
        self.vm.globals.insert("ready", goat);
        self.goat = Some(goat);
        self.game = Some(game);
        // engine settings + profile (menus read them)
        self.setup_settings();
        self.vm.trace_calls = vm_trace;
        Ok(())
    }

    /// Add mutators the way the Custom Game menu + GameInfo do, then apply them to the goat.
    pub fn start(&mut self, mods: &[ModEntry]) {
        on_big_stack(|| self.start_inner(mods))
    }

    fn start_inner(&mut self, mods: &[ModEntry]) {
        let (Some(game), Some(goat)) = (self.game, self.goat) else { return };
        if self.base.is_none() {
            self.publish();
        }
        let vp = self.vm.globals["viewport"];
        for m in mods {
            self.vm.say(format!("== mutator {} ({}) from workshop item {}", m.display, m.class_path, m.id));
            // the menu marks the mutator as chosen for player slot 0
            self.vm.call_named(vp, "AddMutatorEntry", vec![Value::Str(m.class_path.clone()), Value::Int(0)]);
            self.vm.call_named(game, "AddMutator", vec![Value::Str(m.class_path.clone()), Value::Bool(true)]);
        }
        let mut mutators = vec![];
        let mut cur = self.vm.get(game, "BaseMutator").obj();
        while let Some(m) = cur {
            if mutators.contains(&m) {
                break;
            }
            mutators.push(m);
            cur = self.vm.get(m, "NextMutator").obj();
        }
        self.mutators = mutators;
        self.vm.say(format!("== {} mutator(s) active; SetPlayerDefaults(goat)", self.mutators.len()));
        self.vm.call_named(game, "SetPlayerDefaults", vec![Value::Obj(Some(goat))]);
        self.publish();
        self.map_scripts_start();
        self.map_ticks = self.map_scripts_tick_needed();
        self.kismet_start();
        self.progress_start();
        self.race_start();
        self.workshop_start();
    }

    /// One game frame (mutator Tick events, mutator components, timers); call inside `with_vm`.
    pub fn tick_now(&mut self, dt: f32) {
        self.tick_inner(dt)
    }

    /// Call into the VM from outside (events, input) on a big stack.
    pub fn with_vm<R: Send>(&mut self, f: impl FnOnce(&mut Host) -> R + Send) -> R {
        on_big_stack(|| f(self))
    }

    fn tick_inner(&mut self, dt: f32) {
        self.ticks += 1;
        // WorldInfo's clocks (cooldowns, visibility intervals...)
        if let Some(&wi) = self.vm.globals.get("worldinfo") {
            let t = self.vm.get(wi, "TimeSeconds").float() + dt;
            let rt = self.vm.get(wi, "RealTimeSeconds").float() + dt;
            self.vm.set(wi, "TimeSeconds", Value::Float(t));
            self.vm.set(wi, "RealTimeSeconds", Value::Float(rt));
            self.vm.set(wi, "AudioTimeSeconds", Value::Float(rt));
            self.vm.set(wi, "DeltaSeconds", Value::Float(dt));
        }
        // trace the first frames only, then just keep the log of unusual events
        self.vm.trace_calls = self.ticks <= 2;
        for m in self.mutators.clone() {
            if self.vm.obj(m).destroyed {
                continue;
            }
            let c = self.vm.obj(m).class;
            if self.vm.defines(c, "Tick").is_some() {
                self.vm.call_named(m, "Tick", vec![Value::Float(dt)]);
            }
        }
        for comp in self.components() {
            let c = self.vm.obj(comp).class;
            if self.vm.defines(c, "TickMutatorComponent").is_some() {
                self.vm.call_named(comp, "TickMutatorComponent", vec![Value::Float(dt)]);
            }
        }
        self.race_tick();
        super::race::publish(self.race_view());
        // world timers; menu views' timers run from tick_menus (also while paused)
        self.run_timers(dt, false);
        let t0 = std::time::Instant::now();
        self.vm.tick_states(dt);
        let t1 = std::time::Instant::now();
        self.progress_tick(dt);
        let t2 = std::time::Instant::now();
        self.map_scripts_tick(dt);
        let t3 = std::time::Instant::now();
        self.kismet_tick(dt);
        let t4 = std::time::Instant::now();
        self.publish();
        let t5 = std::time::Instant::now();
        if std::env::var_os("GOAT_PROFILE").is_some() && (t5 - t0).as_secs_f32() > 0.0015 {
            let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f32() * 1e3;
            println!("  vm tick: states {:.2} quests {:.2} map scripts {:.2} ({} actors) kismet {:.2} publish {:.2} (timers {})", ms(t0, t1), ms(t1, t2), ms(t2, t3), self.map_actors.len(), ms(t3, t4), ms(t4, t5), self.vm.timers.len());
        }
    }

    /// Bone scales from skel controls on the goat mesh: BoneScale blended by ControlStrength
    /// (SkelControlSingleBone with bApplyScale-style use, as Fatter Goat does).
    pub fn bone_scales(&self) -> Vec<(String, f32)> {
        let Some(g) = self.goat else { return vec![] };
        let vm = &self.vm;
        let get = |o: ObjId, k: &str| vm.syms_get(k).and_then(|y| vm.obj(o).props.get(&y).cloned());
        let Some(Value::Obj(Some(mesh))) = get(g, "Mesh") else { return vec![] };
        let Some(Value::Array(list)) = get(mesh, "uvmSkelControls") else { return vec![] };
        list.iter()
            .filter_map(|v| v.obj())
            .filter_map(|c| {
                let bone = match get(c, "uvmBoneName")? {
                    Value::Name(y) => vm.name_of(y).to_string(),
                    _ => return None,
                };
                let scale = get(c, "BoneScale").map(|v| v.float()).unwrap_or(1.0);
                let strength = get(c, "ControlStrength").map(|v| v.float()).unwrap_or(1.0).clamp(0.0, 1.0);
                let s = 1.0 + (scale - 1.0) * strength;
                (s.is_finite() && s > 0.0 && (s - 1.0).abs() > 1e-4).then(|| (bone, s.min(50.0)))
            })
            .collect()
    }

    /// GGGameInfo.mMutatorComponents[0].MutatorComponents.
    pub fn components(&mut self) -> Vec<ObjId> {
        let Some(game) = self.game else { return vec![] };
        let list = self.vm.sym("MutatorComponents");
        match self.vm.get(game, "mMutatorComponents") {
            Value::Array(slots) => match slots.first() {
                Some(Value::Struct(s)) => match s.field(list) {
                    Some(Value::Array(a)) => a.iter().filter_map(|v| v.obj()).collect(),
                    _ => vec![],
                },
                _ => vec![],
            },
            _ => vec![],
        }
    }

    pub fn publish(&mut self) {
        let Some(g) = self.goat else { return };
        let f = |h: &mut Host, k: &str| h.vm.get(g, k).float();
        let v = GoatValues {
            walk_speed: f(self, "mWalkSpeed"),
            sprint_speed: f(self, "mSprintSpeed"),
            strafe_speed: f(self, "mStrafeSpeed"),
            reverse_speed: f(self, "mReverseSpeed"),
            jump_z: f(self, "JumpZ"),
            multi_jump_boost: f(self, "MultiJumpBoost"),
            max_multi_jump: self.vm.get(g, "MaxMultiJump").int(),
            air_control: f(self, "AirControl"),
            wall_run_speed: f(self, "mWallRunSpeed"),
        };
        let base = *self.base.get_or_insert(v);
        *VALUES.write().unwrap() = Some((v, base));
    }
}

/// Script calls nest deeply (a mutator chain recurses once per mutator), so VM entry points run
/// on a thread with a large stack; the interpreter's recursion then never hits a stack overflow
/// before its own depth limit (exec::MAX_DEPTH).
pub fn on_big_stack<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    std::thread::scope(|s| {
        std::thread::Builder::new().name("uvm".into()).stack_size(512 << 20).spawn_scoped(s, f).expect("uvm thread").join().expect("uvm thread panicked")
    })
}

/// GGCameraModeOrbital zoom (min, max, desired; Unreal units) a mutator set, else defaults.
static ZOOM: RwLock<Option<[f32; 3]>> = RwLock::new(None);

pub fn zoom_limits() -> (f32, f32) {
    use crate::goat::cam;
    match *ZOOM.read().unwrap() {
        Some(z) => (z[0], z[1]),
        None => (cam::MIN_ZOOM, cam::MAX_ZOOM),
    }
}

impl Host {
    /// The orbital camera mode's zoom fields, if the script world has one.
    pub fn camera_zoom(&mut self) -> Option<[f32; 3]> {
        let cam = *self.vm.globals.get("camera")?;
        let modes = self.vm.get(cam, "mCameraModes");
        let Value::Array(a) = modes else { return None };
        let orbit = a.get(1)?.obj()?;
        let f = |h: &mut Host, k: &str| h.vm.get(orbit, k).float();
        Some([f(self, "mMinZoomDistance"), f(self, "mMaxZoomDistance"), f(self, "mDesiredZoomDistance")])
    }
}

pub fn set_zoom(z: Option<[f32; 3]>) {
    *ZOOM.write().unwrap() = z;
}

/// The goat's DrawScale from script (Resizable/Tall Goat...): its collision capsule follows.
static GOAT_SCALE: RwLock<f32> = RwLock::new(1.0);

pub fn goat_scale() -> f32 {
    *GOAT_SCALE.read().unwrap()
}

pub fn set_goat_scale(s: f32) {
    *GOAT_SCALE.write().unwrap() = s;
}

/// Clear published values (no mutators running).
pub fn reset() {
    *VALUES.write().unwrap() = None;
    *GOAT_SCALE.write().unwrap() = 1.0;
    *ZOOM.write().unwrap() = None;
}

/// Headless run for the command line: `goat --uvm [workshop ids...]`.
pub fn run_cli(ids: &[String]) -> anyhow::Result<()> {
    let dir = crate::mods::mods_dir().join("workshop");
    let mut mods = workshop_mutators(&dir);
    if !ids.is_empty() {
        mods.retain(|m| ids.iter().any(|i| i == &m.id || m.class_path.eq_ignore_ascii_case(i)));
        // "gm-<FileName>" or "game" picks base-game mutators
        mods.extend(game_mutators().into_iter().filter(|g| ids.iter().any(|i| i == "game" || i.strip_prefix("gm-").is_some_and(|f| f.eq_ignore_ascii_case(&g.class_path)))));
    }
    // UVM_MAP=Maps/Level_01.goat also links a map (its Kismet runs at start)
    let map = std::env::var("UVM_MAP").ok().map(|m| upk::cooked_dir().join(m));
    if mods.is_empty() && map.is_none() {
        anyhow::bail!("no Workshop mutators found in {} (download some with the mod menu, M)", dir.display());
    }
    let mut pk: Vec<PathBuf> = mods.iter().flat_map(|m| m.packages.clone()).collect();
    pk.sort();
    pk.dedup();
    let mut h = Host::with_map(&pk, map.as_deref())?;
    // UVM_DEBUG=Class:prop,... prints the class chain and CDO values
    if let Ok(d) = std::env::var("UVM_DEBUG") {
        for item in d.split(',') {
            let (cn, prop) = item.split_once(':').unwrap_or((item, ""));
            let Some(c) = h.vm.class_by_name(cn) else { println!("no class {cn}"); continue };
            let mut chain = vec![];
            let mut k = Some(c);
            while let Some(ci) = k {
                chain.push(h.vm.classes[ci].path.clone());
                k = h.vm.classes[ci].super_;
            }
            println!("{cn}: {}", chain.join(" -> "));
            let cdo = h.vm.cdo(c);
            if !prop.is_empty() {
                let v = h.vm.get(cdo, prop);
                println!("  default {prop} = {}", h.vm.describe(&v));
            }
        }
    }
    // UVM_STATE=Class:State runs that state's code for 3 s (latent Sleep / goto checks)
    if let Ok(spec) = std::env::var("UVM_STATE") {
        let (cn, st) = spec.split_once(':').unwrap_or((spec.as_str(), "Begin"));
        if let Some(c) = h.vm.class_by_name(cn) {
            let o = h.vm.spawn_actor(c, None);
            let y = h.vm.sym(st);
            h.vm.trace_calls = true;
            h.with_vm(|h| {
                h.vm.call_named(o, "GotoState", vec![Value::Name(y)]);
            });
            let mark = h.vm.log.len();
            h.with_vm(|h| {
                for _ in 0..30 {
                    h.vm.tick_states(0.1);
                }
            });
            let sleeps = h.vm.log[mark..].iter().filter(|l| l.contains("Actor.Sleep(")).count();
            println!("--- state {cn}.{st}: {sleeps} Sleep calls in 3 s, running={}", h.vm.state_runs.contains_key(&o));
        }
    }
    h.vm.echo = true;
    h.publish();
    let defaults = goat_values();
    h.start(&mods);
    // a scripted minute of play: frames with keys and the game events the Rust goat reports
    use super::sync::{GameEvent as E, GoatState};
    let script: &[(&[&str], &[E])] = &[
        (&[], &[E::Ground]),
        (&["SpaceBar"], &[E::Airborne(1.0)]),
        (&["SpaceBar"], &[E::Airborne(2.0)]),
        (&[], &[E::Airborne(2.0), E::TrickMade("GGTrickRotation")]),
        (&["LeftMouseButton"], &[E::Landed, E::Ground, E::UseAbility("GGAbilityHorn"), E::TrickMade("GGTrickAbility")]),
        (&["one"], &[E::Ground, E::Baa]),
        (&["Q"], &[E::Ragdoll(true), E::TrickMade("GGTrickRagdoll")]),
        (&[], &[E::Ragdoll(false), E::Ground]),
    ];
    for (i, (keys, events)) in script.iter().enumerate() {
        let state = GoatState { location: [0.0, 0.0, 100.0], physics: if events.iter().any(|e| matches!(e, E::Airborne(_))) { 2 } else { 1 }, ..Default::default() };
        h.vm.say(format!("== frame {i}: keys {keys:?}"));
        let changes = h.with_vm(|h| {
            h.sync_in(state);
            h.keys(keys);
            for e in events.iter() {
                h.event(e);
            }
            h.tick_now(1.0 / 60.0);
            h.sync_out()
        });
        if changes.velocity.is_some() || changes.location.is_some() || changes.ragdoll.is_some() {
            h.vm.say(format!("== script changed the goat: {changes:?}"));
        }
    }
    // cost of a plain frame (sync + events + ticks), no call tracing
    let t0 = std::time::Instant::now();
    let frames = 120;
    h.vm.echo = false;
    for _ in 0..frames {
        h.with_vm(|h| {
            h.sync_in(GoatState { location: [0.0, 0.0, 100.0], physics: 1, ..Default::default() });
            h.keys(&[]);
            h.event(&E::Ground);
            h.tick_now(1.0 / 60.0);
            h.sync_out()
        });
    }
    println!("--- {frames} frames with {} mutators: {:.3} ms per frame", h.mutators.len(), t0.elapsed().as_secs_f64() * 1000.0 / frames as f64);
    println!("--- script goat values: defaults {defaults:?}");
    println!("---               after mutators {:?}", goat_values());
    let comps: Vec<String> = h.components().iter().map(|&c| h.vm.obj(c).name.clone()).collect();
    println!("--- registered mutator components: {comps:?}");
    println!("--- unsupported (count):");
    for (k, n) in h.vm.native_report().iter().take(40) {
        println!("  {n:5} {k}");
    }
    Ok(())
}

/// Whether a downloaded Workshop item lists mutators in its GoatMod.ini.
pub fn has_mutators(id: &str) -> bool {
    let dir = crate::mods::mods_dir().join("workshop").join(id);
    ["GoatMod.ini", "goatmod.ini"].iter().any(|n| {
        std::fs::read(dir.join(n)).is_ok_and(|b| String::from_utf8_lossy(&b).to_ascii_lowercase().contains("+mallmutators="))
    })
}
