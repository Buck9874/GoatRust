//! Scripted test runs: `goat --test smoke --shot out/smoke.png`.
//! Runs on the main monitor (`--second-monitor` for the other) with no mouse capture, prints a log
//! and exits with a status code.
//! Aborts early (about 4 s off script) with a clear reason.

use crate::goat::{Goat, ScriptedInput};
use crate::{Args, convert};
use bevy::prelude::*;
use bevy::window::{Monitor, MonitorSelection, PrimaryMonitor, PrimaryWindow, WindowPosition};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

#[derive(Resource)]
struct TestState {
    name: String,
    t: f32,
    start: Option<Vec3>,
    shot_done: bool,
    last_log: f32,
    max_height: f32,
    off_script: f32,
    frames: u32,
    /// Frame times (ms) after warm-up, for the summary's percentiles.
    frame_ms: Vec<f32>,
}

/// True only on the frame where the test clock passes `at` (a single key press).
fn pulse(t: f32, dt: f32, at: f32) -> bool {
    t >= at && t - dt < at
}

pub fn add(app: &mut App) {
    app.add_systems(First, cpu_frame_start).add_systems(Last, cpu_frame_end);
    app.insert_resource(ScriptedInput::default())
        .add_systems(Update, (place_window, (drive, headbutt_scenario, npc_scenario, crate::test_tricks::tricks_scenario, crate::test_npc::npc_reactions_scenario, crate::test_mods::mods_scenario, crate::test_mutators::mutators_scenario, crate::test_mutators::moonjump_scenario, crate::test_mutators::forcepush_scenario, crate::test_mutators::jetpack_scenario, crate::test_mutators::inventory_scenario, crate::test_mutators::kismet_scenario, crate::test_mutators::resize_scenario, crate::test_menus::pausemenu_scenario, crate::test_menu_flow::menuflow_scenario, crate::test_menus::hud_scenario, crate::test_pad::pad_scenario, crate::test_fly::fly_scenario).chain().before(crate::goat::read_input)))
            .add_systems(Update, (crate::test_destruct::destruct_scenario, crate::test_quests::quests_scenario, crate::test_quests::music_scenario, crate::test_menu_flow::mainmenu_scenario, crate::test_destruct::ladder_scenario, crate::test_destruct::water_scenario, crate::test_destruct::baseball_scenario, crate::test_destruct::race_scenario, crate::test_destruct::racepopup_scenario).after(crate::test_menus::hud_scenario).before(crate::goat::read_input));
}

/// "npc": headbutt the nearest person, expect ragdoll, then stand-up + run after mStandUpDelay
/// (the script AI runs back to its spot: StandUp -> ReturnToOriginalPosition -> StopPanic; the
/// Rust AI, GOAT_NPC_VM=0, panics).
#[allow(clippy::type_complexity)]
fn npc_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut commands: Commands,
    mut st: Local<(f32, u32, Option<Entity>, f32, bool, bool)>,
    mut phys: ResMut<crate::physics::Physics>,
    mut input: ResMut<ScriptedInput>,
    mut goats: Query<(&mut Goat, &mut Transform), Without<crate::npc::Npc>>,
    npcs: Query<(Entity, &crate::npc::Npc, &Transform, &Name)>,
) {
    if args.test.as_deref() != Some("npc") {
        return;
    }
    st.1 += 1;
    if st.1 < 5 {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.0 += dt;
    let t = st.0;
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    if st.2.is_none() && t >= 0.5 {
        let here = tr.translation;
        let Some((e, n, nt, name)) = npcs
            .iter()
            .filter(|(_, n, _, _)| n.default_anim.to_lowercase().contains("idle") || n.default_anim.to_lowercase().contains("talk"))
            .min_by(|a, b| a.2.translation.distance_squared(here).total_cmp(&b.2.translation.distance_squared(here)))
            .or_else(|| npcs.iter().min_by(|a, b| a.2.translation.distance_squared(here).total_cmp(&b.2.translation.distance_squared(here))))
        else {
            println!("TEST npc FAIL: no NPCs spawned");
            std::process::exit(10);
        };
        let p = nt.translation;
        let away = Vec3::new(here.x - p.x, 0.0, here.z - p.z).normalize_or(Vec3::Z);
        let stand = p + away * 1.1;
        if let Some(rb) = phys.world.bodies.get_mut(g.body) {
            rb.set_translation(convert::to_rapier(stand), true);
        }
        tr.translation = stand;
        g.velocity = Vec3::ZERO;
        g.yaw = f32::atan2(away.x, away.z);
        st.3 = g.yaw;
        st.2 = Some(e);
        println!("npc target {} ({}) anim {:?} {:.1} m away", name, e, n.default_anim, p.distance(here));
    }
    let Some(target) = st.2 else { return };
    g.yaw = if t < 2.0 { st.3 } else { g.yaw };
    input.cam_yaw = Some(st.3 + 0.9);
    input.cam_dist = Some(4.0);
    input.cam_pitch = Some(-0.25);
    if pulse(t, dt, 1.5) {
        input.headbutt = true;
    }
    let Ok((_, n, nt, _)) = npcs.get(target) else { return };
    if pulse(t, dt, 2.5) {
        println!("npc state after headbutt: {:?} at {:?}", n.state, nt.translation);
        if n.state != crate::npc::NpcState::Ragdoll {
            println!("TEST npc FAIL: headbutt did not knock the NPC over");
            std::process::exit(10);
        }
        if let Some(path) = args.shot.clone() {
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
        }
    }
    if !nt.translation.is_finite() {
        println!("TEST npc FAIL: NPC ragdoll exploded");
        std::process::exit(10);
    }
    // stood up and ran (panic, or back to its spot) at some point after the knock-down
    if t > 2.5 && matches!(n.state, crate::npc::NpcState::Panic | crate::npc::NpcState::Return) && !st.5 {
        st.5 = true;
        println!("npc ran after standing up: {:?} at t={t:.1}", n.state);
    }
    if pulse(t, dt, 10.5) {
        println!("npc state after stand-up delay: {:?} at {:?}", n.state, nt.translation);
        if n.state == crate::npc::NpcState::Ragdoll || !st.5 {
            println!("TEST npc FAIL: NPC did not stand up and panic");
            std::process::exit(10);
        }
        st.4 = true;
    }
}

/// "headbutt": put the goat next to the nearest prop, headbutt it and check that it flew.
#[allow(clippy::type_complexity)]
fn headbutt_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut st: Local<(f32, u32, Option<(rapier3d::prelude::RigidBodyHandle, Vec3)>, f32)>,
    mut phys: ResMut<crate::physics::Physics>,
    mut input: ResMut<ScriptedInput>,
    mut goats: Query<(&mut Goat, &mut Transform)>,
    breakables: Res<crate::destruct::Destructibles>,
    explosives: Res<crate::explosive::Explosives>,
    chains: Res<crate::chain::Chains>,
) {
    if args.test.as_deref() != Some("headbutt") {
        return;
    }
    st.1 += 1;
    if st.1 < 5 {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.0 += dt;
    let t = st.0;
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    if st.2.is_none() && t >= 0.5 {
        let here = tr.translation;
        let best = phys
            .dormant
            .iter()
            .filter_map(|&h| phys.world.bodies.get(h).map(|b| (h, convert::from_rapier(b.translation()))))
            .filter(|(_, p)| (p.y - here.y).abs() < 4.0)
            // props that shatter or explode leave no body to measure
            .filter(|(h, _)| !breakables.is_breakable_kactor(*h) && !explosives.items.contains_key(h))
            .min_by(|a, b| a.1.distance_squared(here).total_cmp(&b.1.distance_squared(here)));
        let Some((h, p)) = best else {
            println!("TEST headbutt FAIL: no prop near the spawn");
            std::process::exit(10);
        };
        let away = Vec3::new(here.x - p.x, 0.0, here.z - p.z).normalize_or(Vec3::Z);
        let stand = p + away * 1.0 + Vec3::Y * 0.8;
        if let Some(rb) = phys.world.bodies.get_mut(g.body) {
            rb.set_translation(convert::to_rapier(stand), true);
        }
        tr.translation = stand;
        g.velocity = Vec3::ZERO;
        g.yaw = f32::atan2(away.x, away.z); // face the prop (forward is -Z)
        println!("headbutt target at {p:?}, {:.1} m from spawn", p.distance(here));
        st.2 = Some((h, p));
        st.3 = g.yaw;
    }
    if st.2.is_some() {
        g.yaw = if t < 1.6 { st.3 } else { g.yaw };
        input.cam_yaw = Some(st.3);
        input.cam_dist = Some(3.5);
        input.cam_pitch = Some(-0.3);
    }
    if pulse(t, dt, 1.5) {
        input.headbutt = true;
    }
    if let Some((h, _)) = st.2 {
        if let Some(b) = phys.world.bodies.get(h) {
            if t >= 1.4 && t < 1.42 {
                println!("  t={t:.2} target dyn={} mass={:.1} vel={:?} pos={:?} colliders={}", b.is_dynamic(), b.mass(), b.linvel(), b.translation(), b.colliders().len());
            }
        }
    }
    if let Some((h, p0)) = st.2 {
        if t >= 3.0 {
            let p1 = phys.world.bodies.get(h).map(|b| convert::from_rapier(b.translation())).unwrap_or(p0);
            let moved = p1.distance(p0);
            if pulse(t, dt, 3.0) { println!("headbutt: {} moved {moved:.2} m (headbutts {})", phys.names.get(&h).cloned().unwrap_or_default(), g.stat_headbutts); }
            if moved < 0.2 {
                println!("TEST headbutt FAIL: prop did not move");
                std::process::exit(10);
            }
            // GGTrickCollision: the headbutted prop is in the goat's chain and scores when it lands
            if t >= 3.3 && chains.collisions == 0 {
                println!("TEST headbutt FAIL: the headbutted prop scored no collision-chain trick");
                std::process::exit(10);
            }
        }
    }
}

/// Put the (initially hidden) test window on the main monitor (or the other one with
/// `--second-monitor`), then show it.
fn place_window(
    args: Res<Args>,
    mut done: Local<u32>,
    monitors: Query<(Entity, &Monitor, Has<PrimaryMonitor>)>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if *done >= 2 {
        return;
    }
    let Ok(mut w) = windows.single_mut() else { return };
    *done += 1;
    let secondary = monitors.iter().find(|(_, _, primary)| *primary == args.main_monitor);
    if let Some((e, m, _)) = secondary {
        if *done == 1 {
            println!("test window -> monitor {:?} at {:?}", m.name, m.physical_position);
        }
        w.position = WindowPosition::Centered(MonitorSelection::Entity(e));
    }
    if *done == 2 {
        w.visible = true;
    }
}

fn drive(
    mut commands: Commands,
    time: Res<Time>,
    args: Res<Args>,
    state: Option<ResMut<TestState>>,
    mut input: ResMut<ScriptedInput>,
    goats: Query<(&Goat, &Transform)>,
    mut exit: MessageWriter<AppExit>,
    diags: Option<Res<bevy::diagnostic::DiagnosticsStore>>,
) {
    let Some(mut s) = state else {
        commands.insert_resource(TestState {
            name: args.test.clone().unwrap_or_default(),
            t: 0.0,
            start: None,
            shot_done: false,
            last_log: -1.0,
            max_height: f32::MIN,
            off_script: 0.0,
            frames: 0,
            frame_ms: vec![],
        });
        return;
    };
    s.frames += 1;
    // the first frames include shader compilation; don't count them as test time
    if s.frames < 5 {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    s.t += dt;
    let t = s.t;
    if t > 1.0 {
        s.frame_ms.push(time.delta_secs() * 1000.0);
        MEASURING.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let Ok((g, tr)) = goats.single() else { return };
    let start = *s.start.get_or_insert(tr.translation);
    s.max_height = s.max_height.max(tr.translation.y - start.y);
    let horiz = Vec2::new(tr.translation.x - start.x, tr.translation.z - start.z).length();

    let fail = |s: &mut TestState, why: &str| {
        println!("TEST {} FAIL at {:.1}s: {why} (pos {:?})", s.name, t, tr.translation);
        std::process::exit(10);
    };

    *input = ScriptedInput::default();
    let end;
    match s.name.as_str() {
        "smoke" => {
            // settle, walk, jump + double jump, sprint, headbutt, ragdoll
            if (1.0..3.0).contains(&t) {
                input.forward = 1.0;
            }
            if pulse(t, dt, 3.0) {
                input.jump = true;
            }
            if pulse(t, dt, 3.45) {
                input.jump = true;
            }
            if (4.0..6.0).contains(&t) {
                input.forward = 1.0;
                input.sprint = true;
            }
            if pulse(t, dt, 6.0) {
                input.headbutt = true;
            }
            if pulse(t, dt, 6.5) {
                input.ragdoll = true;
            }
            // early aborts: falling through the world / not moving while walking
            if tr.translation.y < start.y - 30.0 {
                s.off_script += dt;
            } else if t > 2.6 && t < 3.0 && horiz < 1.0 {
                s.off_script += dt;
            } else {
                s.off_script = 0.0;
            }
            if s.off_script > 1.0 {
                fail(&mut s, "goat fell through the world or did not move");
            }
            if t > 3.9 && t < 4.0 && s.max_height < 2.0 {
                fail(&mut s, "jump did not leave the ground");
            }
            end = 8.0;
        }
        "goat" => {
            // close side view of the animated goat: idle, then walking, then a headbutt
            input.cam_dist = Some(2.2);
            input.cam_pitch = Some(-0.15);
            if (1.5..4.0).contains(&t) {
                input.forward = 1.0;
            }
            if s.name == "goat" && pulse(t, dt, 2.0) {
                // side-on camera: forward is -Z at yaw 0, so look from +X
            }
            input.cam_yaw = Some(std::f32::consts::FRAC_PI_2);
            end = 3.2;
        }
        "ragdoll" => {
            // sprint, go limp at full speed, photograph the tumble, stand up, walk on
            input.cam_dist = Some(4.0);
            input.cam_pitch = Some(-0.35);
            input.cam_yaw = Some(std::f32::consts::FRAC_PI_2 + 0.5);
            if (0.5..2.0).contains(&t) {
                input.forward = 1.0;
                input.sprint = true;
            }
            if pulse(t, dt, 2.0) {
                input.ragdoll = true;
            }
            if pulse(t, dt, 4.5) {
                input.ragdoll = true; // stand up
            }
            if (5.0..6.0).contains(&t) {
                input.forward = 1.0;
            }
            // abort fast if the ragdoll explodes, sinks or flies away
            let bad = !tr.translation.is_finite() || tr.translation.y > start.y + 15.0 || tr.translation.y < start.y - 15.0;
            s.off_script = if bad { s.off_script + dt } else { 0.0 };
            if s.off_script > 0.5 {
                fail(&mut s, "ragdoll left the expected area (exploded / fell through)");
            }
            if t > 2.3 && t < 4.4 && !g.ragdoll {
                fail(&mut s, "goat is not ragdolled");
            }
            if t > 5.9 && g.ragdoll {
                fail(&mut s, "goat did not stand up");
            }
            end = 6.2;
            if !s.shot_done && t >= 3.0 {
                s.shot_done = true;
                if let Some(path) = args.shot.clone() {
                    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
                }
            }
        }
        "headbutt" => {
            end = 3.4;
        }
        "flip" | "wallrun" | "jumpover" | "reactions" | "attack" | "route" | "trip" | "mods" | "mutators" | "moonjump" | "forcepush" | "jetpack" | "inventory" | "kismet" | "resize" | "fx" | "npcvm" | "matinee" | "pausemenu" | "menuflow" | "hud" | "apex" | "explode" | "quests" | "music" | "mainmenu" | "ladder" | "manual" | "bike" | "longboard" | "cart" | "water" | "baseball" | "race" | "racepopup" | "bikerace" | "pad" | "fly" => {
            // driven by test_tricks; keep this harness alive
            s.shot_done = true;
            end = 60.0;
        }
        "npc" => {
            s.shot_done = true; // npc_scenario takes its own screenshot
            end = 11.0;
        }
        "aerial" => {
            input.cam_dist = Some(20.0);
            input.cam_pitch = Some(-0.55);
            input.cam_yaw = Some(0.4);
            end = 2.5;
        }
        "goatidle" => {
            input.cam_dist = Some(2.0);
            input.cam_pitch = Some(-0.2);
            input.cam_yaw = Some(std::f32::consts::FRAC_PI_2 + 0.6);
            end = 2.5;
        }
        _ => {
            // "view": just render the spawn area
            end = 4.0;
        }
    }
    if t - s.last_log >= 0.5 {
        s.last_log = t;
        let sp = Vec2::new(g.velocity.x, g.velocity.z).length() / convert::S;
        println!(
            "t={t:4.1} pos=({:7.2},{:7.2},{:7.2}) speed={sp:5.0}uu/s grounded={} ragdoll={} headbutts={} maxh={:.2}",
            tr.translation.x, tr.translation.y, tr.translation.z, g.grounded, g.ragdoll, g.stat_headbutts, s.max_height
        );
    }
    if !s.shot_done && t >= end - 1.0 {
        s.shot_done = true;
        if let Some(path) = args.shot.clone() {
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
        }
    }
    if t >= end {
        // GOAT_GPU_TIMING=1: GPU time per render pass (smoothed), the heaviest first
        if let Some(d) = diags.as_ref().filter(|_| std::env::var_os("GOAT_GPU_TIMING").is_some()) {
            let mut passes: Vec<(String, f64)> = d
                .iter()
                .filter(|x| x.path().as_str().ends_with("elapsed_gpu"))
                .filter_map(|x| Some((x.path().as_str().trim_start_matches("render/").trim_end_matches("/elapsed_gpu").to_string(), x.smoothed()?)))
                .collect();
            passes.sort_by(|a, b| b.1.total_cmp(&a.1));
            let top: f64 = passes.iter().filter(|(n, _)| !n.contains('/')).map(|p| p.1).sum();
            println!("GPU ms per frame (top-level passes total {top:.2}):");
            for (n, v) in passes.iter().take(14) {
                println!("  {v:6.2}  {n}");
            }
        }
        let n = CPU_MS.lock().map(|v| v.len()).unwrap_or(0) as u32;
        let rep = crate::prof::report(n);
        if !rep.is_empty() {
            println!("CPU ms per frame by system:");
            for (k, v) in rep.iter().take(16) {
                println!("  {v:6.2}  {k}");
            }
        }
        let mut cpu = CPU_MS.lock().map(|v| v.clone()).unwrap_or_default();
        cpu.sort_by(|a, b| a.total_cmp(b));
        if !cpu.is_empty() {
            println!("CPU ms per frame (main update): median {:.1} p95 {:.1} worst {:.1}", cpu[cpu.len() / 2], cpu[(cpu.len() as f32 * 0.95) as usize % cpu.len()], cpu[cpu.len() - 1]);
        }
        let mut ms = s.frame_ms.clone();
        ms.sort_by(|a, b| a.total_cmp(b));
        let pct = |q: f32| ms.get(((ms.len() as f32 * q) as usize).min(ms.len().saturating_sub(1))).copied().unwrap_or(0.0);
        println!(
            "TEST {} PASS (moved {horiz:.1} m, max height {:.2} m, headbutts {}, avg fps {:.0}, frame ms median {:.1} p95 {:.1} worst {:.1})",
            s.name,
            s.max_height,
            g.stat_headbutts,
            s.frames as f32 / t.max(0.01),
            pct(0.5),
            pct(0.95),
            ms.last().copied().unwrap_or(0.0)
        );
        exit.write(AppExit::Success);
    }
}


/// The main app's CPU time per frame (First to Last), after the first seconds (performance).
static CPU_MS: std::sync::Mutex<Vec<f32>> = std::sync::Mutex::new(Vec::new());
static FRAME_START: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// Set once the test is 1 s in (after loading and warm-up).
static MEASURING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn cpu_frame_start(mut started: Local<bool>) {
    if MEASURING.load(std::sync::atomic::Ordering::Relaxed) {
        if !*started {
            *started = true;
            crate::prof::reset();
        }
        if let Ok(mut s) = FRAME_START.lock() {
            *s = Some(std::time::Instant::now());
        }
    }
}

pub fn cpu_frame_end() {
    let Some(t0) = FRAME_START.lock().ok().and_then(|mut s| s.take()) else { return };
    if let Ok(mut v) = CPU_MS.lock() {
        v.push(t0.elapsed().as_secs_f32() * 1000.0);
    }
}
