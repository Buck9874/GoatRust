//! Physics-asset ragdolls for any rigged character (the goat's goat_Physics, NPC physics assets),
//! replacing UE3's PhysX ragdolls.

use crate::convert;
use crate::goat::Goat;
use crate::physics::Physics;
use crate::rig::Rig;
use bevy::prelude::*;
use rapier3d::prelude as rp;
use std::collections::HashMap;

pub const GROUP_WORLD: rp::Group = rp::Group::GROUP_1;
pub const GROUP_RAGDOLL: rp::Group = rp::Group::GROUP_2;
/// The goat's capsule (also in GROUP_WORLD); things it shoots ignore it.
pub const GROUP_GOAT: rp::Group = rp::Group::GROUP_3;

/// The kinematic capsule a character walks with; disabled while ragdolled.
#[derive(Component, Clone, Copy)]
pub struct CharacterBody {
    pub body: rp::RigidBodyHandle,
    pub collider: rp::ColliderHandle,
    /// Half height of the collision cylinder (metres), used when standing back up.
    pub half_height: f32,
}

#[derive(Component)]
pub struct ActiveRagdoll {
    /// (bone index, body)
    pub bodies: Vec<(usize, rp::RigidBodyHandle)>,
    pub joints: Vec<rp::ImpulseJointHandle>,
    pub pelvis: rp::RigidBodyHandle,
}

/// Request a ragdoll; built after the animation pose for this frame is known.
#[derive(Component)]
pub struct StartRagdoll {
    pub velocity: Vec3,
}

#[derive(Component)]
pub struct StopRagdoll;

fn joint_frame(pos: [f32; 3], pri: [f32; 3], sec: [f32; 3]) -> rp::Pose {
    // constraint frames are already in PhysX metres
    let x = convert::dir(pri).normalize_or(Vec3::X);
    let y0 = convert::dir(sec).normalize_or(Vec3::Y);
    let z = x.cross(y0).normalize_or(Vec3::Z);
    let y = z.cross(x);
    let q = Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize();
    rp::Pose::from_parts(convert::to_rapier(convert::dir(pos)), convert::quat_to_rapier(q))
}

/// Create the ragdoll bodies from the current skeleton pose.
pub fn begin(
    mut commands: Commands,
    mut phys: ResMut<Physics>,
    q: Query<(Entity, &Rig, &StartRagdoll, &CharacterBody)>,
    joints: Query<&GlobalTransform>,
) {
    let _prof_scope = crate::prof::scope("ragdoll::begin");
    for (e, rig, start, cb) in &q {
        commands.entity(e).remove::<StartRagdoll>();
        let Some(def) = rig.data.ragdoll.as_ref() else { continue };
        let mut bodies = Vec::new();
        let mut handle_of_bone: HashMap<usize, rp::RigidBodyHandle> = HashMap::new();
        let groups = rp::InteractionGroups::new(GROUP_RAGDOLL, GROUP_WORLD, rp::InteractionTestMode::And);
        for (bi, body) in def.asset.bodies.iter().enumerate() {
            let Some(bone) = def.bone_of_body[bi] else { continue };
            let Ok(gt) = joints.get(rig.joints[bone]) else { continue };
            let (_, rot, tr) = gt.to_scale_rotation_translation();
            let rb = rp::RigidBodyBuilder::dynamic()
                .pose(rp::Pose::from_parts(convert::to_rapier(tr), convert::quat_to_rapier(rot)))
                .linvel(convert::to_rapier(start.velocity))
                .linear_damping(0.1)
                .angular_damping(1.0)
                .ccd_enabled(bone == def.pelvis_bone);
            let h = phys.world.insert_body(rb);
            for (pose, shape) in crate::shapes::colliders(&body.shapes, Vec3::ONE) {
                let col = rp::ColliderBuilder::new(shape)
                    .position(pose)
                    .density(900.0)
                    .friction(0.9)
                    .restitution(0.1)
                    .collision_groups(if body.no_collision { rp::InteractionGroups::none() } else { groups });
                phys.world.insert_collider(col, Some(h));
            }
            handle_of_bone.insert(bone, h);
            bodies.push((bone, h));
        }
        let idx: HashMap<String, usize> =
            def.asset.bodies.iter().enumerate().filter_map(|(i, b)| def.bone_of_body[i].map(|bone| (b.bone.to_lowercase(), bone))).collect();
        let mut jh = Vec::new();
        for c in &def.asset.constraints {
            let (Some(&b1), Some(&b2)) = (idx.get(&c.bone1.to_lowercase()), idx.get(&c.bone2.to_lowercase())) else { continue };
            let (Some(&h1), Some(&h2)) = (handle_of_bone.get(&b1), handle_of_bone.get(&b2)) else { continue };
            let mut jb = rp::GenericJointBuilder::new(rp::JointAxesMask::LOCKED_SPHERICAL_AXES)
                .local_frame1(joint_frame(c.pos2, c.pri2, c.sec2))
                .local_frame2(joint_frame(c.pos1, c.pri1, c.sec1))
                .contacts_enabled(false);
            if c.twist_limited {
                let t = c.twist.max(1.0).to_radians();
                jb = jb.limits(rp::JointAxis::AngX, [-t, t]);
            }
            if c.swing_limited {
                let s1 = c.swing1.max(1.0).to_radians();
                let s2 = c.swing2.max(1.0).to_radians();
                jb = jb.limits(rp::JointAxis::AngZ, [-s1, s1]).limits(rp::JointAxis::AngY, [-s2, s2]);
            }
            // body1 = parent (bone2), body2 = child (bone1)
            jh.push(phys.world.insert_impulse_joint(h2, h1, jb));
        }
        let Some(&pelvis) = handle_of_bone.get(&def.pelvis_bone) else { continue };
        if let Some(c) = phys.world.colliders.get_mut(cb.collider) {
            c.set_enabled(false);
        }
        commands.entity(e).insert(ActiveRagdoll { bodies, joints: jh, pelvis });
    }
}

/// Copy body poses onto the skeleton (bones without bodies keep their animated local pose)
/// and keep the character entity on the pelvis.
pub fn drive(
    phys: Res<Physics>,
    mut chars: Query<(&ActiveRagdoll, &Rig, &mut Transform, Option<&mut Goat>)>,
    mut joints: Query<&mut Transform, Without<Rig>>,
    parents: Query<&ChildOf>,
) {
    let _prof_scope = crate::prof::scope("ragdoll::drive");
    for (rd, rig, mut gt, goat) in &mut chars {
        let Some(pelvis) = phys.world.bodies.get(rd.pelvis) else { continue };
        if let Some(mut g) = goat {
            g.velocity = convert::from_rapier(pelvis.linvel());
        }
        gt.translation = convert::from_rapier(pelvis.translation());
        let Some(&root_joint) = rig.joints.first() else { continue };
        let Ok(mesh_root) = parents.get(root_joint).map(|c| c.parent()) else { continue };
        let Ok(mr_local) = joints.get(mesh_root).map(|t| *t) else { continue };
        let char_world = Transform { translation: gt.translation, rotation: gt.rotation, scale: Vec3::ONE };
        let mr_world = char_world.to_matrix() * mr_local.to_matrix();
        let body_world: HashMap<usize, Mat4> = rd
            .bodies
            .iter()
            .filter_map(|&(bone, h)| {
                let rb = phys.world.bodies.get(h)?;
                Some((bone, Mat4::from_rotation_translation(convert::quat_from_rapier(*rb.rotation()), convert::from_rapier(rb.translation()))))
            })
            .collect();
        let mut world: Vec<Mat4> = Vec::with_capacity(rig.joints.len());
        for (k, &je) in rig.joints.iter().enumerate() {
            let parent_w = if k == 0 { mr_world } else { world[rig.data.parents[k] as usize] };
            let Ok(mut t) = joints.get_mut(je) else {
                world.push(parent_w);
                continue;
            };
            if let Some(w) = body_world.get(&k) {
                let local = parent_w.inverse() * *w;
                let (_, r, tr) = local.to_scale_rotation_translation();
                t.rotation = r;
                t.translation = tr;
                world.push(*w);
            } else {
                world.push(parent_w * t.to_matrix());
            }
        }
    }
}

/// Remove ragdoll bodies and put the character back on its feet at the pelvis.
pub fn end(
    mut commands: Commands,
    mut phys: ResMut<Physics>,
    mut q: Query<(Entity, &ActiveRagdoll, &CharacterBody, &mut Transform, Option<&mut Goat>), With<StopRagdoll>>,
) {
    let _prof_scope = crate::prof::scope("ragdoll::end");
    for (e, rd, cb, mut t, goat) in &mut q {
        let pelvis = phys.world.bodies.get(rd.pelvis);
        let pv = pelvis.map(|b| convert::from_rapier(b.translation())).unwrap_or(t.translation);
        let fwd = pelvis.map(|b| convert::quat_from_rapier(*b.rotation()) * Vec3::X).unwrap_or(Vec3::NEG_Z);
        for &j in &rd.joints {
            phys.world.remove_impulse_joint(j);
        }
        for &(_, h) in &rd.bodies {
            phys.world.remove_body_with_colliders(h, true);
        }
        let stand = pv + Vec3::Y * (cb.half_height + 0.1);
        if let Some(c) = phys.world.colliders.get_mut(cb.collider) {
            c.set_enabled(true);
        }
        if let Some(rb) = phys.world.bodies.get_mut(cb.body) {
            rb.set_translation(convert::to_rapier(stand), true);
        }
        let flat = Vec3::new(fwd.x, 0.0, fwd.z);
        let yaw = if flat.length_squared() > 1e-3 { f32::atan2(-flat.x, -flat.z) } else { 0.0 };
        if let Some(mut g) = goat {
            g.yaw = yaw;
            g.velocity = Vec3::ZERO;
        }
        t.translation = stand;
        t.rotation = Quat::from_rotation_y(yaw);
        commands.entity(e).remove::<(ActiveRagdoll, StopRagdoll)>();
    }
}
