//! NPC reaction tests:
//! `--test reactions`: flip in front of someone (applause), then stand near someone else (notice).
//! `--test attack` (Level_02 by default): stand on an item a person guards and get kicked off it.
//! `--test route` (Level_02 by default): people with scripted routes walk from node to node.
//! `--test trip`: sprint into a prop whose material trips the goat (mShouldTripGoat) -> ragdoll.

use crate::Args;
use crate::goat::{Goat, ScriptedInput};
use crate::npc::{Npc, NpcState};
use crate::physics::Physics;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

#[derive(Default)]
pub struct NpcSt {
    t: f32,
    frames: u32,
    phase: u32,
    npc: Option<Entity>,
    other: Option<Entity>,
    mark: f32,
    shot: bool,
    last_pos: Vec3,
    mark2: f32,
    still: f32,
    yaw: f32,
}

fn fail(name: &str, why: &str, t: f32) -> ! {
    println!("TEST {name} FAIL at {t:.1}s: {why}");
    std::process::exit(10);
}

/// Flat open ground 6 m on the -dir side of a person, at the person's ground level.
fn open_spot(phys: &Physics, p: Vec3, dir: Vec3) -> Option<Vec3> {
    let spot = p - dir * 6.0;
    let ray = rapier3d::prelude::Ray::new(crate::convert::to_rapier(spot + Vec3::Y * 20.0), rapier3d::prelude::Vector::NEG_Y);
    let (_, hit) = phys.world.cast_ray_and_get_normal(&ray, 60.0, true, rapier3d::prelude::QueryFilter::only_fixed())?;
    let y = spot.y + 20.0 - hit.time_of_impact;
    let flat = hit.normal.y > 0.985;
    (flat && (y - (p.y - 0.9)).abs() < 1.0).then_some(Vec3::new(spot.x, y + 0.7, spot.z))
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn npc_reactions_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut commands: Commands,
    mut st: Local<NpcSt>,
    mut phys: ResMut<Physics>,
    mut input: ResMut<ScriptedInput>,
    mut goats: Query<(&mut Goat, &mut Transform), Without<Npc>>,
    npcs: Query<(Entity, &Npc, &Transform, &Name)>,
    mut exit: MessageWriter<AppExit>,
    log: Res<crate::audio::SoundLog>,
) {
    let name = args.test.clone().unwrap_or_default();
    if !matches!(name.as_str(), "reactions" | "attack" | "route" | "trip") {
        return;
    }
    st.frames += 1;
    if st.frames < 5 {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    *input = ScriptedInput::default();
    let here = tr.translation;
    let mut snap = |st: &mut NpcSt| {
        if !st.shot {
            st.shot = true;
            if let Some(p) = args.shot.clone() {
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
            }
        }
    };
    let side_cam = -std::f32::consts::FRAC_PI_2 + 0.9;
    if std::env::var_os("GOAT_TRACE").is_some() && st.phase >= 3 && st.phase < 8 {
        println!("    t={t:.2} ph={} grounded={} ragdoll={} pitch={:.0} vel={:.2?} pos={:.2?}", st.phase, g.grounded, g.ragdoll, g.air_pitch_deg, g.velocity, tr.translation);
    }
    match (name.as_str(), st.phase) {
        ("reactions", 0) if t > 0.5 => {
            // two calm people who can be approached on open ground
            let mut cands: Vec<(Entity, Vec3, f32)> = npcs
                .iter()
                .filter(|(_, n, _, _)| n.state == NpcState::Idle && n.protect.is_none() && n.react.applaud_and_notice && !n.seen_goat)
                .filter_map(|(e, _, nt, _)| open_spot(&phys, nt.translation, Vec3::X).map(|_| (e, nt.translation, nt.translation.distance(here))))
                .collect();
            cands.sort_by(|a, b| a.2.total_cmp(&b.2));
            if cands.len() < 2 {
                fail(&name, "need approachable NPCs", t);
            }
            let (e, p, _) = cands[0];
            let other = cands.iter().find(|c| c.1.distance(p) > 60.0).map(|c| c.0).unwrap_or(cands[1].0);
            let _ = other;
            let spot = open_spot(&phys, p, Vec3::X).unwrap();
            crate::test_tricks::place(&mut phys, &mut g, &mut tr, spot, Vec3::X);
            st.npc = Some(e);
            st.other = Some(other);
            st.phase = 1;
            st.mark = t;
            println!("flipping for {}", npcs.get(e).map(|x| x.3.to_string()).unwrap_or_default());
        }
        ("reactions", 1) => {
            // first the person notices the goat (SeePlayer)
            input.cam_yaw = Some(side_cam);
            input.cam_dist = Some(7.0);
            let (_, n, _, nm) = npcs.get(st.npc.unwrap()).unwrap();
            if n.state == NpcState::Look && n.reason == "notice" {
                println!("  t={t:.2} {nm} noticed the goat");
                st.phase = 2;
                st.mark = t;
            } else if t - st.mark > 5.0 {
                fail(&name, &format!("goat not noticed (state {:?}, seen {})", n.state, n.seen_goat), t);
            }
        }
        ("reactions", 2) => {
            // wait out the notice and the interaction cool-down (7-15 s), then flip
            input.cam_yaw = Some(side_cam);
            input.cam_dist = Some(7.0);
            let now = time.elapsed_secs();
            let (_, n, _, _) = npcs.get(st.npc.unwrap()).unwrap();
            if n.state == NpcState::Idle && n.applaud.ready(now) && n.interaction.ready(now) && g.grounded {
                input.jump = true;
                st.phase = 3;
                st.mark = t;
            } else if t - st.mark > 25.0 {
                fail(&name, &format!("person never became ready to applaud (state {:?})", n.state), t);
            }
        }
        ("reactions", 3) => {
            input.cam_yaw = Some(side_cam);
            input.cam_dist = Some(7.0);
            if !g.grounded {
                let flipping = g.air_pitch_deg < 345.0;
                input.freelook = flipping;
                input.forward = if flipping { 1.0 } else { 0.0 };
            } else if t - st.mark > 0.3 {
                if g.ragdoll {
                    fail(&name, "the flip landed in ragdoll", t);
                }
                st.phase = 4;
                st.mark = t;
            }
        }
        ("reactions", 4) => {
            input.cam_yaw = Some(side_cam);
            input.cam_dist = Some(7.0);
            let (_, n, _, nm) = npcs.get(st.npc.unwrap()).unwrap();
            if matches!(n.state, NpcState::Applaud | NpcState::Look) && n.reason == "applaud" {
                println!("  t={t:.2} {nm} applauds the flip: {:?}", n.state);
                snap(&mut st);
                println!("TEST reactions PASS");
                st.phase = 8;
                st.mark = t;
            } else if t - st.mark > 1.5 {
                fail(&name, &format!("no applause for the flip (state {:?}, reason {:?})", n.state, n.reason), t);
            }
        }
        ("attack", 0) if t > 0.5 => {
            let pick = npcs
                .iter()
                .filter(|(_, n, _, _)| n.protect.is_some())
                .min_by(|a, b| a.1.protect.unwrap().item.distance_squared(here).total_cmp(&b.1.protect.unwrap().item.distance_squared(here)));
            let Some((e, n, nt, nm)) = pick else { fail(&name, "no guards on this map (use Level_02)", t) };
            let item = n.protect.unwrap().item;
            // stand right next to the guarded item, on the guard's side
            let to_npc = Vec3::new(nt.translation.x - item.x, 0.0, nt.translation.z - item.z).normalize_or(Vec3::X);
            let spot = item + to_npc * 1.0;
            let y = crate::test_tricks::ground(&phys, spot).unwrap_or(item.y);
            crate::test_tricks::place(&mut phys, &mut g, &mut tr, Vec3::new(spot.x, y + 0.7, spot.z), to_npc);
            st.npc = Some(e);
            st.phase = 1;
            st.mark = t;
            println!("guard {nm} is {:.1} m from its item (radius {:.1} m)", nt.translation.distance(item), n.protect.unwrap().radius);
        }
        ("attack", 1) => {
            input.cam_dist = Some(7.0);
            input.cam_pitch = Some(-0.3);
            let (_, n, _, _) = npcs.get(st.npc.unwrap()).unwrap();
            if matches!(n.state, NpcState::Attack | NpcState::Angry) {
                snap(&mut st);
            }
            if n.state == NpcState::Angry || g.ragdoll {
                println!("  t={t:.2} guard {:?}, goat speed {:.1} m/s, ragdoll {}", n.state, g.velocity.length(), g.ragdoll);
                println!("TEST attack PASS");
                st.phase = 8;
                st.mark = t;
            } else if t - st.mark > 8.0 {
                fail(&name, &format!("guard never kicked (state {:?})", n.state), t);
            }
        }
        ("route", 0) if t > 0.5 => {
            let routed: Vec<(Entity, &Npc, &Transform, &Name)> = npcs.iter().filter(|(_, n, _, _)| n.route.is_some()).collect();
            if routed.is_empty() {
                fail(&name, "no people with scripted routes (use Level_02)", t);
            }
            let inter: usize = routed.iter().map(|(_, n, _, _)| n.route.as_ref().unwrap().interacts.len()).sum();
            // track someone far from the goat so it isn't distracted by noticing it
            let (e, n, nt, nm) = routed.iter().max_by(|a, b| a.2.translation.distance_squared(here).total_cmp(&b.2.translation.distance_squared(here))).unwrap();
            let r = n.route.as_ref().unwrap();
            println!(
                "{} people walk routes ({} interact items); tracking {nm}: {} nodes, {:?}, {:.0} uu/s",
                routed.len(),
                inter,
                r.nodes.len(),
                r.kind,
                r.speed / crate::convert::S
            );
            st.npc = Some(*e);
            st.last_pos = nt.translation;
            st.phase = 1;
            st.mark = t;
        }
        ("route", 1) => {
            let (_, n, nt, _) = npcs.get(st.npc.unwrap()).unwrap();
            let r = n.route.as_ref().unwrap();
            // early abort: walking but not moving for 4 s
            if n.state == NpcState::FollowRoute && nt.translation.distance(st.last_pos) < 0.01 {
                st.still += dt;
                if st.still > 4.0 {
                    fail(&name, &format!("stuck at {:.2?} heading for node {}", nt.translation, r.index), t);
                }
            } else {
                st.still = 0.0;
            }
            st.last_pos = nt.translation;
            if r.reached >= 2 {
                let moving = npcs.iter().filter(|(_, n, _, _)| n.route.as_ref().is_some_and(|r| r.reached > 0)).count();
                let total = npcs.iter().filter(|(_, n, _, _)| n.route.is_some()).count();
                println!("  t={t:.2} reached 2 nodes; {moving}/{total} route walkers have reached a node");
                if moving * 2 < total {
                    fail(&name, "most route walkers never reached a node", t);
                }
                // look at the walker for the screenshot
                let fwd = nt.rotation * -Vec3::Z;
                let side = Vec3::new(fwd.z, 0.0, -fwd.x);
                let spot = nt.translation + side * 6.0;
                let y = crate::test_tricks::ground(&phys, spot).unwrap_or(nt.translation.y);
                crate::test_tricks::place(&mut phys, &mut g, &mut tr, Vec3::new(spot.x, y + 0.7, spot.z), -side);
                st.phase = 2;
                st.mark = t;
            } else if t - st.mark > 60.0 {
                fail(&name, &format!("only {} nodes reached (state {:?})", r.reached, n.state), t);
            }
        }
        ("route", 2) => {
            input.cam_dist = Some(5.0);
            input.cam_pitch = Some(-0.2);
            input.cam_yaw = Some(g.yaw);
            if t - st.mark > 0.3 {
                snap(&mut st);
                println!("TEST route PASS");
                st.phase = 8;
                st.mark = t;
            }
        }
        ("trip", 0) if t > 0.5 => {
            use rapier3d::prelude as rp;
            println!("{} props trip the goat", phys.trip.len());
            // a tripping prop with flat ground 7 m in front of it that a sprint reaches head-on
            let mut found = None;
            let mut props: Vec<(rp::RigidBodyHandle, Vec3)> =
                phys.trip.iter().filter_map(|&h| phys.world.bodies.get(h).map(|b| (h, crate::convert::from_rapier(b.translation())))).collect();
            props.sort_by(|a, b| a.1.distance_squared(here).total_cmp(&b.1.distance_squared(here)));
            'search: for (h, c) in props {
                for k in 0..16 {
                    let a = k as f32 * std::f32::consts::TAU / 16.0;
                    let dir = Vec3::new(a.cos(), 0.0, a.sin());
                    let spot = c - dir * 7.0;
                    let Some(y) = crate::test_tricks::ground(&phys, spot) else { continue };
                    let from = Vec3::new(spot.x, y + 0.45, spot.z);
                    let ray = rp::Ray::new(crate::convert::to_rapier(from), crate::convert::to_rapier(dir));
                    let filter = rp::QueryFilter::default().exclude_rigid_body(g.body);
                    let Some((col, toi)) = phys.world.cast_ray(&ray, 9.0, true, filter) else { continue };
                    if toi < 5.5 || phys.world.colliders.get(col).and_then(|c| c.parent()) != Some(h) {
                        continue;
                    }
                    // flat run-up
                    let flat = (1..6).all(|i| crate::test_tricks::ground(&phys, spot + dir * i as f32).is_some_and(|gy| (gy - y).abs() < 0.3));
                    if flat {
                        found = Some((h, from, dir));
                        break 'search;
                    }
                }
            }
            let Some((h, from, dir)) = found else { fail(&name, "no tripping prop with a clear run-up", t) };
            println!("sprinting into {}", phys.names.get(&h).cloned().unwrap_or_default());
            crate::test_tricks::place(&mut phys, &mut g, &mut tr, from + Vec3::Y * 0.3, dir);
            st.yaw = f32::atan2(-dir.x, -dir.z);
            st.last_pos = from;
            st.phase = 1;
            st.mark = t;
        }
        ("trip", 1) => {
            input.forward = 1.0;
            input.sprint = true;
            input.cam_yaw = Some(st.yaw);
            input.cam_dist = Some(6.0);
            if g.ragdoll && st.mark2 == 0.0 {
                st.mark2 = t;
            }
            if g.ragdoll && t - st.mark2 > 0.3 {
                snap(&mut st);
                println!("  t={t:.2} tripped after {:.1} m", here.distance(st.last_pos));
                let mut heard: Vec<_> = log.0.iter().collect();
                heard.sort();
                println!("  sounds: {heard:?}");
                // footsteps on the way (GGPawnSoundGroup) and the prop's ImpactSound
                if log.count("foot:") == 0 {
                    fail(&name, "no footstep sounds", t);
                }
                if log.count("Goat_Sounds_Impact") + log.count("Impact") == 0 {
                    fail(&name, "no impact sound", t);
                }
                println!("TEST trip PASS");
                st.phase = 8;
                st.mark = t;
            } else if t - st.mark > 4.0 || here.distance(st.last_pos) > 10.0 {
                fail(&name, &format!("no trip (speed {:.0} uu/s)", g.velocity.length() / crate::convert::S), t);
            }
        }
        // let the screenshot get written before quitting
        (_, 8) => {
            input.cam_dist = Some(7.0);
            if t - st.mark > 0.4 {
                exit.write(AppExit::Success);
                st.phase = 9;
            }
        }
        _ => {}
    }
}
