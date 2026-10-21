//! Settings the menus read and write: GGEngine.mSettings (GGSystemSettings: resolutions, named
//! int/float/bool settings), the player's profile (GGProfileSettings: gamma, volumes... stored as
//! OnlineProfileSetting entries, floats as their bits in Data.Value1) and language natives.
//! What the Options view applies is published for the Rust game (`applied()`).

use super::bridge::Host;
use super::exec::Call;
use super::*;
use std::sync::RwLock;

/// Settings the scripts applied (Options -> Apply), for the Rust game.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Applied {
    pub fov: f32,
    pub sfx_volume: f32,
    pub music_volume: f32,
    pub voice_volume: f32,
    /// PlayerInput.MouseSensitivity relative to its default (60).
    pub mouse_scale: f32,
    pub invert_x: bool,
    pub invert_y: bool,
}

static APPLIED: RwLock<Option<Applied>> = RwLock::new(None);

pub fn applied() -> Option<Applied> {
    *APPLIED.read().unwrap()
}

const SDT_INT32: u8 = 1;
const SDT_FLOAT: u8 = 5;

pub fn register(vm: &mut Vm) {
    let n = &mut vm.natives;
    let mut add = |k: &str, f: NativeFn| {
        n.insert(k.to_ascii_lowercase(), f);
    };
    add("object.getlanguage", |_, _| Value::Str("INT".into()));
    add("playercontroller.setaudiogroupvolume", |vm, c| {
        // volumes reach the game through the profile (publish); keep the group value for scripts
        if let (Some(pc), Value::Name(g)) = (c.this, c.arg(0).clone()) {
            let key = format!("uvmAudioGroup_{}", vm.name_of(g));
            vm.set(pc, &key, Value::Float(c.float(1)));
        }
        Value::Int(0)
    });
    add("playercontroller.setallowmaturelanguage", |vm, c| {
        if let Some(&e) = vm.globals.get("engine") {
            vm.set(e, "bAllowMatureLanguage", Value::Bool(c.arg(0).truthy()));
        }
        Value::Int(0)
    });
    add("gameviewportclient.notifysplitscreenlayoutchanged", |_, _| Value::Int(0));
    // UIManager.GetUIManager: one GGUIManager (popups, message boxes), made on first use
    add("uimanager.getuimanager", |vm, _| {
        if let Some(&m) = vm.globals.get("uimanager") {
            return Value::Obj(Some(m));
        }
        let Some(c) = vm.class_by_name("GGUIManager").or_else(|| vm.class_by_name("UIManager")) else { return Value::Obj(None) };
        let m = vm.spawn_object(c, Some("GGUIManager_0".into()));
        vm.obj_mut(m).outer = vm.globals.get("viewport").copied();
        vm.globals.insert("uimanager", m);
        Value::Obj(Some(m))
    });
    add("localplayer.gettranslationcontext", |_, _| Value::Obj(None));
    // --- GGSystemSettings ---
    add("ggsystemsettings.getavailableresolutions", |vm, c| {
        let (w, h) = if vm.gfx.size.0 > 0 { vm.gfx.size } else { (1600, 900) };
        let mut list: Vec<(u32, u32)> = vec![(1280, 720), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160)];
        if !list.contains(&(w, h)) {
            list.push((w, h));
            list.sort();
        }
        let cur = list.iter().position(|&r| r == (w, h)).unwrap_or(0);
        c.args[0] = Value::Array(list.iter().map(|(a, b)| Value::Str(format!("{a} x {b}"))).collect());
        c.args[1] = Value::Int(cur as i32);
        Value::Int(0)
    });
    add("ggsystemsettings.getintsetting", |vm, c| get_setting(vm, c, |v| Value::Int(v.int())));
    add("ggsystemsettings.getfloatsetting", |vm, c| get_setting(vm, c, |v| Value::Float(v.float())));
    add("ggsystemsettings.getboolsetting", |vm, c| get_setting(vm, c, |v| Value::Byte(v.truthy() as u8)));
    add("ggsystemsettings.setintsetting", set_setting);
    add("ggsystemsettings.setfloatsetting", set_setting);
    add("ggsystemsettings.setboolsetting", set_setting);
    add("ggsystemsettings.gettexturedetail", |vm, c| match c.this {
        Some(o) => match vm.get(o, "uvmTextureDetail") {
            Value::Byte(b) => Value::Byte(b),
            _ => Value::Byte(2),
        },
        None => Value::Byte(2),
    });
    add("ggsystemsettings.settexturedetail", |vm, c| {
        if let Some(o) = c.this {
            vm.set(o, "uvmTextureDetail", Value::Byte(c.int(0) as u8));
        }
        Value::Int(0)
    });
    add("ggsystemsettings.savesystemsettings", |vm, _| {
        publish(vm);
        Value::Int(0)
    });
    for k in ["ggsystemsettings.resetlastsavedsettings", "ggsystemsettings.setapexdetaillevel", "ggsystemsettings.setlanguage", "ggengine.applygraphicsettings"] {
        add(k, |_, _| Value::Int(0));
    }
    add("ggengine.saveprogression", |_, _| Value::Bool(true));
    // --- OnlinePlayerStorage / OnlineProfileSettings ---
    add("onlineplayerstorage.getprofilesettingvaluefloat", |vm, c| {
        let Some(o) = c.this else { return Value::Bool(false) };
        match find_setting(vm, o, c.int(0)) {
            Some((ty, v1)) => {
                c.args[1] = Value::Float(if ty == SDT_FLOAT { f32::from_bits(v1 as u32) } else { v1 as f32 });
                Value::Bool(true)
            }
            None => Value::Bool(false),
        }
    });
    add("onlineplayerstorage.getprofilesettingvalueint", |vm, c| {
        let Some(o) = c.this else { return Value::Bool(false) };
        match find_setting(vm, o, c.int(0)) {
            Some((ty, v1)) => {
                c.args[1] = Value::Int(if ty == SDT_FLOAT { f32::from_bits(v1 as u32) as i32 } else { v1 });
                Value::Bool(true)
            }
            None => Value::Bool(false),
        }
    });
    add("onlineplayerstorage.getprofilesettingvalueid", |vm, c| {
        let Some(o) = c.this else { return Value::Bool(false) };
        match find_setting(vm, o, c.int(0)) {
            Some((_, v1)) => {
                c.args[1] = Value::Int(v1);
                Value::Bool(true)
            }
            None => Value::Bool(false),
        }
    });
    add("onlineplayerstorage.setprofilesettingvaluefloat", |vm, c| {
        let Some(o) = c.this else { return Value::Bool(false) };
        let ok = write_setting(vm, o, c.int(0), SDT_FLOAT, c.float(1).to_bits() as i32);
        publish(vm);
        Value::Bool(ok)
    });
    add("onlineplayerstorage.setprofilesettingvalueint", |vm, c| {
        let Some(o) = c.this else { return Value::Bool(false) };
        Value::Bool(write_setting(vm, o, c.int(0), SDT_INT32, c.int(1)))
    });
    add("onlineplayerstorage.setprofilesettingvalueid", |vm, c| {
        let Some(o) = c.this else { return Value::Bool(false) };
        Value::Bool(write_setting(vm, o, c.int(0), SDT_INT32, c.int(1)))
    });
}

fn setting_key(c: &Call) -> String {
    let name: String = c.arg(0).str().chars().filter(|ch| ch.is_ascii_alphanumeric()).collect();
    format!("uvmSetting_{}", name.to_ascii_lowercase())
}

/// Named settings start from the window state the game runs with (windowed, vsync on).
fn default_setting(name: &str) -> Option<Value> {
    match name.to_ascii_lowercase().as_str() {
        s if s.contains("fullscreen") => Some(Value::Bool(false)),
        s if s.contains("vsync") => Some(Value::Bool(true)),
        _ => None,
    }
}

fn get_setting(vm: &mut Vm, c: &mut Call, conv: fn(&Value) -> Value) -> Value {
    let Some(o) = c.this else { return Value::Bool(false) };
    let key = setting_key(c);
    let stored = vm.syms_get(&key).and_then(|y| vm.obj(o).props.get(&y).cloned());
    match stored.or_else(|| default_setting(c.arg(0).str())) {
        Some(v) => {
            c.args[1] = conv(&v);
            Value::Bool(true)
        }
        None => Value::Bool(false),
    }
}

fn set_setting(vm: &mut Vm, c: &mut Call) -> Value {
    let Some(o) = c.this else { return Value::Bool(false) };
    let key = setting_key(c);
    let v = c.arg(1).clone();
    vm.set(o, &key, v);
    Value::Bool(true)
}

/// The profile's ProfileSettings, filled from DefaultSettings the first time (SetToDefaults).
fn settings_list(vm: &mut Vm, o: ObjId) -> Vec<Value> {
    if let Value::Array(a) = vm.get(o, "ProfileSettings") {
        if !a.is_empty() {
            return a;
        }
    }
    let d = match vm.get(o, "DefaultSettings") {
        Value::Array(a) => a,
        _ => vec![],
    };
    vm.set(o, "ProfileSettings", Value::Array(d.clone()));
    d
}

fn field<'a>(vm: &Vm, v: &'a Value, name: &str) -> Option<&'a Value> {
    let Value::Struct(s) = v else { return None };
    s.fields.iter().find(|f| vm.name_of(f.0).eq_ignore_ascii_case(name)).map(|f| &f.1)
}

/// (Data.Type, Data.Value1) of a profile setting.
fn find_setting(vm: &mut Vm, o: ObjId, id: i32) -> Option<(u8, i32)> {
    let list = settings_list(vm, o);
    list.iter().find_map(|e| {
        let p = field(vm, e, "ProfileSetting")?;
        (field(vm, p, "PropertyId")?.int() == id).then_some(())?;
        let d = field(vm, p, "Data")?;
        Some((field(vm, d, "Type").map(|v| v.int() as u8).unwrap_or(0), field(vm, d, "Value1").map(|v| v.int()).unwrap_or(0)))
    })
}

fn write_setting(vm: &mut Vm, o: ObjId, id: i32, ty: u8, v1: i32) -> bool {
    let mut list = settings_list(vm, o);
    let (ps, pid, data, typ, val) = (vm.sym("ProfileSetting"), vm.sym("PropertyId"), vm.sym("Data"), vm.sym("Type"), vm.sym("Value1"));
    for e in &mut list {
        let Value::Struct(s) = e else { continue };
        let Value::Struct(p) = s.field_mut(ps) else { continue };
        if p.field(pid).map(|v| v.int()) != Some(id) {
            continue;
        }
        if let Value::Struct(d) = p.field_mut(data) {
            *d.field_mut(typ) = Value::Byte(ty);
            *d.field_mut(val) = Value::Int(v1);
        }
        vm.set(o, "ProfileSettings", Value::Array(list));
        return true;
    }
    false
}

/// Publish FOV and volumes from the engine settings and the player's profile (Options -> Apply:
/// also saved).
fn publish(vm: &mut Vm) {
    publish_with(vm, true)
}

fn publish_with(vm: &mut Vm, save_it: bool) {
    let Some(&engine) = vm.globals.get("engine") else { return };
    let fov = vm.get(engine, "mSettings").obj().map(|s| vm.get(s, "mFOV").float()).filter(|f| *f > 1.0).unwrap_or(70.0);
    let profile = vm.globals.get("profile").copied();
    let mut vol = |id: i32| profile.and_then(|p| find_setting(vm, p, id)).map(|(t, v)| if t == SDT_FLOAT { f32::from_bits(v as u32) } else { v as f32 }).unwrap_or(1.0);
    // GGProfileSettings CPSI_SOUND_EFFECTS 104, CPSI_VOICE 105, CPSI_MUSIC 106
    let (sfx_volume, voice_volume, music_volume) = (vol(104), vol(105), vol(106));
    let input = vm.globals.get("goat").copied().and_then(|g| vm.get(g, "Controller").obj()).and_then(|pc| vm.get(pc, "PlayerInput").obj());
    let (mut mouse_scale, mut invert_x, mut invert_y) = (1.0, false, false);
    if let Some(pi) = input {
        let sens = vm.get(pi, "MouseSensitivity").float();
        if sens > 0.0 {
            mouse_scale = sens / 60.0;
        }
        invert_x = vm.get(pi, "bInvertTurn").truthy();
        invert_y = vm.get(pi, "bInvertMouse").truthy();
    }
    let a = Applied { fov, sfx_volume, voice_volume, music_volume, mouse_scale, invert_x, invert_y };
    *APPLIED.write().unwrap() = Some(a);
    if save_it {
        save(vm);
    }
}

fn settings_file() -> std::path::PathBuf {
    super::progress::save_dir().join("settings.json")
}

/// Options -> Apply: profile settings (id -> type, value), engine settings (mFOV + named
/// settings), mouse sensitivity and inversion, as save/settings.json.
fn save(vm: &mut Vm) {
    let mut profile = serde_json::Map::new();
    if let Some(p) = vm.globals.get("profile").copied() {
        for e in settings_list(vm, p) {
            let Some(ps) = field(vm, &e, "ProfileSetting") else { continue };
            let Some(id) = field(vm, ps, "PropertyId").map(|v| v.int()) else { continue };
            let Some(d) = field(vm, ps, "Data") else { continue };
            let ty = field(vm, d, "Type").map(|v| v.int()).unwrap_or(0);
            let v1 = field(vm, d, "Value1").map(|v| v.int()).unwrap_or(0);
            profile.insert(id.to_string(), serde_json::json!([ty, v1]));
        }
    }
    let mut engine = serde_json::Map::new();
    if let Some(st) = vm.globals.get("engine").copied().and_then(|e| vm.get(e, "mSettings").obj()) {
        engine.insert("mFOV".into(), serde_json::json!(vm.get(st, "mFOV").float()));
        let keys: Vec<super::Sym> = vm.obj(st).props.keys().copied().collect();
        for k in keys {
            let name = vm.name_of(k).to_string();
            if !name.starts_with("uvmSetting_") {
                continue;
            }
            let j = match &vm.obj(st).props[&k] {
                Value::Bool(b) => serde_json::json!(b),
                Value::Int(i) => serde_json::json!(i),
                Value::Float(f) => serde_json::json!(f),
                Value::Str(t) => serde_json::json!(t),
                _ => continue,
            };
            engine.insert(name, j);
        }
    }
    let mut input = serde_json::Map::new();
    if let Some(pi) = vm.globals.get("goat").copied().and_then(|g| vm.get(g, "Controller").obj()).and_then(|pc| vm.get(pc, "PlayerInput").obj()) {
        input.insert("MouseSensitivity".into(), serde_json::json!(vm.get(pi, "MouseSensitivity").float()));
        input.insert("bInvertTurn".into(), serde_json::json!(vm.get(pi, "bInvertTurn").truthy()));
        input.insert("bInvertMouse".into(), serde_json::json!(vm.get(pi, "bInvertMouse").truthy()));
    }
    let all = serde_json::json!({ "profile": profile, "engine": engine, "input": input });
    let _ = std::fs::create_dir_all(super::progress::save_dir());
    let _ = std::fs::write(settings_file(), serde_json::to_string_pretty(&all).unwrap_or_default());
    vm.say("settings saved".into());
}

/// save/settings.json back into the profile, engine settings and player input.
fn load(vm: &mut Vm) {
    let Ok(text) = std::fs::read_to_string(settings_file()) else { return };
    let Ok(j) = serde_json::from_str::<serde_json::Value>(&text) else { return };
    if let (Some(p), Some(profile)) = (vm.globals.get("profile").copied(), j.get("profile").and_then(|v| v.as_object())) {
        for (id, v) in profile {
            let (Ok(id), Some(a)) = (id.parse::<i32>(), v.as_array()) else { continue };
            let ty = a.first().and_then(|x| x.as_i64()).unwrap_or(0) as u8;
            let v1 = a.get(1).and_then(|x| x.as_i64()).unwrap_or(0) as i32;
            write_setting(vm, p, id, ty, v1);
        }
    }
    if let (Some(st), Some(engine)) = (vm.globals.get("engine").copied().and_then(|e| vm.get(e, "mSettings").obj()), j.get("engine").and_then(|v| v.as_object())) {
        for (k, v) in engine {
            let val = match v {
                serde_json::Value::Bool(b) => Value::Bool(*b),
                serde_json::Value::Number(n) if k == "mFOV" => Value::Float(n.as_f64().unwrap_or(70.0) as f32),
                serde_json::Value::Number(n) if n.is_i64() => Value::Int(n.as_i64().unwrap_or(0) as i32),
                serde_json::Value::Number(n) => Value::Float(n.as_f64().unwrap_or(0.0) as f32),
                serde_json::Value::String(t) => Value::Str(t.clone()),
                _ => continue,
            };
            vm.set(st, k, val);
        }
    }
    if let (Some(pi), Some(input)) = (vm.globals.get("goat").copied().and_then(|g| vm.get(g, "Controller").obj()).and_then(|pc| vm.get(pc, "PlayerInput").obj()), j.get("input").and_then(|v| v.as_object())) {
        if let Some(f) = input.get("MouseSensitivity").and_then(|v| v.as_f64()) {
            vm.set(pi, "MouseSensitivity", Value::Float(f as f32));
        }
        for k in ["bInvertTurn", "bInvertMouse"] {
            if let Some(b) = input.get(k).and_then(|v| v.as_bool()) {
                vm.set(pi, k, Value::Bool(b));
            }
        }
    }
    vm.say("settings loaded from save/settings.json".into());
}

impl Host {
    /// Engine settings object and the player's profile, as GGEngine.Init / the profile provider
    /// would set them up (the Options view reads both).
    pub fn setup_settings(&mut self) {
        let Some(&engine) = self.vm.globals.get("engine") else { return };
        // LocalPlayers live in the engine (PlayerController.IsSplitscreenPlayer reads LP.Outer)
        if let Value::Array(players) = self.vm.get(engine, "GamePlayers") {
            for lp in players.iter().filter_map(|v| v.obj()) {
                if self.vm.obj(lp).outer.is_none() {
                    self.vm.obj_mut(lp).outer = Some(engine);
                }
            }
        }
        if self.vm.get(engine, "mSettings").obj().is_none() {
            if let Some(c) = self.vm.class_by_name("GGSystemSettings") {
                let s = self.vm.spawn_object(c, Some("GGSystemSettings_0".into()));
                self.vm.obj_mut(s).outer = Some(engine);
                self.vm.set(engine, "mSettings", Value::Obj(Some(s)));
            }
        }
        if let (Some(pc), Some(c)) = (self.player_controller(), self.vm.class_by_name("GGProfileSettings")) {
            if self.vm.get(pc, "mProfileSettings").obj().is_none() {
                let p = self.vm.spawn_object(c, Some("GGProfileSettings_0".into()));
                settings_list(&mut self.vm, p);
                self.vm.set(pc, "mProfileSettings", Value::Obj(Some(p)));
                self.vm.globals.insert("profile", p);
            }
        }
        // saved Options, applied at start (FOV, volumes, mouse) without saving them again
        if std::env::var_os("GOAT_NO_SAVED_SETTINGS").is_none() && settings_file().exists() {
            load(&mut self.vm);
            publish_with(&mut self.vm, false);
        }
    }
}

/// A pending `setres` from the Options menu: (width, height, fullscreen).
static SETRES: std::sync::Mutex<Option<(u32, u32, Option<bool>)>> = std::sync::Mutex::new(None);

/// Parse "1920x1080f" / "1280x720w" / "1600x900".
pub fn parse_setres(s: &str) -> Option<(u32, u32, Option<bool>)> {
    let (w, rest) = s.split_once('x')?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let mode = match rest[digits.len()..].chars().next() {
        Some('f') | Some('F') => Some(true),
        Some('w') | Some('W') => Some(false),
        _ => None,
    };
    Some((w.trim().parse().ok()?, digits.parse().ok()?, mode))
}

pub fn request_setres(s: &str) {
    if let (Some(r), Ok(mut g)) = (parse_setres(s), SETRES.lock()) {
        *g = Some(r);
    }
}

/// Apply a pending setres to the window (not in test runs: their window stays put).
pub fn apply_setres(args: bevy::prelude::Res<crate::Args>, mut windows: bevy::prelude::Query<&mut bevy::window::Window, bevy::prelude::With<bevy::window::PrimaryWindow>>) {
    let Some((w, h, full)) = SETRES.lock().ok().and_then(|mut g| g.take()) else { return };
    if args.test.is_some() {
        bevy::log::info!("setres {w}x{h} {full:?} (not applied in tests)");
        return;
    }
    let Ok(mut win) = windows.single_mut() else { return };
    win.resolution.set(w as f32, h as f32);
    if let Some(f) = full {
        win.mode = if f {
            bevy::window::WindowMode::BorderlessFullscreen(bevy::window::MonitorSelection::Current)
        } else {
            bevy::window::WindowMode::Windowed
        };
    }
    bevy::log::info!("setres {w}x{h} fullscreen {full:?}");
}

#[cfg(test)]
mod setres_tests {
    #[test]
    fn parses_unreal_setres() {
        assert_eq!(super::parse_setres("1920x1080f"), Some((1920, 1080, Some(true))));
        assert_eq!(super::parse_setres("1280x720w"), Some((1280, 720, Some(false))));
        assert_eq!(super::parse_setres("1600x900"), Some((1600, 900, None)));
    }
}
