//! `--test apex`: headbutt the nearest reachable APEX destructible; it must break into debris
//! bodies and score "Broke ...". `--test explode`: headbutt an explosive prop; it must explode,
//! ragdoll and launch the goat, score the explosion; then a wreckable car is blown up and must
//! stay as its wreck. Screenshot after the break / blast. Aborts within a few seconds when off
//! script.

use crate::destruct::Destructibles;
use crate::explosive::Explosives;
use crate::goat::{Goat, ScriptedInput};
use crate::physics::Physics;
use crate::score::Score;
use crate::test_tricks::{ground, place};
use crate::Args;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

#[derive(Default)]
pub struct St {
    t: f32,
    frames: u32,
    phase: u32,
    mark: f32,
    seen: Vec<String>,
    target: Vec3,
    wreck: Option<rapier3d::prelude::RigidBodyHandle>,
}

fn fail(name: &str, why: &str, t: f32) -> ! {
    println!("TEST {name} FAIL at {t:.1}s: {why}");
    std::process::exit(10);
}

fn shot(commands: &mut Commands, args: &Args, suffix: &str) {
    if let Some(p) = args.shot.clone() {
        let p = if suffix.is_empty() { p } else { p.replace(".png", &format!("{suffix}.png")) };
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
    }
}

/// Stand the goat on the ground `dist` metres from `p`, facing it. None if there is no ground.
fn stand_by(phys: &mut Physics, g: &mut Goat, tr: &mut Transform, p: Vec3, dist: f32) -> Option<()> {
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let dir = Vec3::new(a.cos(), 0.0, a.sin());
        let at = p - dir * dist;
        let Some(y) = ground(phys, at + Vec3::Y * 1.5) else { continue };
        if (y - p.y).abs() > 0.9 {
            continue;
        }
        place(phys, g, tr, Vec3::new(at.x, y + 0.75, at.z), dir);
        return Some(());
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub fn destruct_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut commands: Commands,
    mut st: Local<St>,
    mut phys: ResMut<Physics>,
    mut input: ResMut<ScriptedInput>,
    score: Res<Score>,
    mut goats: Query<(&mut Goat, &mut Transform)>,
    d: Res<Destructibles>,
    ex: Res<Explosives>,
    fire: Res<crate::fire::FireStats>,
    mut exit: MessageWriter<AppExit>,
) {
    let name = args.test.clone().unwrap_or_default();
    if !matches!(name.as_str(), "apex" | "explode") {
        return;
    }
    st.frames += 1;
    if st.frames < 5 {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    *input = ScriptedInput::default();
    input.cam_dist = Some(5.0);
    input.cam_pitch = Some(-0.3);
    for l in &score.lines {
        if !st.seen.contains(&l.text) {
            println!("  t={t:.2} trick: {} +{}", l.text, l.score);
            st.seen.push(l.text.clone());
        }
    }
    if !tr.translation.is_finite() || tr.translation.y < -150.0 {
        fail(&name, "goat left the world", t);
    }
    match (name.as_str(), st.phase) {
        ("apex", 0) => {
            if d.actors.is_empty() {
                if t > 8.0 {
                    fail(&name, "no destructibles loaded", t);
                }
                return;
            }
            let here = tr.translation;
            let mut cands: Vec<(usize, Vec3)> = d.intact().map(|(i, _)| (i, d.centre(i))).collect();
            cands.sort_by(|a, b| a.1.distance_squared(here).total_cmp(&b.1.distance_squared(here)));
            let Some((i, c)) = cands.into_iter().find(|&(i, c)| {
                let n = d.asset_name(i);
                !n.contains("Roof") && !n.contains("Pillar") && stand_by(&mut phys, &mut g, &mut tr, c, 1.1).is_some()
            }) else {
                fail(&name, "no destructible stands on reachable ground", t)
            };
            println!("{} destructibles; headbutting {} ({}) at {c:.1?}", d.actors.len(), d.actors[i].name, d.asset_name(i));
            st.target = c;
            st.phase = 1;
            st.mark = t;
        }
        ("apex", 1) if t - st.mark > 0.6 => {
            input.headbutt = true;
            st.phase = 2;
            st.mark = t;
        }
        ("apex", 2) => {
            if d.fractures > 0 {
                println!("  t={t:.2} fractured {:?}: {} debris bodies", d.last_fractured, d.debris);
                if d.debris < 2 {
                    fail(&name, "no debris", t);
                }
                st.phase = 3;
                st.mark = t;
            } else if t - st.mark > 1.5 {
                fail(&name, "headbutt did not fracture it", t);
            }
        }
        ("apex", 3) if t - st.mark > 0.5 => {
            shot(&mut commands, &args, "");
            if !st.seen.iter().any(|s| s.starts_with("Broke")) {
                fail(&name, "no \"Broke\" trick in the combo", t);
            }
            st.phase = 4;
            st.mark = t;
        }
        ("apex", 4) if t - st.mark > 0.4 => {
            println!("TEST apex PASS");
            exit.write(AppExit::Success);
            st.phase = 9;
        }
        ("explode", 0) => {
            if ex.items.is_empty() {
                if t > 8.0 {
                    fail(&name, "no explosives loaded", t);
                }
                return;
            }
            let here = tr.translation;
            let mut cands: Vec<(rapier3d::prelude::RigidBodyHandle, String, Vec3)> =
                ex.list(&phys).filter(|(h, _, _)| !ex.is_wreckable(*h)).map(|(h, n, p)| (h, n.to_string(), p)).collect();
            cands.sort_by(|a, b| a.2.distance_squared(here).total_cmp(&b.2.distance_squared(here)));
            st.wreck = ex.list(&phys).find(|(h, _, _)| ex.is_wreckable(*h)).map(|(h, _, _)| h);
            let Some((_, n, p)) = cands.into_iter().find(|(_, _, p)| stand_by(&mut phys, &mut g, &mut tr, *p, 1.0).is_some()) else {
                fail(&name, "no explosive stands on reachable ground", t)
            };
            println!("{} explosives; headbutting {n} at {p:.1?}", ex.items.len());
            st.target = p;
            st.phase = 1;
            st.mark = t;
        }
        ("explode", 1) if t - st.mark > 0.6 => {
            input.headbutt = true;
            st.phase = 2;
            st.mark = t;
        }
        ("explode", 2) => {
            if ex.exploded > 0 {
                println!("  t={t:.2} exploded {:?}", ex.last);
                st.phase = 3;
                st.mark = t;
            } else if t - st.mark > 1.5 {
                fail(&name, "headbutt did not set it off", t);
            }
        }
        ("explode", 3) if t - st.mark > 0.35 => {
            shot(&mut commands, &args, "");
            let launched = g.ragdoll;
            println!("  t={t:.2} goat ragdoll {launched}, {:.1} m from the blast", tr.translation.distance(st.target));
            if !launched {
                fail(&name, "the blast did not ragdoll the goat", t);
            }
            println!("  pawns set on fire: {}", fire.ignited);
            if fire.ignited == 0 {
                fail(&name, "the blast set nobody on fire", t);
            }
            if !st.seen.iter().any(|s| s.contains("Explosion")) && !score.lines.iter().any(|l| l.score == 10) {
                println!("  (no Explosion line in the combo: it has no name)");
            }
            st.phase = 4;
            st.mark = t;
        }
        ("explode", 4) if t - st.mark > 0.3 => {
            match st.wreck {
                Some(h) => {
                    crate::explosive::force(&ex, &mut phys, h, false);
                    st.phase = 5;
                }
                None => {
                    println!("  no wreckable explosives in this map");
                    st.phase = 6;
                }
            }
            st.mark = t;
        }
        ("explode", 5) if t - st.mark > 0.5 => {
            let h = st.wreck.unwrap();
            if ex.items.contains_key(&h) {
                fail(&name, "the car did not explode", t);
            }
            if phys.world.bodies.get(h).is_none() {
                fail(&name, "the car vanished instead of leaving its wreck", t);
            }
            println!("  t={t:.2} car exploded and stayed as a wreck ({} explosions)", ex.exploded);
            st.phase = 6;
            st.mark = t;
        }
        ("explode", 6) => {
            // look down at the scorch mark
            input.cam_dist = Some(9.0);
            input.cam_pitch = Some(-1.25);
            if t - st.mark > 1.2 {
                shot(&mut commands, &args, "_scorch");
                st.phase = 7;
                st.mark = t;
            }
        }
        ("explode", 7) if t - st.mark > 0.3 => {
            println!("TEST explode PASS");
            exit.write(AppExit::Success);
            st.phase = 9;
        }
        _ => {}
    }
}

/// `--test ladder`: the goat at the foot of the nearest LadderVolume, facing its WallDir, holds
/// forward: PHYS_Ladder must lift it (> 1.5 m in 3 s); screenshot <shot>.
#[allow(clippy::too_many_arguments)]
pub fn ladder_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut commands: Commands,
    mut st: Local<St>,
    mut phys: ResMut<Physics>,
    mut input: ResMut<ScriptedInput>,
    mut goats: Query<(&mut Goat, &mut Transform)>,
    vols: Res<crate::physics::volumes::Volumes>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("ladder") {
        return;
    }
    st.frames += 1;
    if st.frames < 5 {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    *input = ScriptedInput::default();
    input.cam_dist = Some(5.0);
    input.cam_pitch = Some(-0.2);
    match st.phase {
        0 => {
            let here = tr.translation;
            let mut ladders: Vec<(Vec3, Vec3, f32)> = vols
                .others
                .iter()
                .filter_map(|v| match v.kind {
                    crate::physics::volumes::Kind::Ladder { wall_dir } => {
                        let (pose, s) = &v.hulls[0];
                        let bb = s.compute_aabb(pose);
                        let c = crate::convert::from_rapier((bb.mins + bb.maxs) * 0.5);
                        Some((Vec3::new(c.x, bb.mins.y, c.z), wall_dir, bb.maxs.y - bb.mins.y))
                    }
                    _ => None,
                })
                .collect();
            ladders.sort_by(|a, b| a.0.distance_squared(here).total_cmp(&b.0.distance_squared(here)));
            let Some(&(foot, wall, height)) = ladders.iter().find(|(f, _, h)| *h > 2.0 && ground(&phys, *f + Vec3::Y * 0.5).is_some()) else {
                fail("ladder", "no ladder volume with ground under it", t)
            };
            let y = ground(&phys, foot + Vec3::Y * 0.5).unwrap();
            println!("{} ladders; using one {height:.1} m tall at {foot:.1?}, wall {wall:.2?}", ladders.len());
            place(&mut phys, &mut g, &mut tr, Vec3::new(foot.x, y + 0.75, foot.z), wall);
            st.target = tr.translation;
            st.phase = 1;
            st.mark = t;
        }
        1 => {
            input.forward = 1.0;
            input.cam_yaw = Some(g.yaw);
            let rise = tr.translation.y - st.target.y;
            if rise > 1.5 {
                println!("  t={t:.1} climbed {rise:.1} m (env {:?})", g.env.ladder.is_some());
                if let Some(p) = args.shot.clone() {
                    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                }
                st.phase = 2;
                st.mark = t;
            } else if t - st.mark > 3.0 {
                fail("ladder", &format!("rose only {rise:.2} m (in ladder volume: {})", g.env.ladder.is_some()), t);
            }
        }
        2 if t - st.mark > 0.3 => {
            println!("TEST ladder PASS");
            exit.write(AppExit::Success);
            st.phase = 3;
        }
        _ => {}
    }
}

/// `--test water` (Goat City Bay): the goat, set on fire, is put in the biggest water volume and an
/// NPC next to it: the fire must go out, the NPC must drown (ragdoll), and the ragdolled goat's
/// jump must be a ragdoll jump up out of the water.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn water_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut commands: Commands,
    mut st: Local<St>,
    mut phys: ResMut<Physics>,
    mut input: ResMut<ScriptedInput>,
    mut goats: Query<(Entity, &mut Goat, &mut Transform), Without<crate::npc::Npc>>,
    mut npcs: Query<(Entity, &crate::npc::Npc, &mut Transform), Without<Goat>>,
    vols: Res<crate::physics::volumes::Volumes>,
    stats: Res<crate::water::WaterStats>,
    burning: Query<(), With<crate::fire::Burning>>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("water") {
        return;
    }
    st.frames += 1;
    if st.frames < 5 {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    let Ok((ge, mut g, mut tr)) = goats.single_mut() else { return };
    *input = ScriptedInput::default();
    input.cam_dist = Some(5.0);
    input.cam_pitch = Some(-0.35);
    match st.phase {
        0 if t > 0.5 => {
            commands.entity(ge).insert(crate::fire::Ignite);
            st.phase = 1;
        }
        1 if t > 1.2 => {
            if burning.get(ge).is_err() {
                fail("water", "the goat did not catch fire", t);
            }
            // the biggest water volume, just under its surface
            let best = vols
                .others
                .iter()
                .filter(|v| matches!(v.kind, crate::physics::volumes::Kind::Water))
                .flat_map(|v| v.hulls.iter().map(|(pose, s)| s.compute_aabb(pose)))
                .max_by(|a, b| {
                    let area = |x: &rapier3d::parry::bounding_volume::Aabb| (x.maxs.x - x.mins.x) * (x.maxs.z - x.mins.z);
                    area(a).total_cmp(&area(b))
                });
            let Some(aabb) = best else { fail("water", "no water volume on this map", t) };
            let c = crate::convert::from_rapier((aabb.mins + aabb.maxs) * 0.5);
            let at = Vec3::new(c.x, aabb.maxs.y - 0.7, c.z);
            println!("  water at {at:.1?} (volume {:.0} x {:.0} m, {:.1} m deep)", aabb.maxs.x - aabb.mins.x, aabb.maxs.z - aabb.mins.z, aabb.maxs.y - aabb.mins.y);
            place(&mut phys, &mut g, &mut tr, at, Vec3::X);
            // the nearest NPC goes in next to the goat
            let near = npcs.iter_mut().min_by(|a, b| a.2.translation.distance(at).total_cmp(&b.2.translation.distance(at)));
            if let Some((e, _, mut nt)) = near {
                nt.translation = at + Vec3::X * 2.0;
                st.wreck = None;
                st.seen = vec![format!("{e:?}")];
            }
            st.target = at;
            st.phase = 2;
        }
        2 if t > 2.8 => {
            if !g.env.water {
                fail("water", &format!("the goat is not in the water (at {:.1?})", tr.translation), t);
            }
            if burning.get(ge).is_ok() || stats.fires_out == 0 {
                fail("water", "the water did not put the fire out", t);
            }
            if stats.npcs == 0 || !npcs.iter().any(|(e, n, _)| st.seen.contains(&format!("{e:?}")) && n.state == crate::npc::NpcState::Ragdoll) {
                fail("water", "the NPC in the water did not drown (ragdoll)", t);
            }
            println!("  t={t:.2} goat in water, fire out, NPC drowning; ragdolling the goat");
            shot(&mut commands, &args, "");
            input.ragdoll = true;
            st.phase = 3;
        }
        3 if t > 4.0 => {
            if !g.ragdoll {
                fail("water", "Q did not ragdoll the goat", t);
            }
            // wait until it floats steadily in the water
            let settled = g.env.water && (tr.translation.y - st.mark).abs() < 0.01;
            st.mark = tr.translation.y;
            if settled {
                println!("  t={t:.2} floating at y {:.2}", tr.translation.y);
                input.jump = true;
                st.phase = 4;
                st.wreck = None;
                st.target.y = t;
            } else if t > 9.0 {
                fail("water", "the ragdolled goat never settled in the water", t);
            }
        }
        4 if t > st.target.y + 0.35 => {
            let rise = tr.translation.y - st.mark;
            println!("  t={t:.2} ragdoll jump in water rose {rise:.2} m (still ragdoll: {})", g.ragdoll);
            if !g.ragdoll {
                fail("water", "Space in the water stood the goat up instead of a ragdoll jump", t);
            }
            if rise < 0.3 {
                fail("water", "the ragdoll jump did not lift the goat", t);
            }
            st.phase = 5;
            st.mark = t;
        }
        5 if t > st.mark + 0.5 => {
            println!("TEST water PASS");
            exit.write(AppExit::Success);
            st.phase = 6;
        }
        _ => {}
    }
    if st.phase == 4 && st.frames % 6 == 0 {
        let ray = rapier3d::prelude::Ray::new(crate::convert::to_rapier(tr.translation), rapier3d::prelude::Vector::Y);
        if let Some((c, hit)) = phys.world.cast_ray_and_get_normal(&ray, 5.0, true, rapier3d::prelude::QueryFilter::only_fixed()) {
            let surf = phys.surface_at(c, hit.feature).map(|s| s.kind.clone());
            println!("    above the goat: collider {:?} at {:.2} m, surface {surf:?}, normal {:.2?}", c.into_raw_parts(), hit.time_of_impact, hit.normal);
        }
    }
    if (3.0..9.5).contains(&t) && st.frames % 12 == 0 {
        println!("    t={t:.2} goat y {:.2} ragdoll {} env water {} grav {:?}", tr.translation.y, g.ragdoll, g.env.water, g.env.gravity_y);
    }
    if t > 12.0 {
        fail("water", &format!("stuck in phase {}", st.phase), t);
    }
}

/// `--test baseball`: Kismet starts the pitching machine (balls fly on their own); the goat walks
/// into it and wears it; R pitches; a person put in the ball's path is hit: "Beanball at ...".
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn baseball_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut commands: Commands,
    mut st: Local<St>,
    mut phys: ResMut<Physics>,
    mut input: ResMut<ScriptedInput>,
    mut goats: Query<(&mut Goat, &mut Transform), (Without<crate::npc::Npc>, Without<crate::pitch::PitchingMachine>)>,
    mut npcs: Query<(&crate::ragdoll::CharacterBody, &mut Transform), (With<crate::npc::Npc>, Without<Goat>, Without<crate::pitch::PitchingMachine>)>,
    machines: Query<(&crate::pitch::PitchingMachine, &Transform)>,
    balls: Query<&crate::pitch::Baseball>,
    pitching: Res<crate::pitch::Pitching>,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    chains: Res<crate::chain::Chains>,
    score: Res<Score>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("baseball") {
        return;
    }
    st.frames += 1;
    if st.frames < 5 {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    *input = ScriptedInput::default();
    input.cam_dist = Some(5.0);
    input.cam_pitch = Some(-0.3);
    let Some((m, mt)) = machines.iter().next() else { fail("baseball", "no pitching machine on this map", t) };
    match st.phase {
        // watch the machine pitch by itself for a while
        0 => {
            if t > 0.5 && st.mark == 0.0 {
                // TriggerVolume_30 (Kismet Touch -> GGSeqAct_StartPitchingMachine): the batter's box
                let reg = &game.as_ref().expect("game registry").0;
                let map = &reg.packages[0];
                let box_at = (0..map.exports.len())
                    .find(|&i| map.obj_name(i as i32 + 1) == "TriggerVolume_30")
                    .and_then(|i| match upk::registry::find(&reg.merged_props(upk::registry::ObjRef { pkg: 0, export: i }), "Location") {
                        Some((_, upk::props::Value::Vector(v))) => Some(crate::convert::pos(*v)),
                        _ => None,
                    });
                let at = box_at.unwrap_or(mt.translation + Vec3::Z * 4.0) + Vec3::Y * 0.5;
                println!("  batter's box at {at:.1?}, machine at {:.1?}", mt.translation);
                st.target = at;
                place(&mut phys, &mut g, &mut tr, at, (mt.translation - at).with_y(0.0).normalize_or(Vec3::X));
                st.mark = 1.0;
            }
            if pitching.total > 0 && st.seen.is_empty() {
                let v = balls.iter().filter_map(|b| phys.world.bodies.get(b.body)).map(|rb| crate::convert::from_rapier(rb.linvel())).next().unwrap_or_default();
                let to_goat = (tr.translation - mt.translation).normalize_or_zero();
                println!("  t={t:.2} machine pitched {:.2?} at {:.0} m/s; toward the goat {:.2?} (dot {:.2})", v.normalize_or_zero(), v.length(), to_goat, v.normalize_or_zero().dot(to_goat));
                st.seen.push("pitched".into());
            }
            if pitching.total > 1 && t > 1.0 {
                println!("  t={t:.2} the machine pitched {} balls by itself", pitching.total);
                st.phase = 1;
            } else if t > 7.0 {
                println!("  (the machine did not pitch by itself: Kismet never started it)");
                st.phase = 1;
            }
        }
        1 => {
            // walk into it
            let at = mt.translation + Vec3::Y * 0.8;
            place(&mut phys, &mut g, &mut tr, at, Vec3::X);
            st.phase = 2;
            st.mark = t;
        }
        2 => {
            if m.attached {
                println!("  t={t:.2} wearing the pitching machine");
                // out of the cage: the batter's box, facing the most open direction
                let at = st.target;
                let mut best = (0.0f32, Vec3::X);
                for k in 0..48 {
                    let a = k as f32 / 48.0 * std::f32::consts::TAU;
                    let dir = Vec3::new(a.cos(), 0.0, a.sin());
                    let ray = rapier3d::prelude::Ray::new(crate::convert::to_rapier(at + Vec3::Y * 1.0), crate::convert::to_rapier(dir));
                    let free = phys.world.cast_ray(&ray, 30.0, true, rapier3d::prelude::QueryFilter::only_fixed()).map(|(_, toi)| toi).unwrap_or(30.0);
                    if free > best.0 {
                        best = (free, dir);
                    }
                }
                println!("  facing {:.2?} ({:.0} m clear)", best.1, best.0);
                place(&mut phys, &mut g, &mut tr, at, best.1);
                input.cam_yaw = Some(f32::atan2(-best.1.x, -best.1.z));
                st.phase = 3;
                st.mark = t + 0.3;
                st.seen.clear();
            } else if t > st.mark + 1.5 {
                fail("baseball", "touching the machine did not put it on the goat", t);
            }
        }
        3 if t > st.mark + 0.1 => {
            shot(&mut commands, &args, "_wear");
            input.special = true;
            st.phase = 4;
            st.mark = t;
            st.wreck = None;
        }
        4 => {
            // the newest goat ball: put a person just ahead of it
            let newest = balls.iter().filter_map(|b| phys.world.bodies.get(b.body).map(|rb| (b.body, rb))).max_by_key(|(h, _)| h.into_raw_parts().0);
            if let Some((h, rb)) = newest {
                if st.wreck != Some(h) && pitching.by_goat >= 1 {
                    let v = crate::convert::from_rapier(rb.linvel());
                    let p = crate::convert::from_rapier(rb.translation());
                    let target = p + v.normalize_or_zero() * 1.5;
                    println!("  t={t:.2} ball flies {:.2?} at {:.0} m/s", v.normalize_or_zero(), v.length());
                    if let Some((cb, mut nt)) = npcs.iter_mut().min_by(|a, b| a.1.translation.distance(tr.translation).total_cmp(&b.1.translation.distance(tr.translation))) {
                        nt.translation = target - Vec3::Y * 0.3;
                        if let Some(nb) = phys.world.bodies.get_mut(cb.body) {
                            nb.set_translation(crate::convert::to_rapier(nt.translation), true);
                        }
                    }
                    st.wreck = Some(h);
                    st.phase = 5;
                    st.mark = t;
                }
            }
            if t > st.mark + 1.0 {
                fail("baseball", "the second pitch never came", t);
            }
        }
        5 => {
            if chains.beanballs > 0 {
                let line = score.lines.iter().find(|l| l.text.contains("Beanball")).map(|l| (l.text.clone(), l.score));
                println!("  t={t:.2} {line:?}");
                shot(&mut commands, &args, "");
                println!("TEST baseball PASS");
                exit.write(AppExit::Success);
                st.phase = 6;
            } else if t > st.mark + 2.0 {
                fail("baseball", &format!("the ball did not hit the person ({} goat balls)", pitching.by_goat), t);
            }
        }
        _ => {}
    }
    if t > 15.0 {
        fail("baseball", &format!("stuck in phase {}", st.phase), t);
    }
}

/// `--test race` (Goat City Bay RegularRace via GOAT_GAME_OPTIONS): the goat can't move during the
/// countdown; then it is put into each checkpoint in turn; the script must pass them in order and
/// stop the clock at the last one.
#[allow(clippy::too_many_arguments)]
pub fn race_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut commands: Commands,
    mut st: Local<St>,
    mut phys: ResMut<Physics>,
    mut input: ResMut<ScriptedInput>,
    mut goats: Query<(&mut Goat, &mut Transform)>,
    race: Res<crate::race::RaceState>,
    riding: Res<crate::vehicle::Riding>,
    bikes: Query<&crate::vehicle::Bike>,
    mut exit: MessageWriter<AppExit>,
) {
    let bike_race = args.test.as_deref() == Some("bikerace");
    if args.test.as_deref() != Some("race") && !bike_race {
        return;
    }
    st.frames += 1;
    if st.frames < 5 {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    *input = ScriptedInput::default();
    input.cam_dist = Some(6.0);
    let Some(view) = crate::uvm::race::view() else {
        if t > 3.0 {
            fail("race", "no race running (GGGameInfoRace)", t);
        }
        return;
    };
    match st.phase {
        0 => {
            // RaceCountdown: W does nothing
            if st.mark == 0.0 {
                st.target = tr.translation;
                st.mark = 1.0;
            }
            input.forward = 1.0;
            if view.frozen {
                let moved = tr.translation.with_y(0.0).distance(st.target.with_y(0.0));
                if moved > 0.3 {
                    fail("race", &format!("the goat moved {moved:.1} m during the countdown"), t);
                }
            } else if view.running {
                println!("  t={t:.2} GOAT! ({} checkpoints, {} shown)", view.locations.len(), race.shown);
                if view.locations.is_empty() {
                    fail("race", "the race has no checkpoints", t);
                }
                st.phase = if bike_race { 10 } else { 1 };
                st.mark = t;
            }
            if t > 8.0 {
                fail("race", "the countdown never ended", t);
            }
        }
        // BikeRace: get on the bike the race put in front of the goat
        10 => {
            if riding.bike.is_some() {
                println!("  t={t:.2} on the race bike");
                st.phase = 1;
                st.mark = t;
            } else {
                input.special = st.frames % 10 == 0;
                if t > st.mark + 3.0 {
                    fail("race", "could not get on the race bike", t);
                }
            }
        }
        1 => {
            // keep a bike upright (press against its lean now and then)
            if let Some(b) = riding.bike.and_then(|e| bikes.get(e).ok()) {
                let w = b.balance.blend.weight;
                if (w - 0.5).abs() > 0.06 && st.frames % 6 == 0 {
                    input.right = if b.balance.fall_dir == 0 { 1.0 } else { -1.0 };
                }
            }
            // into the next checkpoint not passed yet
            let next = view.passed.iter().position(|p| !p);
            match next {
                Some(i) if t > st.mark + 0.25 => {
                    let at = view.locations[i] + Vec3::Y * 0.5;
                    if std::env::var_os("UVM_TOUCH_TRACE").is_some() {
                        println!("    goat -> checkpoint {i} at {at:.1?}");
                    }
                    match riding.bike.and_then(|e| bikes.get(e).ok()) {
                        // ride the bike there (the bike is the pawn that counts)
                        Some(b) => {
                            // upright, a little above the gate's floor, facing the next one
                            let next_at = view.locations.get(i + 1).copied().unwrap_or(at + Vec3::X);
                            let dir = (next_at - at).with_y(0.0).normalize_or(Vec3::X);
                            if let Some(rb) = phys.world.bodies.get_mut(b.body) {
                                let pose = rapier3d::prelude::Pose::from_parts(crate::convert::to_rapier(at + Vec3::Y * 1.0), crate::convert::quat_to_rapier(Quat::from_rotation_arc(Vec3::X, dir)));
                                rb.set_position(pose, true);
                                rb.set_linvel(rapier3d::prelude::Vector::ZERO, true);
                                rb.set_angvel(rapier3d::prelude::Vector::ZERO, true);
                            }
                        }
                        None => place(&mut phys, &mut g, &mut tr, at, Vec3::X),
                    }
                    if bike_race && riding.bike.is_none() {
                        fail("race", &format!("fell off the bike ({} times)", riding.thrown), t);
                    }
                    st.mark = t;
                    if i == view.locations.len() / 2 && !st.seen.contains(&"shot".to_string()) {
                        shot(&mut commands, &args, "");
                        st.seen.push("shot".into());
                    }
                }
                Some(_) => {}
                None => {
                    if view.time_ms > 0 {
                        println!("  t={t:.2} finished: {} checkpoints, race time {:.1} s", view.locations.len(), view.time_ms as f32 / 1000.0);
                        st.phase = 2;
                        st.mark = t;
                    }
                }
            }
            if t > 4.0 + view.locations.len() as f32 * 0.6 + 6.0 {
                let n = view.passed.iter().filter(|p| **p).count();
                fail("race", &format!("only {n} of {} checkpoints passed", view.locations.len()), t);
            }
        }
        2 => {
            // EndRace: the MID_RACE_ENDED popup (it pauses the game, so no clock here)
            if crate::uvm::menus::ui_open() {
                shot(&mut commands, &args, "_end");
                println!("TEST {} PASS (race-ended popup open)", if bike_race { "bikerace" } else { "race" });
                exit.write(AppExit::Success);
                st.phase = 3;
            } else if st.frames > st.seen.len() as u32 + 100_000 {
                fail("race", "no race-ended popup", t);
            }
        }
        _ => {}
    }
}

/// `--test racepopup` (Goat City Bay, free roam): standing in GGTriggerVolumeRace_1 (RegularRace)
/// opens the script's "start the race?" popup after mStartDelay.
#[allow(clippy::too_many_arguments)]
pub fn racepopup_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut commands: Commands,
    mut st: Local<St>,
    mut phys: ResMut<Physics>,
    mut input: ResMut<ScriptedInput>,
    mut goats: Query<(&mut Goat, &mut Transform)>,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("racepopup") {
        return;
    }
    st.frames += 1;
    if st.frames < 5 {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    *input = ScriptedInput::default();
    if st.phase == 0 && t > 1.0 {
        let reg = &game.as_ref().expect("game registry").0;
        let map = &reg.packages[0];
        let at = (0..map.exports.len()).find(|&i| map.obj_name(i as i32 + 1) == "GGTriggerVolumeRace_1").and_then(|i| {
            match upk::registry::find(&reg.merged_props(upk::registry::ObjRef { pkg: 0, export: i }), "Location") {
                Some((_, upk::props::Value::Vector(v))) => Some(crate::convert::pos(*v)),
                _ => None,
            }
        });
        let Some(at) = at else { fail("racepopup", "no GGTriggerVolumeRace_1 on this map", t) };
        place(&mut phys, &mut g, &mut tr, at + Vec3::Y * 0.5, Vec3::X);
        st.phase = 1;
        st.mark = t;
    }
    if st.phase == 1 {
        if crate::uvm::menus::ui_open() {
            shot(&mut commands, &args, "");
            println!("TEST racepopup PASS");
            exit.write(AppExit::Success);
            st.phase = 2;
        } else if t > st.mark + 8.0 {
            fail("racepopup", "no popup after standing in the race volume", t);
        }
    }
}
