//! UE3 aggregate collision elements (KAggregateGeom) -> rapier shapes.

use crate::convert::{self, S};
use bevy::prelude::*;
use rapier3d::prelude as rp;
use upk::physasset::{Shape, ShapeKind};

/// Element transform (bone/mesh space, Unreal FMatrix rows) -> rapier pose in Bevy space.
pub fn element_pose(tm: &[[f32; 4]; 4], scale: Vec3) -> rp::Pose {
    let ax = [tm[0][0], tm[0][1], tm[0][2]];
    let ay = [tm[1][0], tm[1][1], tm[1][2]];
    let az = [tm[2][0], tm[2][1], tm[2][2]];
    let m = Mat3::from_cols(convert::dir(ax), convert::dir(az), convert::dir(ay));
    let q = Quat::from_mat3(&m).normalize();
    let t = convert::pos([tm[3][0], tm[3][1], tm[3][2]]) * scale;
    rp::Pose::from_parts(convert::to_rapier(t), convert::quat_to_rapier(q))
}

/// Build (local pose, shape) pairs. `scale` is the Bevy-space instance scale.
pub fn colliders(shapes: &[Shape], scale: Vec3) -> Vec<(rp::Pose, rp::SharedShape)> {
    let s_max = scale.abs().max_element().max(1e-3);
    let mut out = Vec::new();
    for sh in shapes {
        let pose = element_pose(&sh.tm, scale);
        let shape = match &sh.kind {
            ShapeKind::Sphere { radius } => Some(rp::SharedShape::ball((radius * S * s_max).max(0.01))),
            ShapeKind::Capsule { radius, length } => {
                Some(rp::SharedShape::capsule_y((length * 0.5 * S * s_max).max(0.001), (radius * S * s_max).max(0.005)))
            }
            ShapeKind::Box { x, y, z } => {
                let h = Vec3::new(x * 0.5, z * 0.5, y * 0.5) * S * scale.abs();
                Some(rp::SharedShape::cuboid(h.x.max(0.005), h.y.max(0.005), h.z.max(0.005)))
            }
            ShapeKind::Convex { verts } => {
                let pts: Vec<rp::Vector> = verts.iter().map(|&v| convert::to_rapier(convert::pos(v) * scale)).collect();
                rp::SharedShape::convex_hull(&pts)
            }
        };
        if let Some(shape) = shape {
            // convex points are already in mesh space; their element pose is identity
            let pose = if matches!(sh.kind, ShapeKind::Convex { .. }) { rp::Pose::IDENTITY } else { pose };
            out.push((pose, shape));
        }
    }
    out
}
