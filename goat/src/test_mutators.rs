//! `--test moonjump`: Moon Jump (456408519) registers a KeyDown listener and checks the jump
//! binding through GGPlayerInputGame.IsKeyIsPressed (DefaultInput.ini); holding jump must lift the
//! goat far above a normal jump (its TickMutatorComponent sets Velocity.Z = 750).
//!
//! `--test forcepush`: Omniscient Goat (3729292931) Force Push on R: its script finds NPCs and
//! props in a cone through VisibleCollidingActors (script proxies for game objects) and calls
//! TakeDamage with momentum; NPCs in the cone must be knocked over or props must start moving.
//!
//! `--test jetpack`: the base game's Jet Pack (GoatVanilla.GGMutatorJetPack from Startup.upk):
//! R ragdolls the goat and enables its RB_Thruster; the ragdoll must climb.
//!
//! `--test kismet`: the map's Kismet in the VM: put the goat in a touch volume whose Touched
//! output leads to a GGSeqAct_GiveScore and expect the Kismet trick in the combo.
//!
//! `--test resize`: Resizable Goat (246864068): H doubles the goat's DrawScale in script; the
//! collision capsule must follow (x2) and the goat must stay on the ground.
//!
//! `--test mutators`: run downloaded Steam Workshop mutators in the UnrealScript VM
//! (Speedy Goat 3130339802 + Infinite Jumps 3128865778 unless GOAT_MUTATORS is set; download
//! them from the mod menu first), check the script goat changed, then sprint and multi-jump
//! with the values the mutators set. Screenshot: <shot>.

use crate::Args;
use crate::goat::{Goat, ScriptedInput};
use crate::uvm::bridge;
use crate::uvm::plugin::Mutators;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

pub const DEFAULT_ITEMS: &str = "3130339802,3128865778";
pub const MOON_JUMP_ITEM: &str = "456408519";
/// Omniscient Goat: GBA_Special (R) = Force Push (VisibleCollidingActors cone + TakeDamage).
pub const FORCE_PUSH_ITEM: &str = "3729292931";

#[derive(Default)]
pub struct St {
    t: f32,
    phase: u32,
    mark: f32,
    top_speed: f32,
    air_jumps: u32,
    air_done: u32,
    was_grounded: bool,
    last_vy: f32,
    off_script: f32,
}

fn fail(why: &str, t: f32) -> ! {
    println!("TEST mutators FAIL at {t:.1}s: {why}");
    std::process::exit(10);
}

#[allow(clippy::too_many_arguments)]
pub fn mutators_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<St>,
    m: Res<Mutators>,
    mut input: ResMut<ScriptedInput>,
    goats: Query<&Goat>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("mutators") {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let Ok(g) = goats.single() else { return };
    let speed = Vec2::new(g.velocity.x, g.velocity.z).length() / crate::convert::S;
    match st.phase {
        0 => {
            if m.host.is_some() {
                let v = bridge::goat_values().unwrap_or_else(|| fail("no script goat values", t));
                let mv = bridge::movement();
                println!("  t={t:.1} {} | script goat {v:?}", m.status);
                let comps = m.host.as_ref().map(|h| h.mutators.len()).unwrap_or(0);
                if comps == 0 {
                    fail("no mutators active", t);
                }
                if mv.walk_speed == crate::goat::tune::WALK_SPEED && mv.sprint_speed == crate::goat::tune::SPRINT_SPEED && mv.extra_jumps == 0 {
                    fail("mutators ran but the goat's values are unchanged", t);
                }
                let log = std::fs::read_to_string(crate::mods::mods_dir().join("uvm_log.txt")).unwrap_or_default();
                let calls = log.lines().filter(|l| l.trim_start().starts_with("->")).count();
                println!("  uvm_log.txt: {} lines, {calls} calls", log.lines().count());
                if !log.contains("AttachToPlayer") {
                    fail("the log has no AttachToPlayer call", t);
                }
                st.phase = 1;
                st.mark = t;
            } else if !m.status.is_empty() && !m.status.starts_with("loading") {
                fail(&m.status, t);
            } else if t > 30.0 {
                fail("UnrealScript packages still loading after 30 s", t);
            }
        }
        1 => {
            // sprint forward for 3 s
            input.forward = 1.0;
            input.sprint = true;
            st.top_speed = st.top_speed.max(speed);
            let want = bridge::movement().sprint_speed * crate::mods::tweak().sprint_speed;
            // early abort: not moving at all for ~4 s
            if speed < 50.0 {
                st.off_script += dt;
                if st.off_script > 4.0 {
                    fail("goat is not moving", t);
                }
            } else {
                st.off_script = 0.0;
            }
            if t - st.mark > 3.0 {
                println!("  t={t:.1} sprint top speed {:.0} uu/s (script sprint {want:.0}, original 1200)", st.top_speed);
                if st.top_speed < crate::goat::tune::SPRINT_SPEED * 1.2 && want > crate::goat::tune::SPRINT_SPEED * 1.5 {
                    fail("sprint speed did not follow the mutator", t);
                }
                input.sprint = false;
                input.forward = 0.0;
                st.phase = 2;
                st.mark = t;
                st.was_grounded = g.grounded;
            }
        }
        2 => {
            // jump, then keep jumping near the apex: more than one air jump needs Infinite Jumps
            let extra = bridge::movement().extra_jumps;
            input.jump = false;
            // a sprint into something (greenhouse glass, a prop) can end in a ragdoll: stand up first
            if g.ragdoll && st.air_jumps == 0 {
                if t - st.mark > 8.0 {
                    fail("the goat stayed ragdolled after the sprint", t);
                }
                // a movement key stands it up (GBA_Forward -> StandUp)
                input.forward = if (t * 2.0).fract() < 0.5 { 1.0 } else { 0.0 };
                return;
            }
            // start from a standstill: the sprint leaves the goat sliding at ~3000 uu/s
            if st.air_jumps == 0 && (speed > 60.0 || !g.grounded) && t - st.mark < 4.0 {
                st.last_vy = g.velocity.y;
                return;
            }
            if g.grounded && t - st.mark > 0.3 && st.air_jumps == 0 {
                input.jump = true;
            } else if !g.grounded && st.last_vy > 0.0 && g.velocity.y <= 0.0 {
                // apex
                input.jump = true;
                st.air_jumps += 1;
            }
            // a jump in the air shows as an upward kick of the vertical speed
            if !g.grounded && g.velocity.y > st.last_vy + 2.0 && st.air_jumps > 0 {
                st.air_done += 1;
            }
            st.last_vy = g.velocity.y;
            if t - st.mark > 9.0 || st.air_jumps >= 4 {
                let h = g.velocity.y;
                println!("  t={t:.1} air jumps: tried {}, done {} (extra jumps from script {extra}), vy {h:.2}", st.air_jumps, st.air_done);
                if extra > 0 && st.air_done < 3 {
                    fail("could not jump repeatedly in the air", t);
                }
                if let Some(p) = args.shot.clone() {
                    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                }
                st.phase = 3;
                st.mark = t;
            }
        }
        3 if t - st.mark > 1.0 => {
            println!("TEST mutators PASS (sprint top {:.0} uu/s, {} air jumps)", st.top_speed, st.air_done);
            exit.write(AppExit::Success);
        }
        _ => {}
    }
    if t > 50.0 {
        fail("timeout", t);
    }
}

#[derive(Default)]
pub struct MoonSt {
    t: f32,
    phase: u32,
    mark: f32,
    start_y: f32,
    max_y: f32,
    stuck: f32,
    flag_seen: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn moonjump_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<MoonSt>,
    mut m: ResMut<Mutators>,
    mut input: ResMut<ScriptedInput>,
    goats: Query<(&Goat, &Transform)>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("moonjump") {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let fail = |why: &str| -> ! {
        println!("TEST moonjump FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    let Ok((g, tr)) = goats.single() else { return };
    match st.phase {
        0 => {
            if m.host.as_ref().is_some_and(|h| !h.mutators.is_empty()) && g.grounded {
                println!("  t={t:.1} {}", m.status);
                st.start_y = tr.translation.y;
                st.max_y = st.start_y;
                st.phase = 1;
                st.mark = t;
            } else if !m.status.is_empty() && !m.status.starts_with("loading") && m.host.is_none() {
                fail(&m.status);
            } else if t > 30.0 {
                fail("Moon Jump not running after 30 s (download item 456408519 first)");
            }
        }
        1 => {
            // hold jump for 1.5 s
            input.jump = true;
            st.max_y = st.max_y.max(tr.translation.y);
            // MoonJumpComponent.KeyDown -> IsKeyIsPressed("GBA_Jump", "SpaceBar") -> IsMoonJumping
            if let Some(h) = m.host.as_mut() {
                for c in h.components() {
                    if h.vm.class_name(h.vm.obj(c).class) == "MoonJumpComponent" && h.vm.get(c, "IsMoonJumping").truthy() {
                        st.flag_seen = true;
                    }
                }
            }
            let rise = st.max_y - st.start_y;
            // early abort: holding jump but not leaving the ground for ~4 s
            if rise < 0.2 {
                st.stuck += dt;
                if st.stuck > 4.0 {
                    fail("goat never left the ground");
                }
            }
            if t - st.mark > 1.5 {
                input.jump = false;
                st.phase = 2;
                st.mark = t;
            }
        }
        2 => {
            st.max_y = st.max_y.max(tr.translation.y);
            if t - st.mark > 1.0 {
                let rise = st.max_y - st.start_y;
                // a normal GGGoat jump peaks around 650^2 / (2 * 750) uu = 5.6 m
                println!("  t={t:.1} rose {rise:.1} m holding jump (normal jump about 5.6 m)");
                if let Some(p) = args.shot.clone() {
                    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                }
                if !st.flag_seen {
                    fail("MoonJumpComponent.IsMoonJumping never became true (key input not reaching the script)");
                }
                if rise < 9.0 {
                    fail("holding jump did not moon-jump");
                }
                st.phase = 3;
                st.mark = t;
            }
        }
        3 if t - st.mark > 0.5 => {
            println!("TEST moonjump PASS");
            exit.write(AppExit::Success);
        }
        _ => {}
    }
    if t > 45.0 {
        fail("timeout");
    }
}

#[derive(Default)]
pub struct PushSt {
    t: f32,
    phase: u32,
    mark: f32,
    knocked_before: usize,
}

#[allow(clippy::too_many_arguments)]
pub fn forcepush_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<PushSt>,
    mut m: ResMut<Mutators>,
    npcs: Query<&crate::npc::Npc>,
    phys: Res<crate::physics::Physics>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("forcepush") {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let fail = |why: &str| -> ! {
        println!("TEST forcepush FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    let knocked = || npcs.iter().filter(|n| n.state == crate::npc::NpcState::Ragdoll).count();
    let moving = || phys.world.bodies.iter().filter(|(h, b)| b.is_dynamic() && phys.names.contains_key(h) && b.linvel().length() > 1.0).count();
    match st.phase {
        0 => {
            if m.host.as_ref().is_some_and(|h| !h.mutators.is_empty()) && t > 2.0 {
                st.knocked_before = knocked();
                println!("  t={t:.1} {} | {} NPCs, {} knocked, {} props moving", m.status, npcs.iter().count(), st.knocked_before, moving());
                m.test_keys = vec!["R".into()];
                st.phase = 1;
                st.mark = t;
            } else if !m.status.is_empty() && !m.status.starts_with("loading") && m.host.is_none() {
                fail(&m.status);
            } else if t > 30.0 {
                fail("Omniscient Goat not running after 30 s (download item 3729292931 first)");
            }
        }
        1 if t - st.mark > 0.1 => {
            m.test_keys.clear();
            st.phase = 2;
            st.mark = t;
        }
        2 if t - st.mark > 1.5 => {
            let (k, mv) = (knocked(), moving());
            let proxies = m.host.as_ref().map(|h| h.vm.proxy_things.len()).unwrap_or(0);
            println!("  t={t:.1} after Force Push: {k} NPCs knocked (before {}), {mv} props moving, {proxies} script proxies", st.knocked_before);
            if let Some(p) = args.shot.clone() {
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
            }
            if k <= st.knocked_before && mv == 0 {
                fail("Force Push moved nothing");
            }
            st.phase = 3;
            st.mark = t;
        }
        3 if t - st.mark > 0.5 => {
            println!("TEST forcepush PASS");
            exit.write(AppExit::Success);
        }
        _ => {}
    }
    if t > 45.0 {
        fail("timeout");
    }
}

#[derive(Default)]
pub struct JetSt {
    t: f32,
    phase: u32,
    mark: f32,
    start_y: f32,
    max_y: f32,
}

#[allow(clippy::too_many_arguments)]
pub fn jetpack_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<JetSt>,
    mut m: ResMut<Mutators>,
    mut input: ResMut<ScriptedInput>,
    goats: Query<(&Goat, &Transform)>,
    ragdolls: Query<&crate::ragdoll::ActiveRagdoll>,
    phys: Res<crate::physics::Physics>,
    fxs: Query<(&crate::particles::ParticleFx, &Name)>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("jetpack") {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let fail = |why: &str| -> ! {
        println!("TEST jetpack FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    let Ok((g, tr)) = goats.single() else { return };
    // the goat's height: the ragdoll pelvis while ragdolled
    let y = ragdolls
        .iter()
        .next()
        .and_then(|r| phys.world.bodies.get(r.pelvis))
        .map(|b| crate::convert::from_rapier(b.translation()).y)
        .unwrap_or(tr.translation.y);
    match st.phase {
        0 => {
            if m.host.as_ref().is_some_and(|h| !h.mutators.is_empty()) && g.grounded && t > 2.0 {
                println!("  t={t:.1} {}", m.status);
                st.start_y = y;
                st.max_y = y;
                // JETPACK_JUMP=1: fire the thruster in the air (diagnostic for ground contact)
                if std::env::var_os("JETPACK_JUMP").is_some() {
                    input.jump = true;
                    st.phase = 10;
                } else {
                    m.test_keys = vec!["R".into()];
                    st.phase = 1;
                }
                st.mark = t;
            } else if !m.status.is_empty() && !m.status.starts_with("loading") && m.host.is_none() {
                fail(&m.status);
            } else if t > 30.0 {
                fail("Jet Pack not running after 30 s");
            }
        }
        10 if t - st.mark > 0.35 => {
            input.jump = false;
            m.test_keys = vec!["R".into()];
            st.phase = 1;
            st.mark = t;
        }
        1 if t - st.mark > 0.1 => {
            m.test_keys.clear();
            st.phase = 2;
            st.mark = t;
        }
        2 => {
            st.max_y = st.max_y.max(y);
            // early abort: no ragdoll ~4 s after pressing R
            if !g.ragdoll && t - st.mark > 4.0 {
                fail("R did not ragdoll the goat");
            }
            if t - st.mark > 3.0 {
                let rise = st.max_y - st.start_y;
                println!("  t={t:.1} ragdoll={} climbed {rise:.1} m with the thruster on", g.ragdoll);
                if let Some(p) = args.shot.clone() {
                    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                }
                if rise < 3.0 {
                    fail("the jet pack did not lift the goat");
                }
                // the thrust effects (SpawnEmitterMeshAttachment on the jet pack's sockets) play
                let thrust: Vec<usize> = fxs.iter().filter(|(_, n)| n.as_str().contains("JetThrust") && n.as_str().starts_with("scriptfx")).map(|(f, _)| f.live()).collect();
                println!("  t={t:.1} thrust particles {thrust:?}");
                if thrust.len() != 2 || thrust.contains(&0) {
                    fail("the jet pack's thrust particles are not playing");
                }
                st.phase = 3;
                st.mark = t;
            }
        }
        3 if t - st.mark > 0.5 => {
            println!("TEST jetpack PASS");
            exit.write(AppExit::Success);
        }
        _ => {}
    }
    if t > 45.0 {
        fail("timeout");
    }
}

#[derive(Default)]
pub struct KisSt {
    t: f32,
    phase: u32,
    mark: f32,
    target: usize,
    score_before: i64,
}

#[allow(clippy::too_many_arguments)]
pub fn kismet_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<KisSt>,
    m: Res<Mutators>,
    score: Res<crate::score::Score>,
    mut phys: ResMut<crate::physics::Physics>,
    mut goats: Query<(&mut Goat, &mut Transform)>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("kismet") {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let fail = |why: &str| -> ! {
        println!("TEST kismet FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    match st.phase {
        0 => {
            if m.host.is_some() && !m.touch_centers.is_empty() && t > 1.0 {
                let scoring: Vec<usize> = (0..m.touch_centers.len()).filter(|&i| m.touch_centers[i].2).collect();
                println!("  t={t:.1} {} | {} touch volumes, {} lead to a score", m.status, m.touch_centers.len(), scoring.len());
                let Some(&i) = scoring.first() else { fail("no touch volume leads to a GiveScore") };
                st.target = i;
                st.score_before = score.total + score.combo_total() as i64;
                // teleport into the volume
                let c = m.touch_centers[i].1;
                if let Some(rb) = phys.world.bodies.get_mut(g.body) {
                    rb.set_translation(crate::convert::to_rapier(c), true);
                }
                tr.translation = c;
                g.velocity = Vec3::ZERO;
                println!("  goat -> volume {} at {:.1?}", i, c);
                st.phase = 1;
                st.mark = t;
            } else if t > 30.0 {
                fail("no Kismet touch volumes after 30 s");
            }
        }
        1 => {
            if m.kismet_scored > 0 {
                println!("  t={t:.1} Kismet trick scored ({} so far), combo lines: {:?}", m.kismet_scored, score.lines.iter().map(|l| (&l.text, l.score)).collect::<Vec<_>>());
                if let Some(p) = args.shot.clone() {
                    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                }
                st.phase = 2;
                st.mark = t;
            } else if t - st.mark > 4.0 {
                fail("touching the volume gave no Kismet score");
            }
        }
        2 if t - st.mark > 0.6 => {
            println!("TEST kismet PASS");
            exit.write(AppExit::Success);
        }
        _ => {}
    }
    if t > 45.0 {
        fail("timeout");
    }
}

#[derive(Default)]
pub struct ResizeSt {
    t: f32,
    phase: u32,
    mark: f32,
}

#[allow(clippy::too_many_arguments)]
pub fn resize_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<ResizeSt>,
    mut m: ResMut<Mutators>,
    goats: Query<(&Goat, &Transform)>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("resize") {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let fail = |why: &str| -> ! {
        println!("TEST resize FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    let Ok((g, tr)) = goats.single() else { return };
    match st.phase {
        0 => {
            if m.host.as_ref().is_some_and(|h| !h.mutators.is_empty()) && g.grounded && t > 1.5 {
                println!("  t={t:.1} {} | scale {:.2}", m.status, bridge::goat_scale());
                m.test_keys = vec!["H".into()];
                st.phase = 1;
                st.mark = t;
            } else if t > 30.0 {
                fail("Resizable Goat not running after 30 s (download item 246864068 first)");
            }
        }
        1 if t - st.mark > 0.1 => {
            m.test_keys.clear();
            st.phase = 2;
            st.mark = t;
        }
        2 if t - st.mark > 1.5 => {
            let s = bridge::goat_scale();
            println!("  t={t:.1} goat scale {s:.2}, grounded {}, y {:.2}", g.grounded, tr.translation.y);
            if let Some(p) = args.shot.clone() {
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
            }
            if (s - 2.0).abs() > 0.05 {
                fail("the collision capsule did not follow DrawScale x2");
            }
            st.phase = 3;
            st.mark = t;
        }
        3 if t - st.mark > 0.5 => {
            if !g.grounded {
                fail("the scaled goat is not standing on the ground");
            }
            println!("TEST resize PASS");
            exit.write(AppExit::Success);
        }
        _ => {}
    }
    if t > 40.0 {
        fail("timeout");
    }
}

#[derive(Default)]
pub struct InvSt {
    t: f32,
    phase: u32,
    mark: f32,
    item: Option<rapier3d::prelude::RigidBodyHandle>,
}

/// `--test inventory`: the base game's Inventory mutator. Lick a prop, Baa stores it (it leaves
/// the world), I opens the list, Baa takes it out again in front of the goat, I closes the list.
#[allow(clippy::too_many_arguments)]
pub fn inventory_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<InvSt>,
    mut m: ResMut<Mutators>,
    mut input: ResMut<ScriptedInput>,
    mut goats: Query<(&mut Goat, &mut Transform)>,
    mut phys: ResMut<crate::physics::Physics>,
    vis: Query<(&crate::physics::PhysBody, &Visibility)>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("inventory") {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let fail = |why: &str| -> ! {
        println!("TEST inventory FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    *input = ScriptedInput::default();
    let slots = |m: &mut Mutators| -> usize {
        let Some(h) = m.host.as_mut() else { return 0 };
        let Some(goat) = h.goat else { return 0 };
        let Some(inv) = h.vm.get(goat, "mInventory").obj() else { return 0 };
        match h.vm.get(inv, "mInventorySlots") {
            crate::uvm::value::Value::Array(a) => a.len(),
            _ => 0,
        }
    };
    let hidden = |b| vis.iter().any(|(pb, v)| pb.handle == b && *v == Visibility::Hidden);
    match st.phase {
        0 => {
            if m.host.as_ref().is_some_and(|h| !h.mutators.is_empty()) && g.grounded && t > 2.0 {
                // the nearest light prop on the ground: stand next to it, facing it
                let here = tr.translation;
                let best = phys
                    .prop_info
                    .keys()
                    .filter_map(|&b| {
                        let rb = phys.world.bodies.get(b)?;
                        let p = crate::convert::from_rapier(rb.translation());
                        (rb.mass() < 40.0 && (p.y - here.y).abs() < 1.5).then_some((b, p, p.distance(here)))
                    })
                    .min_by(|a, b| a.2.total_cmp(&b.2));
                let Some((b, p, _)) = best else { fail("no prop to pick up near the goat") };
                let dir = (p - here).with_y(0.0).normalize_or(Vec3::X);
                crate::test_tricks::place(&mut phys, &mut g, &mut tr, p - dir * 0.9 + Vec3::Y * 0.4, dir);
                println!("  t={t:.1} picking up {} ({:.0} m away)", phys.names.get(&b).cloned().unwrap_or_default(), p.distance(here));
                st.item = Some(b);
                st.phase = 1;
                st.mark = t;
            } else if !m.status.is_empty() && !m.status.starts_with("loading") && m.host.is_none() {
                fail(&m.status);
            } else if t > 30.0 {
                fail("the Inventory mutator is not running after 30 s");
            }
        }
        1 if t - st.mark > 0.6 => {
            input.lick = true;
            st.phase = 2;
            st.mark = t;
        }
        2 => {
            if g.grabbed.is_some() {
                st.item = g.grabbed.map(|(b, _)| b);
                println!("  t={t:.1} licking {}", st.item.and_then(|b| phys.names.get(&b).cloned()).unwrap_or_default());
                st.phase = 3;
                st.mark = t;
            } else if t - st.mark > 1.0 {
                fail("the lick did not grab the prop");
            }
        }
        3 if t - st.mark > 0.3 => {
            input.baa = true;
            st.phase = 4;
            st.mark = t;
        }
        4 => {
            let b = st.item.unwrap();
            let n = slots(&mut m);
            if n == 1 && hidden(b) && g.grabbed.is_none() {
                println!("  t={t:.1} stored: 1 slot, prop hidden");
                m.test_keys = vec!["I".into()];
                st.phase = 5;
                st.mark = t;
            } else if t - st.mark > 1.5 {
                fail(&format!("Baa did not store the item (slots {n}, hidden {}, still licking {})", hidden(b), g.grabbed.is_some()));
            }
        }
        5 => {
            m.test_keys.clear();
            if crate::uvm::inventory::open() {
                println!("  t={t:.1} inventory list open");
                st.phase = 6;
                st.mark = t;
            } else if t - st.mark > 1.0 {
                fail("I did not open the inventory");
            }
        }
        6 if t - st.mark > 0.3 && st.mark >= 0.0 => {
            if let Some(p) = args.shot.clone() {
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
            }
            st.phase = 60;
        }
        60 if t - st.mark > 0.9 => {
            m.test_keys = vec!["one".into()];
            st.phase = 7;
            st.mark = t;
        }
        7 => {
            m.test_keys.clear();
            let b = st.item.unwrap();
            let n = slots(&mut m);
            let back = phys.world.bodies.get(b).is_some_and(|rb| rb.is_dynamic());
            if n == 0 && back && !hidden(b) {
                let p = phys.world.bodies.get(b).map(|rb| crate::convert::from_rapier(rb.translation())).unwrap_or_default();
                let d = p.distance(tr.translation);
                println!("  t={t:.1} taken out {d:.1} m from the goat");
                if d > 4.0 {
                    fail("the item came out far from the goat");
                }
                m.test_keys = vec!["I".into()];
                st.phase = 8;
                st.mark = t;
            } else if t - st.mark > 1.5 {
                fail(&format!("Baa in the list did not take the item out (slots {n}, dynamic {back}, hidden {})", hidden(b)));
            }
        }
        8 => {
            m.test_keys.clear();
            if !crate::uvm::inventory::open() {
                println!("TEST inventory PASS");
                exit.write(AppExit::Success);
            } else if t - st.mark > 1.0 {
                fail("I did not close the inventory");
            }
        }
        _ => {}
    }
    if t > 45.0 {
        fail("timeout");
    }
}
