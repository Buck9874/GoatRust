//! Script actors in the Bevy world: static-mesh actors mutator scripts spawn (rapier bodies when
//! they use rigid-body/falling/projectile physics or are KActors), components scripts attach to
//! the goat's sockets (hats, helmets), the goat's DrawScale, and the rapier-backed tracer used by
//! Trace/FastTrace.

use super::plugin::Mutators;
use super::*;
use crate::convert;
use bevy::prelude::*;
use rapier3d::prelude as rp;
use std::sync::Arc;
use upk::registry::{ObjRef, Registry};

/// The game's own registry (map + GoatGame.u + Startup.upk), for assets the VM's packages
/// don't hold (props and hats cooked into Startup or the map).
#[derive(Resource, Clone)]
pub struct GameRegistry(pub Arc<Registry>);

struct PropActor {
    entity: Entity,
    comp: ObjId,
    parts: Vec<Entity>,
    body: Option<rp::RigidBodyHandle>,
    /// Location last written into the script actor (to spot script teleports).
    written: [f32; 3],
}

#[derive(Default)]
pub(crate) struct Built {
    pub parts: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    verts: Vec<Vec3>,
}

#[derive(Resource, Default)]
pub struct ScriptActors {
    props: HashMap<ObjId, PropActor>,
    scanned: usize,
    /// Actors without a mesh yet (a script may set one after spawning), with seconds left.
    pending: Vec<(ObjId, f32)>,
    attached: HashMap<ObjId, Entity>,
    pub attach_queue: Vec<(Option<ObjId>, Option<ObjId>, String)>,
    pub detach_queue: Vec<ObjId>,
    /// MaterialInstanceConstants whose parameters changed.
    pub tint_queue: Vec<ObjId>,
    /// AddImpulse from script, handed to proxies::proxies (also for script-spawned props).
    pub impulses: Vec<(ObjId, [f32; 3], bool)>,
    built: HashMap<String, Option<Arc<Built>>>,
    /// Body -> script actor, for traces.
    pub by_body: HashMap<rp::RigidBodyHandle, ObjId>,
    /// Skeletal meshes attached to sockets.
    skel_cache: crate::rig::SkelCache,
    /// Attached rigs on the goat's skeleton (their joints copy the goat's pose).
    pub followers: Vec<Entity>,
    /// Particle commands from script, handled by fx::script_fx.
    pub fx_queue: Vec<super::fx::FxCmd>,
    /// Matinee moves of map actors, handled by matinee::movers.
    pub mover_queue: Vec<super::matinee::MoverCmd>,
    /// Map actors hidden / shown by script, handled by matinee::map_visibility.
    pub hidden_queue: Vec<(String, bool)>,
}

impl ScriptActors {
    /// The entity showing a script actor, if it has one.
    pub fn entity_of(&self, o: ObjId) -> Option<Entity> {
        self.props.get(&o).map(|p| p.entity)
    }

    /// The entity of a component attached to the goat (socket meshes).
    pub fn attached_entity(&self, comp: ObjId) -> Option<Entity> {
        self.attached.get(&comp).copied()
    }
}

pub fn asset_path_of(vm: &Vm, v: &Value) -> Option<String> {
    asset_path(vm, v)
}

/// A skeletal mesh socket: (bone, transform relative to the bone).
pub fn socket_on(vm_reg: &Registry, game: Option<&Registry>, mesh_path: &str, socket: &str) -> Option<(String, Transform)> {
    let (sreg, sock) = find_socket(vm_reg, game, mesh_path, socket)?;
    let props = sreg.merged_props(sock);
    let get = |k: &str| upk::registry::find(&props, k).map(|(_, v)| v.clone());
    let bone = match get("BoneName") {
        Some(upk::props::Value::Name(b)) => b,
        _ => return None,
    };
    let rel_loc = match get("RelativeLocation") {
        Some(upk::props::Value::Vector(v)) => v,
        _ => [0.0; 3],
    };
    let rel_rot = match get("RelativeRotation") {
        Some(upk::props::Value::Rotator(r)) => r,
        _ => [0; 3],
    };
    Some((bone, Transform { translation: convert::pos(rel_loc), rotation: convert::rot(rel_rot), scale: Vec3::ONE }))
}

/// Unreal units -> Bevy metres and back.
fn to_bevy(v: [f32; 3]) -> Vec3 {
    convert::pos(v)
}
fn to_ue(v: Vec3) -> [f32; 3] {
    [v.x / convert::S, v.z / convert::S, v.y / convert::S]
}

/// A skeletal mesh by path in the VM's packages, then the game's (skipping same-path objects of
/// other classes).
pub(crate) fn find_skel<'a>(vm_reg: &'a Registry, game: Option<&'a Registry>, path: &str) -> Option<(&'a Registry, ObjRef)> {
    if let Some(r) = vm_reg.find_path_class(path, "SkeletalMesh") {
        return Some((vm_reg, r));
    }
    let g = game?;
    g.find_path_class(path, "SkeletalMesh").map(|r| (g, r))
}

/// Find an object path in the VM's packages, then the game's.
fn find<'a>(vm_reg: &'a Registry, game: Option<&'a Registry>, path: &str) -> Option<(&'a Registry, ObjRef)> {
    if let Some(r) = vm_reg.find_path(path) {
        return Some((vm_reg, r));
    }
    let g = game?;
    g.find_path(path).map(|r| (g, r))
}

fn asset_path(vm: &Vm, v: &Value) -> Option<String> {
    match &vm.obj(v.obj()?).kind {
        Kind::Asset { path, .. } => Some(path.clone()),
        _ => None,
    }
}

/// The first StaticMeshComponent (with a mesh) among an actor's object properties.
fn mesh_component(vm: &mut Vm, o: ObjId) -> Option<(ObjId, String)> {
    let smc = vm.class_by_name("StaticMeshComponent")?;
    let vals: Vec<Value> = vm.obj(o).props.values().cloned().collect();
    for v in vals {
        let Some(c) = v.obj() else { continue };
        if c == o || !matches!(vm.obj(c).kind, Kind::Instance) || !vm.is_child_of(vm.obj(c).class, smc) {
            continue;
        }
        let mesh = vm.get(c, "StaticMesh");
        if let Some(p) = asset_path(vm, &mesh) {
            return Some((c, p));
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build(
    cache: &mut HashMap<String, Option<Arc<Built>>>,
    vm: &Vm,
    game: Option<&Registry>,
    mesh_path: &str,
    overrides: &[Option<String>],
    sa: &mut crate::rig::SkelAssets,
) -> Option<Arc<Built>> {
    let key = format!("{mesh_path}|{}", overrides.iter().map(|o| o.clone().unwrap_or_default()).collect::<Vec<_>>().join(","));
    if let Some(b) = cache.get(&key) {
        return b.clone();
    }
    let built = (|| {
        let (reg, r) = find(&vm.reg, game, mesh_path)?;
        let p = reg.pkg(r);
        let m = upk::mesh::StaticMesh::parse(p, r.export).ok()?;
        let verts: Vec<Vec3> = m.positions.iter().map(|&q| convert::pos(q)).collect();
        let normals: Vec<Vec3> = m.normals.iter().map(|&n| convert::dir(n).normalize_or_zero()).collect();
        let mut parts = vec![];
        for (k, s) in m.sections.iter().enumerate() {
            let lo = s.first_index as usize;
            let hi = (lo + s.num_faces as usize * 3).min(m.indices.len());
            if hi <= lo {
                continue;
            }
            let mut remap: HashMap<u32, u32> = HashMap::new();
            let (mut pp, mut nn, mut uv, mut idx) = (vec![], vec![], vec![], vec![]);
            for &i in &m.indices[lo..hi] {
                let ni = *remap.entry(i).or_insert_with(|| {
                    pp.push(verts[i as usize].to_array());
                    nn.push(normals.get(i as usize).copied().unwrap_or(Vec3::Y).to_array());
                    uv.push(m.uvs.get(i as usize).copied().unwrap_or([0.0, 0.0]));
                    pp.len() as u32 - 1
                });
                idx.push(ni);
            }
            let mut mesh = Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList, bevy::asset::RenderAssetUsages::RENDER_WORLD);
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pp);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nn);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
            mesh.insert_indices(bevy::mesh::Indices::U32(idx));
            // component material override, else the section's own material
            let mat = match overrides.get(k).cloned().flatten().and_then(|op| find(&vm.reg, game, &op)) {
                Some((mreg, mr)) => crate::rig::material_for(mreg, mr, sa),
                None => match reg.resolve(r.pkg, s.material) {
                    Some(mr) => crate::rig::material_for(reg, mr, sa),
                    None => sa.materials.add(StandardMaterial { base_color: Color::srgb(0.7, 0.7, 0.7), ..default() }),
                },
            };
            parts.push((sa.meshes.add(mesh), mat));
        }
        Some(Arc::new(Built { parts, verts }))
    })();
    cache.insert(key, built.clone());
    built
}

fn material_overrides(vm: &Vm, comp: ObjId) -> Vec<Option<String>> {
    let Some(y) = vm.syms_get("Materials") else { return vec![] };
    match vm.obj(comp).props.get(&y) {
        Some(Value::Array(a)) => a.iter().map(|v| asset_path(vm, v)).collect(),
        _ => vec![],
    }
}

/// An actor's or component's scale (DrawScale x DrawScale3D, or Scale x Scale3D).
fn scale_of(vm: &mut Vm, o: ObjId, uniform: &str, three: &str) -> Vec3 {
    let u = match vm.get(o, uniform) {
        Value::Float(f) if f > 0.0 => f,
        _ => 1.0,
    };
    let v3 = vm.get(o, three);
    let s3 = match v3 {
        Value::Struct(_) => {
            let a = vm.vec3(&v3);
            if a == [0.0; 3] { [1.0; 3] } else { a }
        }
        _ => [1.0; 3],
    };
    convert::scale(s3) * u
}

fn rotation_of(vm: &mut Vm, o: ObjId, prop: &str) -> Quat {
    let r = vm.get(o, prop);
    match r {
        Value::Struct(_) => convert::rot(vm.rot3(&r)),
        _ => Quat::IDENTITY,
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn script_actors(
    time: Res<Time>,
    mut m: ResMut<Mutators>,
    game: Option<Res<GameRegistry>>,
    mut sa: ResMut<ScriptActors>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut phys: ResMut<crate::physics::Physics>,
    mut tfs: Query<&mut Transform>,
    mut vis: Query<&mut Visibility>,
    goats: Query<(Entity, &crate::rig::Rig), With<crate::goat::Goat>>,
    roots: Query<(Entity, &Name, &ChildOf)>,
    skinned: Query<(Entity, &ChildOf), With<bevy::mesh::skinning::SkinnedMesh>>,
    mut mats: Query<&mut MeshMaterial3d<StandardMaterial>>,
) {
    let _prof_scope = crate::prof::scope("actors::script_actors");
    let dt = time.delta_secs();
    let _prof = super::proxies::Prof(std::time::Instant::now(), "script_actors");
    if std::env::var("GOAT_UVM_OFF").is_ok_and(|v| v.contains("actors")) {
        return;
    }
    let Some(h) = m.host.as_mut() else {
        // mutators turned off: remove everything the scripts put in the world
        let sa = &mut *sa;
        for (_, p) in sa.props.drain() {
            commands.entity(p.entity).despawn();
            if let Some(b) = p.body {
                phys.world.remove_body_with_colliders(b, true);
            }
        }
        for (_, e) in sa.attached.drain() {
            commands.entity(e).despawn();
        }
        sa.by_body.clear();
        sa.scanned = 0;
        sa.pending.clear();
        return;
    };
    let game = game.map(|g| g.0.clone());
    let game = game.as_deref();
    let mut assets = crate::rig::SkelAssets { meshes: &mut meshes, materials: &mut materials, images: &mut images, bindposes: &mut bindposes };
    let sa = &mut *sa;
    let vm = &mut h.vm;
    let Some(actor_class) = vm.class_by_name("Actor") else { return };
    let kactor = vm.class_by_name("KActor");
    let world_objs: Vec<ObjId> = vm.globals.values().copied().collect();
    // --- new script actors ---------------------------------------------------------------------
    let n = vm.objects.len();
    for o in sa.scanned as ObjId..n as ObjId {
        let ob = vm.obj(o);
        // proxies stand for game objects that already have their own entities
        // map actors (from the map package) already exist in the level: only script spawns count
        if matches!(ob.kind, Kind::Instance) && ob.template_of.is_none() && !ob.name.starts_with("Default__") && !world_objs.contains(&o) && !vm.proxy_things.contains_key(&o) && !vm.ref_of.contains_key(&o) && vm.is_child_of(ob.class, actor_class) {
            sa.pending.push((o, 10.0));
        }
    }
    sa.scanned = n;
    let mut still = vec![];
    for (o, left) in std::mem::take(&mut sa.pending) {
        if vm.obj(o).destroyed || left <= 0.0 {
            continue;
        }
        let Some((comp, mesh_path)) = mesh_component(vm, o) else {
            still.push((o, left - dt));
            continue;
        };
        let overrides = material_overrides(vm, comp);
        let Some(built) = build(&mut sa.built, vm, game, &mesh_path, &overrides, &mut assets) else {
            vm.say(format!("{}: static mesh {mesh_path} not found", vm.obj(o).name));
            continue;
        };
        let loc = vm.get(o, "Location");
        let loc = vm.vec3(&loc);
        let rot = rotation_of(vm, o, "Rotation");
        let scale = scale_of(vm, o, "DrawScale", "DrawScale3D") * scale_of(vm, comp, "Scale", "Scale3D");
        let physics = vm.get(o, "Physics").int();
        let dynamic = matches!(physics, 2 | 6 | 10) || kactor.is_some_and(|k| vm.is_child_of(vm.obj(o).class, k));
        let xf = Transform { translation: to_bevy(loc), rotation: rot, scale };
        let entity = commands.spawn((xf, Visibility::default(), Name::new(format!("script:{}", vm.obj(o).name)))).id();
        let parts: Vec<Entity> = built.parts.iter().map(|(mesh, mat)| commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()))).id()).collect();
        commands.entity(entity).add_children(&parts);
        let mut body = None;
        if dynamic {
            let pts: Vec<rp::Vector> = built.verts.iter().map(|&v| convert::to_rapier(v * scale)).collect();
            if let Some(shape) = rp::SharedShape::convex_hull(&pts) {
                let vel = vm.get(o, "Velocity");
                let v = to_bevy(vm.vec3(&vel));
                let pose = rp::Pose::from_parts(convert::to_rapier(xf.translation), convert::quat_to_rapier(rot));
                let rb = rp::RigidBodyBuilder::dynamic().pose(pose).linvel(convert::to_rapier(v)).gravity_scale(if physics == 6 { 0.0 } else { 1.0 });
                let bh = phys.world.insert_body(rb);
                phys.world.insert_collider(rp::ColliderBuilder::new(shape).density(65.0).friction(0.7).restitution(0.3), Some(bh));
                phys.names.insert(bh, vm.obj(o).name.clone());
                commands.entity(entity).insert(crate::physics::PhysBody { handle: bh, scale });
                sa.by_body.insert(bh, o);
                body = Some(bh);
            }
        }
        let what = if body.is_some() { "dynamic" } else { "placed" };
        vm.say(format!("{} in the world: {mesh_path} ({what}) at ({:.0}, {:.0}, {:.0}) scale {:.2}", vm.obj(o).name, loc[0], loc[1], loc[2], scale.x));
        sa.props.insert(o, PropActor { entity, comp, parts, body, written: loc });
    }
    sa.pending.extend(still);
    // --- existing ones: physics -> script, script moves -> world, destroy/hide ---------------------
    let mut gone = vec![];
    for (&o, p) in sa.props.iter_mut() {
        if vm.obj(o).destroyed {
            gone.push(o);
            continue;
        }
        let loc_v = vm.get(o, "Location");
        let script_loc = vm.vec3(&loc_v);
        let moved_by_script = script_loc.iter().zip(p.written).any(|(a, b)| (a - b).abs() > 0.5);
        if let Some(rb) = p.body.and_then(|b| phys.world.bodies.get_mut(b)) {
            if moved_by_script {
                rb.set_translation(convert::to_rapier(to_bevy(script_loc)), true);
                rb.set_linvel(rp::Vector::ZERO, true);
            }
            let t = convert::from_rapier(rb.translation());
            let v = convert::from_rapier(rb.linvel());
            let loc = to_ue(t);
            let vel = to_ue(v);
            let lv = vm.vector(loc[0], loc[1], loc[2]);
            let vv = vm.vector(vel[0], vel[1], vel[2]);
            vm.set(o, "Location", lv);
            vm.set(o, "Velocity", vv);
            p.written = loc;
        } else if let Ok(mut t) = tfs.get_mut(p.entity) {
            t.translation = to_bevy(script_loc);
            t.rotation = rotation_of(vm, o, "Rotation");
            p.written = script_loc;
        }
        let hidden = vm.get(o, "bHidden").truthy();
        if let Ok(mut v) = vis.get_mut(p.entity) {
            let want = if hidden { Visibility::Hidden } else { Visibility::Inherited };
            if *v != want {
                *v = want;
            }
        }
    }
    for o in gone {
        if let Some(p) = sa.props.remove(&o) {
            commands.entity(p.entity).despawn();
            if let Some(b) = p.body {
                phys.world.remove_body_with_colliders(b, true);
                sa.by_body.remove(&b);
            }
        }
    }
    // --- goat sockets ------------------------------------------------------------------------------
    let goat = h.goat;
    let goat_mesh = goat.and_then(|g| h.vm.get(g, "Mesh").obj());
    let vm = &mut h.vm;
    for (comp, mesh_comp, socket) in std::mem::take(&mut sa.attach_queue) {
        let Some(comp) = comp else { continue };
        if mesh_comp != goat_mesh {
            vm.say(format!("attach to socket {socket}: only the goat's mesh is supported"));
            continue;
        }
        let Ok((_, rig)) = goats.single() else { continue };
        // socket on the goat's current skeletal mesh
        let mesh_path = goat_mesh.map(|gm| vm.get(gm, "SkeletalMesh")).and_then(|v| asset_path(vm, &v)).unwrap_or_else(|| "goat.Mesh.goat".into());
        let Some((sreg, sock)) = find_socket(&vm.reg, game, &mesh_path, &socket).or_else(|| find_socket(&vm.reg, game, "goat.Mesh.goat", &socket)) else {
            vm.say(format!("socket {socket} not found on {mesh_path}"));
            continue;
        };
        let props = sreg.merged_props(sock);
        let get = |k: &str| upk::registry::find(&props, k).map(|(_, v)| v.clone());
        let bone = match get("BoneName") {
            Some(upk::props::Value::Name(b)) => b,
            _ => continue,
        };
        let Some(joint) = rig.data.bone_names.iter().position(|b| b.eq_ignore_ascii_case(&bone)).and_then(|i| rig.joints.get(i).copied()) else { continue };
        let rel_loc = match get("RelativeLocation") {
            Some(upk::props::Value::Vector(v)) => v,
            _ => [0.0; 3],
        };
        let rel_rot = match get("RelativeRotation") {
            Some(upk::props::Value::Rotator(r)) => r,
            _ => [0; 3],
        };
        let rel_scale = match get("RelativeScale") {
            Some(upk::props::Value::Vector(v)) => v,
            _ => [1.0; 3],
        };
        let socket_xf = Transform { translation: convert::pos(rel_loc), rotation: convert::rot(rel_rot), scale: convert::scale(rel_scale) };
        let tr = vm.get(comp, "Translation");
        let comp_t = match tr {
            Value::Struct(_) => convert::pos(vm.vec3(&tr)),
            _ => Vec3::ZERO,
        };
        let comp_xf = Transform { translation: comp_t, rotation: rotation_of(vm, comp, "Rotation"), scale: scale_of(vm, comp, "Scale", "Scale3D") };
        let mesh = vm.get(comp, "StaticMesh");
        let Some(mpath) = asset_path(vm, &mesh) else {
            // a SkeletalMeshComponent (hair, jet pack, wings): its mesh in reference pose
            let sk = vm.get(comp, "SkeletalMesh");
            let Some(spath) = asset_path(vm, &sk) else {
                let cn = vm.class_name(vm.obj(comp).class).to_string();
                vm.say(format!("attach {cn} to {socket}: nothing to show"));
                continue;
            };
            let Some((kreg, kref)) = find_skel(&vm.reg, game, &spath) else {
                vm.say(format!("attach: skeletal mesh {spath} not found"));
                continue;
            };
            let data = match crate::rig::load(kreg, &mut sa.skel_cache, kref, None, None, &mut assets) {
                Ok(d) => d,
                Err(e) => {
                    vm.say(format!("attach: skeletal mesh {spath} ({} export {}): {e:#}", kreg.pkg(kref).name, kref.export));
                    continue;
                }
            };
            let overrides: Vec<Option<Handle<StandardMaterial>>> = material_overrides(vm, comp)
                .into_iter()
                .map(|o| o.and_then(|op| find(&vm.reg, game, &op)).map(|(mreg, mr)| crate::rig::material_for(mreg, mr, &mut assets)))
                .collect();
            if let Some(old) = sa.attached.remove(&comp) {
                commands.entity(old).despawn();
            }
            // a mesh on the goat's own skeleton (hair, wings...) is posed like the goat
            // (SetParentAnimComponent); anything else hangs off the socket in its reference pose
            let shared = data.bone_names.iter().filter(|b| rig.data.bone_names.iter().any(|g| g.eq_ignore_ascii_case(b))).count();
            let follows = !data.bone_names.is_empty() && shared * 10 >= data.bone_names.len() * 8;
            let holder = if follows {
                let Ok((goat_e, _)) = goats.single() else { continue };
                let h = commands.spawn((Transform::IDENTITY, Visibility::default(), Name::new(format!("follow:{spath}")))).id();
                commands.entity(goat_e).add_child(h);
                crate::rig::spawn(&mut commands, h, data, crate::rig::goat_mesh_xf(), &overrides, "Idle");
                sa.followers.push(h);
                h
            } else {
                let h = commands.spawn((socket_xf * comp_xf, Visibility::default(), Name::new(format!("socket:{socket}")))).id();
                commands.entity(joint).add_child(h);
                crate::rig::spawn(&mut commands, h, data, Transform::IDENTITY, &overrides, "Idle");
                h
            };
            sa.attached.insert(comp, holder);
            let how = if follows { "posed with the goat" } else { "reference pose" };
            vm.say(format!("attached {spath} to the goat ({socket}, bone {bone}, {how})"));
            continue;
        };
        let overrides = material_overrides(vm, comp);
        let Some(built) = build(&mut sa.built, vm, game, &mpath, &overrides, &mut assets) else {
            vm.say(format!("attach: static mesh {mpath} not found"));
            continue;
        };
        if let Some(old) = sa.attached.remove(&comp) {
            commands.entity(old).despawn();
        }
        let e = commands
            .spawn((socket_xf * comp_xf, Visibility::default(), Name::new(format!("socket:{socket}"))))
            .with_children(|c| {
                for (mesh, mat) in &built.parts {
                    c.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone())));
                }
            })
            .id();
        commands.entity(joint).add_child(e);
        sa.attached.insert(comp, e);
        vm.say(format!("attached {mpath} to goat socket {socket} (bone {bone})"));
    }
    for comp in std::mem::take(&mut sa.detach_queue) {
        if let Some(e) = sa.attached.remove(&comp) {
            commands.entity(e).despawn();
        }
    }
    for (&comp, &e) in &sa.attached {
        let hidden = vm.get(comp, "HiddenGame").truthy();
        if let Ok(mut v) = vis.get_mut(e) {
            let want = if hidden { Visibility::Hidden } else { Visibility::Inherited };
            if *v != want {
                *v = want;
            }
        }
    }
    // --- material instance tints (SetVectorParameterValue) ---------------------------------------------
    for mic in std::mem::take(&mut sa.tint_queue) {
        let color = mic_color(vm, mic);
        if color.is_none() && !mic_has_texture(vm, mic) {
            continue;
        }
        let uses = |vm: &Vm, comp: ObjId| match vm.syms_get("Materials").and_then(|y| vm.obj(comp).props.get(&y)) {
            Some(Value::Array(a)) => a.iter().any(|v| v.obj() == Some(mic)),
            _ => false,
        };
        let mut targets: Vec<Entity> = sa.props.values().filter(|p| uses(vm, p.comp)).flat_map(|p| p.parts.clone()).collect();
        if goat_mesh.is_some_and(|gm| uses(vm, gm)) {
            if let Ok((ge, _)) = goats.single() {
                for (e, name, parent) in &roots {
                    if parent.parent() == ge && name.as_str() == "MeshRoot" {
                        targets.extend(skinned.iter().filter(|(_, p)| p.parent() == e).map(|(k, _)| k));
                    }
                }
            }
        }
        let tex = mic_texture(vm, mic, game, &mut assets);
        for t in targets {
            if let Ok(mut mm) = mats.get_mut(t) {
                let mut m = assets.materials.get(&mm.0).cloned().unwrap_or_default();
                if let Some(c) = color {
                    m.base_color = Color::linear_rgb(c[0], c[1], c[2]);
                }
                if let Some(t) = &tex {
                    m.base_color_texture = Some(t.clone());
                }
                mm.0 = assets.materials.add(m);
            }
        }
        vm.say(format!("material instance: colour {color:?}, texture {}", tex.is_some()));
    }
    // --- goat DrawScale ------------------------------------------------------------------------------
    if let (Some(g), Ok((ge, _))) = (goat, goats.single()) {
        let s = scale_of(vm, g, "DrawScale", "DrawScale3D");
        for (e, name, parent) in &roots {
            if parent.parent() == ge && name.as_str() == "MeshRoot" {
                if let Ok(mut t) = tfs.get_mut(e) {
                    if (t.scale - s).length() > 1e-4 {
                        t.scale = s;
                    }
                }
            }
        }
    }
}

/// A SkeletalMeshSocket export under a skeletal mesh, by SocketName.
fn find_socket<'a>(vm_reg: &'a Registry, game: Option<&'a Registry>, mesh_path: &str, socket: &str) -> Option<(&'a Registry, ObjRef)> {
    let (reg, mesh) = find(vm_reg, game, mesh_path)?;
    let p = reg.pkg(mesh);
    let mesh_idx = mesh.export as i32 + 1;
    (0..p.exports.len())
        .filter(|&i| p.exports[i].outer == mesh_idx && p.class_name(i as i32 + 1) == "SkeletalMeshSocket")
        .map(|i| ObjRef { pkg: mesh.pkg, export: i })
        .find(|&r| matches!(upk::props::find(&reg.props(r), "SocketName"), Some(upk::props::Value::Name(n)) if n.eq_ignore_ascii_case(socket)))
        .map(|r| (reg, r))
}

/// Raw pointer to the physics world for the tracer. The VM runs on its own thread while the
/// calling Bevy system waits for it (see bridge::on_big_stack), so the world is not touched
/// by anything else during the call; the tracer is removed again right after.
pub struct PhysPtr(pub *const crate::physics::Physics);
unsafe impl Send for PhysPtr {}
unsafe impl Sync for PhysPtr {}

/// Tracer for one VM call: rays in Unreal units, ignoring the goat's own capsule.
pub fn tracer(phys: &crate::physics::Physics, ignore: rp::RigidBodyHandle, by_body: HashMap<rp::RigidBodyHandle, ObjId>, goat: Option<ObjId>) -> Box<dyn Fn([f32; 3], [f32; 3]) -> Option<TraceHit> + Send + Sync> {
    let ptr = PhysPtr(phys);
    Box::new(move |start, end| {
        let ptr = &ptr;
        // SAFETY: see PhysPtr; the pointer is only used during the VM call it was made for
        let phys = unsafe { &*ptr.0 };
        let (s, e) = (to_bevy(start), to_bevy(end));
        let d = e - s;
        let len = d.length();
        if len < 1e-4 {
            return None;
        }
        let ray = rp::Ray::new(convert::to_rapier(s), convert::to_rapier(d / len));
        let filter = rp::QueryFilter::default().exclude_rigid_body(ignore);
        let (ch, hit) = phys.world.cast_ray_and_get_normal(&ray, len, true, filter)?;
        let p = s + d / len * hit.time_of_impact;
        let n = convert::from_rapier(hit.normal);
        let body = phys.world.colliders.get(ch).and_then(|c| c.parent());
        let actor = body.and_then(|b| by_body.get(&b).copied()).or(if body == Some(ignore) { goat } else { None });
        let thing = if actor.is_some() { None } else { thing_of(phys, ch) };
        Some(TraceHit { location: to_ue(p), normal: [n.x, n.z, n.y], actor, thing })
    })
}

/// The colour a MaterialInstanceConstant was given (its first LinearColor vector parameter).
fn mic_color(vm: &Vm, mic: ObjId) -> Option<[f32; 3]> {
    let (r, g, b) = (vm.syms_get("R")?, vm.syms_get("G")?, vm.syms_get("B")?);
    let mut keys: Vec<(&Sym, &Value)> = vm.obj(mic).props.iter().filter(|(k, _)| vm.name_of(**k).starts_with("uvmParam_")).collect();
    keys.sort_by_key(|(k, _)| vm.name_of(**k).to_ascii_lowercase());
    keys.iter().find_map(|(_, v)| match v {
        Value::Struct(s) => Some([s.field(r)?.float(), s.field(g)?.float(), s.field(b)?.float()]),
        _ => None,
    })
}

/// The NPC or prop a collider belongs to.
fn thing_of(phys: &crate::physics::Physics, ch: rp::ColliderHandle) -> Option<WorldThing> {
    if let Some(&e) = phys.owners.get(&ch) {
        return Some(WorldThing::Npc { entity: e.to_bits(), name: String::new() });
    }
    let body = phys.world.colliders.get(ch)?.parent()?;
    let name = phys.names.get(&body)?.clone();
    let (i, g) = body.into_raw_parts();
    Some(WorldThing::Prop { body: (i as u64) | ((g as u64) << 32), name })
}

/// Prop bounding boxes for one VM call (GetComponentsBoundingBox): the body's collider AABBs.
pub fn bounds(phys: &crate::physics::Physics) -> super::BoundsFn {
    let ptr = PhysPtr(phys);
    Box::new(move |thing| {
        let ptr = &ptr;
        // SAFETY: see PhysPtr
        let phys = unsafe { &*ptr.0 };
        let WorldThing::Prop { body, .. } = thing else { return None };
        let rb = phys.world.bodies.get(super::proxies::body_handle(*body))?;
        let mut aabb: Option<(Vec3, Vec3)> = None;
        for &c in rb.colliders() {
            let a = phys.world.colliders.get(c)?.compute_aabb();
            let (lo, hi) = (convert::from_rapier(a.mins), convert::from_rapier(a.maxs));
            aabb = Some(match aabb {
                Some((l, h)) => (l.min(lo), h.max(hi)),
                None => (lo, hi),
            });
        }
        let (lo, hi) = aabb?;
        Some((to_ue(lo), to_ue(hi)))
    })
}

/// Overlap query for one VM call: NPCs and props within a radius (Unreal units).
pub fn overlap(phys: &crate::physics::Physics, ignore: rp::RigidBodyHandle) -> OverlapFn {
    let ptr = PhysPtr(phys);
    Box::new(move |center, radius| {
        let ptr = &ptr;
        // SAFETY: see PhysPtr
        let phys = unsafe { &*ptr.0 };
        let c = to_bevy(center);
        let shape = rp::SharedShape::ball((radius * convert::S).max(0.01));
        let pose = rp::Pose::from_parts(convert::to_rapier(c), rp::Rotation::IDENTITY);
        let qp = phys.world.query_pipeline_with_filter(rp::QueryFilter::default().exclude_rigid_body(ignore));
        let mut out = vec![];
        for (ch, col) in qp.intersect_shape(pose, &*shape) {
            if let Some(t) = thing_of(phys, ch) {
                let p = col.parent().and_then(|b| phys.world.bodies.get(b)).map(|b| convert::from_rapier(b.translation())).unwrap_or(c);
                if !out.iter().any(|(o, _): &(WorldThing, [f32; 3])| o.key() == t.key()) {
                    out.push((t, to_ue(p)));
                }
            }
        }
        out
    })
}

fn mic_has_texture(vm: &Vm, mic: ObjId) -> bool {
    vm.obj(mic).props.iter().any(|(k, v)| vm.name_of(*k).starts_with("uvmParam_") && asset_path(vm, v).is_some())
}

/// A texture a MaterialInstanceConstant was given (SetTextureParameterValue), decoded.
fn mic_texture(vm: &Vm, mic: ObjId, game: Option<&Registry>, a: &mut crate::rig::SkelAssets) -> Option<Handle<Image>> {
    let path = vm.obj(mic).props.iter().filter(|(k, _)| vm.name_of(**k).starts_with("uvmParam_")).find_map(|(_, v)| asset_path(vm, v))?;
    let (reg, r) = find(&vm.reg, game, &path)?;
    if !reg.class_name(r).contains("Texture2D") {
        return None;
    }
    let p = reg.pkg(r);
    let t = upk::texture::Texture::parse(p, r.export).ok()?;
    let img = t.decode(p, r.export, 1024).ok()?;
    Some(a.images.add(crate::world::make_image(img, t.srgb)))
}

/// Attached meshes that share the goat's skeleton copy its joint transforms (after animation).
pub fn follow_goat_pose(sa: Res<ScriptActors>, goats: Query<&crate::rig::Rig, With<crate::goat::Goat>>, rigs: Query<&crate::rig::Rig, Without<crate::goat::Goat>>, mut joints: Query<&mut Transform>) {
    let _prof_scope = crate::prof::scope("actors::follow_goat_pose");
    let Ok(goat) = goats.single() else { return };
    for &h in &sa.followers {
        let Ok(rig) = rigs.get(h) else { continue };
        for (i, name) in rig.data.bone_names.iter().enumerate() {
            let Some(gi) = goat.data.bone_names.iter().position(|g| g.eq_ignore_ascii_case(name)) else { continue };
            let (Some(&src), Some(&dst)) = (goat.joints.get(gi), rig.joints.get(i)) else { continue };
            let Ok(t) = joints.get(src).copied() else { continue };
            if let Ok(mut d) = joints.get_mut(dst) {
                *d = t;
            }
        }
    }
}
