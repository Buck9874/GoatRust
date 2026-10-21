//! The game's Scaleform menus in the host: open them through the original scripts
//! (GGPlayerControllerGame.ShowPauseMenu -> GGHUDGfxMenuPause -> GGUIViewPauseMenu), tick them,
//! and the headless check `goat --uvm-menu [pause] [out.png]`.

use super::bridge::Host;
use super::*;

impl Host {
    /// The local player's controller (GGPlayerControllerGame).
    pub fn player_controller(&mut self) -> Option<ObjId> {
        let goat = self.goat?;
        self.vm.get(goat, "Controller").obj()
    }

    /// Escape in game: GGPlayerControllerGame.ShowPauseMenu (exec function, as the
    /// "ShowPauseMenu" key binding runs it). Returns whether a menu is open afterwards.
    pub fn open_pause_menu(&mut self) -> bool {
        let Some(pc) = self.player_controller() else { return false };
        self.vm.call_named(pc, "ShowPauseMenu", vec![]);
        gfx::menu_open(&self.vm)
    }

    /// The in-game HUD movie the way the game makes it: GGPlayerControllerGame.ClientSetHUD sets a
    /// timer for StartHUD -> GGHUD.StartHUD (new mHUDClass, Init -> Start, root.mRoot widgets).
    pub fn start_hud(&mut self) -> bool {
        let Some(pc) = self.player_controller() else { return false };
        let trace = std::mem::replace(&mut self.vm.trace_calls, std::env::var_os("GOAT_TRACE_HUD").is_some());
        self.vm.call_named(pc, "StartHUD", vec![]);
        self.vm.trace_calls = trace;
        self.hud_movie().is_some()
    }

    /// GGHUD(myHUD).mHUDMovie while it is open.
    pub fn hud_movie(&mut self) -> Option<ObjId> {
        let pc = self.player_controller()?;
        let hud = self.vm.get(pc, "myHUD").obj()?;
        let m = self.vm.get(hud, "mHUDMovie").obj()?;
        self.vm.gfx.order.contains(&m).then_some(m)
    }

    /// What GGComboManager / GGHUD.DrawHUD do with the HUD movie, for the Rust combo system:
    /// AddCombo per trick, EndCombo + UpdateScore when a combo is banked (GGGameInfo.GetScore
    /// reads mTotalScoreData), and every frame UpdateComboBar + TickScore.
    pub fn hud_feed(&mut self, events: &[crate::score::HudEvent], combo_timer: (f32, f32)) {
        use crate::score::HudEvent;
        // GGComboManager.EndCurrentCombo -> GGGameInfo.AddScore (OnNewScore to the quests) and
        // OnEndCombo (score/combo quests, achievements), with or without the HUD movie
        if let Some(game) = self.game {
            for ev in events {
                if let HudEvent::ComboEnd { total, combo } = ev {
                    self.set_total_score(*total);
                    self.vm.call_named(game, "OnEndCombo", vec![Value::Int(*combo)]);
                    if let Some(q) = self.quests {
                        self.vm.call_named(q, "OnNewScore", vec![Value::Int((*total).min(i32::MAX as i64) as i32)]);
                    }
                }
            }
        }
        let Some(m) = self.hud_movie() else { return };
        for ev in events {
            match ev {
                HudEvent::Trick { name, clean_name, has_name, frequency, trick_score, score, combo_score, multiplier, eligible } => {
                    let c = self.vm.obj(m).class;
                    let Some(f) = self.vm.defines(c, "AddCombo") else { continue };
                    let Some(Ty::Struct(sd)) = self.vm.funcs[f].params.first().map(|p| p.ty.clone()) else { continue };
                    let mut ti = self.vm.struct_zero(sd);
                    let mut put = |vm: &mut Vm, k: &str, v: Value| *ti.field_mut(vm.sym(k)) = v;
                    put(&mut self.vm, "TrickName", Value::Str(name.clone()));
                    put(&mut self.vm, "CleanTrickName", Value::Str(clean_name.clone()));
                    put(&mut self.vm, "HasName", Value::Bool(*has_name));
                    put(&mut self.vm, "Frequency", Value::Int(*frequency as i32));
                    put(&mut self.vm, "TrickScore", Value::Int(*trick_score));
                    put(&mut self.vm, "BaseTrickScore", Value::Int(*score));
                    put(&mut self.vm, "MaxFrequency", Value::Int(4));
                    let args = vec![
                        Value::Struct(Box::new(ti)),
                        Value::Int(*score),
                        Value::Int(*combo_score),
                        Value::Float(*multiplier),
                        Value::Bool(*eligible),
                        Value::Str(name.clone()),
                    ];
                    self.vm.call_named(m, "AddCombo", args);
                }
                HudEvent::ComboEnd { total, .. } => {
                    self.vm.call_named(m, "EndCombo", vec![]);
                    self.vm.call_named(m, "UpdateScore", vec![]);
                }
                HudEvent::BreakCombo(msg) => {
                    self.vm.call_named(m, "BreakCombo", vec![Value::Str(msg.clone())]);
                }
                HudEvent::ManualStart => {
                    self.vm.call_named(m, "StartManual", vec![]);
                }
                HudEvent::ManualStop => {
                    self.vm.call_named(m, "StopManual", vec![]);
                }
                HudEvent::Manual { value, score } => {
                    if let Some(w) = self.vm.get(m, "mManualWidget").obj() {
                        self.vm.call_named(w, "UpdatePluppPosition", vec![Value::Int(*value as i32)]);
                    }
                    self.vm.call_named(m, "UpdateManualScore", vec![Value::Int(*score)]);
                }
            }
        }
        self.vm.call_named(m, "UpdateComboBar", vec![Value::Float(combo_timer.0), Value::Float(combo_timer.1)]);
        self.vm.call_named(m, "TickScore", vec![]);
    }

    /// GGGameInfo.mTotalScoreData = [{PC, Score}] (what GetScore(PC) returns).
    fn set_total_score(&mut self, total: i64) {
        let (Some(game), Some(pc)) = (self.game, self.player_controller()) else { return };
        let gc = self.vm.obj(game).class;
        let y = self.vm.sym("mTotalScoreData");
        let ty = self.vm.classes[gc].props.iter().find(|p| p.name == y).map(|p| p.ty.clone());
        let ty = ty.or_else(|| {
            let mut c = self.vm.classes[gc].super_;
            while let Some(k) = c {
                if let Some(p) = self.vm.classes[k].props.iter().find(|p| p.name == y) {
                    return Some(p.ty.clone());
                }
                c = self.vm.classes[k].super_;
            }
            None
        });
        let Some(Ty::Array(inner)) = ty else { return };
        let Ty::Struct(sd) = *inner else { return };
        let mut e = self.vm.struct_zero(sd);
        *e.field_mut(self.vm.sym("PC")) = Value::Obj(Some(pc));
        *e.field_mut(self.vm.sym("Score")) = Value::Int(total.min(i32::MAX as i64) as i32);
        self.vm.set(game, "mTotalScoreData", Value::Array(vec![Value::Struct(Box::new(e))]));
    }

    /// The pause menu object (GGHUDGfxMenuPause) while it is open.
    pub fn pause_menu(&mut self) -> Option<ObjId> {
        let pc = self.player_controller()?;
        let m = self.vm.get(pc, "mPauseMenu").obj()?;
        self.vm.gfx.order.contains(&m).then_some(m)
    }

    /// Menu frame: advance the open movies (PostWidgetInit, OnPostAdvance).
    /// Fire the timers that came due: UI objects' (`ui`: menu views, set on the player
    /// controller, which UE3 keeps ticking while the game is paused) or the world's.
    pub fn run_timers(&mut self, dt: f32, ui: bool) {
        let actor = self.vm.class_by_name("Actor");
        let is_ui = |vm: &Vm, o: ObjId| actor.is_some_and(|a| !vm.is_child_of(vm.obj(o).class, a));
        let mut fire = vec![];
        let vm = &mut self.vm;
        let flags: Vec<bool> = vm.timers.iter().map(|t| is_ui(vm, t.obj) == ui).collect();
        for (t, mine) in vm.timers.iter_mut().zip(&flags) {
            if !*mine {
                continue;
            }
            t.left -= dt;
            if t.left <= 0.0 {
                fire.push((t.obj, t.func));
                // looping timers re-arm; one-shots are done
                t.left = if t.looping { t.left + t.rate } else { f32::NEG_INFINITY };
            }
        }
        vm.timers.retain(|t| t.left > f32::NEG_INFINITY);
        for (o, f) in fire {
            let n = self.vm.name_of(f).to_string();
            self.vm.call_named(o, &n, vec![]);
        }
    }

    pub fn tick_menus(&mut self, dt: f32) {
        self.run_timers(dt, true);
        let failed: Vec<ObjId> = super::natives_engine::RESOLVE_FAILED
            .lock()
            .map(|mut q| {
                q.iter_mut().for_each(|e| e.1 -= dt);
                let due = q.iter().filter(|e| e.1 <= 0.0).map(|e| e.0).collect();
                q.retain(|e| e.1 > 0.0);
                due
            })
            .unwrap_or_default();
        for o in failed {
            if !self.vm.obj(o).destroyed {
                self.vm.call_named(o, "ResolveFailed", vec![]);
            }
        }
        if !self.vm.gfx.order.is_empty() {
            if self.vm.frames.is_empty() {
                self.vm.budget = exec::BUDGET;
                self.vm.aborted = false;
            }
            gfx::tick(&mut self.vm, dt);
        }
    }

    /// GGUIViewMainMenu.SubPostWidgetInit places the news text area at (1385, 200); when the text
    /// area's own widget init lands after that call it stays at (0, 0), on top of the buttons, and
    /// takes their clicks. Re-apply the script's position while the menu is new.
    pub fn settle_main_menu(&mut self) {
        let Some(mm) = self.main_menu() else { return };
        if self.vm.gfx.movies.get(&mm).is_none_or(|s| s.frames > 240) {
            return;
        }
        let Value::Array(stack) = self.vm.get(mm, "mStack") else { return };
        for v in stack.iter().filter_map(|v| v.obj()) {
            if let Some(area) = self.vm.get(v, "mNewsArea").obj() {
                self.vm.call_named(area, "SetPosition", vec![Value::Float(1385.0), Value::Float(200.0)]);
            }
        }
    }

    /// The main menu (GGHUD.StartMainMenu -> GGHUDGfxMenuMain -> GGUIViewMainMenu), as the
    /// menu level's HUD opens it.
    pub fn open_main_menu(&mut self) -> bool {
        let Some(pc) = self.player_controller() else { return false };
        let Some(hud) = self.vm.get(pc, "myHUD").obj() else { return false };
        self.workshop_start();
        self.vm.call_named(hud, "StartMainMenu", vec![]);
        self.main_menu().is_some()
    }

    /// GGHUD(myHUD).mMainMenuMovie while it is open.
    pub fn main_menu(&mut self) -> Option<ObjId> {
        let pc = self.player_controller()?;
        let hud = self.vm.get(pc, "myHUD").obj()?;
        let m = self.vm.get(hud, "mMainMenuMovie").obj()?;
        self.vm.gfx.order.contains(&m).then_some(m)
    }

    /// Screen rectangles of the pause menu's buttons (label, x, y, w, h in viewport pixels).
    pub fn pause_buttons(&mut self) -> Vec<(String, [f32; 4])> {
        match self.pause_menu() {
            Some(menu) => self.menu_buttons(menu),
            None => vec![],
        }
    }

    /// Buttons (GGUIView.mMenuButtons) of a GGHUDGfxMenu's top view.
    pub fn menu_buttons(&mut self, menu: ObjId) -> Vec<(String, [f32; 4])> {
        let Value::Array(stack) = self.vm.get(menu, "mStack") else { return vec![] };
        let Some(view) = stack.last().and_then(|v| v.obj()) else { return vec![] };
        let Value::Array(buttons) = self.vm.get(view, "mMenuButtons") else { return vec![] };
        let mut out = vec![];
        for b in buttons.iter().filter_map(|v| v.obj()) {
            let label = match self.vm.call_named(b, "GetString", vec![Value::Str("label".into())]) {
                Some(Value::Str(s)) => s,
                _ => String::new(),
            };
            if let Some(r) = gfx::screen_rect(&mut self.vm, b) {
                out.push((label, r));
            }
        }
        out
    }
}

/// `goat --uvm-menu [pause] [out.png]`: open a menu headless through the scripts, run it for a
/// second, click its first button (Resume) and save the picture.
pub fn run_cli(args: &[String]) -> anyhow::Result<()> {
    let which = args.first().map(|s| s.as_str()).unwrap_or("pause");
    let out = args.get(1).cloned().unwrap_or_else(|| "out/menu_pause.png".into());
    let mut h = Host::with_map(&[], None)?;
    h.vm.echo = std::env::var_os("UVM_ECHO").is_some();
    h.vm.gfx.log_calls = std::env::var_os("UVM_GFX_LOG").is_some();
    let t = std::time::Instant::now();
    let opened = h.with_vm(|h| {
        gfx::set_size(&mut h.vm, 1280, 720);
        h.vm.trace_calls = std::env::var_os("UVM_TRACE").is_some();
        let ok = match which {
            "pause" => h.open_pause_menu(),
            "hud" => h.start_hud(),
            "main" => h.open_main_menu(),
            other => {
                h.vm.say(format!("unknown menu {other}"));
                false
            }
        };
        for _ in 0..30 {
            h.tick_menus(1.0 / 30.0);
        }
        for m in h.vm.gfx.order.clone() {
            let ci = h.vm.get(m, "bCaptureInput");
            let cm = h.vm.get(m, "bCaptureMouseInput");
            let ig = h.vm.get(m, "bIgnoreMouseInput");
            let name = h.vm.obj(m).name.clone();
            h.vm.say(format!("GFx movie {name}: bCaptureInput {ci} bCaptureMouseInput {cm} bIgnoreMouseInput {ig}"));
        }
        ok
    });
    println!("--- {which} menu open: {opened} ({:.0} ms)", t.elapsed().as_secs_f32() * 1000.0);
    for l in h.vm.log.iter().filter(|l| l.contains("GFx") || l.contains("WARN") || l.contains("ScriptTrace")).take(60) {
        println!("  {l}");
    }
    if let Ok(tex) = std::env::var("UVM_GFX_IMG") {
        let r = h.with_vm(|h| gfx::probe_image(&mut h.vm, &tex));
        println!("--- img://{tex}: {r:?}");
    }
    let buttons = h.with_vm(|h| match h.main_menu() {
        Some(mm) => h.menu_buttons(mm),
        None => h.pause_buttons(),
    });
    for (label, r) in &buttons {
        println!("  button {label:?} at {:.0},{:.0} {:.0}x{:.0}", r[0], r[1], r[2], r[3]);
    }
    let imgs = h.with_vm(|h| gfx::capture(&mut h.vm));
    if let Some((_, img)) = imgs.last() {
        let mut bg = image::RgbaImage::from_pixel(img.width(), img.height(), image::Rgba([90, 120, 150, 255]));
        image::imageops::overlay(&mut bg, img, 0, 0);
        bg.save(&out)?;
        for (x, y) in [(1000u32, 300u32), (100, 84), (640, 600)] {
            println!("  raw pixel {x},{y} = {:?}", img.get_pixel(x.min(img.width() - 1), y.min(img.height() - 1)));
        }
        println!("--- saved {out}");
    }
    // click the first button (Resume): the menu must close through the scripts
    if let Some((label, r)) = buttons.first().cloned() {
        let (x, y) = (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
        let open_after = h.with_vm(|h| {
            if std::env::var_os("UVM_GFX_PROBE").is_some() {
                gfx::probe_mouse_events(&mut h.vm);
            }
            gfx::mouse_move(&mut h.vm, x, y);
            h.tick_menus(1.0 / 30.0);
            gfx::mouse_button(&mut h.vm, x, y, true);
            h.tick_menus(1.0 / 30.0);
            gfx::mouse_button(&mut h.vm, x, y, false);
            h.tick_menus(1.0 / 30.0);
            gfx::menu_open(&h.vm)
        });
        println!("--- clicked {label:?} at {x:.0},{y:.0}: menu open afterwards: {open_after}");
    }
    let _ = std::fs::write("out/uvm_menu_log.txt", h.vm.log.join("
"));
    println!("--- unsupported (count):");
    for (k, n) in h.vm.native_report().iter().take(40) {
        println!("  {n:5} {k}");
    }
    Ok(())
}

// --- in game -------------------------------------------------------------------------------------

use bevy::asset::RenderAssetUsages;
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::PrimaryWindow;
use std::sync::atomic::{AtomicBool, Ordering};

static UI_OPEN: AtomicBool = AtomicBool::new(false);

/// A Scaleform menu has the keyboard and mouse (the goat and camera ignore input).
static HUD_FRAME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn ui_open() -> bool {
    UI_OPEN.load(Ordering::Relaxed)
}

/// Synthetic menu input (tests) and what the menu system reports back.
#[derive(Resource, Default)]
pub struct MenuTest {
    pub open_pause: bool,
    /// Mouse events in viewport pixels: (x, y, Some(down) | None = move).
    pub mouse: Vec<(f32, f32, Option<bool>)>,
    /// Keys (Unreal names) pressed/released for the open menu.
    pub keys: Vec<(&'static str, bool)>,
    /// Filled while a menu is open when `want_buttons` is set.
    pub want_buttons: bool,
    pub buttons: Vec<(String, [f32; 4])>,
    pub open: bool,
    pub opened_count: u32,
}

/// One drawn movie layer (a full-window ImageNode).
#[derive(Default)]
struct Layer {
    /// The movie last drawn into it.
    movie: Option<ObjId>,
    image: Option<Handle<Image>>,
    node: Option<Entity>,
    size: (u32, u32),
}

#[derive(Resource, Default)]
pub struct MenuView {
    /// One per open movie, back to front: movies that only draw (the HUD) first, then the menus
    /// in the order they opened (a popup over the pause menu over the HUD).
    layers: Vec<Layer>,
    cursor: Option<Vec2>,
    paused: bool,
    /// Keys pressed while the menu was open: only their releases reach it (the Escape that
    /// opened the pause menu must not close it again on release).
    down: Vec<&'static str>,
    /// Cost of this system (seconds, frames) since the last log line.
    cost: (f32, u32, f32),
    /// Real time not yet given to the movies (they advance at most ~120 times a second: at
    /// thousands of game frames a second, steps of 0.1 ms left CLIK widgets stuck mid-setup).
    acc: f32,
}

#[derive(Component)]
pub struct MenuOverlay(usize);

static HUD_SHOWN: AtomicBool = AtomicBool::new(false);

/// The game's Scaleform HUD is drawn (the debug score text steps aside).
pub fn hud_shown() -> bool {
    HUD_SHOWN.load(Ordering::Relaxed)
}

const UI_KEYS: [(KeyCode, &str); 9] = [
    (KeyCode::Escape, "Escape"),
    (KeyCode::Enter, "Enter"),
    (KeyCode::ArrowUp, "Up"),
    (KeyCode::ArrowDown, "Down"),
    (KeyCode::ArrowLeft, "Left"),
    (KeyCode::ArrowRight, "Right"),
    (KeyCode::Tab, "Tab"),
    (KeyCode::Space, "SpaceBar"),
    (KeyCode::Backspace, "BackSpace"),
];

/// The HUD movie (GGHUDGfxInGame, started with the host) is drawn every frame and fed by the
/// Rust combo system. Escape opens the pause menu (GGPlayerControllerGame.ShowPauseMenu). While a
/// menu is open the newest one gets the mouse and keys, menus advance with real time, and every
/// open movie is drawn over the game (HUD, then menus in opening order: a popup keeps the pause
/// menu visible under its dim background); the game itself is paused (virtual time) under them.
#[allow(clippy::too_many_arguments)]
pub fn scaleform_menus(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    scroll: Res<AccumulatedMouseScroll>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mod_menu: Option<Res<crate::mods::ModMenu>>,
    scripted: Option<Res<crate::goat::ScriptedInput>>,
    real: Res<Time<Real>>,
    mut vtime: ResMut<Time<Virtual>>,
    mut m: ResMut<super::plugin::Mutators>,
    mut view: ResMut<MenuView>,
    mut test: ResMut<MenuTest>,
    mut score: ResMut<crate::score::Score>,
    mut images: ResMut<Assets<Image>>,
    mut overlay: Query<(&mut Visibility, &MenuOverlay)>,
    mut commands: Commands,
) {
    let _prof_scope = crate::prof::scope("menus::scaleform_menus");
    let Some(h) = m.host.as_mut() else {
        UI_OPEN.store(false, Ordering::Relaxed);
        HUD_SHOWN.store(false, Ordering::Relaxed);
        score.hud.clear();
        for (mut v, _) in &mut overlay {
            *v = Visibility::Hidden;
        }
        return;
    };
    let Ok(win) = windows.single() else { return };
    let scale = win.resolution.scale_factor();
    let (w, hgt) = (win.resolution.physical_width().max(1), win.resolution.physical_height().max(1));
    let mod_menu_open = mod_menu.is_some_and(|mm| mm.open);
    let was_open = gfx::menu_open(&h.vm);
    // test runs only take the synthetic input in MenuTest, never the user's mouse and keys
    let live = scripted.is_none();
    let open_request = test.open_pause || (live && !was_open && !mod_menu_open && keys.just_pressed(KeyCode::Escape));
    if std::mem::take(&mut h.pending_main_menu) {
        let ok = h.with_vm(|h| {
            gfx::set_size(&mut h.vm, w, hgt);
            let ok = h.open_main_menu();
            if std::env::var_os("UVM_GFX_PROBE").is_some() {
                gfx::probe_mouse_events(&mut h.vm);
            }
            ok
        });
        info!("[uvm] main menu opened: {ok}");
    }
    if h.vm.gfx.order.is_empty() && !open_request {
        UI_OPEN.store(false, Ordering::Relaxed);
        HUD_SHOWN.store(false, Ordering::Relaxed);
        score.hud.clear();
        return;
    }
    test.open_pause = false;
    // input for the movie, in viewport pixels
    let cursor = win.cursor_position().map(|p| p * scale);
    let mut mouse_events: Vec<(f32, f32, Option<bool>)> = std::mem::take(&mut test.mouse);
    if was_open && live {
        if let Some(c) = cursor {
            if view.cursor != Some(c) {
                mouse_events.push((c.x, c.y, None));
            }
            if mouse.just_pressed(MouseButton::Left) {
                mouse_events.push((c.x, c.y, Some(true)));
            }
            if mouse.just_released(MouseButton::Left) {
                mouse_events.push((c.x, c.y, Some(false)));
            }
        }
        view.cursor = cursor;
    }
    let mut key_events: Vec<(&str, bool)> = vec![];
    if was_open && live {
        for (k, n) in UI_KEYS {
            if keys.just_pressed(k) {
                key_events.push((n, true));
                view.down.push(n);
            }
            if keys.just_released(k) && view.down.contains(&n) {
                key_events.push((n, false));
                view.down.retain(|d| *d != n);
            }
        }
    } else {
        view.down.clear();
    }
    key_events.extend(std::mem::take(&mut test.keys));
    let game_paused = view.paused;
    let wheel = if was_open && live { scroll.delta.y } else { 0.0 };
    view.acc += real.delta_secs();
    let dt = if view.acc >= 1.0 / 120.0 { std::mem::take(&mut view.acc).min(0.1) } else { 0.0 };
    let want_buttons = test.want_buttons;
    let hud_events = std::mem::take(&mut score.hud);
    let combo_timer = score.combo_timer();
    let t0 = std::time::Instant::now();
    let shown: Vec<Option<ObjId>> = view.layers.iter().map(|l| l.movie).collect();
    let (imgs, open, buttons, hud_on, t_cap) = h.with_vm(|h| {
        gfx::set_size(&mut h.vm, w, hgt);
        if open_request && !was_open {
            h.open_pause_menu();
        }
        for (x, y, b) in mouse_events {
            match b {
                None => gfx::mouse_move(&mut h.vm, x, y),
                Some(down) => gfx::mouse_button(&mut h.vm, x, y, down),
            }
        }
        if wheel != 0.0 {
            gfx::mouse_wheel(&mut h.vm, wheel);
        }
        for (k, down) in key_events {
            if gfx::menu_open(&h.vm) {
                gfx::key(&mut h.vm, k, down);
            }
        }
        // WorldInfo.RealTimeSeconds keeps running while the game is paused (input timing)
        if game_paused {
            if let Some(&wi) = h.vm.globals.get("worldinfo") {
                let rt = h.vm.get(wi, "RealTimeSeconds").float() + dt;
                h.vm.set(wi, "RealTimeSeconds", Value::Float(rt));
            }
        }
        h.hud_feed(&hud_events, combo_timer);
        h.tick_menus(dt);
        h.settle_main_menu();
        let open = gfx::menu_open(&h.vm);
        let hud_on = h.hud_movie().is_some();
        let buttons = if open && want_buttons { h.pause_buttons() } else { vec![] };
        // every open movie, back to front; a movie is only rendered again (GPU readback) when a
        // movie frame ran or input changed it, else its layer keeps the last picture
        let tc = std::time::Instant::now();
        let order = h.vm.gfx.order.clone();
        let (menus, huds): (Vec<ObjId>, Vec<ObjId>) = order.into_iter().partition(|&mv| gfx::captures_input(&h.vm, mv));
        // the in-game HUD is refreshed at most every other frame (its readback costs ~2-3 ms);
        // menus every frame (they follow the pointer)
        let hud_turn = HUD_FRAME.fetch_add(1, Ordering::Relaxed) % 2 == 0;
        let n_huds = huds.len();
        let imgs: Vec<(Option<ObjId>, Option<image::RgbaImage>)> = huds
            .into_iter()
            .chain(menus)
            .enumerate()
            .map(|(i, mv)| {
                let new_layer = shown.get(i).copied().flatten() != Some(mv);
                if i < n_huds && !hud_turn && !new_layer {
                    return (Some(mv), None);
                }
                (Some(mv), gfx::capture_if_changed(&mut h.vm, mv, new_layer))
            })
            .collect();
        let t_cap = tc.elapsed().as_secs_f32();
        (imgs, open, buttons, hud_on, t_cap)
    });
    if open && !was_open {
        test.opened_count += 1;
    }
    test.open = open;
    test.buttons = buttons;
    UI_OPEN.store(open, Ordering::Relaxed);
    HUD_SHOWN.store(hud_on, Ordering::Relaxed);
    // pause the game under the menu
    if open && !view.paused {
        vtime.pause();
        view.paused = true;
    } else if !open && view.paused {
        vtime.unpause();
        view.paused = false;
    }
    let n = imgs.len().max(view.layers.len());
    view.layers.resize_with(n, Layer::default);
    for (i, (mv, img)) in imgs.into_iter().chain(std::iter::repeat_with(|| (None, None))).take(n).enumerate() {
        show_layer(i, mv, img, &mut view.layers[i], &mut images, &mut overlay, &mut commands);
    }
    view.cost.0 += t0.elapsed().as_secs_f32();
    view.cost.1 += 1;
    view.cost.2 += t_cap;
    if view.cost.1 >= 300 {
        let cap = view.cost.2;
        info!("[gfx] Scaleform movies: {:.2} ms/frame (render + readback {:.2})", view.cost.0 * 1000.0 / view.cost.1 as f32, cap * 1000.0 / view.cost.1 as f32);
        view.cost = (0.0, 0, 0.0);
    }
}

/// Upload a new frame of `movie` into layer `i` (GlobalZIndex 40 + i); no image = keep the last
/// one; no movie = hide the layer.
fn show_layer(
    i: usize,
    movie: Option<ObjId>,
    img: Option<image::RgbaImage>,
    layer: &mut Layer,
    images: &mut Assets<Image>,
    overlay: &mut Query<(&mut Visibility, &MenuOverlay)>,
    commands: &mut Commands,
) {
    if movie.is_none() {
        layer.movie = None;
        for (mut v, o) in overlay.iter_mut() {
            if o.0 == i {
                *v = Visibility::Hidden;
            }
        }
        return;
    }
    let Some(img) = img else { return };
    layer.movie = movie;
    let (iw, ih) = img.dimensions();
    let mut rgba = img.into_raw();
    // Ruffle draws premultiplied colour; UI images are straight alpha. Scaleform blended in gamma
    // space and Bevy blends in linear space, so partly transparent pixels get the alpha that
    // gives the same darkening: 1 - (1 - a)^2.2.
    let lut: [u8; 256] = std::array::from_fn(|a| ((1.0 - (1.0 - a as f32 / 255.0).powf(2.2)) * 255.0).round() as u8);
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
            p[3] = lut[a as usize];
        }
    }
    let handle = match (&layer.image, layer.size == (iw, ih)) {
        (Some(hd), true) => {
            if let Some(mut im) = images.get_mut(hd) {
                im.data = Some(rgba);
            }
            hd.clone()
        }
        _ => {
            let im = Image::new(
                Extent3d { width: iw, height: ih, depth_or_array_layers: 1 },
                TextureDimension::D2,
                rgba,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
            );
            let hd = images.add(im);
            layer.image = Some(hd.clone());
            layer.size = (iw, ih);
            if let Some(e) = layer.node.take() {
                commands.entity(e).despawn();
            }
            hd
        }
    };
    match layer.node {
        Some(_) => {
            for (mut v, o) in overlay.iter_mut() {
                if o.0 == i {
                    *v = Visibility::Visible;
                }
            }
        }
        None => {
            let e = commands
                .spawn((
                    ImageNode::new(handle),
                    Node { position_type: PositionType::Absolute, left: px(0), top: px(0), width: percent(100), height: percent(100), ..default() },
                    GlobalZIndex(40 + i as i32),
                    MenuOverlay(i),
                ))
                .id();
            layer.node = Some(e);
        }
    }
}
