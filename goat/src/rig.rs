//! Skeletal meshes (goat, NPCs) with their animation sets and physics assets, GPU-skinned through
//! Bevy's SkinnedMesh with our own sampler/blender.

use crate::convert;
use crate::goat::Goat;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::DynamicSkinnedMeshBounds;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use upk::physasset::PhysicsAsset;
use upk::registry::{ObjRef, Registry};

/// Unreal quaternion -> Bevy (axes Y/Z swapped, which also mirrors the rotation sense).
fn quat(q: [f32; 4]) -> Quat {
    Quat::from_xyzw(-q[0], -q[2], -q[1], q[3]).normalize()
}

pub struct Clip {
    pub frames: usize,
    pub length: f32,
    pub rate: f32,
    /// [track][key]
    pub pos: Vec<Vec<Vec3>>,
    pub rot: Vec<Vec<Quat>>,
    /// joint index per track
    pub joint: Vec<Option<usize>>,
    /// AnimNotify_Footstep times.
    pub footsteps: Vec<f32>,
    /// AnimNotify_Sound: (time, cue path).
    pub sounds: Vec<(f32, String)>,
}

impl Clip {
    pub(crate) fn sample_into(&self, t: f32, looping: bool, out: &mut [Transform]) {
        let n = self.frames.max(1);
        let step = if n > 1 { self.length / (n - 1) as f32 } else { 1.0 };
        let mut f = t / step.max(1e-4);
        if looping {
            f = f.rem_euclid((n - 1).max(1) as f32);
        } else {
            f = f.clamp(0.0, (n - 1) as f32);
        }
        let i0 = f.floor() as usize;
        let i1 = (i0 + 1).min(n - 1);
        let a = f - i0 as f32;
        for (k, j) in self.joint.iter().enumerate() {
            let Some(j) = *j else { continue };
            let p = &self.pos[k];
            let r = &self.rot[k];
            let pos = if p.len() > 1 { p[i0.min(p.len() - 1)].lerp(p[i1.min(p.len() - 1)], a) } else { p.first().copied().unwrap_or(out[j].translation) };
            let rot = if r.len() > 1 { r[i0.min(r.len() - 1)].slerp(r[i1.min(r.len() - 1)], a) } else { r.first().copied().unwrap_or(out[j].rotation) };
            out[j].translation = pos;
            out[j].rotation = rot;
        }
    }
}

/// Ragdoll description bound to a skeleton.
pub struct RagdollDef {
    pub asset: PhysicsAsset,
    pub bone_of_body: Vec<Option<usize>>,
    /// Bone index used as the "pelvis" (root-most body).
    pub pelvis_bone: usize,
}

/// Everything shared by all instances of one skeletal mesh + anim set.
pub struct SkelData {
    pub sections: Vec<(Handle<Mesh>, usize)>,
    /// The mesh's own materials (for physical-material lookups).
    pub material_refs: Vec<Option<ObjRef>>,
    pub materials: Vec<Handle<StandardMaterial>>,
    pub bindposes: Handle<SkinnedMeshInverseBindposes>,
    pub ref_pose: Vec<Transform>,
    pub parents: Vec<i32>,
    /// Bone names (index = joint index).
    pub bone_names: Vec<String>,
    pub rot_origin: [i32; 3],
    pub clips: Vec<Clip>,
    pub by_name: HashMap<String, usize>,
    pub ragdoll: Option<RagdollDef>,
}

impl SkelData {
    pub fn clip(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied().or_else(|| self.by_name.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, &v)| v))
    }

    /// Like `clip`, but "Idle" also matches "Idle_01" style variants.
    pub fn clip_or_variant(&self, name: &str) -> Option<usize> {
        self.clip(name).or_else(|| {
            let lower = name.to_lowercase();
            let mut v: Vec<(&String, &usize)> = self.by_name.iter().filter(|(k, _)| k.to_lowercase().starts_with(&lower)).collect();
            v.sort();
            v.first().map(|(_, i)| **i)
        })
    }
}

/// Cache of loaded skeletal assets keyed by (mesh, first anim set, physics asset).
#[derive(Resource, Default)]
pub struct SkelCache(pub HashMap<(ObjRef, Option<ObjRef>, Option<ObjRef>), Arc<SkelData>>);

#[derive(Component)]
pub struct Rig {
    pub joints: Vec<Entity>,
    pub data: Arc<SkelData>,
}

/// Current/previous clip with a crossfade, plus a one-shot overlay (headbutt, bite...).
#[derive(Component)]
pub struct Animator {
    pub clip: usize,
    pub time: f32,
    pub speed: f32,
    pub prev: Option<(usize, f32)>,
    pub fade: f32,
    pub oneshot: Option<(usize, f32)>,
    /// Footstep notifies passed since the owner last took them (PlayFootStepSound).
    pub steps: u32,
    /// AnimNotify_Sound cues passed since the owner last took them.
    pub sounds: Vec<String>,
}

impl Animator {
    pub fn new(clip: usize) -> Self {
        Self { clip, time: 0.0, speed: 1.0, prev: None, fade: 1.0, oneshot: None, steps: 0, sounds: Vec::new() }
    }
    pub fn play(&mut self, clip: usize, speed: f32) {
        self.speed = speed;
        if clip == self.clip {
            return;
        }
        self.prev = Some((self.clip, self.time));
        self.fade = 0.0;
        self.clip = clip;
        self.time = 0.0;
    }
}

pub struct SkelAssets<'a> {
    pub meshes: &'a mut Assets<Mesh>,
    pub materials: &'a mut Assets<StandardMaterial>,
    pub images: &'a mut Assets<Image>,
    pub bindposes: &'a mut Assets<SkinnedMeshInverseBindposes>,
}

/// Material for an object reference (diffuse texture approximation).
pub fn material_for(reg: &Registry, r: ObjRef, a: &mut SkelAssets) -> Handle<StandardMaterial> {
    load_material(reg, Some(r), a)
}

fn load_material(reg: &Registry, r: Option<ObjRef>, a: &mut SkelAssets) -> Handle<StandardMaterial> {
    let info = r.map(|r| upk::material::resolve_in(reg, r)).unwrap_or_default();
    // a constant DiffuseColor (Constant3Vector / VectorParameter) when there is no texture
    let color = info.color;
    // DiffuseColor graphs beyond one plain texture (multiplied detail textures, channel masks,
    // tints) are baked into one texture
    let graph = r.and_then(|r| upk::material::diffuse_graph(reg, r)).filter(crate::matbake::needs_bake);
    let baked = graph.as_ref().and_then(|n| {
        let mut refs = Vec::new();
        n.textures(&mut refs);
        let texs: std::collections::HashMap<ObjRef, (upk::texture::Image, bool)> = refs.into_iter().map(|t| Some((t, decode_texture(reg, t)?))).collect::<Option<_>>()?;
        crate::matbake::bake(n, &texs).map(|i| (i, true))
    });
    let tex = baked.or_else(|| info.diffuse.and_then(|tr| decode_texture(reg, tr))).map(|(mut img, srgb)| {
        // BLEND_Masked: alpha from the OpacityMask texture's channel (OneMinus inverts it)
        if let Some((mt, ch, inv)) = info.mask.filter(|_| info.masked) {
            if let Some((m, _)) = decode_texture(reg, mt) {
                crate::matbake::apply_mask(&mut img, &m, ch, inv);
            }
        }
        a.images.add(crate::world::make_image(img, srgb))
    });
    let masked = info.masked && info.mask.is_some() && tex.is_some();
    a.materials.add(StandardMaterial {
        base_color: match (&tex, color) {
            (Some(_), _) => Color::WHITE,
            (None, Some(c)) => Color::linear_rgb(c[0], c[1], c[2]),
            (None, None) => Color::srgb(0.8, 0.78, 0.75),
        },
        base_color_texture: tex,
        perceptual_roughness: 0.9,
        reflectance: 0.2,
        alpha_mode: if masked { AlphaMode::Mask(0.33) } else { AlphaMode::Opaque },
        double_sided: masked || info.two_sided,
        cull_mode: if masked || info.two_sided { None } else { Some(bevy::render::render_resource::Face::Back) },
        ..default()
    })
}

fn decode_texture(reg: &Registry, tr: ObjRef) -> Option<(upk::texture::Image, bool)> {
    let tp = reg.pkg(tr);
    let tx = upk::texture::Texture::parse(tp, tr.export).ok()?;
    Some((tx.decode(tp, tr.export, 1024).ok()?, tx.srgb))
}

/// Load (or reuse) a skeletal mesh with an anim set and an optional physics asset.
pub fn load(
    reg: &Registry,
    cache: &mut SkelCache,
    mesh_ref: ObjRef,
    anim_ref: Option<ObjRef>,
    phys_ref: Option<ObjRef>,
    a: &mut SkelAssets,
) -> anyhow::Result<Arc<SkelData>> {
    let key = (mesh_ref, anim_ref, phys_ref);
    if let Some(d) = cache.0.get(&key) {
        return Ok(d.clone());
    }
    let mp = reg.pkg(mesh_ref);
    let m = upk::skel::SkeletalMesh::parse(mp, mesh_ref.export)?;
    let ref_pose: Vec<Transform> = m
        .bones
        .iter()
        .map(|b| Transform { translation: convert::pos(b.position), rotation: quat(b.rotation), scale: Vec3::ONE })
        .collect();
    let mut global: Vec<Mat4> = Vec::with_capacity(m.bones.len());
    for (k, b) in m.bones.iter().enumerate() {
        let local = ref_pose[k].to_matrix();
        global.push(if k == 0 { local } else { global[b.parent as usize] * local });
    }
    let inv: Vec<Mat4> = global.iter().map(|g| g.inverse()).collect();

    let positions: Vec<[f32; 3]> = m.positions.iter().map(|&p| convert::pos(p).to_array()).collect();
    let normals: Vec<[f32; 3]> = m.normals.iter().map(|&n| convert::dir(n).normalize_or_zero().to_array()).collect();
    let weights: Vec<[f32; 4]> = m
        .weights
        .iter()
        .map(|w| {
            let s: f32 = w.iter().map(|&x| x as f32).sum::<f32>().max(1.0);
            [w[0] as f32 / s, w[1] as f32 / s, w[2] as f32 / s, w[3] as f32 / s]
        })
        .collect();
    let mut sections = Vec::new();
    for s in &m.sections {
        let lo = s.first_index as usize;
        let hi = (lo + s.num_triangles as usize * 3).min(m.indices.len());
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, m.uvs.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(m.joints.clone()));
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights.clone());
        mesh.insert_indices(Indices::U32(m.indices[lo..hi].to_vec()));
        let mesh = mesh.with_generated_skinned_mesh_bounds().map_err(|e| anyhow::anyhow!("{e:?}"))?;
        sections.push((a.meshes.add(mesh), s.material));
    }
    let materials: Vec<Handle<StandardMaterial>> = m.materials.iter().map(|&o| load_material(reg, reg.resolve(mesh_ref.pkg, o), a)).collect();

    // animations
    let bone_index: HashMap<String, usize> = m.bones.iter().enumerate().map(|(i, b)| (b.name.to_lowercase(), i)).collect();
    let mut clips = Vec::new();
    let mut by_name = HashMap::new();
    // an inherited anim set may belong to another skeleton (e.g. the head-bobber creature gets the
    // human Characters set from Default__GGNpc); fall back to a set from the mesh's own package
    let fits = |ar: ObjRef| -> usize {
        upk::anim::AnimSet::parse(reg.pkg(ar), ar.export)
            .map(|set| set.track_bones.iter().filter(|n| bone_index.contains_key(&n.to_lowercase())).count())
            .unwrap_or(0)
    };
    let mut anim_ref = anim_ref;
    if anim_ref.is_none_or(|ar| fits(ar) * 2 < m.bones.len()) {
        let mesh_path = reg.path(mesh_ref);
        let prefix = format!("{}.", mesh_path.split('.').next().unwrap_or_default()).to_lowercase();
        let own = reg
            .packages
            .iter()
            .enumerate()
            .flat_map(|(pkg, p)| {
                let prefix = prefix.clone();
                (0..p.exports.len())
                    .filter(move |&i| p.class_name(i as i32 + 1) == "AnimSet" && p.obj_path(i as i32 + 1).to_lowercase().starts_with(&prefix))
                    .map(move |export| ObjRef { pkg, export })
            })
            .max_by_key(|&ar| fits(ar));
        if let Some(own) = own.filter(|&o| fits(o) * 2 >= m.bones.len()) {
            info!("{mesh_path}: using its own anim set {}", reg.path(own));
            anim_ref = Some(own);
        }
    }
    if let Some(ar) = anim_ref {
        let set = upk::anim::AnimSet::parse(reg.pkg(ar), ar.export)?;
        let track_joint: Vec<Option<usize>> = set.track_bones.iter().map(|n| bone_index.get(&n.to_lowercase()).copied()).collect();
        let matched = track_joint.iter().filter(|j| j.is_some()).count();
        if matched * 2 < m.bones.len() {
            warn!(
                "{} has {} bones but anim set {} only drives {matched} of them",
                reg.path(mesh_ref),
                m.bones.len(),
                reg.path(ar)
            );
        }
        for s in &set.sequences {
            let mut pos = Vec::new();
            let mut rot = Vec::new();
            for (k, t) in s.tracks.iter().enumerate() {
                let is_root = track_joint.get(k).copied().flatten() == Some(0);
                pos.push(t.pos.iter().map(|&p| convert::pos(p)).collect());
                // anim keys store non-root rotations conjugated relative to the ref skeleton
                rot.push(t.rot.iter().map(|&q| quat(if is_root { q } else { [-q[0], -q[1], -q[2], q[3]] })).collect());
            }
            by_name.insert(s.name.clone(), clips.len());
            clips.push(Clip {
                frames: s.num_frames as usize,
                length: s.length,
                rate: s.rate_scale,
                pos,
                rot,
                joint: track_joint.clone(),
                footsteps: s.footsteps.clone(),
                sounds: s.sounds.clone(),
            });
        }
    }
    let ragdoll = phys_ref.and_then(|pr| {
        let asset = PhysicsAsset::parse(reg.pkg(pr), pr.export).ok()?;
        let bone_of_body: Vec<Option<usize>> = asset.bodies.iter().map(|b| bone_index.get(&b.bone.to_lowercase()).copied()).collect();
        let pelvis_bone = bone_of_body.iter().flatten().copied().min()?;
        Some(RagdollDef { asset, bone_of_body, pelvis_bone })
    });
    let data = Arc::new(SkelData {
        material_refs: m.materials.iter().map(|&o| reg.resolve(mesh_ref.pkg, o)).collect(),
        sections,
        materials,
        bindposes: a.bindposes.add(inv),
        ref_pose,
        parents: m.bones.iter().map(|b| b.parent).collect(),
        bone_names: m.bones.iter().map(|b| b.name.clone()).collect(),
        rot_origin: m.rot_origin,
        clips,
        by_name,
        ragdoll,
    });
    cache.0.insert(key, data.clone());
    Ok(data)
}

/// Attach a rig under `parent`. `mesh_xf` is the pawn->mesh transform (before RotOrigin).
pub fn spawn(
    commands: &mut Commands,
    parent: Entity,
    data: Arc<SkelData>,
    mesh_xf: Transform,
    material_overrides: &[Option<Handle<StandardMaterial>>],
    start_clip: &str,
) {
    let root_xf = mesh_xf * Transform::from_rotation(convert::rot(data.rot_origin));
    let mesh_root = commands.spawn((root_xf, Visibility::default(), Name::new("MeshRoot"))).id();
    commands.entity(parent).add_child(mesh_root);
    let mut joints: Vec<Entity> = Vec::with_capacity(data.ref_pose.len());
    for (k, &pose) in data.ref_pose.iter().enumerate() {
        let e = commands.spawn(pose).id();
        if k == 0 {
            commands.entity(mesh_root).add_child(e);
        } else {
            commands.entity(joints[data.parents[k] as usize]).add_child(e);
        }
        joints.push(e);
    }
    for (mesh, mat_idx) in &data.sections {
        let mat = material_overrides
            .get(*mat_idx)
            .cloned()
            .flatten()
            .or_else(|| data.materials.get(*mat_idx).cloned())
            .unwrap_or_default();
        let skinned = commands
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat),
                SkinnedMesh { inverse_bindposes: data.bindposes.clone(), joints: joints.clone() },
                DynamicSkinnedMeshBounds,
            ))
            .id();
        commands.entity(mesh_root).add_child(skinned);
    }
    let clip = data.clip_or_variant(start_clip).unwrap_or(0);
    commands.entity(parent).insert((Rig { joints, data }, Animator::new(clip)));
}

/// The goat's pawn -> mesh transform: pawn +X faces Bevy -Z, component Translation (0,0,8).
pub fn goat_mesh_xf() -> Transform {
    Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)) * Transform::from_translation(convert::pos([0.0, 0.0, 8.0]))
}

/// Pick the goat's clip from its movement state (a small stand-in for Goat_AnimTree).
pub fn choose_anim(mut q: Query<(&Goat, &Rig, &mut Animator)>, riding: Res<crate::vehicle::Riding>) {
    let _prof_scope = crate::prof::scope("rig::choose_anim");
    for (g, rig, mut a) in &mut q {
        let speed = Vec2::new(g.velocity.x, g.velocity.z).length() / convert::S;
        let (name, rate) = if g.ragdoll {
            ("Falling", 1.0)
        } else if riding.bike.is_some() {
            // GGBicycleAbstract.GetInVechile: PlayCustomAnim('Biker'), rate from AdaptBikerSpeed
            (riding.anim, riding.anim_rate)
        } else if g.wall.active {
            // wall on the right/left or running straight up it
            let fwd = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
            let right = fwd.cross(Vec3::Y);
            let along = Vec2::new(g.velocity.x, g.velocity.z).length() / convert::S;
            if along < 200.0 {
                ("WallRunUp", 1.0)
            } else if g.wall.normal.dot(right) < 0.0 {
                ("WallRunR", 1.0)
            } else {
                ("WallRunL", 1.0)
            }
        } else if g.env.ladder.is_some() && !g.grounded {
            // no climb animation in Goat_Anim_01: the wall-run-up legs while moving on a ladder
            if g.velocity.y.abs() > 0.1 { ("WallRunUp", 0.8) } else { ("Idle", 1.0) }
        } else if !g.grounded {
            if g.velocity.y > 0.0 { ("JumpIdle", 1.0) } else { ("Falling", 1.0) }
        } else if g.manual.active {
            // manualBlendNode -> UDKAnimNodeSequence "Manual"
            ("Manual", 1.0)
        } else if g.brake_slide {
            ("Brake", 1.0)
        } else if speed < 20.0 {
            ("Idle", 1.0)
        } else if speed <= crate::goat::tune::WALK_SPEED + 50.0 {
            ("Run", (speed / crate::goat::tune::WALK_SPEED).clamp(0.4, 1.5))
        } else {
            ("Sprint", (speed / crate::goat::tune::SPRINT_SPEED).clamp(0.6, 1.4))
        };
        if let Some(c) = rig.data.clip(name) {
            a.play(c, rate);
        }
    }
}

pub fn play_oneshot(a: &mut Animator, rig: &Rig, name: &str) {
    if let Some(c) = rig.data.clip(name) {
        a.oneshot = Some((c, 0.0));
    }
}

pub fn animate(time: Res<Time>, mut q: Query<(&Rig, &mut Animator)>, mut joints: Query<&mut Transform>) {
    let _prof_scope = crate::prof::scope("rig::animate");
    let dt = time.delta_secs();
    for (rig, mut a) in &mut q {
        let data = &rig.data;
        if data.clips.is_empty() {
            continue;
        }
        let clip = &data.clips[a.clip.min(data.clips.len() - 1)];
        let t0 = a.time;
        a.time += dt * a.speed * clip.rate;
        // footstep notifies crossed this frame (the clip loops)
        if clip.length > 0.0 && a.time > t0 {
            let laps = |t: f32, s: f32| ((t - s) / clip.length).floor();
            for &s in &clip.footsteps {
                a.steps += (laps(a.time, s) - laps(t0, s)).clamp(0.0, 4.0) as u32;
            }
            for (s, cue) in &clip.sounds {
                if laps(a.time, *s) > laps(t0, *s) {
                    a.sounds.push(cue.clone());
                }
            }
        }
        let mut pose = data.ref_pose.clone();
        clip.sample_into(a.time, true, &mut pose);
        if let Some((pc, pt)) = a.prev {
            a.fade += dt / 0.2;
            if a.fade >= 1.0 {
                a.prev = None;
            } else {
                let mut old = data.ref_pose.clone();
                data.clips[pc].sample_into(pt, true, &mut old);
                let w = a.fade;
                for (p, o) in pose.iter_mut().zip(old.iter()) {
                    p.translation = o.translation.lerp(p.translation, w);
                    p.rotation = o.rotation.slerp(p.rotation, w);
                }
            }
        }
        if let Some((oc, ot)) = a.oneshot {
            let c = &data.clips[oc];
            let nt = ot + dt * c.rate;
            // one-shots (Ram, Bite) fire their sound notifies once
            for (s, cue) in &c.sounds {
                if ot <= *s && *s < nt {
                    a.sounds.push(cue.clone());
                }
            }
            if nt >= c.length {
                a.oneshot = None;
            } else {
                a.oneshot = Some((oc, nt));
                let mut over = data.ref_pose.clone();
                c.sample_into(nt, false, &mut over);
                let w = (nt / 0.1).min((c.length - nt) / 0.1).clamp(0.0, 1.0);
                for (p, o) in pose.iter_mut().zip(over.iter()) {
                    p.translation = p.translation.lerp(o.translation, w);
                    p.rotation = p.rotation.slerp(o.rotation, w);
                }
            }
        }
        for (k, &e) in rig.joints.iter().enumerate() {
            if let Ok(mut t) = joints.get_mut(e) {
                t.translation = pose[k].translation;
                t.rotation = pose[k].rotation;
            }
        }
    }
}
