//! Scripted trick tests: `--test flip`, `--test wallrun`, `--test jumpover`.
//! Each drives the goat adaptively (from its live state), checks the combo list and aborts early.

use crate::convert;
use crate::goat::{Goat, ScriptedInput};
use crate::physics::Physics;
use crate::score::Score;
use crate::Args;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use rapier3d::prelude as rp;

#[derive(Default)]
pub struct St {
    t: f32,
    frames: u32,
    phase: u32,
    target: Option<Vec3>,
    dir: Vec3,
    shot: bool,
    seen: Vec<String>,
    /// When the scenario's last phase was reached (its end is fixed from there).
    done_at: Option<f32>,
    /// Phase start time and a height (cart).
    mark: f32,
    base_y: f32,
    max_y: f32,
}

fn fail(name: &str, why: &str, t: f32) -> ! {
    println!("TEST {name} FAIL at {t:.1}s: {why}");
    std::process::exit(10);
}

pub fn place(phys: &mut Physics, g: &mut Goat, tr: &mut Transform, at: Vec3, dir: Vec3) {
    if let Some(rb) = phys.world.bodies.get_mut(g.body) {
        rb.set_translation(convert::to_rapier(at), true);
    }
    tr.translation = at;
    g.velocity = Vec3::ZERO;
    g.yaw = f32::atan2(-dir.x, -dir.z);
}

/// Ground height under a point (static geometry only).
pub fn ground(phys: &Physics, p: Vec3) -> Option<f32> {
    let ray = rp::Ray::new(convert::to_rapier(p + Vec3::Y * 20.0), rp::Vector::NEG_Y);
    phys.world.cast_ray(&ray, 60.0, true, rp::QueryFilter::only_fixed()).map(|(_, toi)| p.y + 20.0 - toi)
}

#[allow(clippy::too_many_arguments)]
pub fn tricks_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut commands: Commands,
    mut st: Local<St>,
    mut phys: ResMut<Physics>,
    mut input: ResMut<ScriptedInput>,
    score: Res<Score>,
    mut goats: Query<(&mut Goat, &mut Transform), Without<crate::npc::Npc>>,
    npcs: Query<(&crate::npc::Npc, &Transform)>,
    mut exit: MessageWriter<AppExit>,
    vols: Res<crate::physics::volumes::Volumes>,
    riding: Res<crate::vehicle::Riding>,
    bikes: Query<(&crate::vehicle::Bike, &GlobalTransform)>,
) {
    let name = args.test.clone().unwrap_or_default();
    if !matches!(name.as_str(), "flip" | "wallrun" | "jumpover" | "trampoline" | "manual" | "bike" | "longboard" | "cart") {
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
    for l in &score.lines {
        if !st.seen.contains(&l.text) {
            println!("  t={t:.2} trick: {} +{}", l.text, l.score);
            st.seen.push(l.text.clone());
        }
    }
    let have = |s: &St, needle: &str| s.seen.iter().any(|x| x.contains(needle));
    if std::env::var_os("GOAT_TRACE").is_some() && st.phase >= 1 {
        let tgt = st.target.unwrap_or(Vec3::ZERO);
        println!(
            "    t={t:.2} ph={} pos={:.2?} vel={:.2?} grounded={} wall={} ready={} dist={:.2} yaw={:.2}",
            st.phase,
            tr.translation,
            g.velocity,
            g.grounded,
            g.wall.active,
            g.ready_to_wall_run,
            Vec2::new(tgt.x - tr.translation.x, tgt.z - tr.translation.z).length(),
            g.yaw
        );
    }
    if !tr.translation.is_finite() || tr.translation.y < -150.0 {
        fail(&name, "goat left the world", t);
    }
    let mut end = 14.0;
    match name.as_str() {
        // drop onto the nearest real trampoline (GGTrampolineVolume) and bounce back up
        "trampoline" => {
            input.cam_dist = Some(7.0);
            input.cam_pitch = Some(-0.35);
            match st.phase {
                0 => {
                    let here = tr.translation;
                    let Some(t0) = vols.trampolines.iter().filter(|v| v.display == "Trampoline").min_by(|a, b| a.top().distance_squared(here).total_cmp(&b.top().distance_squared(here))) else {
                        fail(&name, "no trampolines in this map", t)
                    };
                    let top = t0.top();
                    println!("{} trampolines; dropping onto {} at {:.1?}", vols.trampolines.len(), t0.name, top);
                    place(&mut phys, &mut g, &mut tr, top + Vec3::Y * 3.0, Vec3::X);
                    st.target = Some(top);
                    st.phase = 1;
                }
                1 => {
                    if vols.bounces > 0 && g.velocity.y > 0.0 {
                        println!("  t={t:.2} bounced: vy {:.0} uu/s", g.velocity.y / crate::convert::S);
                        st.phase = 2;
                    } else if t > 3.0 {
                        fail(&name, &format!("no bounce (bounces {}, vy {:.0} uu/s, grounded {})", vols.bounces, g.velocity.y / crate::convert::S, g.grounded), t);
                    }
                }
                2 => {
                    let top = st.target.unwrap();
                    if tr.translation.y > top.y + 1.0 && !st.shot {
                        st.shot = true;
                        if let Some(p) = args.shot.clone() {
                            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                        }
                        println!("  t={t:.2} {:.1} m above the trampoline", tr.translation.y - top.y);
                        st.phase = 3;
                    } else if t > 3.5 {
                        fail(&name, "bounce did not lift the goat", t);
                    }
                }
                3 => {
                    if t > 3.7 {
                        if vols.bounce_tricks == 0 {
                            fail(&name, "the bounce scored no GGTrickBounce trick", t);
                        }
                        println!("TEST trampoline PASS ({} bounce tricks)", vols.bounce_tricks);
                        exit.write(AppExit::Success);
                        st.phase = 4;
                    }
                }
                _ => {}
            }
        }
        "flip" => {
            // jump, then hold free look + W until one full backflip, land; then a 360 spin
            input.cam_dist = Some(6.0);
            input.cam_pitch = Some(-0.2);
            input.cam_yaw = Some(g.yaw + 1.2);
            match st.phase {
                0 if t > 1.0 => {
                    input.jump = true;
                    st.phase = 1;
                }
                1 if !g.grounded => {
                    input.freelook = true;
                    input.forward = 1.0;
                    // the turn carries on ~30 degrees after letting go: release early enough that a
                    // slow frame doesn't overshoot into a bad landing
                    if g.air_pitch_deg >= 340.0 {
                        st.phase = 2;
                    }
                }
                2 if g.grounded => {
                    if g.ragdoll {
                        fail(&name, "landed the backflip in ragdoll", t);
                    }
                    st.phase = 3;
                    st.t = 2.5;
                }
                3 if t > 3.5 => {
                    input.jump = true;
                    st.phase = 4;
                }
                4 if !g.grounded => {
                    input.freelook = true;
                    input.right = 1.0;
                    if g.air_yaw_deg.abs() >= 350.0 {
                        st.phase = 5;
                    }
                }
                5 if g.grounded => {
                    st.phase = 6;
                }
                _ => {}
            }
            if st.phase == 1 && t > 4.0 {
                fail(&name, "never got airborne / flip never completed", t);
            }
            // early abort: the spin jump landed (or never left) without a full turn
            if st.phase == 4 && t > 8.0 {
                fail(&name, "spin jump never reached 350 degrees", t);
            }
            if st.phase == 6 {
                if !have(&st, "BackFlip") {
                    fail(&name, "no BackFlip trick", t);
                }
                if !have(&st, "180") && !have(&st, "360") {
                    fail(&name, "no spin trick", t);
                }
                end = *st.done_at.get_or_insert(t) + 0.5;
            }
            if st.phase >= 2 && !st.shot && g.air_pitch_deg > 150.0 {
                st.shot = true;
                if let Some(p) = args.shot.clone() {
                    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                }
            }
        }
        "bike" | "longboard" => {
            let want = if name == "bike" { "Bicycle" } else { "Longboard" };
            // walk up to the nearest bike, R to get on, ride 3 s balancing, R to get off
            input.cam_dist = Some(5.0);
            input.cam_pitch = Some(-0.25);
            let bike_yaw = |t: &GlobalTransform| {
                let f = t.rotation() * Vec3::X;
                f32::atan2(-f.x, -f.z)
            };
            match st.phase {
                0 if t > 0.5 => {
                    let here = tr.translation;
                    // a bike on the ground near the spawn (some stand on roofs)
                    let Some((_, bt)) = bikes
                        .iter()
                        .filter(|(b, _)| b.def.name == want)
                        .filter(|(_, t)| (t.translation().y - here.y).abs() < 2.5 || want == "Longboard")
                        .min_by(|a, b| a.1.translation().distance(here).total_cmp(&b.1.translation().distance(here)))
                    else {
                        fail(&name, "no bicycle in the level", t);
                    };
                    // turn the bike toward the most open direction (rays at handlebar height)
                    let from = bt.translation() + Vec3::Y * 1.0;
                    let mut best = (0.0f32, Vec3::X);
                    for k in 0..48 {
                        let a = k as f32 / 48.0 * std::f32::consts::TAU;
                        let dir = Vec3::new(a.cos(), 0.0, a.sin());
                        let ray = rp::Ray::new(convert::to_rapier(from), convert::to_rapier(dir));
                        let free = phys.world.cast_ray(&ray, 40.0, true, rp::QueryFilter::only_fixed()).map(|(_, toi)| toi).unwrap_or(40.0);
                        if free > best.0 {
                            best = (free, dir);
                        }
                    }
                    let heading = Quat::from_rotation_arc(Vec3::X, best.1);
                    let bike_body = phys.world.bodies.iter().find(|(_, rb)| convert::from_rapier(rb.translation()).distance(bt.translation()) < 0.05).map(|(h, _)| h);
                    if let Some(rb) = bike_body.and_then(|h| phys.world.bodies.get_mut(h)) {
                        // lifted a little so the turned deck can't swing into a kerb and get pushed under it
                        rb.set_rotation(convert::quat_to_rapier(heading), true);
                        let lifted = rb.translation() + convert::to_rapier(Vec3::Y * 0.3);
                        rb.set_translation(lifted, true);
                    }
                    println!("  bike faces {:.2?} ({:.0} m clear)", best.1, best.0);
                    let side = heading * Vec3::Z;
                    let at = bt.translation() + side * 1.2 + Vec3::Y * 0.6;
                    place(&mut phys, &mut g, &mut tr, at, -side);
                    st.target = Some(bt.translation());
                    st.phase = 1;
                }
                0 => {}
                1 if t > 1.2 => {
                    input.special = true;
                    st.phase = 2;
                }
                1 => {}
                2 => {
                    if riding.bike.is_some() {
                        println!("  t={t:.2} on the bike");
                        st.phase = 3;
                        st.t = 2.0;
                        st.target = Some(tr.translation);
                    } else if t > 1.6 {
                        fail(&name, "R next to a bike did not get on it", t);
                    }
                }
                3 => {
                    let Some((b, bt)) = riding.bike.and_then(|_| bikes.iter().find(|(b, _)| b.driven)) else {
                        fail(&name, &format!("thrown off the bike ({} times)", riding.thrown), t);
                    };
                    input.forward = 1.0;
                    input.cam_yaw = Some(bike_yaw(bt));
                    // keep it upright: press against the side it leans to, a few times a second
                    let w = b.balance.blend.weight;
                    if (w - 0.5).abs() > 0.06 && (t * 6.0) as i32 % 2 == 0 && st.frames % 3 == 0 {
                        input.right = if b.balance.fall_dir == 0 { 1.0 } else { -1.0 };
                    }
                    if !st.shot && t > 3.5 {
                        st.shot = true;
                        if let Some(p) = args.shot.clone() {
                            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                        }
                    }
                    if t > 5.0 {
                        let moved = st.target.map(|p| p.distance(tr.translation)).unwrap_or(0.0);
                        println!("  t={t:.2} rode {moved:.1} m, lean {w:.2}, speed {:.0} uu/s", bt.translation().distance(bt.translation()) + g.velocity.length() / crate::convert::S);
                        if moved < 6.0 {
                            fail(&name, &format!("the bike only moved {moved:.1} m in 3 s"), t);
                        }
                        st.phase = 4;
                    }
                }
                4 => {
                    input.special = true;
                    st.phase = 5;
                }
                5 => {
                    if riding.bike.is_some() {
                        if t > 5.5 {
                            fail(&name, "R did not get off the bike", t);
                        }
                    } else if g.ragdoll {
                        fail(&name, "getting off ragdolled the goat", t);
                    } else {
                        end = *st.done_at.get_or_insert(t) + 1.0;
                    }
                }
                _ => {}
            }
        }
        "cart" => {
            // the Shopping Goat mutator spawns its cart and puts the goat in: W 3 s, R can't leave,
            // Space jumps
            input.cam_dist = Some(6.0);
            input.cam_pitch = Some(-0.25);
            let cart = riding.bike.and_then(|e| bikes.get(e).ok()).filter(|(b, _)| b.def.name == "ShoppingCart");
            if st.phase > 0 && cart.is_none() {
                fail(&name, "the goat left the cart", t);
            }
            match st.phase {
                0 => {
                    if let Some((_, bt)) = cart {
                        println!("  t={t:.2} in the cart");
                        st.target = Some(bt.translation());
                        st.mark = t;
                        st.phase = 1;
                    } else if t > 10.0 {
                        fail(&name, "the Shopping Goat cart never took the goat", t);
                    }
                }
                1 => {
                    let (_, bt) = cart.unwrap();
                    input.forward = 1.0;
                    let moved = st.target.map(|p| p.distance(bt.translation())).unwrap_or(0.0);
                    if !st.shot && t - st.mark > 1.2 {
                        st.shot = true;
                        if let Some(p) = args.shot.clone() {
                            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                        }
                    }
                    if t - st.mark > 4.0 && moved < 0.5 {
                        fail(&name, &format!("the cart only moved {moved:.1} m"), t);
                    }
                    if t - st.mark > 3.0 && moved >= 4.0 {
                        println!("  t={t:.2} drove {moved:.1} m");
                        st.phase = 2;
                    }
                }
                2 => {
                    input.special = true;
                    st.mark = t;
                    st.phase = 3;
                }
                3 => {
                    if t - st.mark > 0.5 {
                        let (_, bt) = cart.unwrap();
                        println!("  t={t:.2} R did not get out (mUsedByMutator)");
                        st.base_y = bt.translation().y;
                        st.max_y = st.base_y;
                        st.mark = t;
                        st.phase = 4;
                    }
                }
                4 => {
                    let (_, bt) = cart.unwrap();
                    input.jump = t - st.mark < 0.2;
                    st.max_y = st.max_y.max(bt.translation().y);
                    if t - st.mark > 1.5 && st.done_at.is_none() {
                        let rise = st.max_y - st.base_y;
                        if rise < 0.2 {
                            fail(&name, &format!("the cart jumped only {rise:.2} m"), t);
                        }
                        println!("  t={t:.2} jumped {rise:.2} m");
                        end = *st.done_at.get_or_insert(t) + 0.3;
                    }
                }
                _ => {}
            }
        }
        "manual" => {
            // S then W within 0.08 s rears the goat up; balance the meter for 2 s, jump, land
            input.cam_dist = Some(4.0);
            input.cam_pitch = Some(-0.2);
            match st.phase {
                0 if t > 1.0 => {
                    input.forward = -1.0;
                    if t > 1.2 {
                        st.phase = 1;
                    }
                }
                0 => {}
                1 => {
                    input.forward = 1.0;
                    if g.manual.active {
                        println!("  t={t:.2} manual started");
                        st.phase = 2;
                        st.t = 2.0;
                    } else if t > 1.6 {
                        fail(&name, "the back-forward tap did not start a manual", t);
                    }
                }
                2 => {
                    // a player keeping the plupp in the middle
                    let m = &g.manual;
                    input.forward = (-(m.value - 50.0) / 10.0 - m.speed / 8.0).clamp(-1.0, 1.0);
                    if g.ragdoll || !g.manual.active {
                        fail(&name, &format!("fell off the manual (value {:.0}, speed {:.1})", m.value, m.speed), t);
                    }
                    if !st.shot && t > 3.0 {
                        st.shot = true;
                        if let Some(p) = args.shot.clone() {
                            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                        }
                    }
                    if t > 4.0 {
                        println!("  t={t:.2} manual score {} (meter {:.0})", m.score, m.value);
                        input.jump = true;
                        st.phase = 3;
                    }
                }
                3 if !g.grounded => st.phase = 4,
                3 if t > 4.5 => fail(&name, "could not jump out of the manual", t),
                4 if g.grounded => {
                    if g.manual.active {
                        fail(&name, "landing did not end the manual", t);
                    }
                    st.phase = 5;
                }
                4 if t > 6.0 => fail(&name, "never landed", t),
                5 => {
                    let line = score.lines.iter().find(|l| l.text == "manual");
                    match line {
                        Some(l) if l.score >= 300 => {}
                        Some(l) => fail(&name, &format!("manual scored only {}", l.score), t),
                        None if t > 6.5 => fail(&name, "no manual trick in the combo", t),
                        None => {}
                    }
                    if line.is_some() {
                        end = *st.done_at.get_or_insert(t) + 0.5;
                    }
                }
                _ => {}
            }
        }
        "wallrun" => {
            // find a wall near the spawn, run at it, jump, wall-run, wall-jump off
            if st.phase == 0 && t > 0.5 {
                let here = tr.translation;
                let mut best: Option<(f32, Vec3, Vec3)> = None;
                for k in 0..64 {
                    let a = k as f32 / 64.0 * std::f32::consts::TAU;
                    let dir = Vec3::new(a.cos(), 0.0, a.sin());
                    let cast = |h: f32| {
                        let ray = rp::Ray::new(convert::to_rapier(here + Vec3::Y * h), convert::to_rapier(dir));
                        let static_only = |_: rp::ColliderHandle, c: &rp::Collider| c.parent().is_none();
                        phys.world.cast_ray_and_get_normal(&ray, 120.0, true, rp::QueryFilter::default().predicate(&static_only))
                    };
                    if let Some((_, hit)) = cast(0.5) {
                        let n = convert::from_rapier(hit.normal);
                        let d = hit.time_of_impact;
                        // a real wall: also there 3 and 6 m up (a wall run climbs ~4.5 m)
                        let tall = [3.0, 6.0].iter().all(|&h| cast(h).is_some_and(|(_, h2)| (h2.time_of_impact - d).abs() < 0.5));
                        if tall && n.y.abs() < 0.2 && d > 6.0 && n.dot(-dir) > 0.9 && best.is_none_or(|b| d < b.0) {
                            // run straight into it (head-on gets mWallRunBoostZ)
                            let into = -Vec3::new(n.x, 0.0, n.z).normalize();
                            best = Some((d, here + dir * d, into));
                        }
                    }
                }
                let Some((d, wall, dir)) = best else { fail(&name, "no wall found near spawn", t) };
                let start = wall - dir * 7.0;
                let y = ground(&phys, start).unwrap_or(here.y - 0.6) + 0.7;
                place(&mut phys, &mut g, &mut tr, Vec3::new(start.x, y, start.z), dir);
                st.target = Some(wall);
                st.dir = dir;
                println!("wall {d:.1} m away at {wall:?}");
                st.phase = 1;
                st.t = 0.6;
            }
            if st.phase >= 1 {
                input.cam_yaw = Some(f32::atan2(-st.dir.x, -st.dir.z) + 0.7);
                input.cam_dist = Some(7.0);
                input.cam_pitch = Some(-0.15);
            }
            let wall = st.target.unwrap_or(Vec3::ZERO);
            let dist = Vec2::new(wall.x - tr.translation.x, wall.z - tr.translation.z).length();
            match st.phase {
                1 if t > 1.2 => {
                    input.forward = 1.0;
                    input.sprint = true;
                    // steer straight at the wall: camera looks along dir
                    input.cam_yaw = Some(f32::atan2(-st.dir.x, -st.dir.z));
                    if dist < 2.6 && g.grounded {
                        input.jump = true;
                        st.phase = 2;
                    }
                }
                2 => {
                    input.forward = 1.0;
                    input.cam_yaw = Some(f32::atan2(-st.dir.x, -st.dir.z));
                    if g.wall.active && !st.shot {
                        st.shot = true;
                        if let Some(p) = args.shot.clone() {
                            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                        }
                    }
                    if g.wall.active && g.wall.scored {
                        input.jump = true; // wall jump
                        st.phase = 3;
                    }
                    if g.grounded && t > 3.0 && !g.wall.active {
                        fail(&name, "landed without a scored wall run + wall jump", t);
                    }
                }
                3 if g.grounded => {
                    for need in ["Wall Run", "Wall Jump"] {
                        if !have(&st, need) {
                            fail(&name, &format!("missing trick {need}"), t);
                        }
                    }
                    end = t + 0.3;
                    st.phase = 4;
                }
                4 => end = t,
                _ => {}
            }
            if t > 9.0 && st.phase < 3 {
                fail(&name, "wall run did not happen in time", t);
            }
        }
        "jumpover" => {
            // sprint at a standing person and hop over them low
            if st.phase == 0 && t > 0.5 {
                let here = tr.translation;
                let pick = npcs
                    .iter()
                    .filter(|(n, nt)| n.state == crate::npc::NpcState::Idle && nt.translation.distance(here) < 400.0)
                    .filter(|(_, nt)| {
                        // open ground in front of them
                        let p = nt.translation;
                        (1..=8).all(|k| {
                            let a = Vec3::new(p.x - k as f32 * 1.0, p.y, p.z);
                            ground(&phys, a).is_some_and(|y| (y - (p.y - 0.9)).abs() < 0.6)
                        })
                    })
                    .min_by(|a, b| a.1.translation.distance_squared(here).total_cmp(&b.1.translation.distance_squared(here)));
                let Some((n, nt)) = pick else { fail(&name, "no suitable NPC", t) };
                let dir = Vec3::X;
                let start = nt.translation - dir * 9.0;
                let y = ground(&phys, start).unwrap_or(nt.translation.y - 0.9) + 0.7;
                place(&mut phys, &mut g, &mut tr, Vec3::new(start.x, y, start.z), dir);
                st.target = Some(nt.translation);
                st.dir = dir;
                println!("jumping over {} ({:?})", n.actor_name, n.default_anim);
                st.phase = 1;
                st.t = 0.6;
            }
            if st.phase >= 1 {
                input.cam_yaw = Some(f32::atan2(-st.dir.x, -st.dir.z) + 0.9);
                input.cam_dist = Some(6.0);
            }
            let target = st.target.unwrap_or(Vec3::ZERO);
            let dist = Vec2::new(target.x - tr.translation.x, target.z - tr.translation.z).length();
            match st.phase {
                1 if t > 1.0 => {
                    input.forward = 1.0;
                    input.sprint = true;
                    input.cam_yaw = Some(f32::atan2(-st.dir.x, -st.dir.z));
                    if dist < 5.5 && g.grounded {
                        input.jump = true;
                        st.phase = 2;
                    }
                }
                2 => {
                    input.forward = 1.0;
                    input.cam_yaw = Some(f32::atan2(-st.dir.x, -st.dir.z));
                    if !st.shot && dist < 0.8 {
                        st.shot = true;
                        if let Some(p) = args.shot.clone() {
                            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                        }
                    }
                    if g.grounded && t > 1.6 {
                        st.phase = 3;
                    }
                }
                3 => {
                    if !have(&st, "jumped over") {
                        fail(&name, "no jumped-over trick", t);
                    }
                    end = t + 0.3;
                    st.phase = 4;
                }
                4 => end = t,
                _ => {}
            }
            if t > 8.0 && st.phase < 3 {
                fail(&name, "jump did not complete", t);
            }
        }
        _ => {}
    }
    if t >= end {
        println!("TEST {name} PASS ({} tricks: {:?})", st.seen.len(), st.seen);
        exit.write(AppExit::Success);
    }
}
