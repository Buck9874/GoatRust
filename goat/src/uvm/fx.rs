//! Script particle effects: ParticleSystemComponents made by EmitterPool.SpawnEmitter* or attached
//! to actors/sockets play through crate::particles. The natives queue FxCmds (HostEvent::Fx);
//! `script_fx` turns them into ParticleFx entities parented to the goat's bones, attached meshes,
//! script actors or a world spot, and mirrors HiddenGame / destruction every frame.

use super::actors::{GameRegistry, ScriptActors};
use super::bridge::HostEvent;
use super::exec::Call;
use super::plugin::Mutators;
use super::value::Value;
use super::{NativeFn, ObjId, Vm};
use crate::convert;
use crate::particles::{FxLibrary, ParticleFx};
use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum FxAttach {
    /// World location / rotation (Unreal units).
    World([f32; 3], [i32; 3]),
    /// A mesh component's socket or bone (empty = the component itself), plus an offset.
    Comp { mesh: ObjId, socket: String, offset: [f32; 3] },
    /// An actor's origin.
    Actor(ObjId),
}

#[derive(Debug, Clone)]
pub enum FxCmd {
    Spawn { comp: ObjId, template: String, at: FxAttach, active: bool, one_shot: bool },
    Active(ObjId, bool),
    Kill(ObjId),
    Template(ObjId, String),
    /// Kismet Toggle on a map Emitter actor (None = toggle).
    MapEmitter { name: String, on: Option<bool> },
}

fn template_path(vm: &Vm, v: &Value) -> Option<String> {
    match &vm.obj(v.obj()?).kind {
        super::Kind::Asset { path, .. } => Some(path.clone()),
        _ => None,
    }
}

fn is_psc(vm: &mut Vm, o: Option<ObjId>) -> bool {
    let Some(o) = o else { return false };
    vm.class_by_name("ParticleSystemComponent").is_some_and(|c| vm.is_child_of(vm.obj(o).class, c))
}

fn new_psc(vm: &mut Vm, template: &Value) -> Option<ObjId> {
    let class = vm.class_by_name("ParticleSystemComponent")?;
    let p = vm.spawn_object(class, None);
    vm.set(p, "Template", template.clone());
    vm.set(p, "bIsActive", Value::Bool(true));
    Some(p)
}

fn spawn_cmd(vm: &mut Vm, comp: ObjId, at: FxAttach, active: bool, one_shot: bool) {
    let t = vm.get(comp, "Template");
    let Some(template) = template_path(vm, &t) else {
        vm.say("particle component without a template".into());
        return;
    };
    vm.say(format!("particles {template} ({at:?})"));
    vm.events.push(HostEvent::Fx(FxCmd::Spawn { comp, template, at, active, one_shot }));
}

pub fn register(vm: &mut Vm) {
    let n = &mut vm.natives;
    let mut add = |k: &str, f: NativeFn| {
        n.insert(k.to_ascii_lowercase(), f);
    };
    // SpawnEmitter(Template, Location, Rotation, AttachToActor, ...)
    add("emitterpool.spawnemitter", |vm, c| {
        let Some(p) = new_psc(vm, c.arg(0)) else { return Value::Obj(None) };
        let at = match c.obj(3) {
            Some(a) => FxAttach::Actor(a),
            None => FxAttach::World(vm.vec3(c.arg(1)), vm.rot3(c.arg(2))),
        };
        spawn_cmd(vm, p, at, true, true);
        Value::Obj(Some(p))
    });
    // SpawnEmitterMeshAttachment(Template, Mesh, SocketName, bAttachToSocket, RelativeLoc, RelativeRot)
    add("emitterpool.spawnemittermeshattachment", |vm, c| {
        let Some(p) = new_psc(vm, c.arg(0)) else { return Value::Obj(None) };
        if let Some(mesh) = c.obj(1) {
            let socket = vm.to_string(c.arg(2));
            let offset = if matches!(c.arg(4), Value::Struct(_)) { vm.vec3(c.arg(4)) } else { [0.0; 3] };
            spawn_cmd(vm, p, FxAttach::Comp { mesh, socket, offset }, true, true);
        }
        Value::Obj(Some(p))
    });
    // SpawnEmitterCustomLifetime(Template, bSkipAutoActivate): placed later by the script
    add("emitterpool.spawnemittercustomlifetime", |vm, c| {
        let Some(p) = new_psc(vm, c.arg(0)) else { return Value::Obj(None) };
        let skip = c.arg(1).truthy();
        vm.set(p, "bIsActive", Value::Bool(!skip));
        Value::Obj(Some(p))
    });
    add("particlesystemcomponent.activatesystem", |vm, c| {
        if let Some(o) = c.this {
            vm.set(o, "bIsActive", Value::Bool(true));
            vm.events.push(HostEvent::Fx(FxCmd::Active(o, true)));
        }
        Value::Int(0)
    });
    add("particlesystemcomponent.deactivatesystem", |vm, c| {
        if let Some(o) = c.this {
            vm.set(o, "bIsActive", Value::Bool(false));
            vm.events.push(HostEvent::Fx(FxCmd::Active(o, false)));
        }
        Value::Int(0)
    });
    add("particlesystemcomponent.setactive", |vm, c| {
        if let Some(o) = c.this {
            let on = c.arg(0).truthy();
            vm.set(o, "bIsActive", Value::Bool(on));
            vm.events.push(HostEvent::Fx(FxCmd::Active(o, on)));
        }
        Value::Int(0)
    });
    add("particlesystemcomponent.killparticlesforced", |vm, c| {
        if let Some(o) = c.this {
            vm.events.push(HostEvent::Fx(FxCmd::Kill(o)));
        }
        Value::Int(0)
    });
    add("particlesystemcomponent.settemplate", |vm, c| {
        if let Some(o) = c.this {
            let t = c.arg(0).clone();
            vm.set(o, "Template", t.clone());
            if let Some(p) = template_path(vm, &t) {
                vm.events.push(HostEvent::Fx(FxCmd::Template(o, p)));
            }
        }
        Value::Int(0)
    });
    // components attached by script: particle components play, the rest as before
    add("actor.attachcomponent", |vm, c| {
        let comp = c.obj(0);
        if let (true, Some(p), Some(a)) = (is_psc(vm, comp), comp, c.this) {
            let auto = !matches!(vm.get(p, "bAutoActivate"), Value::Bool(false));
            spawn_cmd(vm, p, FxAttach::Actor(a), auto, false);
            return Value::Int(0);
        }
        let what = vm.describe(&Value::Obj(comp));
        let who = c.this.map(|o| vm.obj(o).name.clone()).unwrap_or_default();
        vm.say(format!("AttachComponent({what}) on {who}"));
        Value::Int(0)
    });
    add("skeletalmeshcomponent.attachcomponenttosocket", |vm, c| {
        let comp = c.obj(0);
        let sock = vm.to_string(c.arg(1));
        if let (true, Some(p), Some(mesh)) = (is_psc(vm, comp), comp, c.this) {
            let auto = !matches!(vm.get(p, "bAutoActivate"), Value::Bool(false));
            spawn_cmd(vm, p, FxAttach::Comp { mesh, socket: sock, offset: [0.0; 3] }, auto, false);
            return Value::Int(0);
        }
        let what = vm.describe(&Value::Obj(comp));
        let who = comp_owner(vm, c);
        vm.say(format!("AttachComponentToSocket({what}, {sock}) on {who}"));
        vm.events.push(HostEvent::AttachToSocket { component: comp, mesh: c.this, socket: sock });
        Value::Int(0)
    });
    add("skeletalmeshcomponent.attachcomponent", |vm, c| {
        let comp = c.obj(0);
        let bone = vm.to_string(c.arg(1));
        if let (true, Some(p), Some(mesh)) = (is_psc(vm, comp), comp, c.this) {
            let auto = !matches!(vm.get(p, "bAutoActivate"), Value::Bool(false));
            spawn_cmd(vm, p, FxAttach::Comp { mesh, socket: bone, offset: [0.0; 3] }, auto, false);
            return Value::Int(0);
        }
        let what = vm.describe(&Value::Obj(comp));
        let who = comp_owner(vm, c);
        vm.say(format!("AttachComponent({what}, {bone}) on {who}"));
        Value::Int(0)
    });
}

fn comp_owner(vm: &Vm, c: &Call) -> String {
    c.this.and_then(|o| vm.obj(o).outer).map(|o| vm.obj(o).name.clone()).unwrap_or_default()
}

struct Playing {
    entity: Entity,
    /// Follow this actor's Location each frame (no entity to parent to).
    follow: Option<ObjId>,
    /// Frames until the entity exists (spawned through Commands).
    fresh: u8,
}

#[derive(Resource, Default)]
pub struct ScriptFx {
    playing: HashMap<ObjId, Playing>,
    /// Spawns waiting for their attachment (an attached mesh appears a frame later), seconds left.
    pending: Vec<(FxCmd, f32)>,
    generation: u64,
}

/// Where an effect hangs: (parent entity, local transform), or a followed actor.
enum Place {
    Parent(Entity, Transform),
    World(Transform),
    Follow(ObjId),
}

#[allow(clippy::too_many_arguments)]
fn place(vm: &mut Vm, goat_obj: Option<ObjId>, at: &FxAttach, sa: &ScriptActors, game: Option<&upk::registry::Registry>, goat: Option<(Entity, &crate::rig::Rig)>, rigs: &Query<&crate::rig::Rig>) -> Option<Place> {
    let goat_mesh = goat_obj.and_then(|g| vm.get(g, "Mesh").obj());
    match at {
        FxAttach::World(l, r) => Some(Place::World(Transform { translation: convert::pos(*l), rotation: convert::rot(*r), scale: Vec3::ONE })),
        FxAttach::Actor(a) => {
            if Some(*a) == goat_obj {
                return goat.map(|(e, _)| Place::Parent(e, Transform::IDENTITY));
            }
            match sa.entity_of(*a) {
                Some(e) => Some(Place::Parent(e, Transform::IDENTITY)),
                None => Some(Place::Follow(*a)),
            }
        }
        FxAttach::Comp { mesh, socket, offset } => {
            let off = Transform::from_translation(convert::pos(*offset));
            // the rig and skeletal mesh path behind the component
            let (holder, rig, mesh_path) = if Some(*mesh) == goat_mesh {
                let (e, rig) = goat?;
                let sk = vm.get(*mesh, "SkeletalMesh");
                (e, Some(rig), super::actors::asset_path_of(vm, &sk).unwrap_or_else(|| "goat.Mesh.goat".into()))
            } else if let Some(h) = sa.attached_entity(*mesh) {
                let sk = vm.get(*mesh, "SkeletalMesh");
                (h, rigs.get(h).ok(), super::actors::asset_path_of(vm, &sk).unwrap_or_default())
            } else {
                // a component of a script actor / proxy: that actor's origin
                let owner = vm.obj(*mesh).outer?;
                if Some(owner) == goat_obj {
                    return goat.map(|(e, _)| Place::Parent(e, off));
                }
                return Some(match sa.entity_of(owner) {
                    Some(e) => Place::Parent(e, off),
                    None => Place::Follow(owner),
                });
            };
            if socket.is_empty() || socket.eq_ignore_ascii_case("None") {
                return Some(Place::Parent(holder, off));
            }
            let Some(rig) = rig else {
                // static mesh: no sockets read yet, use its origin
                return Some(Place::Parent(holder, off));
            };
            let joint_of = |bone: &str| rig.data.bone_names.iter().position(|b| b.eq_ignore_ascii_case(bone)).and_then(|i| rig.joints.get(i).copied());
            if let Some((bone, xf)) = super::actors::socket_on(&vm.reg, game, &mesh_path, socket) {
                if let Some(j) = joint_of(&bone) {
                    return Some(Place::Parent(j, xf * off));
                }
            }
            // a bone name (bAttachToSocket=false)
            joint_of(socket).map(|j| Place::Parent(j, off))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn script_fx(
    time: Res<Time>,
    mut m: ResMut<Mutators>,
    game: Option<Res<GameRegistry>>,
    mut sa: ResMut<ScriptActors>,
    mut st: ResMut<ScriptFx>,
    mut lib: ResMut<FxLibrary>,
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    goats: Query<(Entity, &crate::rig::Rig), With<crate::goat::Goat>>,
    rigs: Query<&crate::rig::Rig>,
    mut fxs: Query<(&mut ParticleFx, Option<&crate::particles::MapEmitter>)>,
    mut tfs: Query<&mut Transform>,
) {
    let _prof_scope = crate::prof::scope("fx::script_fx");
    let st = &mut *st;
    if m.host.is_none() {
        for (_, p) in st.playing.drain() {
            commands.entity(p.entity).despawn();
        }
        st.pending.clear();
        sa.fx_queue.clear();
        return;
    };
    if st.generation != m.generation {
        st.generation = m.generation;
        for (_, p) in st.playing.drain() {
            commands.entity(p.entity).despawn();
        }
        st.pending.clear();
    }
    let Some(h) = m.host.as_mut() else { return };
    let goat_obj = h.goat;
    let vm = &mut h.vm;
    let game = game.map(|g| g.0.clone());
    let goat = goats.single().ok();
    let dt = time.delta_secs();
    let mut cmds: Vec<(FxCmd, f32)> = std::mem::take(&mut st.pending);
    cmds.extend(sa.fx_queue.drain(..).map(|c| (c, 3.0)));
    for (cmd, left) in cmds {
        match cmd {
            FxCmd::Spawn { comp, ref template, ref at, active, one_shot } => {
                let regs: Vec<&upk::registry::Registry> = game.as_deref().into_iter().chain(std::iter::once(&vm.reg)).collect();
                let Some(fx) = lib.get(&regs, template, &mut materials, &mut images) else { continue };
                let Some(pl) = place(vm, goat_obj, at, &sa, game.as_deref(), goat, &rigs) else {
                    if left > 0.0 {
                        st.pending.push((cmd.clone(), left - dt));
                    } else {
                        vm.say(format!("particles {template}: attachment not found"));
                    }
                    continue;
                };
                if let Some(old) = st.playing.remove(&comp) {
                    commands.entity(old.entity).despawn();
                }
                let mut pfx = ParticleFx::new(fx, active);
                pfx.despawn_when_done = one_shot;
                pfx.hidden = vm.get(comp, "HiddenGame").truthy();
                let name = Name::new(format!("scriptfx:{template}"));
                let (entity, follow) = match pl {
                    Place::Parent(parent, xf) => {
                        let e = commands.spawn((xf, Visibility::default(), pfx, name)).id();
                        commands.entity(parent).add_child(e);
                        (e, None)
                    }
                    Place::World(xf) => (commands.spawn((xf, Visibility::default(), pfx, name)).id(), None),
                    Place::Follow(a) => {
                        let l = vm.get(a, "Location");
                        let xf = Transform::from_translation(convert::pos(vm.vec3(&l)));
                        (commands.spawn((xf, Visibility::default(), pfx, name)).id(), Some(a))
                    }
                };
                st.playing.insert(comp, Playing { entity, follow, fresh: 3 });
            }
            FxCmd::Active(comp, on) => {
                if let Some((mut f, _)) = st.playing.get(&comp).and_then(|p| fxs.get_mut(p.entity).ok()) {
                    if on && !f.active {
                        f.restart();
                    }
                    f.active = on;
                } else if on {
                    // a component never placed (SpawnEmitterCustomLifetime): play it at its owner
                    if let Some(owner) = vm.obj(comp).outer {
                        let t = vm.get(comp, "Template");
                        if let Some(template) = template_path(vm, &t) {
                            st.pending.push((FxCmd::Spawn { comp, template, at: FxAttach::Actor(owner), active: true, one_shot: true }, 1.0));
                        }
                    }
                }
            }
            FxCmd::Kill(comp) => {
                if let Some((mut f, _)) = st.playing.get(&comp).and_then(|p| fxs.get_mut(p.entity).ok()) {
                    f.kill = true;
                }
            }
            FxCmd::MapEmitter { ref name, on } => {
                for (mut f, me) in &mut fxs {
                    if me.is_some_and(|m| m.0.eq_ignore_ascii_case(name)) {
                        let v = on.unwrap_or(!f.active);
                        if v && !f.active {
                            f.restart();
                        }
                        f.active = v;
                        vm.say(format!("Kismet: emitter {name} {}", if v { "on" } else { "off" }));
                    }
                }
            }
            FxCmd::Template(comp, template) => {
                let regs: Vec<&upk::registry::Registry> = game.as_deref().into_iter().chain(std::iter::once(&vm.reg)).collect();
                if let (Some(fx), Some((mut f, _))) = (lib.get(&regs, &template, &mut materials, &mut images), st.playing.get(&comp).and_then(|p| fxs.get_mut(p.entity).ok())) {
                    let (active, hidden, one) = (f.active, f.hidden, f.despawn_when_done);
                    *f = ParticleFx::new(fx, active);
                    f.hidden = hidden;
                    f.despawn_when_done = one;
                }
            }
        }
    }
    // mirror script state: hidden, destroyed, followed actors
    let mut gone = vec![];
    for (&comp, p) in st.playing.iter_mut() {
        let Ok((mut f, _)) = fxs.get_mut(p.entity) else {
            // finished one-shots despawn themselves
            if p.fresh > 0 {
                p.fresh -= 1;
            } else {
                gone.push(comp);
            }
            continue;
        };
        p.fresh = 0;
        if vm.obj(comp).destroyed || p.follow.is_some_and(|a| vm.obj(a).destroyed) {
            commands.entity(p.entity).despawn();
            gone.push(comp);
            continue;
        }
        let hidden = vm.get(comp, "HiddenGame").truthy();
        if f.hidden != hidden {
            f.hidden = hidden;
        }
        if let Some(a) = p.follow {
            let l = vm.get(a, "Location");
            if let Ok(mut t) = tfs.get_mut(p.entity) {
                t.translation = convert::pos(vm.vec3(&l));
            }
        }
    }
    for c in gone {
        st.playing.remove(&c);
    }
}
