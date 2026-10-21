//! The pitching machine (GGPitchingMachineAbstract) and its baseballs (GGKActorBaseBallAbstract):
//! Kismet's GGSeqAct_StartPitchingMachine starts the VM's Pitch timer (mPitchingInterval 2 s);
//! each GGPitchingMachineAbstract.Pitch (overridden in the VM -> HostEvent::Pitch) fires a ball
//! from PitchingSocket with ApplyImpulse(mPitchVelocity 7500). Touching the machine straps it to
//! the goat's jetPackSocket (SetRelativeRotation(0, 16384, -1000)); then Special (R) pitches, the
//! ball joins the goat's collision chain (SetCollisionChainGoatNr), and is lit up when the goat
//! holds something grilled (PhysMat_Grill -> LightUp). GGTrickBaseball (chain.rs): a goat ball
//! hitting an NPC scores "[Flaming] Beanball at <name>" = mBaseScore 100 + the NPC's score.
//! Balls live LifeSpan 120 s.

use crate::convert::{self, S};
use crate::goat::{Goat, InputState};
use crate::physics::{PhysBody, Physics};
use crate::rig::{self, SkelAssets, SkelCache};
use bevy::prelude::*;
use rapier3d::prelude as rp;
use upk::props::Value;
use upk::registry::{self, ObjRef, Registry};

/// Default__GGPitchingMachineAbstract.mPitchVelocity (an impulse) over the ball's PhysX mass.
const PITCH_IMPULSE: f32 = 7500.0;
const BALL_PHYSX_MASS: f32 = 3.0;
const BALL_MASS: f32 = 0.5;
/// Default__GGKActorBaseBallAbstract.LifeSpan.
const LIFE_SPAN: f32 = 120.0;

pub struct BallDef {
    parts: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    radius: f32,
    scale: Vec3,
    score: (i32, String),
    fire_fx: Option<String>,
}

/// Pitch calls from the VM (GGPitchingMachineAbstract.Pitch override), drained by `machines`.
static VM_PITCHES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
/// The goat wears a pitching machine (R pitches instead of respawning).
pub static WEARING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn vm_pitch(machine: String) {
    if let Ok(mut q) = VM_PITCHES.lock() {
        if q.len() < 16 {
            q.push(machine);
        }
    }
}

#[derive(Resource, Default)]
pub struct Pitching {
    pub ball: Option<BallDef>,
    /// mPitchingSound (cue path).
    sound: Option<String>,
    /// The goat mesh's jetPackSocket: bone name and transform relative to it.
    goat_socket: Option<(String, Transform)>,
    /// GGPitchingMachineAbstract.Pitch calls from the VM (machine object names).
    pub queue: Vec<String>,
    /// Balls pitched so far (tests): (by the goat, total).
    pub by_goat: u32,
    pub total: u32,
}

#[derive(Component)]
pub struct PitchingMachine {
    pub name: String,
    rig_host: Entity,
    socket: Option<(usize, Transform)>,
    radius: f32,
    pub attached: bool,
    scale: f32,
}

#[derive(Component)]
pub struct Baseball {
    pub body: rp::RigidBodyHandle,
    life: f32,
}

/// Load the map's pitching machines and the baseball (call from setup).
pub fn load(reg: &Registry, cache: &mut SkelCache, a: &mut SkelAssets, commands: &mut Commands, goat_mesh: Option<ObjRef>) -> Pitching {
    let mut out = Pitching::default();
    let map = &reg.packages[0];
    out.goat_socket = goat_mesh.and_then(|m| crate::vehicle::socket(reg, m, "jetPackSocket"));
    for i in 0..map.exports.len() {
        let class = map.class_name(i as i32 + 1);
        let path = map.obj_path(i as i32 + 1);
        if class.contains("BaseBall") && path.contains("Default__") && out.ball.is_none() {
            out.ball = ball_def(reg, ObjRef { pkg: 0, export: i }, a);
        }
        if !class.contains("PitchingMachine") || !path.contains(".PersistentLevel.") {
            continue;
        }
        let aps = reg.merged_props(ObjRef { pkg: 0, export: i });
        if out.sound.is_none() {
            out.sound = match registry::find(&aps, "mPitchingSound") {
                Some((pk, Value::Object(o))) => reg.resolve(pk, *o).map(|r| reg.path(r)),
                _ => None,
            };
        }
        let Some(comp) = crate::npc::obj(reg, &aps, "SkeletalMeshComponent") else { continue };
        let cps = reg.merged_props(comp);
        let Some(mesh) = crate::npc::obj(reg, &cps, "SkeletalMesh") else { continue };
        let data = match rig::load(reg, cache, mesh, None, None, a) {
            Ok(d) => d,
            Err(e) => {
                warn!("pitching machine: {e:#}");
                continue;
            }
        };
        let scale = crate::npc::float(&aps, "DrawScale", 1.0);
        let loc = convert::pos(crate::npc::vec3(&aps, "Location", [0.0; 3]));
        let rot = match registry::find(&aps, "Rotation") {
            Some((_, Value::Rotator(r))) => *r,
            _ => [0; 3],
        };
        let radius = crate::npc::obj(reg, &aps, "CylinderComponent").map(|c| crate::npc::float(&reg.merged_props(c), "CollisionRadius", 40.0)).unwrap_or(40.0) * scale;
        let socket = crate::vehicle::socket(reg, mesh, "PitchingSocket").and_then(|(b, t)| data.bone_names.iter().position(|n| n.eq_ignore_ascii_case(&b)).map(|k| (k, t)));
        let name = map.obj_name(i as i32 + 1);
        let e = commands.spawn((Transform::from_translation(loc).with_rotation(convert::rot(rot)).with_scale(Vec3::splat(scale)), Visibility::default(), Name::new(name.clone()))).id();
        let host = commands.spawn((Transform::default(), Visibility::default())).id();
        commands.entity(e).add_child(host);
        let mesh_xf = Transform::from_translation(convert::pos(crate::npc::vec3(&cps, "Translation", [0.0; 3])));
        rig::spawn(commands, host, data, mesh_xf, &[], "Idle");
        commands.entity(e).insert(PitchingMachine { name, rig_host: host, socket, radius: radius * S, attached: false, scale });
    }
    out
}

fn ball_def(reg: &Registry, def: ObjRef, a: &mut SkelAssets) -> Option<BallDef> {
    let ps = reg.merged_props(def);
    let comp = crate::npc::obj(reg, &ps, "StaticMeshComponent")?;
    let cps = reg.merged_props(comp);
    let mesh = crate::npc::obj(reg, &cps, "StaticMesh")?;
    let scale = Vec3::splat(match registry::find(&cps, "Scale3D") {
        Some((_, Value::Vector(v))) => v[0],
        _ => 1.0,
    });
    let (parts, verts, body_setup) = crate::world::static_mesh_parts(reg, mesh, a.meshes)?;
    let radius = verts.iter().map(|v| v.length()).fold(0.0, f32::max) * scale.x;
    let parts = parts.into_iter().map(|(m, mat)| (m, mat.map(|r| rig::material_for(reg, r, a)).unwrap_or_default())).collect();
    // GetScore: the body setup's physical material, else the material's
    let pm = upk::physasset::body_setup(reg.pkg(mesh), body_setup).and_then(|(_, pm, _)| (pm != 0).then(|| reg.resolve(mesh.pkg, pm)).flatten());
    let score = pm.and_then(|p| upk::physasset::score_info(reg, p)).unwrap_or((10, "Baseball".into()));
    let fire_fx = match registry::find(&ps, "mFirePSTemplate") {
        Some((pk, Value::Object(o))) => reg.resolve(pk, *o).map(|r| reg.path(r)),
        _ => None,
    };
    Some(BallDef { parts, radius: radius.max(0.05), scale, score, fire_fx })
}

/// Touch, wearing the machine, Special to pitch, the VM's pitches, ball lifetimes.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn machines(
    time: Res<Time>,
    input: Res<InputState>,
    mut pitching: ResMut<Pitching>,
    mut phys: ResMut<Physics>,
    mut chains: ResMut<crate::chain::Chains>,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    mut commands: Commands,
    mut machines: Query<(&mut PitchingMachine, &mut Transform), Without<Goat>>,
    goats: Query<(&Goat, &Transform, Option<&rig::Rig>)>,
    rigs: Query<&rig::Rig, Without<Goat>>,
    joints: Query<&GlobalTransform>,
    mut balls: Query<(Entity, &mut Baseball)>,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    mut lib: ResMut<crate::particles::FxLibrary>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let dt = time.delta_secs();
    for (e, mut b) in &mut balls {
        if std::env::var_os("GOAT_TRACE").is_some() && b.life > LIFE_SPAN - 0.3 {
            if let Some(rb) = phys.world.bodies.get(b.body) {
                println!("    ball {:?}: pos {:.2?} vel {:.1?} sleeping {}", b.body.into_raw_parts(), convert::from_rapier(rb.translation()), convert::from_rapier(rb.linvel()), rb.is_sleeping());
            }
        }
        b.life -= dt;
        if b.life <= 0.0 {
            phys.world.remove_body(b.body);
            commands.entity(e).despawn();
        }
    }
    let Ok((g, gt, grig)) = goats.single() else { return };
    let mut queued = std::mem::take(&mut pitching.queue);
    if let Ok(mut q) = VM_PITCHES.lock() {
        queued.append(&mut q);
    }
    let mut fire: Vec<(Transform, bool)> = Vec::new();
    for (mut m, mut t) in &mut machines {
        // Touch: the goat walks into the machine's cylinder
        if !m.attached && gt.translation.with_y(0.0).distance(t.translation.with_y(0.0)) < m.radius + 0.5 && (gt.translation.y - t.translation.y).abs() < 1.5 {
            m.attached = true;
            info!("pitching machine {} on the goat", m.name);
        }
        if m.attached {
            WEARING.store(true, std::sync::atomic::Ordering::Relaxed);
            // SetBase(goat, jetPackSocket) + SetRelativeRotation(rot(0, 16384, -1000))
            let socket = pitching.goat_socket.as_ref().and_then(|(bone, local)| {
                let r = grig?;
                let k = r.data.bone_names.iter().position(|n| n.eq_ignore_ascii_case(bone))?;
                let j = joints.get(*r.joints.get(k)?).ok()?;
                Some(j.compute_transform() * *local)
            });
            let at = socket.unwrap_or(*gt * Transform::from_xyz(0.0, 0.3, 0.0));
            if std::env::var_os("GOAT_TRACE").is_some() && time.elapsed_secs() % 1.0 < dt {
                let goat_fwd = gt.rotation * -Vec3::Z;
                println!("    machine on goat: goat fwd {goat_fwd:.2?}, socket X {:.2?} Y {:.2?} Z {:.2?}, machine X {:.2?}", at.rotation * Vec3::X, at.rotation * Vec3::Y, at.rotation * Vec3::Z, at.rotation * convert::rot([0, 16384, -1000]) * Vec3::X);
            }
            t.translation = at.translation;
            t.rotation = at.rotation * convert::rot([0, 16384, -1000]);
            t.scale = Vec3::splat(m.scale);
        }
        let by_goat = m.attached && input.0.special;
        let by_vm = !m.attached && queued.iter().any(|n| n.eq_ignore_ascii_case(&m.name));
        if by_goat || by_vm {
            // PitchingSocket's world transform
            let from = m.socket.and_then(|(k, local)| {
                let r = rigs.get(m.rig_host).ok()?;
                let j = joints.get(*r.joints.get(k)?).ok()?;
                Some(j.compute_transform() * local)
            });
            let from = from.unwrap_or(*t * Transform::from_xyz(0.0, 1.0, 0.0));
            fire.push((from, by_goat));
        }
    }
    let pitching = &mut *pitching;
    let Some(ball) = pitching.ball.as_ref() else { return };
    let (mut n_goat, mut n_total) = (0, 0);
    let grilled = g.grabbed.and_then(|(b, _)| phys.physmat.get(&b).copied()).zip(game.as_ref()).is_some_and(|(pm, gr)| gr.0.path(pm).contains("PhysMat_Grill"));
    for (from, by_goat) in fire {
        let dir = (from.rotation * Vec3::X).normalize_or_zero();
        let pos = from.translation + dir * (ball.radius + 0.15);
        let body = phys.world.insert_body(
            rp::RigidBodyBuilder::dynamic()
                .translation(convert::to_rapier(pos))
                .linvel(convert::to_rapier(dir * PITCH_IMPULSE / BALL_PHYSX_MASS * S))
                .ccd_enabled(true),
        );
        // the goat's own balls don't hit the goat (they start on its back)
        let groups = if by_goat {
            rp::InteractionGroups::new(rp::Group::ALL, rp::Group::ALL ^ (crate::ragdoll::GROUP_WORLD | crate::ragdoll::GROUP_GOAT), rp::InteractionTestMode::And)
        } else {
            rp::InteractionGroups::all()
        };
        phys.world.insert_collider(rp::ColliderBuilder::ball(ball.radius).mass(BALL_MASS).restitution(0.5).friction(0.6).collision_groups(groups), Some(body));
        phys.prop_info.insert(body, ball.score.clone());
        phys.names.insert(body, "Baseball".into());
        let e = commands
            .spawn((Transform::from_translation(pos).with_scale(ball.scale), Visibility::default(), PhysBody { handle: body, scale: ball.scale }, Name::new("Baseball")))
            .with_children(|c| {
                for (m, mat) in &ball.parts {
                    c.spawn((Mesh3d(m.clone()), MeshMaterial3d(mat.clone())));
                }
            })
            .id();
        commands.entity(e).insert(Baseball { body, life: LIFE_SPAN });
        let lit = by_goat && grilled;
        chains.add_baseball(body, lit);
        if by_goat {
            // baseBall.SetCollisionChainGoatNr(mGoat)
            chains.touched_by_goat(body);
            n_goat += 1;
        }
        if lit {
            if let Some(fx) = ball.fire_fx.as_deref().zip(game.as_ref()).and_then(|(p, gr)| lib.get(&[&gr.0], p, &mut materials, &mut images)) {
                let f = commands.spawn((Transform::default(), Visibility::default(), crate::particles::ParticleFx::new(fx, true))).id();
                commands.entity(e).add_child(f);
            }
        }
        n_total += 1;
        if let Some(s) = pitching.sound.clone() {
            sounds.play(&s, Some(pos));
        }
    }
    pitching.by_goat += n_goat;
    pitching.total += n_total;
}

pub struct PitchPlugin;

impl Plugin for PitchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Pitching>().add_systems(Update, machines.after(crate::goat::goat_move));
    }
}
