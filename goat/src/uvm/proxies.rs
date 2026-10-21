//! Script proxies for the game's NPCs and level props. Every NPC gets one when mutators start
//! (so AllActors(class'GGNpc') finds them); props get one when a Trace or radius search finds
//! them. Each frame the game state is written into the proxies, and what scripts did to them
//! since (SetRagdoll, Velocity, SetLocation, AddImpulse/TakeDamage, Destroy) is applied to the
//! real NPC or prop.

use super::plugin::Mutators;
use super::*;
use crate::convert;
use bevy::prelude::*;
use rapier3d::prelude as rp;

#[derive(Clone, Copy, Default)]
struct Written {
    location: [f32; 3],
    velocity: [f32; 3],
    ragdoll: bool,
    /// Actor.Physics as last seen (props: PHYS_None while in the goat's inventory).
    physics: u8,
}

#[derive(Resource, Default)]
pub struct Proxies {
    written: HashMap<ObjId, Written>,
    /// Map script actors with physics bodies got their proxies.
    map_registered: bool,
    /// RB_Thruster / RB_RadialForceActor actors scripts spawned.
    forces: Vec<ObjId>,
    scanned: usize,
    npcs_made: bool,
    /// AddImpulse from script: (actor, impulse in Unreal units, velocity change).
    pub impulses: Vec<(ObjId, [f32; 3], bool)>,
}

fn to_ue(v: Vec3) -> [f32; 3] {
    [v.x / convert::S, v.z / convert::S, v.y / convert::S]
}

fn differs(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
    a.iter().zip(b).any(|(x, y)| (x - y).abs() > eps)
}

pub fn body_handle(bits: u64) -> rp::RigidBodyHandle {
    rp::RigidBodyHandle::from_raw_parts(bits as u32, (bits >> 32) as u32 & 0x7FFF_FFFF)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn proxies(
    mut m: ResMut<Mutators>,
    mut st: ResMut<Proxies>,
    mut actors: ResMut<super::actors::ScriptActors>,
    mut commands: Commands,
    mut phys: ResMut<crate::physics::Physics>,
    npcs: Query<(Entity, &Name, &Transform, &crate::npc::Npc, Option<&crate::ragdoll::CharacterBody>)>,
    bodies: Query<(Entity, &crate::physics::PhysBody)>,
    mut goats: Query<&mut crate::goat::Goat>,
    ragdolls: Query<&crate::ragdoll::ActiveRagdoll, With<crate::goat::Goat>>,
    time: Res<Time>,
) {
    let _prof_scope = crate::prof::scope("proxies::proxies");
    let t0 = std::time::Instant::now();
    let _prof = Prof(t0, "proxies");
    if std::env::var("GOAT_UVM_OFF").is_ok_and(|v| v.contains("proxies")) {
        return;
    }
    let Some(h) = m.host.as_mut() else {
        st.written.clear();
        st.forces.clear();
        st.scanned = 0;
        st.map_registered = false;
        st.npcs_made = false;
        st.impulses.clear();
        return;
    };
    let goat_obj = h.goat;
    // map script actors that are physics props: proxies (their Location/Velocity sync both ways)
    if !st.map_registered && !h.map_actors.is_empty() {
        let by_name: HashMap<String, rp::RigidBodyHandle> = phys.names.iter().map(|(b, n)| (n.to_ascii_lowercase(), *b)).collect();
        let actors = h.map_actors.clone();
        let vm = &mut h.vm;
        for o in actors {
            let name = vm.obj(o).name.clone();
            if let Some(&b) = by_name.get(&name.to_ascii_lowercase()) {
                let (i, g) = b.into_raw_parts();
                let loc = vm.get(o, "Location");
                let l = vm.vec3(&loc);
                vm.proxy_for(&WorldThing::Prop { body: (i as u64) | ((g as u64) << 32), name }, l);
            }
        }
        st.map_registered = true;
    }
    let vm = &mut h.vm;
    if !st.npcs_made {
        for (e, name, t, _, _) in &npcs {
            vm.proxy_for(&WorldThing::Npc { entity: e.to_bits(), name: name.as_str().to_string() }, to_ue(t.translation));
        }
        st.npcs_made = true;
        let n = vm.proxy_things.len();
        vm.say(format!("{n} NPCs have script actors"));
    }
    // impulses from script (props, script props, NPCs, the goat)
    let mut queue = std::mem::take(&mut st.impulses);
    queue.append(&mut actors.impulses);
    for (actor, imp, vel_change) in queue {
        let imp_m = Vec3::new(imp[0], imp[2], imp[1]) * convert::S;
        if Some(actor) == goat_obj {
            if let Ok(mut g) = goats.single_mut() {
                g.velocity += if vel_change { imp_m } else { imp_m / crate::goat::tune::MASS };
            }
            continue;
        }
        match vm.proxy_things.get(&actor) {
            Some(WorldThing::Prop { body, .. }) => {
                let h = body_handle(*body);
                phys.wake(h);
                if let Some(rb) = phys.world.bodies.get_mut(h) {
                    let v = if vel_change { imp_m } else { imp_m / rb.mass().max(1.0) };
                    let lv = rb.linvel();
                    rb.set_linvel(lv + convert::to_rapier(v), true);
                }
            }
            Some(WorldThing::Npc { entity, .. }) => {
                commands.entity(Entity::from_bits(*entity)).insert(crate::npc::Knocked { velocity: if vel_change { imp_m } else { imp_m / 60.0 } });
            }
            None => {
                // a prop the script spawned itself
                if let Some(&bh) = actors.by_body.iter().find(|(_, a)| **a == actor).map(|(b, _)| b) {
                    if let Some(rb) = phys.world.bodies.get_mut(bh) {
                        let v = if vel_change { imp_m } else { imp_m / rb.mass().max(1.0) };
                        let lv = rb.linvel();
                        rb.set_linvel(lv + convert::to_rapier(v), true);
                    }
                }
            }
        }
    }
    let ys = Syms { location: vm.sym("Location"), velocity: vm.sym("Velocity"), ragdoll: vm.sym("mIsRagdoll"), physics: vm.sym("Physics"), x: vm.sym("X"), y: vm.sym("Y"), z: vm.sym("Z") };
    force_actors(vm, &mut st, goat_obj, &mut goats, &ragdolls, &mut phys, time.delta_secs());
    // the goat's DrawScale -> collision capsule (uniform part: the largest axis)
    if let Some(g) = goat_obj {
        let ds = match vm.get(g, "DrawScale") {
            Value::Float(f) if f > 0.0 => f,
            _ => 1.0,
        };
        let s3 = vm.get(g, "DrawScale3D");
        let s3 = if matches!(s3, Value::Struct(_)) { vm.vec3(&s3) } else { [1.0; 3] };
        let s3 = if s3 == [0.0; 3] { [1.0; 3] } else { s3 };
        let scale = (ds * s3[0].max(s3[1]).max(s3[2])).clamp(0.05, 50.0);
        if (scale - super::bridge::goat_scale()).abs() > 1e-3 {
            if let Ok(gm) = goats.single() {
                let half = crate::goat::tune::COLLISION_HEIGHT * convert::S * scale;
                let r = crate::goat::tune::COLLISION_RADIUS * convert::S * scale;
                if let Some(c) = phys.world.colliders.get_mut(gm.collider) {
                    c.set_shape(rp::SharedShape::capsule_y((half - r).max(0.01), r));
                }
            }
            super::bridge::set_goat_scale(scale);
            vm.say(format!("goat collision scaled x{scale:.2}"));
        }
    }
    let things: Vec<(ObjId, WorldThing)> = vm.proxy_things.iter().map(|(&o, t)| (o, t.clone())).collect();
    let mut gone = vec![];
    for (o, thing) in things {
        let prev = st.written.get(&o).copied();
        let props = &vm.obj(o).props;
        let script_loc = props.get(&ys.location).map(|v| vec3_of(v, &ys)).unwrap_or_default();
        let script_vel = props.get(&ys.velocity).map(|v| vec3_of(v, &ys)).unwrap_or_default();
        let script_ragdoll = props.get(&ys.ragdoll).is_some_and(|v| v.truthy());
        let script_physics = props.get(&ys.physics).map(|v| v.int() as u8).unwrap_or(10);
        let destroyed = vm.obj(o).destroyed;
        match &thing {
            WorldThing::Npc { entity, .. } => {
                let e = Entity::from_bits(*entity);
                let Ok((_, _, t, npc, cb)) = npcs.get(e) else {
                    gone.push(o);
                    continue;
                };
                if destroyed {
                    if let Some(cb) = cb {
                        phys.world.remove_body_with_colliders(cb.body, true);
                    }
                    commands.entity(e).despawn();
                    vm.say(format!("{} destroyed by script", vm.obj(o).name));
                    gone.push(o);
                    continue;
                }
                let ragdolled = npc.state == crate::npc::NpcState::Ragdoll;
                if let Some(w) = prev {
                    let vel_changed = differs(script_vel, w.velocity, 1.0);
                    if (script_ragdoll && !w.ragdoll && !ragdolled) || vel_changed {
                        let v = if vel_changed { Vec3::new(script_vel[0], script_vel[2], script_vel[1]) * convert::S } else { Vec3::Y * 2.0 };
                        commands.entity(e).insert(crate::npc::Knocked { velocity: v });
                        vm.say(format!("{} knocked by script", vm.obj(o).name));
                    }
                }
                let loc = to_ue(t.translation);
                let lv = vm.vector(loc[0], loc[1], loc[2]);
                let zero = vm.vector(0.0, 0.0, 0.0);
                vm.set(o, "Location", lv);
                vm.set(o, "Velocity", zero);
                vm.set(o, "mIsRagdoll", Value::Bool(ragdolled));
                vm.set(o, "Physics", Value::Byte(if ragdolled { 10 } else { 1 }));
                st.written.insert(o, Written { location: loc, velocity: [0.0; 3], ragdoll: ragdolled, physics: 1 });
            }
            WorldThing::Prop { body, .. } => {
                let bh = body_handle(*body);
                // resting props the script left alone need no update
                if let (Some(w), Some(rb)) = (prev, phys.world.bodies.get(bh)) {
                    if !destroyed && (!rb.is_dynamic() || rb.is_sleeping()) && !differs(script_loc, w.location, 1.0) && !differs(script_vel, w.velocity, 1.0) && script_physics == w.physics {
                        continue;
                    }
                }
                if destroyed {
                    for (e, pb) in &bodies {
                        if pb.handle == bh {
                            commands.entity(e).despawn();
                        }
                    }
                    phys.world.remove_body_with_colliders(bh, true);
                    gone.push(o);
                    continue;
                }
                if let Some(w) = prev {
                    // SetPhysics(PHYS_None) + SetCollision(false) (GGKActorBase.OnAddToInventory): the
                    // prop leaves the world; PHYS_RigidBody brings it back (OnRemoveFromInventory)
                    if script_physics != w.physics && (script_physics == 0 || script_physics == 10) {
                        let away = script_physics == 0;
                        for mut g in &mut goats {
                            if g.grabbed.is_some_and(|(b, _)| b == bh) {
                                crate::goat::drop_grabbed(&mut g, &mut phys);
                            }
                        }
                        if !away {
                            phys.wake(bh);
                        }
                        if let Some(rb) = phys.world.bodies.get_mut(bh) {
                            rb.set_body_type(if away { rp::RigidBodyType::KinematicPositionBased } else { rp::RigidBodyType::Dynamic }, true);
                            rb.set_linvel(rp::Vector::ZERO, true);
                            rb.set_angvel(rp::Vector::ZERO, true);
                            let cs: Vec<rp::ColliderHandle> = rb.colliders().to_vec();
                            for c in cs {
                                if let Some(c) = phys.world.colliders.get_mut(c) {
                                    c.set_enabled(!away);
                                }
                            }
                        }
                        for (e, pb) in &bodies {
                            if pb.handle == bh {
                                commands.entity(e).insert(if away { Visibility::Hidden } else { Visibility::Inherited });
                            }
                        }
                        vm.say(format!("{} {} by script", vm.obj(o).name, if away { "put away" } else { "brought back" }));
                    }
                    if differs(script_vel, w.velocity, 1.0) || differs(script_loc, w.location, 1.0) {
                        phys.wake(bh);
                    }
                    if let Some(rb) = phys.world.bodies.get_mut(bh) {
                        if differs(script_loc, w.location, 1.0) {
                            rb.set_translation(convert::to_rapier(convert::pos(script_loc)), true);
                        }
                        if differs(script_vel, w.velocity, 1.0) {
                            rb.set_linvel(convert::to_rapier(Vec3::new(script_vel[0], script_vel[2], script_vel[1]) * convert::S), true);
                        }
                    }
                }
                let Some(rb) = phys.world.bodies.get(bh) else {
                    gone.push(o);
                    continue;
                };
                let loc = to_ue(convert::from_rapier(rb.translation()));
                let vel = to_ue(convert::from_rapier(rb.linvel()));
                let lv = vm.vector(loc[0], loc[1], loc[2]);
                let vv = vm.vector(vel[0], vel[1], vel[2]);
                vm.set(o, "Location", lv);
                vm.set(o, "Velocity", vv);
                st.written.insert(o, Written { location: loc, velocity: vel, ragdoll: false, physics: script_physics });
            }
        }
    }
    for o in gone {
        if let Some(t) = vm.proxy_things.remove(&o) {
            vm.proxies.remove(&t.key());
        }
        st.written.remove(&o);
    }
}

/// GOAT_PROFILE=1: time spent in a VM system, printed when it is over 2 ms.
pub struct Prof(pub std::time::Instant, pub &'static str);
impl Drop for Prof {
    fn drop(&mut self) {
        let ms = self.0.elapsed().as_secs_f32() * 1000.0;
        if ms > 2.0 && std::env::var_os("GOAT_PROFILE").is_some() {
            println!("profile: {} {ms:.2} ms", self.1);
        }
    }
}

struct Syms {
    location: Sym,
    velocity: Sym,
    ragdoll: Sym,
    physics: Sym,
    x: Sym,
    y: Sym,
    z: Sym,
}

fn vec3_of(v: &Value, ys: &Syms) -> [f32; 3] {
    match v {
        Value::Struct(s) => [ys.x, ys.y, ys.z].map(|k| s.field(k).map(|f| f.float()).unwrap_or(0.0)),
        _ => [0.0; 3],
    }
}

/// RB_Thruster (Jet Pack) and RB_RadialForceActor (Tornado Goat) actors spawned by scripts.
/// Actors based on the goat (SetBase) follow it. The thrust direction is approximated as
/// world up: the ThrustSocket's orientation on the goat mesh isn't modelled.
fn force_actors(
    vm: &mut Vm,
    st: &mut Proxies,
    goat_obj: Option<ObjId>,
    goats: &mut Query<&mut crate::goat::Goat>,
    ragdolls: &Query<&crate::ragdoll::ActiveRagdoll, With<crate::goat::Goat>>,
    phys: &mut crate::physics::Physics,
    dt: f32,
) {
    let (Some(thruster), Some(radial)) = (vm.class_by_name("RB_Thruster"), vm.class_by_name("RB_RadialForceActor")) else { return };
    let n = vm.objects.len();
    for o in st.scanned as ObjId..n as ObjId {
        let ob = vm.obj(o);
        if matches!(ob.kind, Kind::Instance) && !ob.name.starts_with("Default__") && (vm.is_child_of(ob.class, thruster) || vm.is_child_of(ob.class, radial)) {
            st.forces.push(o);
        }
    }
    st.scanned = n;
    st.forces.retain(|&o| !vm.obj(o).destroyed);
    let goat_loc = goat_obj.map(|g| vm.get(g, "Location")).map(|l| vm.vec3(&l));
    for &o in &st.forces.clone() {
        let based_on_goat = goat_obj.is_some() && vm.get(o, "Base").obj() == goat_obj;
        if based_on_goat {
            if let Some(l) = goat_loc {
                let lv = vm.vector(l[0], l[1], l[2]);
                vm.set(o, "Location", lv);
            }
        }
        let class = vm.obj(o).class;
        if vm.is_child_of(class, thruster) {
            let on = vm.get(o, "bThrustEnabled").truthy() && based_on_goat;
            // Thrust: the ragdoll climbs at ThrustStrength x 0.2 uu/s (Jet Pack 2000 -> 400 uu/s),
            // horizontal motion kept. A force alone can't lift a ragdoll lying on the ground in
            // rapier (its contacts/joints hold it), so the vertical speed is driven directly.
            let climb = vm.get(o, "ThrustStrength").float() * 0.2 * convert::S;
            match ragdolls.single() {
                Ok(r) => {
                    if std::env::var_os("GOAT_TRACE").is_some() {
                        if let Some(p) = phys.world.bodies.get(r.pelvis) {
                            println!("jet on={on} pelvis y={:.2} vy={:.2}", p.translation().y, p.linvel().y);
                        }
                    }
                    if on {
                        for (_, b) in &r.bodies {
                            if let Some(rb) = phys.world.bodies.get_mut(*b) {
                                let mut v = rb.linvel();
                                if v.y < climb {
                                    v.y = climb;
                                    rb.set_linvel(v, true);
                                }
                            }
                        }
                    }
                }
                Err(_) => {
                    if on {
                        if let Ok(mut g) = goats.single_mut() {
                            g.velocity.y = g.velocity.y.max(climb);
                        }
                    }
                }
            }
        } else if vm.is_child_of(class, radial) {
            if !vm.get(o, "bForceActive").truthy() {
                continue;
            }
            let loc_v = vm.get(o, "Location");
            let c = convert::pos(vm.vec3(&loc_v));
            let radius = vm.get(o, "ForceRadius").float() * convert::S;
            let strength = vm.get(o, "ForceStrength").float();
            let swirl = vm.get(o, "SwirlStrength").float();
            if radius <= 0.0 {
                continue;
            }
            let shape = rp::SharedShape::ball(radius);
            let pose = rp::Pose::from_parts(convert::to_rapier(c), rp::Rotation::IDENTITY);
            let hits: Vec<rp::RigidBodyHandle> = {
                let qp = phys.world.query_pipeline_with_filter(rp::QueryFilter::default());
                qp.intersect_shape(pose, &*shape).filter_map(|(_, col)| col.parent()).collect()
            };
            for b in hits {
                if !phys.names.contains_key(&b) || !phys.wake(b) {
                    continue;
                }
                let Some(rb) = phys.world.bodies.get_mut(b) else { continue };
                let p = convert::from_rapier(rb.translation());
                let d = p - c;
                let dist = d.length().max(0.01);
                let dir = d / dist;
                // linear falloff; negative strength pulls in, swirl spins around the vertical axis
                let fall = (1.0 - dist / radius).max(0.0);
                let tangent = Vec3::Y.cross(dir).normalize_or_zero();
                let acc = (dir * strength + tangent * swirl) * fall * convert::S;
                let v = rb.linvel();
                rb.set_linvel(v + convert::to_rapier(acc * dt), true);
            }
        }
    }
}
