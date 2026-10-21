//! Unreal (left-handed, Z up, unreal units) -> Bevy (right-handed, Y up, metres).
//! Swapping Y and Z both changes the up axis and flips handedness.

use bevy::prelude::*;
use upk::level::Xform;

/// Metres per Unreal unit.
pub const S: f32 = 0.02;

pub fn pos(v: [f32; 3]) -> Vec3 {
    Vec3::new(v[0] * S, v[2] * S, v[1] * S)
}

pub fn dir(v: [f32; 3]) -> Vec3 {
    Vec3::new(v[0], v[2], v[1])
}

pub fn scale(s: [f32; 3]) -> Vec3 {
    Vec3::new(s[0], s[2], s[1])
}

/// Unreal FRotator (Pitch, Yaw, Roll; 65536 = 360 deg) to a Bevy rotation.
pub fn rot(r: [i32; 3]) -> Quat {
    let k = std::f32::consts::TAU / 65536.0;
    let (sp, cp) = (r[0] as f32 * k).sin_cos();
    let (sy, cy) = (r[1] as f32 * k).sin_cos();
    let (sr, cr) = (r[2] as f32 * k).sin_cos();
    // FRotationMatrix rows = images of the Unreal X/Y/Z axes.
    let ax = [cp * cy, cp * sy, sp];
    let ay = [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp];
    let az = [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp];
    // Bevy basis: X <- Unreal X, Y <- Unreal Z, Z <- Unreal Y.
    let m = Mat3::from_cols(dir(ax), dir(az), dir(ay));
    Quat::from_mat3(&m).normalize()
}

/// An Unreal world matrix (row vectors, row 3 = translation) as a Bevy transform: the axes swap
/// Y and Z on both sides (the conversion is its own inverse) and the translation is scaled.
pub fn matrix(m: &[f32; 16]) -> Transform {
    let r = |i: usize| Vec3::new(m[i * 4], m[i * 4 + 2], m[i * 4 + 1]);
    // Bevy columns = converted Unreal rows, with Y and Z swapped
    let (x, y, z) = (r(0), r(2), r(1));
    let t = pos([m[12], m[13], m[14]]);
    Transform::from_matrix(Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), t.extend(1.0)))
}

/// vector(Rotator): the direction a (pitch, yaw, roll) rotator points, in Bevy space.
pub fn rotator_dir(r: [i32; 3]) -> Vec3 {
    let k = std::f32::consts::TAU / 65536.0;
    let (pitch, yaw) = (r[0] as f32 * k, r[1] as f32 * k);
    let ue = [pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin()];
    Vec3::new(ue[0], ue[2], ue[1]).normalize_or_zero()
}

pub fn xform(x: &Xform) -> Transform {
    Transform { translation: pos(x.location), rotation: rot(x.rotation), scale: scale(x.scale) }
}

/// Component transform relative to its actor, scaled into metres.
pub fn local_xform(x: &Xform) -> Transform {
    Transform { translation: pos(x.location), rotation: rot(x.rotation), scale: scale(x.scale) }
}

pub fn to_rapier(v: Vec3) -> rapier3d::math::Vector {
    rapier3d::math::Vector::new(v.x, v.y, v.z)
}

pub fn from_rapier(v: rapier3d::math::Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

pub fn quat_to_rapier(q: Quat) -> rapier3d::math::Rotation {
    rapier3d::math::Rotation::from_xyzw(q.x, q.y, q.z, q.w)
}

pub fn quat_from_rapier(q: rapier3d::math::Rotation) -> Quat {
    Quat::from_xyzw(q.x, q.y, q.z, q.w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_90_turns_x_toward_y() {
        // Unreal yaw +16384 maps +X to +Y (Unreal), i.e. +Z in Bevy.
        let q = rot([0, 16384, 0]);
        let v = q * Vec3::X;
        assert!((v - Vec3::Z).length() < 1e-4, "{v:?}");
    }

    #[test]
    fn pitch_up_raises_nose() {
        let q = rot([16384, 0, 0]);
        let v = q * Vec3::X;
        assert!((v - Vec3::Y).length() < 1e-4, "{v:?}");
    }

    #[test]
    fn rotation_is_proper() {
        let q = rot([1234, -21920, 17372]);
        let m = Mat3::from_quat(q);
        assert!((m.determinant() - 1.0).abs() < 1e-4);
        let r = rot([4612, -21920, 17372]);
        // compare against the direct matrix to make sure from_mat3 got a rotation
        let k = std::f32::consts::TAU / 65536.0;
        let (sp, cp) = (4612.0 * k as f32).sin_cos();
        let (sy, cy) = (-21920.0 * k as f32).sin_cos();
        let ax = dir([cp * cy, cp * sy, sp]);
        assert!((r * Vec3::X - ax).length() < 1e-4);
    }
}
