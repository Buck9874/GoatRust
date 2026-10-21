//! Kismet matinee (SeqAct_Interp): InterpData movement tracks drive the bound actors. The VM side
//! keeps the playing matinees (Play / Reverse / Stop / Pause / Change Dir inputs, Completed and
//! Reversed outputs, event-track outputs, looping, PlayRate), writes the actors' Location/Rotation
//! and sends HostEvent::Mover; `movers` moves the actors' entities and static colliders.

use super::bridge::{Host, HostEvent};
use super::kismet::{field, struct_list};
use super::*;
use crate::convert;
use bevy::prelude::*;
use rapier3d::prelude as rp;
use std::sync::Arc;
use upk::matinee::InterpData;

pub struct Playing {
    op: ObjId,
    data: Arc<InterpData>,
    pos: f32,
    dir: f32,
    playing: bool,
    paused: bool,
    /// Per group: (group index, actors).
    groups: Vec<(usize, Vec<ObjId>)>,
}

#[derive(Default)]
pub struct Matinees {
    pub list: Vec<Playing>,
    /// Actor placement when a matinee first touched it: (Location, Rotation).
    initial: HashMap<ObjId, ([f32; 3], [i32; 3])>,
}

impl Matinees {
    pub fn busy(&self) -> bool {
        self.list.iter().any(|p| p.playing && !p.paused)
    }
}

fn rot_axes(r: [i32; 3]) -> [[f32; 3]; 3] {
    let k = std::f32::consts::TAU / 65536.0;
    let (sp, cp) = (r[0] as f32 * k).sin_cos();
    let (sy, cy) = (r[1] as f32 * k).sin_cos();
    let (sr, cr) = (r[2] as f32 * k).sin_cos();
    [
        [cp * cy, cp * sy, sp],
        [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp],
        [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// FMatrix::Rotator from axis rows.
fn axes_rotator(m: [[f32; 3]; 3]) -> [i32; 3] {
    let k = 65536.0 / std::f32::consts::TAU;
    let x = m[0];
    let pitch = x[2].atan2((x[0] * x[0] + x[1] * x[1]).sqrt());
    let yaw = x[1].atan2(x[0]);
    let (p, y) = ((pitch * k).round() as i32, (yaw * k).round() as i32);
    let sy = rot_axes([p, y, 0])[1];
    let roll = dot(m[2], sy).atan2(dot(m[1], sy));
    [p, y, (roll * k).round() as i32]
}

/// Rows of `a` (in the frame of `b`) taken to world: a * b.
fn mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let row = |r: [f32; 3]| [0, 1, 2].map(|j| r[0] * b[0][j] + r[1] * b[1][j] + r[2] * b[2][j]);
    [row(a[0]), row(a[1]), row(a[2])]
}

/// Track (pos, euler degrees roll/pitch/yaw) -> world (Location, Rotation).
fn world_of(init: ([f32; 3], [i32; 3]), relative: bool, pos: [f32; 3], euler: [f32; 3]) -> ([f32; 3], [i32; 3]) {
    let d = 65536.0 / 360.0;
    let rel_rot = [(euler[1] * d).round() as i32, (euler[2] * d).round() as i32, (euler[0] * d).round() as i32];
    if !relative {
        return (pos, rel_rot);
    }
    let ia = rot_axes(init.1);
    let loc = [0, 1, 2].map(|j| init.0[j] + pos[0] * ia[0][j] + pos[1] * ia[1][j] + pos[2] * ia[2][j]);
    let rot = axes_rotator(mul(rot_axes(rel_rot), ia));
    (loc, rot)
}

impl Host {
    fn interp_link_objects(&mut self, op: ObjId, desc: &str) -> Vec<ObjId> {
        let mut out = vec![];
        for l in struct_list(&self.vm.get(op, "VariableLinks")) {
            let d = field(&self.vm, &l, "LinkDesc").map(|v| v.str().to_string()).unwrap_or_default();
            if !d.eq_ignore_ascii_case(desc) {
                continue;
            }
            let vars: Vec<ObjId> = match field(&self.vm, &l, "LinkedVariables") {
                Some(Value::Array(a)) => a.iter().filter_map(|x| x.obj()).collect(),
                _ => vec![],
            };
            for v in vars {
                let cn = self.vm.class_name(self.vm.obj(v).class).to_string();
                if cn == "SeqVar_Object" {
                    if let Some(o) = self.vm.get(v, "ObjValue").obj() {
                        out.push(o);
                    }
                } else if cn == "SeqVar_Player" {
                    out.extend(self.goat);
                } else {
                    out.push(v);
                }
            }
        }
        out
    }

    /// SeqAct_Interp got an impulse on `input` (Play, Reverse, Stop, Pause, Change Dir).
    pub fn matinee_input(&mut self, op: ObjId, input: usize) {
        let i = match self.kismet.matinee.list.iter().position(|p| p.op == op) {
            Some(i) => i,
            None => {
                let Some(&data_obj) = self.interp_link_objects(op, "Data").first() else {
                    self.vm.say(format!("matinee {}: no InterpData", self.vm.obj(op).name));
                    return;
                };
                let Some(r) = self.vm.ref_of.get(&data_obj).copied() else { return };
                let Some(data) = upk::matinee::load_interp(&self.vm.reg, r) else { return };
                let mut groups = vec![];
                for (gi, g) in data.groups.iter().enumerate() {
                    let has_keys = g.movement.as_ref().is_some_and(|m| !m.pos.is_empty() || !m.euler.is_empty() || !m.axes.is_empty());
                    if !has_keys {
                        continue;
                    }
                    let actors = self.interp_link_objects(op, &g.name);
                    for &a in &actors {
                        if !self.kismet.matinee.initial.contains_key(&a) {
                            let l = self.vm.get(a, "Location");
                            let r = self.vm.get(a, "Rotation");
                            let init = (self.vm.vec3(&l), self.vm.rot3(&r));
                            self.kismet.matinee.initial.insert(a, init);
                        }
                    }
                    groups.push((gi, actors));
                }
                let names: Vec<String> = groups.iter().flat_map(|(_, a)| a.iter().map(|&o| self.vm.obj(o).name.clone())).collect();
                self.vm.say(format!("matinee {} ({}, {:.1} s) moves [{}]", self.vm.obj(op).name, data.path, data.length, names.join(", ")));
                self.kismet.matinee.list.push(Playing { op, data: Arc::new(data), pos: 0.0, dir: 1.0, playing: false, paused: false, groups });
                self.kismet.matinee.list.len() - 1
            }
        };
        let rewind_if_playing = self.vm.get(op, "bRewindIfAlreadyPlaying").truthy();
        let p = &mut self.kismet.matinee.list[i];
        match input {
            0 => {
                if p.playing && !p.paused && p.dir > 0.0 && !rewind_if_playing {
                    return;
                }
                if !p.playing || rewind_if_playing || p.pos >= p.data.length {
                    if !(p.paused && p.playing) {
                        p.pos = 0.0;
                    }
                }
                p.dir = 1.0;
                p.playing = true;
                p.paused = false;
            }
            1 => {
                if !p.playing && p.pos <= 0.0 {
                    p.pos = p.data.length;
                }
                p.dir = -1.0;
                p.playing = true;
                p.paused = false;
            }
            2 => p.playing = false,
            3 => p.paused = !p.paused,
            _ => p.dir = -p.dir,
        }
    }

    /// Advance playing matinees, move their actors, fire outputs.
    pub fn matinee_tick(&mut self, dt: f32) {
        let mut fire: Vec<(ObjId, Option<usize>, Option<String>)> = vec![];
        let mut moves: Vec<(ObjId, bool, [f32; 3], [f32; 3])> = vec![];
        for idx in 0..self.kismet.matinee.list.len() {
            let op = self.kismet.matinee.list[idx].op;
            if !self.kismet.matinee.list[idx].playing || self.kismet.matinee.list[idx].paused {
                continue;
            }
            let rate = match self.vm.get(op, "PlayRate") {
                Value::Float(r) if r > 0.0 => r,
                _ => 1.0,
            };
            let looping = self.vm.get(op, "bLooping").truthy();
            let p = &mut self.kismet.matinee.list[idx];
            let len = p.data.length.max(1e-3);
            let old = p.pos;
            let mut new = old + dt * rate * p.dir;
            // event keys crossed this step
            for g in &p.data.groups {
                for (t, name) in &g.events {
                    let crossed = if p.dir > 0.0 { *t > old && *t <= new } else { *t < old && *t >= new };
                    if crossed {
                        fire.push((op, None, Some(name.clone())));
                    }
                }
            }
            if new >= len && p.dir > 0.0 {
                if looping {
                    new -= len;
                } else {
                    new = len;
                    p.playing = false;
                    fire.push((op, Some(0), None));
                }
            } else if new <= 0.0 && p.dir < 0.0 {
                if looping {
                    new += len;
                } else {
                    new = 0.0;
                    p.playing = false;
                    fire.push((op, Some(1), None));
                }
            }
            p.pos = new;
            for (gi, actors) in &p.groups {
                let Some(m) = &p.data.groups[*gi].movement else { continue };
                let (pos, euler) = m.eval(new);
                for &a in actors {
                    moves.push((a, m.relative, pos, euler));
                }
            }
        }
        for (a, relative, pos, euler) in moves {
            if self.vm.obj(a).destroyed {
                continue;
            }
            let Some(&init) = self.kismet.matinee.initial.get(&a) else { continue };
            let (loc, rot) = world_of(init, relative, pos, euler);
            let lv = self.vm.vector(loc[0], loc[1], loc[2]);
            let rv = self.vm.rotator(rot[0], rot[1], rot[2]);
            self.vm.set(a, "Location", lv);
            self.vm.set(a, "Rotation", rv);
            let name = self.vm.obj(a).name.clone();
            self.vm.events.push(HostEvent::Mover { name, location: loc, rotation: rot, initial: init });
        }
        for (op, out, event) in fire {
            match (out, event) {
                (Some(o), _) => self.fire_output(op, o),
                (None, Some(name)) => {
                    let outs = struct_list(&self.vm.get(op, "OutputLinks"));
                    let i = outs.iter().position(|l| field(&self.vm, l, "LinkDesc").is_some_and(|v| v.str().eq_ignore_ascii_case(&name)));
                    if let Some(i) = i {
                        self.fire_output(op, i);
                    }
                }
                _ => {}
            }
        }
    }
}

impl Host {
    /// Every SeqAct_Interp of the map: (op, moved actors, farthest travel in Unreal units).
    /// Loads them without playing (a Stop impulse).
    pub fn matinee_plan(&mut self) -> Vec<(ObjId, Vec<String>, f32)> {
        let ops: Vec<ObjId> = self.kismet.ops.clone();
        let mut out = vec![];
        for op in ops {
            if self.vm.class_name(self.vm.obj(op).class) != "SeqAct_Interp" {
                continue;
            }
            if !self.kismet.matinee.list.iter().any(|p| p.op == op) {
                self.matinee_input(op, 2);
            }
            let Some(p) = self.kismet.matinee.list.iter().find(|p| p.op == op) else { continue };
            let mut names = vec![];
            let mut travel: f32 = 0.0;
            for (gi, actors) in &p.groups {
                let Some(m) = &p.data.groups[*gi].movement else { continue };
                let (a, _) = m.eval(0.0);
                for k in 1..=8 {
                    let (b, _) = m.eval(p.data.length * k as f32 / 8.0);
                    travel = travel.max(((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt());
                }
                names.extend(actors.iter().map(|&o| self.vm.obj(o).name.clone()));
            }
            out.push((op, names, travel));
        }
        out
    }

    /// An impulse on a sequence op's input, run with the next Kismet tick.
    pub fn kismet_impulse(&mut self, op: ObjId, input: usize) {
        self.kismet.queue.push_back((op, input));
    }
}

#[derive(Default)]
pub struct MatineeTest {
    t: f32,
    mark: f32,
    target: Option<(Entity, Vec3, String)>,
    collider: Option<(rp::ColliderHandle, Vec3)>,
    shot: bool,
}

/// `goat --test matinee`: play the Goatville matinee whose mover travels farthest (through the
/// Kismet queue, Play input) and check its entity and static collider move.
#[allow(clippy::too_many_arguments)]
pub fn matinee_scenario(
    args: Res<crate::Args>,
    time: Res<Time<Real>>,
    mut st: Local<MatineeTest>,
    mut m: ResMut<super::plugin::Mutators>,
    phys: Res<crate::physics::Physics>,
    q: Query<(Entity, &Name, &GlobalTransform), Without<ChildOf>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("matinee") {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    let fail = |why: &str| -> ! {
        println!("TEST matinee FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    if st.target.is_none() {
        let Some(h) = m.host.as_mut() else {
            if t > 30.0 {
                fail("no script host after 30 s");
            }
            return;
        };
        if h.kismet.ops.is_empty() {
            if t > 30.0 {
                fail("no Kismet loaded after 30 s");
            }
            return;
        }
        let mut plan = h.matinee_plan();
        println!("  t={t:.1} {} matinees, {} with movers", plan.len(), plan.iter().filter(|p| !p.1.is_empty() && p.2 > 1.0).count());
        plan.sort_by(|a, b| b.2.total_cmp(&a.2));
        let found = plan.iter().find_map(|(op, names, travel)| {
            names.iter().find_map(|n| q.iter().find(|(_, en, _)| en.as_str().eq_ignore_ascii_case(n)).map(|(e, _, gt)| (*op, e, gt.translation(), n.clone(), *travel)))
        });
        let Some((op, e, at, name, travel)) = found else { fail("no matinee mover has a world entity") };
        let opname = h.vm.obj(op).name.clone();
        h.kismet_impulse(op, 0);
        println!("  t={t:.1} playing {opname}: {name} travels {travel:.0} uu");
        st.target = Some((e, at, name.clone()));
        st.collider = phys.movers.get(&name.to_ascii_lowercase()).and_then(|l| l.first()).and_then(|(ch, _, _)| phys.world.colliders.get(*ch).map(|c| (*ch, crate::convert::from_rapier(c.translation()))));
        st.mark = t;
        return;
    }
    let Some((e, start, name)) = st.target.clone() else { return };
    let Ok((_, _, gt)) = q.get(e) else { fail("the mover entity is gone") };
    let moved = gt.translation().distance(start);
    // early abort: nothing moved ~2 s after Play
    if t - st.mark > 2.0 && moved < 0.05 {
        fail(&format!("{name} did not move after Play"));
    }
    if t - st.mark > 2.5 && !st.shot {
        st.shot = true;
        if let Some(p) = args.shot.clone() {
            commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(p));
        }
    }
    if t - st.mark > 3.0 {
        let col = st.collider.and_then(|(ch, p0)| phys.world.colliders.get(ch).map(|c| crate::convert::from_rapier(c.translation()).distance(p0)));
        println!("  t={t:.1} {name} moved {moved:.2} m, its collider {col:?} m");
        if col.is_some_and(|d| d < 0.05) {
            fail("the mover's collider did not follow");
        }
        println!("TEST matinee PASS");
        exit.write(AppExit::Success);
    }
}

/// A map actor moved by matinee: (actor name, Location, Rotation, initial Location, Rotation).
pub type MoverCmd = (String, [f32; 3], [i32; 3], ([f32; 3], [i32; 3]));

/// Every entity named like the actor (one per mesh component, world.rs) keeps its offset from the
/// actor; static colliders recorded for the actor (Physics::movers) follow.
pub fn movers(
    mut sa: ResMut<super::actors::ScriptActors>,
    mut phys: ResMut<crate::physics::Physics>,
    mut q: Query<(Entity, &Name, &mut Transform), Without<ChildOf>>,
    mut initial: Local<HashMap<Entity, Transform>>,
) {
    let _prof_scope = crate::prof::scope("matinee::movers");
    if sa.mover_queue.is_empty() {
        return;
    }
    // the latest command per actor
    let mut latest: HashMap<String, MoverCmd> = HashMap::new();
    for c in sa.mover_queue.drain(..) {
        latest.insert(c.0.to_ascii_lowercase(), c);
    }
    let phys = &mut *phys;
    for (key, (name, loc, rot, init)) in latest {
        let start = Transform { translation: convert::pos(init.0), rotation: convert::rot(init.1), scale: Vec3::ONE };
        let now = Transform { translation: convert::pos(loc), rotation: convert::rot(rot), scale: Vec3::ONE };
        // delta = now * start^-1 (applied to each part's placement at load)
        let delta = now.to_matrix() * start.to_matrix().inverse();
        let (_, dr, _) = delta.to_scale_rotation_translation();
        for (e, n, mut t) in &mut q {
            if !n.as_str().eq_ignore_ascii_case(&name) {
                continue;
            }
            let base = *initial.entry(e).or_insert(*t);
            let m = Transform::from_matrix(delta * base.to_matrix());
            if *t != m {
                *t = m;
            }
        }
        if let Some(list) = phys.movers.get(&key) {
            for &(ch, p0, r0) in list {
                if let Some(c) = phys.world.colliders.get_mut(ch) {
                    let p = delta.transform_point3(p0);
                    c.set_position(rp::Pose::from_parts(convert::to_rapier(p), convert::quat_to_rapier((dr * r0).normalize())));
                }
            }
        }
    }
}
