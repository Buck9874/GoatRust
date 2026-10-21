//! NPC behaviour through the VM (GOAT_NPC_VM=1): each NPC's script pawn (its proxy) gets its
//! real AI controller (Default ControllerClass: GGAIController, ...Agressive, ...GangMember,
//! ...PassiveGoat, or a mod's own). The controller's Tick, timers and state code run in the VM;
//! sight (SeePlayer) and ragdolls (OnRagdoll) are fed in from the game, and its latent moves
//! (MoveTo/MoveToward), rotation and SetAnimationInfoStruct animations drive the NPC through
//! npc::ScriptDrive. Knock-downs, ragdolls and scoring stay with npc.rs.

use super::plugin::Mutators;
use super::value::Value;
use super::{ObjId, Vm, WorldThing};
use crate::convert;
use crate::npc::{Npc, NpcState, ScriptDrive};
use crate::physics::Physics;
use bevy::prelude::*;
use std::collections::HashMap;

pub fn enabled() -> bool {
    forced() || std::env::var("GOAT_NPC_VM").is_ok_and(|v| v != "0")
}

struct Driven {
    pawn: ObjId,
    ctrl: ObjId,
    ragdoll: bool,
    /// Seconds until the next sight check.
    sight: f32,
    /// Pawn rotation yaw last written (Unreal units), to spot SetRotation from script.
    yaw_written: i32,
}

#[derive(Resource, Default)]
pub struct ScriptNpcs {
    driven: HashMap<Entity, Driven>,
    generation: u64,
    pub controllers: usize,
}

fn to_ue(v: Vec3) -> [f32; 3] {
    [v.x / convert::S, v.z / convert::S, v.y / convert::S]
}

/// Bevy yaw (npc.rs) <-> Unreal Rotation.Yaw.
fn yaw_to_ue(b: f32) -> i32 {
    (-(b + std::f32::consts::FRAC_PI_2) * 65536.0 / std::f32::consts::TAU).round() as i32
}

fn yaw_from_ue(u: i32) -> f32 {
    -(u as f32 * std::f32::consts::TAU / 65536.0) - std::f32::consts::FRAC_PI_2
}

/// GGNpc.SetAnimationInfoStruct, natively: remember the animation for the host and set the
/// movement speed like the script does. Returns the animation's length.
pub fn set_animation_info(vm: &mut Vm, c: &mut super::exec::Call) -> Value {
    let Some(p) = c.this else { return Value::Float(-1.0) };
    let info = c.arg(0).clone();
    let Value::Struct(s) = &info else { return Value::Float(-1.0) };
    let f = |vm: &mut Vm, n: &str| {
        let y = vm.sym(n);
        s.field(y).cloned()
    };
    let names: Vec<String> = match f(vm, "AnimationNames") {
        Some(Value::Array(a)) => a.iter().map(|v| vm.to_string(v)).filter(|n| !n.is_empty() && n != "None").collect(),
        _ => vec![],
    };
    let rate = match f(vm, "AnimationRate") {
        Some(Value::Float(r)) if r > 0.0 => r,
        _ => 1.0,
    };
    let speed = match f(vm, "MovementSpeed") {
        Some(Value::Float(v)) => v,
        _ => 0.0,
    };
    let looping = f(vm, "LoopAnimation").is_some_and(|v| v.truthy());
    let mut len = -1.0;
    if !names.is_empty() {
        let i = (super::natives::rand01() * names.len() as f32) as usize % names.len();
        let name = names[i].clone();
        len = vm.anim_lengths.get(&name.to_ascii_lowercase()).copied().unwrap_or(1.0) / rate;
        vm.set(p, "uvmAnim", Value::Str(name));
        vm.set(p, "uvmAnimRate", Value::Float(rate));
        vm.set(p, "uvmAnimLoop", Value::Bool(looping));
        let serial = vm.get(p, "uvmAnimSerial").int() + 1;
        vm.set(p, "uvmAnimSerial", Value::Int(serial));
    }
    let ground = if speed > 0.0 {
        speed
    } else {
        let cdo = vm.cdo(vm.obj(p).class);
        match vm.get(cdo, "GroundSpeed") {
            Value::Float(g) => g,
            _ => 200.0,
        }
    };
    vm.set(p, "mGroundSpeedForward", Value::Float(ground));
    vm.set(p, "GroundSpeed", Value::Float(ground));
    vm.set(p, "bIsWalking", Value::Bool(speed <= 100.0));
    vm.set(p, "mCurrentAnimationInfo", info);
    Value::Float(len)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn script_npcs(
    time: Res<Time>,
    mut m: ResMut<Mutators>,
    mut st: ResMut<ScriptNpcs>,
    phys: Res<Physics>,
    goats: Query<&Transform, With<crate::goat::Goat>>,
    mut npcs: Query<(Entity, &mut Npc, &Transform, &crate::rig::Rig)>,
) {
    if !enabled() {
        return;
    }
    let st = &mut *st;
    let generation = m.generation;
    let Some(h) = m.host.as_mut() else {
        st.driven.clear();
        for (_, mut n, _, _) in &mut npcs {
            n.script = None;
        }
        return;
    };
    if st.generation != generation {
        st.generation = generation;
        st.driven.clear();
    }
    let _prof = super::proxies::Prof(std::time::Instant::now(), "script_npcs");
    let dt = time.delta_secs().min(0.1);
    let goat_obj = h.goat;
    let vm = &mut h.vm;
    // animation lengths for SetAnimationInfoStruct's return value
    if vm.anim_lengths.is_empty() {
        for (_, _, _, rig) in &npcs {
            for (name, &i) in &rig.data.by_name {
                let c = &rig.data.clips[i];
                vm.anim_lengths.entry(name.to_ascii_lowercase()).or_insert(c.length / c.rate.max(0.01));
            }
        }
    }
    // new NPCs: possess their proxies with their AI controllers
    for (e, mut n, t, _) in &mut npcs {
        if st.driven.contains_key(&e) {
            continue;
        }
        let thing = WorldThing::Npc { entity: e.to_bits(), name: n.actor_name.clone() };
        let pawn = vm.proxy_for(&thing, to_ue(t.translation));
        let cv = vm.get(pawn, "ControllerClass");
        let Some(class) = vm.as_class(&cv).or_else(|| vm.class_by_name("GGAIController")) else { continue };
        let ctrl = vm.spawn_actor(class, None);
        vm.set(ctrl, "Pawn", Value::Obj(Some(pawn)));
        vm.set(pawn, "Controller", Value::Obj(Some(ctrl)));
        let yaw = yaw_to_ue(n.yaw);
        let r = vm.rotator(0, yaw, 0);
        vm.set(pawn, "Rotation", r);
        vm.call_named(ctrl, "PreBeginPlay", vec![]);
        vm.call_named(ctrl, "PostBeginPlay", vec![]);
        vm.call_named(ctrl, "Possess", vec![Value::Obj(Some(pawn)), Value::Bool(false)]);
        n.script = Some(ScriptDrive::default());
        st.driven.insert(e, Driven { pawn, ctrl, ragdoll: false, sight: (e.to_bits() % 10) as f32 * 0.05, yaw_written: yaw });
        st.controllers += 1;
    }
    let goat_pos = goats.single().ok().map(|t| t.translation);
    let mut ragdoll_events: Vec<(ObjId, bool)> = vec![];
    // game -> script: rotation, sight
    for (e, n, t, _) in &npcs {
        let Some(d) = st.driven.get_mut(&e) else { continue };
        if vm.obj(d.pawn).destroyed {
            continue;
        }
        let ragdoll = n.state == NpcState::Ragdoll;
        if ragdoll != d.ragdoll {
            d.ragdoll = ragdoll;
            ragdoll_events.push((d.pawn, ragdoll));
        }
        let yaw = yaw_to_ue(n.yaw);
        if yaw != d.yaw_written {
            let r = vm.rotator(0, yaw, 0);
            vm.set(d.pawn, "Rotation", r);
            d.yaw_written = yaw;
        }
        d.sight -= dt;
        if d.sight <= 0.0 && !ragdoll {
            d.sight = 0.5;
            if let (Some(g), Some(gp)) = (goat_obj, goat_pos) {
                let head = t.translation + Vec3::Y * 1.4;
                let near = gp.distance(t.translation) < 60.0;
                let clear = near && crate::npc::clear_line(&phys, head, gp + Vec3::Y * 0.3);
                if std::env::var_os("UVM_NPC_TRACE").is_some() && gp.distance(t.translation) < 8.0 {
                    println!("sight {} dist {:.1} clear {clear}", n.actor_name, gp.distance(t.translation));
                }
                if clear {
                    let trace = vm.trace_calls;
                    vm.trace_calls = std::env::var_os("UVM_NPC_TRACE").is_some();
                    vm.call_named(d.ctrl, "SeePlayer", vec![Value::Obj(Some(g))]);
                    vm.trace_calls = trace;
                }
            }
        }
    }
    // OnRagdoll goes to every controller (people react to others falling over)
    for (pawn, on) in ragdoll_events {
        let ctrls: Vec<ObjId> = st.driven.values().map(|d| d.ctrl).collect();
        for c in ctrls {
            vm.call_named(c, "OnRagdoll", vec![Value::Obj(Some(pawn)), Value::Bool(on)]);
        }
    }
    // controller Tick
    let ticking: Vec<ObjId> = st.driven.values().map(|d| d.ctrl).filter(|&c| !vm.obj(c).destroyed).collect();
    if let Some(first) = ticking.first() {
        let class = vm.obj(*first).class;
        if vm.defines(class, "Tick").is_some() {
            for c in ticking {
                vm.call_named(c, "Tick", vec![Value::Float(dt)]);
            }
        }
    }
    // script -> game: moves, rotation, animations
    for (e, mut n, _, _) in &mut npcs {
        let Some(d) = st.driven.get_mut(&e) else { continue };
        let goal = vm.move_goal(d.ctrl);
        let pawn = d.pawn;
        let speed = match vm.get(pawn, "GroundSpeed") {
            Value::Float(g) if g > 0.0 => g,
            _ => 200.0,
        };
        let walk = goal.is_some_and(|g| g.1) || vm.get(pawn, "bIsWalking").truthy();
        let rot = vm.get(pawn, "Rotation");
        let ue_yaw = vm.rot3(&rot)[1];
        let script_turned = ue_yaw != d.yaw_written;
        let anim = match vm.get(pawn, "uvmAnim") {
            Value::Str(s) => Some((s, vm.get(pawn, "uvmAnimRate").float().max(0.05), vm.get(pawn, "uvmAnimLoop").truthy())),
            _ => None,
        };
        let serial = vm.get(pawn, "uvmAnimSerial").int() as u32;
        let Some(sd) = n.script.as_mut() else { continue };
        sd.goal = goal.map(|(g, _)| convert::pos(g));
        sd.speed = if walk { speed.min(150.0) } else { speed } * convert::S;
        if script_turned {
            sd.yaw = Some(yaw_from_ue(ue_yaw));
        }
        if serial != sd.anim_serial {
            sd.anim_serial = serial;
            sd.anim = anim;
        }
    }
}

static FORCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn forced() -> bool {
    FORCE.load(std::sync::atomic::Ordering::Relaxed)
}

#[derive(Default)]
pub struct NpcVmTest {
    t: f32,
    start: HashMap<Entity, Vec3>,
    mark: f32,
    watched: Option<Entity>,
    serial0: u32,
    shot: bool,
}

/// `--test npcvm`: every NPC gets its script AI controller; the controllers' state code walks NPCs
/// along their scripted routes (FollowRoute -> MoveToward); an idle NPC the goat stands in front
/// of sees it (SeePlayer -> mLastSeenGoat) and reacts.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn npcvm_scenario(
    args: Res<crate::Args>,
    time: Res<Time<Real>>,
    mut t: Local<NpcVmTest>,
    st: Res<ScriptNpcs>,
    m: Res<Mutators>,
    mut phys: ResMut<Physics>,
    npcs: Query<(Entity, &Npc, &Transform), Without<crate::goat::Goat>>,
    mut goats: Query<(&mut crate::goat::Goat, &mut Transform)>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("npcvm") {
        return;
    }
    FORCE.store(true, std::sync::atomic::Ordering::Relaxed);
    t.t += time.delta_secs().min(0.1);
    let now = t.t;
    let fail = |why: &str| -> ! {
        println!("TEST npcvm FAIL at {now:.1}s: {why}");
        std::process::exit(10);
    };
    let n = npcs.iter().count();
    if t.start.is_empty() {
        if n > 0 && st.driven.len() == n {
            for (e, _, tr) in &npcs {
                t.start.insert(e, tr.translation);
            }
            t.mark = now;
            let scripted = npcs.iter().filter(|(_, n, _)| n.script.is_some()).count();
            println!("  t={now:.1} {} NPCs possessed by script controllers ({scripted} driven), {}", st.driven.len(), m.status);
            // stand the goat 4 m in front of the nearest idle (route-less) NPC
            let Ok((mut g, mut gt)) = goats.single_mut() else { fail("no goat") };
            let here = gt.translation;
            // the nearest idle (route-less) NPC with a clear view of a spot 4 m in front of it
            let stand_of = |nt: &Transform| {
                let fwd = nt.rotation * Vec3::NEG_Z;
                nt.translation + Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::Z) * 4.0 + Vec3::Y * 0.3
            };
            let mut idle: Vec<(Entity, &Npc, &Transform)> = npcs.iter().filter(|(_, n, _)| n.route.is_none() && n.protect.is_none()).collect();
            idle.sort_by(|a, b| a.2.translation.distance_squared(here).total_cmp(&b.2.translation.distance_squared(here)));
            let Some(&(e, _, nt)) = idle.iter().find(|(_, _, nt)| crate::npc::clear_line(&phys, nt.translation + Vec3::Y * 1.4, stand_of(nt))) else {
                fail("no idle NPC with a clear view")
            };
            let stand = stand_of(nt);
            if let Some(rb) = phys.world.bodies.get_mut(g.body) {
                rb.set_translation(crate::convert::to_rapier(stand), true);
            }
            gt.translation = stand;
            g.velocity = Vec3::ZERO;
            t.watched = Some(e);
            t.serial0 = npcs.get(e).ok().and_then(|(_, n, _)| n.script.as_ref().map(|s| s.anim_serial)).unwrap_or(0);
        } else if now > 20.0 {
            fail(&format!("controllers for {} of {n} NPCs after 20 s", st.driven.len()));
        }
        return;
    }
    if now - t.mark > 5.5 && !t.shot {
        t.shot = true;
        if let Some(p) = args.shot.clone() {
            commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(p));
        }
    }
    if now - t.mark > 6.0 {
        let routed: Vec<f32> = npcs.iter().filter(|(_, n, _)| n.route.is_some()).filter_map(|(e, _, tr)| t.start.get(&e).map(|s| s.distance(tr.translation))).collect();
        let moving = npcs.iter().filter(|(_, n, _)| n.script.as_ref().is_some_and(|s| s.goal.is_some())).count();
        let walked = routed.iter().filter(|&&d| d > 1.0).count();
        println!("  t={now:.1} route NPCs {} (walked >1 m: {walked}), moving now {moving}", routed.len());
        let anims = npcs.iter().filter(|(_, n, _)| n.script.as_ref().is_some_and(|s| s.anim_serial > 0)).count();
        let mut states = std::collections::BTreeMap::<String, usize>::new();
        let (mut saw, mut reacted, mut wstate) = (false, false, String::new());
        if let Some(h) = m.host.as_ref() {
            let vm = &h.vm;
            for (e, d) in &st.driven {
                let s = vm.obj(d.ctrl).state.map(|y| vm.name_of(y).to_string()).unwrap_or_else(|| "-".into());
                if Some(*e) == t.watched {
                    let y = vm.syms_get("mLastSeenGoat");
                    saw = y.and_then(|y| vm.obj(d.ctrl).props.get(&y)).is_some_and(|v| v.obj().is_some());
                    let serial = npcs.get(*e).ok().and_then(|(_, n, _)| n.script.as_ref().map(|s| s.anim_serial)).unwrap_or(0);
                    reacted = serial != t.serial0 || s != "-";
                    wstate = s.clone();
                }
                *states.entry(s).or_default() += 1;
            }
        }
        println!("  t={now:.1} NPCs with a script animation {anims}; controller states {states:?}");
        println!("  t={now:.1} the NPC in front of the goat: saw it {saw}, reacted {reacted} (state {wstate})");
        if !routed.is_empty() && walked == 0 {
            fail("no route NPC walked under its script controller");
        }
        if !saw {
            fail("the NPC in front of the goat never saw it (SeePlayer)");
        }
        println!("TEST npcvm PASS");
        exit.write(AppExit::Success);
    }
}

/// GGAIControllerBase.ShouldCheckVisibility: the visibility interval ran out
/// (TimeStampVisiblityCheck sets LastTimeStamp and a random CurrentInterval).
pub fn should_check_visibility(vm: &mut Vm, c: &mut super::exec::Call) -> Value {
    let Some(o) = c.this else { return Value::Bool(false) };
    let now = match vm.globals.get("worldinfo").copied() {
        Some(wi) => vm.get(wi, "TimeSeconds").float(),
        None => 0.0,
    };
    let info = vm.get(o, "mVisibilityCheckIntervalInfo");
    let Value::Struct(s) = &info else { return Value::Bool(true) };
    let (yl, yc) = (vm.sym("LastTimeStamp"), vm.sym("CurrentInterval"));
    let last = s.field(yl).map(|v| v.float()).unwrap_or(0.0);
    let cur = s.field(yc).map(|v| v.float()).unwrap_or(0.0);
    Value::Bool(now - last >= cur)
}
