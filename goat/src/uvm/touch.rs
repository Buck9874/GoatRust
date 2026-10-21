//! Kismet Touch events: the goat entering/leaving an event's originator, a volume (its
//! BrushComponent's BrushAggGeom hull, like the level loader reads them) or a Trigger
//! (CylinderComponent). Touched = output 0, UnTouched = output 1, Instigator = the goat.

use super::plugin::Mutators;
use super::*;
use crate::convert;
use bevy::prelude::*;
use rapier3d::prelude as rp;

struct Shape {
    event: ObjId,
    parts: Vec<(rp::Pose, rp::SharedShape)>,
    /// ClassProximityTypes: the goat (Pawn types) and/or physics props (KActor types).
    goat: bool,
    props: bool,
    inside: bool,
    /// Props inside last frame (body handle).
    props_inside: Vec<rp::RigidBodyHandle>,
}

/// Which instigators an event reacts to (SeqEvent_Touch.ClassProximityTypes, default Pawn).
fn proximity(vm: &mut Vm, ev: ObjId) -> (bool, bool) {
    let types: Vec<usize> = match vm.get(ev, "ClassProximityTypes") {
        Value::Array(a) => a.iter().filter_map(|v| vm.as_class(v)).collect(),
        _ => vec![],
    };
    let ignored: Vec<usize> = match vm.get(ev, "IgnoredClassProximityTypes") {
        Value::Array(a) => a.iter().filter_map(|v| vm.as_class(v)).collect(),
        _ => vec![],
    };
    let matches = |vm: &mut Vm, c: Option<usize>| match c {
        Some(c) => (types.is_empty() || types.iter().any(|&t| vm.is_child_of(c, t))) && !ignored.iter().any(|&t| vm.is_child_of(c, t)),
        None => false,
    };
    let goat_class = vm.class_by_name("GGGoat");
    let prop_class = vm.class_by_name("GGKActor");
    let goat = if types.is_empty() { true } else { matches(vm, goat_class) };
    let props = !types.is_empty() && matches(vm, prop_class);
    (goat, props)
}

#[derive(Default)]
pub struct TouchState {
    generation: u64,
    shapes: Vec<Shape>,
    /// Map script actors with Touch/UnTouch: (actor, shape, goat inside).
    actors: Vec<(ObjId, Vec<(rp::Pose, rp::SharedShape)>, bool)>,
    /// The goat's position last frame.
    prev_goat: Option<Vec3>,
}

/// The originator actor's pose and scale from its (map) properties.
fn actor_pose(vm: &mut Vm, o: ObjId) -> (rp::Pose, Vec3) {
    let l = vm.get(o, "Location");
    let loc = if matches!(l, Value::Struct(_)) { vm.vec3(&l) } else { [0.0; 3] };
    let r = vm.get(o, "Rotation");
    let rot = if matches!(r, Value::Struct(_)) { vm.rot3(&r) } else { [0; 3] };
    let ds = match vm.get(o, "DrawScale") {
        Value::Float(f) if f > 0.0 => f,
        _ => 1.0,
    };
    let s3 = vm.get(o, "DrawScale3D");
    let s3 = if matches!(s3, Value::Struct(_)) { vm.vec3(&s3) } else { [1.0; 3] };
    let s3 = if s3 == [0.0; 3] { [1.0; 3] } else { s3 };
    let pose = rp::Pose::from_parts(convert::to_rapier(convert::pos(loc)), convert::quat_to_rapier(convert::rot(rot)));
    (pose, convert::scale([s3[0] * ds, s3[1] * ds, s3[2] * ds]))
}

/// Brush/Cylinder components of the map by owner export (built once per host).
fn component_index(vm: &Vm) -> HashMap<i32, Vec<usize>> {
    let mut m: HashMap<i32, Vec<usize>> = HashMap::new();
    let Some(mp) = vm.map_pkg else { return m };
    let p = &vm.reg.packages[mp];
    for i in 0..p.exports.len() {
        let outer = p.exports[i].outer;
        if outer > 0 {
            let c = p.class_name(i as i32 + 1);
            if c == "BrushComponent" || c == "CylinderComponent" {
                m.entry(outer).or_default().push(i);
            }
        }
    }
    m
}

fn shapes_for(vm: &mut Vm, originator: ObjId, index: &HashMap<i32, Vec<usize>>) -> Vec<(rp::Pose, rp::SharedShape)> {
    let Some(r) = vm.ref_of.get(&originator).copied() else { return vec![] };
    let (pose, scale) = actor_pose(vm, originator);
    let p = &vm.reg.packages[r.pkg];
    let actor_idx = r.export as i32 + 1;
    let comps: Vec<usize> = if Some(r.pkg) == vm.map_pkg { index.get(&actor_idx).cloned().unwrap_or_default() } else { (0..p.exports.len()).filter(|&i| p.exports[i].outer == actor_idx).collect() };
    // a volume: its BrushComponent's convex hull(s)
    for &i in &comps {
        if p.class_name(i as i32 + 1) != "BrushComponent" {
            continue;
        }
        let Ok((_, cps)) = upk::props::export_props(p, i) else { continue };
        let Some(upk::props::Value::Struct(_, agg)) = upk::props::find(&cps, "BrushAggGeom") else { continue };
        let shapes = upk::physasset::agg_geom(p, agg);
        let parts: Vec<(rp::Pose, rp::SharedShape)> = crate::shapes::colliders(&shapes, scale).into_iter().map(|(local, s)| (pose * local, s)).collect();
        if !parts.is_empty() {
            return parts;
        }
    }
    // a Trigger: its collision cylinder
    for &i in &comps {
        if p.class_name(i as i32 + 1) != "CylinderComponent" {
            continue;
        }
        let props = vm.reg.merged_props(ObjRef { pkg: r.pkg, export: i });
        let f = |k: &str, d: f32| match upk::registry::find(&props, k) {
            Some((_, upk::props::Value::Float(v))) => *v,
            _ => d,
        };
        let (radius, height) = (f("CollisionRadius", 40.0), f("CollisionHeight", 40.0));
        let s = rp::SharedShape::cylinder(height * convert::S, radius * convert::S);
        return vec![(pose, s)];
    }
    vec![]
}

#[allow(clippy::too_many_arguments)]
pub fn kismet_touch(
    mut m: ResMut<Mutators>,
    mut st: Local<TouchState>,
    goats: Query<&Transform, With<crate::goat::Goat>>,
    phys: Res<crate::physics::Physics>,
    riding: Res<crate::vehicle::Riding>,
    bikes: Query<&crate::vehicle::Bike>,
) {
    let mut centers = None;
    if std::env::var_os("GOAT_NO_TOUCH").is_some() {
        return;
    }
    let generation = m.generation;
    let Some(h) = m.host.as_mut() else {
        st.shapes.clear();
        return;
    };
    if st.generation != generation || (st.shapes.is_empty() && !h.kismet.touch.is_empty() && h.ticks < 3) {
        st.generation = generation;
        let t0 = std::time::Instant::now();
        let touch = h.kismet.touch.clone();
        let index = component_index(&h.vm);
        st.shapes = touch
            .into_iter()
            .map(|(event, orig)| {
                let (goat, props) = proximity(&mut h.vm, event);
                Shape { event, parts: shapes_for(&mut h.vm, orig, &index), goat, props, inside: false, props_inside: vec![] }
            })
            .filter(|s| !s.parts.is_empty())
            .collect();
        let index2 = component_index(&h.vm);
        let mut mapped: Vec<ObjId> = h.map_actors.clone();
        // GoatGame's own trigger volumes placed in the map react to the goat through their
        // script Touch/Untouch (GGTriggerVolumeMusic, Race, Ritual, TimeTrial, CTF, Hockey...)
        if let (Some(mp), Some(tv)) = (h.vm.map_pkg, h.vm.class_by_name("GGTriggerVolume")) {
            let p = &h.vm.reg.packages[mp];
            let idx: Vec<i32> = (0..p.exports.len())
                .filter(|&i| {
                    let c = p.class_name(i as i32 + 1);
                    (c.starts_with("GGTriggerVolume") || c.starts_with("GGRaceCheckpoint")) && p.obj_path(i as i32 + 1).contains(".PersistentLevel.")
                })
                .map(|i| i as i32 + 1)
                .collect();
            let cp = h.vm.class_by_name("GGRaceCheckpointBase");
            for i in idx {
                if let Some(o) = h.vm.obj_for_ref(mp, i) {
                    let c = h.vm.obj(o).class;
                    let ok = h.vm.is_child_of(c, tv) || cp.is_some_and(|cp| h.vm.is_child_of(c, cp));
                    if ok && !mapped.contains(&o) {
                        mapped.push(o);
                    }
                }
            }
        }
        let mut touchy = vec![];
        for o in mapped {
            let c = h.vm.obj(o).class;
            if h.vm.defines(c, "Touch").is_some() || h.vm.defines(c, "UnTouch").is_some() {
                let parts = shapes_for(&mut h.vm, o, &index2);
                if !parts.is_empty() {
                    touchy.push((o, parts, false));
                }
            }
        }
        let cps = touchy.iter().filter(|(o, _, _)| h.vm.obj(*o).name.contains("Checkpoint")).count();
        if std::env::var_os("UVM_TOUCH_TRACE").is_some() {
            println!("    touch actors: {} ({cps} race checkpoints)", touchy.len());
            for (o, parts, _) in touchy.iter().filter(|(o, _, _)| h.vm.obj(*o).name.contains("Checkpoint")) {
                let ab: Vec<String> = parts.iter().map(|(pose, sh)| { let a = sh.compute_aabb(pose); format!("{:.1?}..{:.1?}", convert::from_rapier(a.mins), convert::from_rapier(a.maxs)) }).collect();
                println!("      {} parts {ab:?}", h.vm.obj(*o).name);
            }
        }
        st.actors = touchy;
        let n = st.shapes.len();
        h.vm.say(format!("kismet: {n} touch volumes ({:.0} ms)", t0.elapsed().as_secs_f32() * 1000.0));
        // volume centres, and whether touching leads to a score (tests)
        let mut c = vec![];
        for s in &st.shapes {
            let Some((pose, shape)) = s.parts.first() else { continue };
            let aabb = shape.compute_aabb(pose);
            let mid = convert::from_rapier((aabb.mins + aabb.maxs) * 0.5);
            let scores = s.goat && h.leads_to_score(s.event);
            c.push((s.event, mid, scores));
        }
        centers = Some(c);
    }
    if let Some(c) = centers {
        m.touch_centers = c;
    }
    let Some(h) = m.host.as_mut() else { return };
    let Ok(t) = goats.single() else { return };
    let pt = convert::to_rapier(t.translation);
    // Touch overlaps the pawn's collision cylinder (radius 25, half-height 30 uu), and a goat
    // that moved through a thin volume since last frame touched it too
    let r = crate::goat::tune::COLLISION_RADIUS * convert::S;
    let hh = crate::goat::tune::COLLISION_HEIGHT * convert::S;
    let goat_shape = rp::SharedShape::capsule_y((hh - r).max(0.01), r);
    let goat_pose = rp::Pose::from_translation(pt);
    let prev = st.prev_goat.unwrap_or(t.translation);
    let travel = t.translation - prev;
    st.prev_goat = Some(t.translation);
    let overlaps = |pose: &rp::Pose, shape: &rp::SharedShape| -> bool {
        if shape.contains_point(pose, pt) {
            return true;
        }
        if rapier3d::parry::query::intersection_test(&goat_pose, &*goat_shape, pose, &**shape).map(|x| x.intersecting).unwrap_or(false) {
            return true;
        }
        let len = travel.length();
        len > 0.05
            && shape
                .cast_ray(pose, &rp::Ray::new(convert::to_rapier(prev), convert::to_rapier(travel / len)), len, true)
                .is_some()
    };
    // (event, output, instigator: None = the goat / Some(prop body))
    let mut fired: Vec<(ObjId, usize, Option<(rp::RigidBodyHandle, String, Vec3)>)> = vec![];
    // moving props (KActor touch events)
    let moving: Vec<(rp::RigidBodyHandle, Vec3)> = if st.shapes.iter().any(|s| s.props) {
        phys.world
            .bodies
            .iter()
            .filter(|(h, b)| b.is_dynamic() && !b.is_sleeping() && phys.names.contains_key(h))
            .map(|(h, b)| (h, convert::from_rapier(b.translation())))
            .collect()
    } else {
        vec![]
    };
    for s in &mut st.shapes {
        if s.goat {
            let inside = s.parts.iter().any(|(pose, shape)| overlaps(pose, shape));
            if inside != s.inside {
                s.inside = inside;
                fired.push((s.event, if inside { 0 } else { 1 }, None));
            }
        }
        if s.props {
            for (bh, p) in &moving {
                let inside = s.parts.iter().any(|(pose, shape)| shape.contains_point(pose, convert::to_rapier(*p)));
                let was = s.props_inside.contains(bh);
                if inside && !was {
                    s.props_inside.push(*bh);
                    fired.push((s.event, 0, Some((*bh, phys.names[bh].clone(), *p))));
                } else if !inside && was {
                    s.props_inside.retain(|b| b != bh);
                    fired.push((s.event, 1, Some((*bh, phys.names[bh].clone(), *p))));
                }
            }
        }
    }
    let mut touches: Vec<(ObjId, bool)> = vec![];
    let trace = std::env::var_os("UVM_TOUCH_TRACE").is_some();
    for (o, parts, inside) in &mut st.actors {
        let now = parts.iter().any(|(pose, shape)| overlaps(pose, shape));
        if trace && h.vm.obj(*o).name == "GGRaceCheckpoint_33" && h.ticks % 30 == 0 {
            let info: Vec<String> = parts.iter().map(|(pose, sh)| format!("{:?} dist {:.2} local {:.2?}", sh.shape_type(), sh.distance_to_point(pose, pt, true), convert::from_rapier(pose.inverse_transform_point(pt)))).collect();
            println!("      cp33 goat {:.1?} inside {now} {info:?}", t.translation);
        }
        if now != *inside {
            *inside = now;
            touches.push((*o, now));
        }
    }
    if fired.is_empty() && touches.is_empty() {
        return;
    }
    // on a bike the pawn touching things is the vehicle (GGGameInfoRace BikeRace wants it)
    let ridden = riding.bike.and_then(|e| bikes.get(e).ok()).and_then(|b| b.export);
    let goat = h.goat;
    let at = [t.translation.x / convert::S, t.translation.z / convert::S, t.translation.y / convert::S];
    if !touches.is_empty() {
        h.with_vm(move |h| {
            let pawn = match ridden.and_then(|x| h.map_actor(x)) {
                Some(bike) => {
                    // DriverEnter: the vehicle is possessed by the player's controller
                    let pc = goat.and_then(|g| h.vm.get(g, "Controller").obj());
                    h.vm.set(bike, "Controller", Value::Obj(pc));
                    Some(bike)
                }
                None => goat,
            };
            for (o, enter) in touches {
                if std::env::var_os("UVM_TOUCH_TRACE").is_some() {
                    println!("    touch {} enter {enter}", h.vm.obj(o).name);
                }
                if enter {
                    let l = h.vm.vector(at[0], at[1], at[2]);
                    let n = h.vm.vector(0.0, 0.0, 1.0);
                    h.vm.call_named(o, "Touch", vec![Value::Obj(pawn), Value::Obj(None), l, n]);
                } else {
                    h.vm.call_named(o, "UnTouch", vec![Value::Obj(pawn)]);
                }
            }
        });
    }
    if fired.is_empty() {
        return;
    }
    h.with_vm(move |h| {
        for (ev, out, prop) in fired {
            let who = match prop {
                None => goat,
                Some((bh, name, p)) => {
                    let (i, g) = bh.into_raw_parts();
                    let thing = WorldThing::Prop { body: (i as u64) | ((g as u64) << 32), name };
                    Some(h.vm.proxy_for(&thing, [p.x / convert::S, p.z / convert::S, p.y / convert::S]))
                }
            };
            h.trigger_event(ev, out, who);
        }
        h.kismet_run();
    });
}
