//! UE3 particle systems on screen: the emitters `upk::particles` reads are simulated on the CPU
//! (spawn rate and bursts, lifetime, size/colour/velocity over life, location primitives, sub-UV
//! frames) and drawn as camera-facing quads, one mesh per emitter, with the material's texture and
//! blend mode. Sources: the map's Emitter actors (here) and script ParticleSystemComponents
//! (uvm::fx).
//!
//! Mesh emitters (TypeDataMesh) draw their static mesh per particle, lit, with MeshRotation /
//! MeshRotationRate; Orbit offsets particles around their centre; OrientationAxisLock locks the
//! quads to an axis; Collision bounces particles off the static world. Ribbon emitters draw each
//! trail as one camera-facing strip through its particles; beam emitters draw each particle as a
//! strip from its source to its target (BeamNoise points held for NoiseLockTime).

use crate::convert;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::sync::Arc;
use upk::particles::{Blend, Emitter, Module, System};
use upk::registry::Registry;

pub struct ParticlesPlugin;

impl Plugin for ParticlesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FxLibrary>()
            .add_systems(Update, (map_emitters, fx_scenario))
            .add_systems(PostUpdate, (simulate, draw).chain().after(bevy::transform::TransformSystems::Propagate));
    }
}

/// A particle system with its emitters' materials (and mesh emitters' geometry).
pub struct Loaded {
    pub sys: System,
    mats: Vec<Option<Handle<StandardMaterial>>>,
    geo: Vec<Option<Arc<MeshGeo>>>,
}

/// A mesh emitter's static mesh (Bevy space, metres): drawn once per particle.
pub struct MeshGeo {
    pos: Vec<Vec3>,
    nrm: Vec<Vec3>,
    uv: Vec<[f32; 2]>,
    idx: Vec<u32>,
}

fn mesh_geo(reg: &Registry, r: upk::registry::ObjRef) -> Option<MeshGeo> {
    let m = upk::mesh::StaticMesh::parse(reg.pkg(r), r.export).ok()?;
    Some(MeshGeo {
        pos: m.positions.iter().map(|&p| convert::pos(p)).collect(),
        nrm: (0..m.positions.len()).map(|i| m.normals.get(i).map(|&n| convert::dir(n).normalize_or_zero()).unwrap_or(Vec3::Y)).collect(),
        uv: (0..m.positions.len()).map(|i| m.uvs.get(i).copied().unwrap_or([0.0, 0.0])).collect(),
        idx: m.indices.clone(),
    })
}

/// The first material of a static mesh's sections.
fn mesh_section_material(reg: &Registry, r: upk::registry::ObjRef) -> Option<upk::registry::ObjRef> {
    let m = upk::mesh::StaticMesh::parse(reg.pkg(r), r.export).ok()?;
    m.sections.iter().find_map(|s| reg.resolve(r.pkg, s.material))
}

#[derive(Resource, Default)]
pub struct FxLibrary {
    systems: HashMap<String, Option<Arc<Loaded>>>,
    mats: HashMap<(String, u8), Handle<StandardMaterial>>,
    pub spawned: usize,
}

impl FxLibrary {
    /// The ParticleSystem at `path` ("Pkg.Group.Name"), from the first registry that has it.
    pub fn get(&mut self, regs: &[&Registry], path: &str, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> Option<Arc<Loaded>> {
        let key = path.to_ascii_lowercase();
        if let Some(l) = self.systems.get(&key) {
            return l.clone();
        }
        let mut out = None;
        for reg in regs {
            let Some(r) = reg.find_path(path) else { continue };
            let Some(sys) = upk::particles::load_system(reg, r) else { continue };
            let mats = sys
                .emitters
                .iter()
                .map(|e| match e.mesh {
                    Some(m) => self.mesh_material(reg, e.mesh_material.or_else(|| mesh_section_material(reg, m)), materials, images),
                    None => self.material(reg, e, materials, images),
                })
                .collect();
            let geo = sys.emitters.iter().map(|e| e.mesh.and_then(|m| mesh_geo(reg, m)).map(Arc::new)).collect();
            let skipped: Vec<String> = sys.emitters.iter().filter_map(|e| e.unsupported.clone()).collect();
            info!("[fx] {path}: {} emitters{}", sys.emitters.len(), if skipped.is_empty() { String::new() } else { format!(" ({} not drawn: {})", skipped.len(), skipped.join(", ")) });
            out = Some(Arc::new(Loaded { sys, mats, geo }));
            break;
        }
        if out.is_none() {
            warn!("[fx] particle system {path} not found");
        }
        self.systems.insert(key, out.clone());
        out
    }

    /// A mesh particle's material: lit, its diffuse texture (or colour).
    fn mesh_material(&mut self, reg: &Registry, mat: Option<upk::registry::ObjRef>, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> Option<Handle<StandardMaterial>> {
        let key = (format!("mesh:{}", mat.map(|m| reg.path(m)).unwrap_or_default()), 255);
        if let Some(h) = self.mats.get(&key) {
            return Some(h.clone());
        }
        let info = mat.map(|m| upk::material::resolve_in(reg, m)).unwrap_or_default();
        let image = info.diffuse.and_then(|t| {
            let p = reg.pkg(t);
            let tex = upk::texture::Texture::parse(p, t.export).ok()?;
            let img = tex.decode(p, t.export, 512).ok()?;
            Some(images.add(crate::world::make_image(img, tex.srgb)))
        });
        let base = match (&image, info.color) {
            (None, Some(c)) => Color::linear_rgb(c[0], c[1], c[2]),
            _ => Color::WHITE,
        };
        let h = materials.add(StandardMaterial {
            base_color: base,
            base_color_texture: image,
            perceptual_roughness: 0.8,
            alpha_mode: if info.masked { AlphaMode::Mask(0.33) } else { AlphaMode::Opaque },
            double_sided: info.two_sided,
            cull_mode: if info.two_sided { None } else { Some(bevy::render::render_resource::Face::Back) },
            ..default()
        });
        self.mats.insert(key, h.clone());
        Some(h)
    }

    fn material(&mut self, reg: &Registry, e: &Emitter, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> Option<Handle<StandardMaterial>> {
        if e.unsupported.is_some() {
            return None;
        }
        // refraction-only materials (heat haze: a Distortion input, no colour texture) draw nothing
        if e.texture.is_none() && e.material.is_some_and(|m| upk::registry::find(&reg.merged_props(m), "Distortion").is_some_and(|(_, v)| format!("{v:?}").contains("Expression"))) {
            return None;
        }
        let tex_path = e.texture.map(|t| reg.path(t)).unwrap_or_default();
        let op_path = e.opacity.map(|t| reg.path(t)).unwrap_or_default();
        let key = (format!("{tex_path}|{op_path}"), e.blend as u8);
        if let Some(h) = self.mats.get(&key) {
            return Some(h.clone());
        }
        let decode = |t: upk::registry::ObjRef| -> Option<(upk::texture::Image, bool)> {
            let p = reg.pkg(t);
            let tex = upk::texture::Texture::parse(p, t.export).ok()?;
            Some((tex.decode(p, t.export, 512).ok()?, tex.srgb))
        };
        let colour = e.texture.and_then(decode);
        let opacity = e.opacity.filter(|o| Some(*o) != e.texture).and_then(decode);
        let image = colour.map(|(mut img, srgb)| {
            let has_alpha = |i: &upk::texture::Image| i.rgba.chunks_exact(4).any(|c| c[3] != 255);
            if let Some((op, _)) = &opacity {
                // a separate opacity texture: its alpha, or its brightness (sampled at this size)
                let alpha_ch = has_alpha(op);
                for y in 0..img.height {
                    for x in 0..img.width {
                        let ox = (x * op.width / img.width.max(1)).min(op.width - 1);
                        let oy = (y * op.height / img.height.max(1)).min(op.height - 1);
                        let o = ((oy * op.width + ox) * 4) as usize;
                        let a = if alpha_ch { op.rgba[o + 3] } else { op.rgba[o] };
                        let i = ((y * img.width + x) * 4) as usize;
                        img.rgba[i + 3] = ((img.rgba[i + 3] as u32 * a as u32) / 255) as u8;
                    }
                }
            } else if !has_alpha(&img) && e.blend == Blend::Translucent {
                // greyscale sprites without alpha: their brightness is the opacity
                for c in img.rgba.chunks_exact_mut(4) {
                    c[3] = c[0].max(c[1]).max(c[2]);
                }
            }
            let mut im = crate::world::make_image(img, srgb);
            let mut sd = bevy::image::ImageSamplerDescriptor::linear();
            sd.address_mode_u = bevy::image::ImageAddressMode::ClampToEdge;
            sd.address_mode_v = bevy::image::ImageAddressMode::ClampToEdge;
            im.sampler = bevy::image::ImageSampler::Descriptor(sd);
            images.add(im)
        });
        let alpha_mode = match e.blend {
            Blend::Additive => AlphaMode::Add,
            Blend::Modulate => AlphaMode::Multiply,
            Blend::Masked => AlphaMode::Mask(0.3),
            _ => AlphaMode::Blend,
        };
        let h = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: image,
            unlit: true,
            alpha_mode,
            cull_mode: None,
            double_sided: true,
            fog_enabled: true,
            ..default()
        });
        self.mats.insert(key, h.clone());
        Some(h)
    }
}

/// One playing particle system; its world transform is the emitter origin.
#[derive(Component)]
pub struct ParticleFx {
    pub fx: Arc<Loaded>,
    /// Spawning (ActivateSystem / DeactivateSystem); live particles finish either way.
    pub active: bool,
    pub hidden: bool,
    /// Remove the entity once every emitter finished (EmitterPool one-shots).
    pub despawn_when_done: bool,
    /// Clear all particles now (KillParticlesForced).
    pub kill: bool,
    emitters: Vec<EmState>,
    rng: u32,
    last_origin: Option<Vec3>,
    warmed: bool,
}

impl ParticleFx {
    pub fn new(fx: Arc<Loaded>, active: bool) -> Self {
        let emitters = fx.sys.emitters.iter().map(|e| EmState { delay: e.delay, ..Default::default() }).collect();
        ParticleFx { fx, active, hidden: false, despawn_when_done: false, kill: false, emitters, rng: 0x9e37_79b9, last_origin: None, warmed: false }
    }

    /// Restart every emitter's timeline (ActivateSystem on a finished system).
    pub fn restart(&mut self) {
        for (s, e) in self.emitters.iter_mut().zip(&self.fx.sys.emitters) {
            s.t = 0.0;
            s.loops = 0;
            s.accum = 0.0;
            s.bursts_done = 0;
            s.finished = false;
            s.delay = e.delay;
        }
    }

    pub fn live(&self) -> usize {
        self.emitters.iter().map(|e| e.ps.len()).sum()
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    fn rand3(&mut self) -> [f32; 3] {
        [self.rand(), self.rand(), self.rand()]
    }
}

#[derive(Default)]
struct EmState {
    t: f32,
    loops: i32,
    accum: f32,
    bursts_done: usize,
    finished: bool,
    delay: f32,
    ps: Vec<P>,
    mesh: Option<(Entity, Handle<Mesh>)>,
    /// SpawnPerUnit's fractional particles carried over.
    unit_accum: f32,
}

#[derive(Clone, Copy)]
struct P {
    pos: Vec3,
    vel: Vec3,
    base_vel: Vec3,
    accel: Vec3,
    base_size: Vec3,
    size: Vec3,
    base_color: Vec4,
    color: Vec4,
    rot: f32,
    rot_rate: f32,
    age: f32,
    inv_life: f32,
    frame: f32,
    seed: f32,
    /// Orbit: offset (Bevy space, metres), rotation and rate (turns per Unreal axis).
    orbit: Vec3,
    orbit_rot: Vec3,
    orbit_rate: Vec3,
    /// Mesh particles: rotation and rate (turns per Unreal axis).
    mrot: Vec3,
    mrot_rate: Vec3,
    /// Collision: hits left (-1 = no collision module), damping, what happens after the last.
    hits_left: i32,
    damping: Vec3,
    completion: u8,
    frozen: bool,
}

/// A rotation of `turns` about each Unreal axis (X roll, Y pitch, Z yaw) in Bevy space.
fn ue_turns(t: Vec3) -> Quat {
    let ax = |v: [f32; 3]| Vec3::from(convert::dir(v)).normalize_or_zero();
    Quat::from_axis_angle(ax([0.0, 0.0, 1.0]), t.z * TAU) * Quat::from_axis_angle(ax([0.0, 1.0, 0.0]), t.y * TAU) * Quat::from_axis_angle(ax([1.0, 0.0, 0.0]), t.x * TAU)
}

fn uv3(v: [f32; 3]) -> Vec3 {
    convert::dir(v) * convert::S
}

fn v4(c: [f32; 3], a: f32) -> Vec4 {
    Vec4::new(c[0], c[1], c[2], a)
}

/// Spawn one particle at emitter time `t`; `origin` is the emitter's world transform.
fn spawn_one(fx: &mut ParticleFx, ei: usize, t: f32, origin: &Transform, age: f32) {
    let loaded = fx.fx.clone();
    let e = &loaded.sys.emitters[ei];
    let mut p = P {
        pos: Vec3::ZERO,
        vel: Vec3::ZERO,
        base_vel: Vec3::ZERO,
        accel: Vec3::ZERO,
        base_size: Vec3::ONE,
        size: Vec3::ONE,
        base_color: Vec4::ONE,
        color: Vec4::ONE,
        rot: 0.0,
        rot_rate: 0.0,
        age: 0.0,
        inv_life: 0.0,
        frame: 0.0,
        seed: fx.rand(),
        orbit: Vec3::ZERO,
        orbit_rot: Vec3::ZERO,
        orbit_rate: Vec3::ZERO,
        mrot: Vec3::ZERO,
        mrot_rate: Vec3::ZERO,
        hits_left: -1,
        damping: Vec3::ONE,
        completion: 0,
        frozen: false,
    };
    let rot = origin.rotation;
    let mut offset = Vec3::ZERO;
    let mut radial = 0.0;
    for m in &e.modules {
        match m {
            Module::Lifetime(d) => {
                let life = d.eval1(t, fx.rand());
                p.inv_life = if life > 1e-4 { 1.0 / life } else { 0.0 };
            }
            Module::Size(d) => p.base_size = Vec3::from(d.eval(t, fx.rand3())),
            Module::Velocity { d, radial: r, world } => {
                let v = uv3(d.eval(t, fx.rand3()));
                p.base_vel += if *world { v } else { rot * v };
                radial += r.eval1(t, fx.rand()) * convert::S;
            }
            Module::Acceleration { d, world } => {
                let a = uv3(d.eval(t, fx.rand3()));
                p.accel += if *world { a } else { rot * a };
            }
            Module::Color { c, a } => p.base_color = v4(c.eval(t, fx.rand3()), a.eval1(t, fx.rand())),
            Module::ColorOverLife { c, a } => p.base_color = v4(c.eval(0.0, fx.rand3()), a.eval1(0.0, fx.rand())),
            Module::Rotation(d) => p.rot = d.eval1(t, fx.rand()) * TAU,
            Module::RotationRate(d) => p.rot_rate = d.eval1(t, fx.rand()) * TAU,
            Module::Location(d) => offset += uv3(d.eval(t, fx.rand3())),
            Module::Sphere { radius, offset: o, axes, surface, velocity, vel_scale } => {
                let mut dir = Vec3::ZERO;
                for _ in 0..8 {
                    let r = fx.rand3();
                    let c = |i: usize| {
                        let v = r[i] * 2.0 - 1.0;
                        match (axes[i], axes[i + 3]) {
                            (true, true) => v,
                            (true, false) => v.abs(),
                            (false, true) => -v.abs(),
                            _ => 0.0,
                        }
                    };
                    dir = Vec3::from(convert::dir([c(0), c(1), c(2)]));
                    if dir.length_squared() > 1e-4 && dir.length_squared() <= 1.0 {
                        break;
                    }
                }
                let dir = dir.normalize_or_zero();
                let r = radius.eval1(t, fx.rand()) * convert::S;
                let along = if *surface { r } else { r * fx.rand() };
                offset += dir * along + uv3(o.eval(t, fx.rand3()));
                if *velocity {
                    p.base_vel += rot * (dir * along * vel_scale.eval1(t, fx.rand()));
                }
            }
            Module::Cylinder { radius, height, offset: o, axes, surface, velocity, vel_scale, axis } => {
                let ang = fx.rand() * TAU;
                let r = radius.eval1(t, fx.rand()) * convert::S;
                let along = if *surface { r } else { r * fx.rand().sqrt() };
                let h = height.eval1(t, fx.rand()) * convert::S;
                let hz = (fx.rand() - 0.5) * h;
                let (a, b) = (ang.cos() * along, ang.sin() * along);
                let ue = match axis {
                    0 => [hz, a, b],
                    1 => [a, hz, b],
                    _ => [a, b, hz],
                };
                let _ = axes;
                let local = Vec3::from(convert::dir(ue));
                offset += local + uv3(o.eval(t, fx.rand3()));
                if *velocity {
                    let mut flat = ue;
                    flat[*axis as usize] = 0.0;
                    p.base_vel += rot * (Vec3::from(convert::dir(flat)) * vel_scale.eval1(t, fx.rand()));
                }
            }
            Module::SubUV(d) if matches!(e.sub_interp, 3 | 4) => p.frame = d.eval1(0.0, fx.rand()),
            Module::Orbit { offset, rotation, rate } => {
                // chained orbits add up (ChainMode Add)
                p.orbit += rot * uv3(offset.eval(t, fx.rand3()));
                p.orbit_rot += Vec3::from(rotation.eval(t, fx.rand3()));
                p.orbit_rate += Vec3::from(rate.eval(t, fx.rand3()));
            }
            Module::MeshRotation(d) => p.mrot = Vec3::from(d.eval(t, fx.rand3())),
            Module::MeshRotationRate(d) => p.mrot_rate = Vec3::from(d.eval(t, fx.rand3())),
            Module::Collision { damping, max, completion } => {
                p.hits_left = max.eval1(t, fx.rand()).round().max(0.0) as i32;
                let k = damping.eval(t, fx.rand3());
                p.damping = Vec3::from(convert::dir(k)).abs();
                p.completion = *completion;
            }
            _ => {}
        }
    }
    if matches!(e.sub_interp, 3 | 4) && !e.modules.iter().any(|m| matches!(m, Module::SubUV(_))) {
        p.frame = (fx.rand() * (e.sub_h * e.sub_v) as f32).floor();
    }
    if radial != 0.0 {
        p.base_vel += (rot * offset).normalize_or_zero() * radial;
    }
    p.pos = if e.local_space { offset } else { origin.transform_point(offset) };
    p.vel = p.base_vel;
    p.size = p.base_size;
    p.color = p.base_color;
    p.age = age;
    let ps = &mut fx.emitters[ei].ps;
    if ps.len() < e.max_particles as usize {
        ps.push(p);
    }
}

fn update_particles(e: &Emitter, ps: &mut Vec<P>, dt: f32, emitter_frac: f32, phys: Option<&crate::physics::Physics>) {
    ps.retain_mut(|p| {
        p.age += dt;
        let rel = p.age * p.inv_life;
        if p.inv_life > 0.0 && rel >= 1.0 {
            return false;
        }
        p.orbit_rot += p.orbit_rate * dt;
        if p.frozen {
            return true;
        }
        let rel = rel.min(1.0);
        let mut v = p.base_vel;
        let mut size = p.base_size;
        let mut color = p.base_color;
        for m in &e.modules {
            match m {
                Module::AccelerationOverLife { d, .. } => p.base_vel += uv3(d.eval(rel, [p.seed; 3])) * dt,
                Module::VelocityOverLife { d, absolute, .. } => {
                    let k = d.eval(rel, [p.seed; 3]);
                    if *absolute {
                        v = uv3(k);
                    } else {
                        v *= Vec3::from(convert::dir(k));
                    }
                }
                Module::Drag(d) => p.base_vel *= (1.0 - d.eval1(rel, p.seed) * dt).max(0.0),
                Module::SizeMultiplyLife { d, axes } => {
                    let k = d.eval(rel, [p.seed; 3]);
                    for i in 0..3 {
                        if axes[i] {
                            size[i] *= k[i];
                        }
                    }
                }
                Module::SizeScale { d, axes } => {
                    let k = d.eval(rel, [p.seed; 3]);
                    for i in 0..3 {
                        if axes[i] {
                            size[i] = p.base_size[i] * k[i];
                        }
                    }
                }
                Module::ColorOverLife { c, a } => color = v4(c.eval(rel, [p.seed; 3]), a.eval1(rel, p.seed)),
                Module::ColorScaleOverLife { c, a, emitter_time } => {
                    let tt = if *emitter_time { emitter_frac } else { rel };
                    color *= v4(c.eval(tt, [p.seed; 3]), a.eval1(tt, p.seed));
                }
                Module::SubUV(d) if !matches!(e.sub_interp, 3 | 4) => p.frame = d.eval1(rel, p.seed),
                _ => {}
            }
        }
        p.base_vel += p.accel * dt;
        // Collision: this frame's move against the static world (world-space particles)
        let step = v * dt;
        if p.hits_left >= 0 && !e.local_space && step.length_squared() > 1e-10 {
            if let Some(phys) = phys {
                let len = step.length();
                let ray = rapier3d::prelude::Ray::new(convert::to_rapier(p.pos), convert::to_rapier(step / len));
                if let Some((_, hit)) = phys.world.cast_ray_and_get_normal(&ray, len, true, rapier3d::prelude::QueryFilter::only_fixed()) {
                    let n = convert::from_rapier(hit.normal);
                    p.pos += step / len * (hit.time_of_impact - 0.01).max(0.0) + n * 0.01;
                    if p.hits_left == 0 {
                        match p.completion {
                            0 => return false,
                            2 => p.hits_left = -1,
                            _ => {
                                p.frozen = true;
                                return true;
                            }
                        }
                    } else {
                        p.hits_left -= 1;
                        p.base_vel = (p.base_vel - 2.0 * p.base_vel.dot(n) * n) * p.damping;
                    }
                    p.vel = p.base_vel;
                    p.size = size;
                    p.color = color;
                    return true;
                }
            }
        }
        p.pos += step;
        p.vel = v;
        p.size = size;
        p.color = color;
        p.rot += p.rot_rate * dt;
        p.mrot += p.mrot_rate * dt;
        if matches!(e.sub_interp, 1 | 2) && !e.modules.iter().any(|m| matches!(m, Module::SubUV(_))) {
            p.frame = (rel * (e.sub_h * e.sub_v) as f32).floor();
        }
        true
    });
}

/// Advance every system: emitter timelines, spawning, particle updates, cleanup.
pub fn simulate(time: Res<Time>, mut commands: Commands, mut q: Query<(Entity, &GlobalTransform, &mut ParticleFx)>, phys: Option<Res<crate::physics::Physics>>) {
    let _prof_scope = crate::prof::scope("particles::simulate");
    let phys = phys.as_deref();
    let dt = time.delta_secs().min(0.1);
    for (ent, gt, mut fx) in &mut q {
        let origin = gt.compute_transform();
        if !fx.warmed {
            fx.warmed = true;
            let warm = fx.fx.sys.warmup.min(5.0);
            let mut w = 0.0;
            while w < warm {
                step(&mut fx, 1.0 / 30.0, &origin, &origin, phys);
                w += 1.0 / 30.0;
            }
        }
        let prev = fx.last_origin.map(|p| Transform { translation: p, ..origin }).unwrap_or(origin);
        fx.last_origin = Some(origin.translation);
        step(&mut fx, dt, &prev, &origin, phys);
        let done = fx.emitters.iter().all(|e| e.finished && e.ps.is_empty()) || (!fx.active && fx.live() == 0 && fx.despawn_when_done);
        if done && fx.despawn_when_done {
            for e in &fx.emitters {
                if let Some((m, _)) = &e.mesh {
                    commands.entity(*m).despawn();
                }
            }
            commands.entity(ent).despawn();
        }
    }
}

fn step(fx: &mut ParticleFx, dt: f32, prev: &Transform, origin: &Transform, phys: Option<&crate::physics::Physics>) {
    if fx.kill {
        fx.kill = false;
        for e in &mut fx.emitters {
            e.ps.clear();
        }
    }
    let loaded = fx.fx.clone();
    for (ei, e) in loaded.sys.emitters.iter().enumerate() {
        if e.unsupported.is_some() {
            continue;
        }
        let spawning = fx.active && !fx.hidden;
        let st = &mut fx.emitters[ei];
        let frac = (st.t / e.duration).clamp(0.0, 1.0);
        let collides = e.modules.iter().any(|m| matches!(m, Module::Collision { .. }));
        update_particles(e, &mut st.ps, dt, frac, if collides { phys } else { None });
        if !spawning || st.finished {
            continue;
        }
        if st.delay > 0.0 {
            st.delay -= dt;
            continue;
        }
        let t0 = st.t;
        st.t += dt;
        // bursts that came due this frame
        let mut burst = 0;
        while st.bursts_done < e.bursts.len() && e.bursts[st.bursts_done].2 * e.duration <= st.t {
            let (count, low, _) = e.bursts[st.bursts_done];
            st.bursts_done += 1;
            // CountLow..Count: the middle of the range
            burst += if low >= 0 && low < count { (low + count + 1) / 2 } else { count };
        }
        let mut rate = (e.rate.eval1(t0, 0.5) * e.rate_scale.eval1(t0, 0.5)).max(0.0);
        // SpawnPerUnit: by the distance the emitter moved this frame
        let moved = prev.translation.distance(origin.translation) / convert::S;
        let mut per_unit = 0usize;
        for m in &e.modules {
            if let Module::SpawnPerUnit { unit, per, ignore_rate } = m {
                if moved > 0.1 {
                    st.unit_accum += moved / unit * per.eval1(t0, 0.5);
                    if *ignore_rate {
                        rate = 0.0;
                    }
                }
                per_unit += st.unit_accum.floor() as usize;
                st.unit_accum -= st.unit_accum.floor();
            }
        }
        st.accum += rate * dt;
        let n = ((st.accum.floor() as usize) + per_unit).min(e.max_particles as usize) + burst as usize;
        st.accum -= st.accum.floor();
        if st.t >= e.duration {
            st.loops += 1;
            if e.loops > 0 && st.loops >= e.loops {
                st.finished = true;
            } else {
                st.t -= e.duration;
                st.bursts_done = 0;
            }
        }
        // spread spawns along the emitter's path this frame (smooth trails when it moves fast)
        for i in 0..n {
            let a = (i as f32 + 1.0) / n as f32;
            let at = Transform { translation: prev.translation.lerp(origin.translation, a), ..*origin };
            spawn_one(fx, ei, t0 + dt * a, &at, dt * (1.0 - a));
        }
    }
}

/// Rebuild each emitter's quads facing the camera.
#[allow(clippy::type_complexity)]
pub fn draw(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    cams: Query<&GlobalTransform, With<Camera3d>>,
    mut q: Query<(&GlobalTransform, &mut ParticleFx)>,
    mut vis: Query<&mut Visibility>,
) {
    let _prof_scope = crate::prof::scope("particles::draw");
    let Ok(cam) = cams.single() else { return };
    let cam = cam.compute_transform();
    let right = cam.rotation * Vec3::X;
    let up = cam.rotation * Vec3::Y;
    let fwd = cam.rotation * Vec3::NEG_Z;
    for (gt, mut fx) in &mut q {
        let origin = gt.compute_transform();
        let scale = origin.scale.max_element();
        let loaded = fx.fx.clone();
        let hidden = fx.hidden;
        for (ei, e) in loaded.sys.emitters.iter().enumerate() {
            let Some(mat) = loaded.mats.get(ei).cloned().flatten() else { continue };
            let st = &mut fx.emitters[ei];
            let show = !st.ps.is_empty() && !hidden;
            if let Some((ent, _)) = &st.mesh {
                if let Ok(mut v) = vis.get_mut(*ent) {
                    let want = if show { Visibility::Visible } else { Visibility::Hidden };
                    if *v != want {
                        *v = want;
                    }
                }
            }
            if !show {
                continue;
            }
            let n = st.ps.len();
            if let Some(geo) = loaded.geo.get(ei).cloned().flatten() {
                // mesh particles: the static mesh per particle (size = scale, MeshRotation)
                let (mut pos, mut nrm, mut uv, mut idx) = (Vec::with_capacity(n * geo.pos.len()), Vec::with_capacity(n * geo.pos.len()), Vec::with_capacity(n * geo.pos.len()), Vec::with_capacity(n * geo.idx.len()));
                for p in &st.ps {
                    let c = if e.local_space { origin.transform_point(p.pos) } else { p.pos } + p.orbit;
                    let r = if e.local_space { origin.rotation } else { Quat::IDENTITY } * ue_turns(p.mrot);
                    let k = Vec3::from(convert::dir(p.size.to_array())).abs() * scale;
                    let b = pos.len() as u32;
                    for i in 0..geo.pos.len() {
                        pos.push((c + r * (geo.pos[i] * k)).to_array());
                        nrm.push((r * geo.nrm[i]).to_array());
                        uv.push(geo.uv[i]);
                    }
                    idx.extend(geo.idx.iter().map(|&i| b + i));
                }
                let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, nrm)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
                    .with_inserted_indices(Indices::U32(idx));
                match &st.mesh {
                    Some((_, handle)) => {
                        if let Some(mut m) = meshes.get_mut(handle) {
                            *m = mesh;
                        }
                    }
                    None => {
                        let handle = meshes.add(mesh);
                        let ent = commands.spawn((Mesh3d(handle.clone()), MeshMaterial3d(mat), Transform::IDENTITY, NoFrustumCulling, Visibility::Visible, Name::new(format!("fxmesh:{}", e.name)))).id();
                        st.mesh = Some((ent, handle));
                    }
                }
                continue;
            }
            if e.ribbon.is_some() || e.beam.is_some() {
                let (mut pos, mut uv, mut col, mut idx): (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 4]>, Vec<u32>) = (vec![], vec![], vec![], vec![]);
                let colour = |p: &P| {
                    let rgb = p.color.truncate().max(Vec3::ZERO);
                    let m = rgb.max_element();
                    let rgb = if m > 1.0 { rgb / m * (1.0 + m.ln() * 0.25) } else { rgb };
                    [rgb.x, rgb.y, rgb.z, p.color.w.clamp(0.0, 1.0)]
                };
                // a camera-facing strip through `pts` (centre, half width, colour, u)
                let mut strip = |pts: &[(Vec3, f32, [f32; 4], f32)]| {
                    if pts.len() < 2 {
                        return;
                    }
                    let b = pos.len() as u32;
                    for (i, &(c, hw, cc, u)) in pts.iter().enumerate() {
                        let along = if i + 1 < pts.len() { pts[i + 1].0 - c } else { c - pts[i - 1].0 };
                        let side = along.cross(cam.translation - c).normalize_or_zero() * hw;
                        pos.push((c - side).to_array());
                        pos.push((c + side).to_array());
                        uv.push([u, 1.0]);
                        uv.push([u, 0.0]);
                        col.extend_from_slice(&[cc, cc]);
                    }
                    for i in 0..pts.len() as u32 - 1 {
                        let k = b + i * 2;
                        idx.extend_from_slice(&[k, k + 1, k + 3, k, k + 3, k + 2]);
                    }
                };
                let world = |p: Vec3| if e.local_space { origin.transform_point(p) } else { p };
                if let Some(rb) = &e.ribbon {
                    // the trail: spawn order is the trail order, newest last
                    let n = st.ps.len().min(rb.max_in_trail as usize);
                    let tail = &st.ps[st.ps.len() - n..];
                    let mut dist_along = 0.0;
                    let mut pts = Vec::with_capacity(n);
                    for (i, p) in tail.iter().enumerate() {
                        let c = world(p.pos);
                        if i > 0 {
                            dist_along += c.distance(world(tail[i - 1].pos));
                        }
                        let u = if rb.tiling > 0.0 { dist_along / (rb.tiling * convert::S) } else { i as f32 / (n.max(2) - 1) as f32 };
                        pts.push((c, p.size.x * convert::S * 0.5 * scale, colour(p), u));
                    }
                    strip(&pts);
                } else if let Some(bm) = &e.beam {
                    let x_axis = origin.rotation * Vec3::from(convert::dir([1.0, 0.0, 0.0]));
                    for p in &st.ps {
                        let src = bm.source.as_ref().map(|d| origin.transform_point(uv3(d.eval(0.0, [p.seed; 3])))).unwrap_or(origin.translation);
                        let dst = match (bm.method, bm.target.as_ref()) {
                            (1, Some(d)) => origin.transform_point(uv3(d.eval(0.0, [p.seed; 3]))),
                            _ => src + x_axis * bm.distance.eval1(0.0, p.seed) * convert::S,
                        };
                        let hw = p.size.x * convert::S * 0.5 * scale;
                        let cc = colour(p);
                        let n = bm.noise.as_ref().map(|n| n.0 as usize + 1).unwrap_or(1);
                        // the noise pattern holds for NoiseLockTime, then jumps (lightning)
                        let bucket = bm.noise.as_ref().map(|n| if n.2 > 0.0 { (p.age / n.2).floor() } else { (p.age * 30.0).floor() }).unwrap_or(0.0);
                        let dir = (dst - src).normalize_or_zero();
                        let perp_a = dir.any_orthonormal_vector();
                        let perp_b = dir.cross(perp_a);
                        let mut pts = Vec::with_capacity(n + 1);
                        for k in 0..=n {
                            let t = k as f32 / n as f32;
                            let mut c = src.lerp(dst, t);
                            if let (Some(nz), true) = (&bm.noise, k > 0 && k < n) {
                                let h = |s: f32| ((p.seed * 9283.0 + k as f32 * 127.1 + bucket * 311.7 + s).sin() * 43758.545).fract() * 2.0 - 1.0;
                                let r = uv3(nz.1.eval(0.0, [0.5; 3])).abs().max_element();
                                c += (perp_a * h(1.0) + perp_b * h(2.0)) * r;
                            }
                            pts.push((c, hw, cc, t * bm.tex_tile));
                        }
                        strip(&pts);
                    }
                }
                if idx.is_empty() {
                    continue;
                }
                let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, col)
                    .with_inserted_indices(Indices::U32(idx));
                match &st.mesh {
                    Some((_, handle)) => {
                        if let Some(mut m) = meshes.get_mut(handle) {
                            *m = mesh;
                        }
                    }
                    None => {
                        let handle = meshes.add(mesh);
                        let ent = commands.spawn((Mesh3d(handle.clone()), MeshMaterial3d(mat), Transform::IDENTITY, NoFrustumCulling, bevy::light::NotShadowCaster, Visibility::Visible, Name::new(format!("fx:{}", e.name)))).id();
                        st.mesh = Some((ent, handle));
                    }
                }
                continue;
            }
            // OrientationAxisLock: quads facing an axis, or turning about it (emitter space)
            let lock_axis = |k: u8| -> Vec3 {
                let ue = match k {
                    1 | 7 => [1.0, 0.0, 0.0],
                    2 | 8 => [0.0, 1.0, 0.0],
                    4 => [-1.0, 0.0, 0.0],
                    5 => [0.0, -1.0, 0.0],
                    6 => [0.0, 0.0, -1.0],
                    _ => [0.0, 0.0, 1.0],
                };
                origin.rotation * Vec3::from(convert::dir(ue)).normalize_or_zero()
            };
            let (lock_r, lock_u) = match e.axis_lock {
                1..=6 => {
                    let a = lock_axis(e.axis_lock);
                    let r = a.any_orthonormal_vector();
                    (r, a.cross(r))
                }
                7..=9 => {
                    let a = lock_axis(e.axis_lock);
                    (a.cross(fwd).normalize_or_zero(), a)
                }
                _ => (right, up),
            };
            let mut pos = Vec::with_capacity(n * 4);
            let mut uv = Vec::with_capacity(n * 4);
            let mut col = Vec::with_capacity(n * 4);
            let mut idx: Vec<u32> = Vec::with_capacity(n * 6);
            let frames = (e.sub_h * e.sub_v).max(1);
            for p in &st.ps {
                let orbit = if p.orbit == Vec3::ZERO { Vec3::ZERO } else { ue_turns(p.orbit_rot) * p.orbit };
                let c = if e.local_space { origin.transform_point(p.pos) } else { p.pos } + orbit;
                let w = p.size.x * convert::S * 0.5 * scale;
                let h = if e.align == 1 { p.size.y } else { p.size.x } * convert::S * 0.5 * scale;
                let (mut r, mut u) = (lock_r, lock_u);
                if e.align == 2 {
                    let v = p.vel - fwd * p.vel.dot(fwd);
                    if v.length_squared() > 1e-8 {
                        u = v.normalize();
                        r = u.cross(fwd).normalize_or_zero();
                    }
                } else if p.rot != 0.0 {
                    let (s, co) = p.rot.sin_cos();
                    (r, u) = (lock_r * co + lock_u * s, lock_u * co - lock_r * s);
                }
                let b = pos.len() as u32;
                pos.push((c - r * w - u * h).to_array());
                pos.push((c + r * w - u * h).to_array());
                pos.push((c + r * w + u * h).to_array());
                pos.push((c - r * w + u * h).to_array());
                let f = (p.frame.max(0.0) as u32) % frames;
                let (fx0, fy0) = ((f % e.sub_h) as f32 / e.sub_h as f32, (f / e.sub_h) as f32 / e.sub_v as f32);
                let (du, dv) = (1.0 / e.sub_h as f32, 1.0 / e.sub_v as f32);
                uv.extend_from_slice(&[[fx0, fy0 + dv], [fx0 + du, fy0 + dv], [fx0 + du, fy0], [fx0, fy0]]);
                // over-bright (HDR) colours: keep the hue, roll the brightness off softly
                let rgb = p.color.truncate().max(Vec3::ZERO);
                let m = rgb.max_element();
                let rgb = if m > 1.0 { rgb / m * (1.0 + m.ln() * 0.25) } else { rgb };
                let cc = [rgb.x, rgb.y, rgb.z, p.color.w.clamp(0.0, 1.0)];
                col.extend_from_slice(&[cc; 4]);
                idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
            }
            // the mesh is only ever created or replaced whole (never left without attributes)
            let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
                .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
                .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, col)
                .with_inserted_indices(Indices::U32(idx));
            match &st.mesh {
                Some((_, handle)) => {
                    if let Some(mut m) = meshes.get_mut(handle) {
                        *m = mesh;
                    }
                }
                None => {
                    let handle = meshes.add(mesh);
                    let ent = commands.spawn((Mesh3d(handle.clone()), MeshMaterial3d(mat), Transform::IDENTITY, NoFrustumCulling, bevy::light::NotShadowCaster, Visibility::Visible, Name::new(format!("fx:{}", e.name)))).id();
                    st.mesh = Some((ent, handle));
                }
            }
        }
    }
}

/// The map's Emitter actors (auto-activating ones play; the rest wait for Kismet/script).
#[derive(Component)]
pub struct MapEmitter(pub String);

#[allow(clippy::too_many_arguments)]
fn map_emitters(
    mut done: Local<bool>,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    mut lib: ResMut<FxLibrary>,
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if *done {
        return;
    }
    let Some(game) = game else { return };
    *done = true;
    if std::env::var("GOAT_NO_FX").is_ok() {
        return;
    }
    let t0 = std::time::Instant::now();
    let reg = game.0.clone();
    let p = &reg.packages[0];
    let (mut n, mut on) = (0, 0);
    for i in 0..p.exports.len() {
        let r = upk::registry::ObjRef { pkg: 0, export: i };
        if reg.class_name(r) != "Emitter" || !p.obj_path(i as i32 + 1).contains("PersistentLevel") {
            continue;
        }
        let props = reg.merged_props(r);
        let get = |k: &str| upk::registry::find(&props, k).map(|(pk, v)| (pk, v.clone()));
        let Some((pk, upk::props::Value::Object(ci))) = get("ParticleSystemComponent") else { continue };
        let Some(comp) = reg.resolve(pk, ci) else { continue };
        let cprops = reg.merged_props(comp);
        let cget = |k: &str| upk::registry::find(&cprops, k).map(|(pk, v)| (pk, v.clone()));
        let Some((tpk, upk::props::Value::Object(ti))) = cget("Template") else { continue };
        let Some(tr) = reg.resolve(tpk, ti) else { continue };
        let auto = !matches!(cget("bAutoActivate"), Some((_, upk::props::Value::Bool(false))));
        let path = reg.path(tr);
        let Some(fx) = lib.get(&[&reg], &path, &mut materials, &mut images) else { continue };
        let loc = match get("Location") {
            Some((_, upk::props::Value::Vector(v))) => v,
            _ => [0.0; 3],
        };
        let rot = match get("Rotation") {
            Some((_, upk::props::Value::Rotator(r))) => r,
            _ => [0; 3],
        };
        let ds = match get("DrawScale") {
            Some((_, upk::props::Value::Float(f))) => f,
            _ => 1.0,
        };
        let ds3 = match get("DrawScale3D") {
            Some((_, upk::props::Value::Vector(v))) => v,
            _ => [1.0; 3],
        };
        let xf = Transform { translation: convert::pos(loc), rotation: convert::rot(rot), scale: convert::scale(ds3) * ds };
        let name = p.obj_name(i as i32 + 1);
        commands.spawn((xf, Visibility::default(), ParticleFx::new(fx, auto), MapEmitter(name.clone()), Name::new(format!("emitter:{name}"))));
        n += 1;
        on += auto as usize;
    }
    lib.spawned += n;
    info!("[fx] {n} map emitters ({on} auto-active) in {:?}", t0.elapsed());
}

#[derive(Default)]
pub struct FxTest {
    t: f32,
    spawned: Vec<Entity>,
    shot: bool,
}

/// `--test fx`: three effects next to the goat (Jet Pack thrust, fire, fireworks); every one must
/// have live particles after a second, then a screenshot.
#[allow(clippy::too_many_arguments)]
pub fn fx_scenario(
    args: Res<crate::Args>,
    time: Res<Time<Real>>,
    mut st: Local<FxTest>,
    mut commands: Commands,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    mut lib: ResMut<FxLibrary>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    goats: Query<&Transform, With<crate::goat::Goat>>,
    cams: Query<&GlobalTransform, With<Camera3d>>,
    fxs: Query<&ParticleFx>,
    mut exit: MessageWriter<AppExit>,
    mut moving: Query<&mut Transform, (With<ParticleFx>, Without<crate::goat::Goat>)>,
) {
    if args.test.as_deref() != Some("fx") {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    let fail = |why: &str| -> ! {
        println!("TEST fx FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    if st.spawned.is_empty() && t > 1.0 {
        let (Some(game), Ok(g), Ok(cam)) = (game, goats.single(), cams.single()) else {
            if t > 20.0 {
                fail("world not loaded");
            }
            return;
        };
        let right = cam.compute_transform().rotation * Vec3::X;
        let reg = game.0.clone();
        // the Blue Streak ribbon (a trail: it is moved in a circle below) and a beam
        for (i, path) in ["JetPack.Effects.JetThrust", "Goat_Effects.Effects.Effects_Fire_01", "Goat_Effects.Effects.Effects_Fireworks_01", "Goat_Effects.Effects.Effects_Bluestreak_Ribbon_01", "Space_TetherGoat.Particles.Tether_Beam_ParticleSystem"].iter().enumerate() {
            let Some(fx) = lib.get(&[&reg], path, &mut materials, &mut images) else { fail(&format!("{path} did not load")) };
            let at = g.translation + right * (i as f32 - 2.0) * 2.5 + Vec3::Y * 0.5;
            let e = commands.spawn((Transform::from_translation(at), Visibility::default(), ParticleFx::new(fx, true), Name::new(format!("fxtest:{path}")))).id();
            st.spawned.push(e);
        }
        println!("  t={t:.1} spawned {} effects", st.spawned.len());
    }
    if st.spawned.is_empty() {
        return;
    }
    if let Some(mut tr) = st.spawned.get(3).and_then(|&e| moving.get_mut(e).ok()) {
        let base = goats.single().map(|g| g.translation).unwrap_or_default() + Vec3::Y * 1.0;
        tr.translation = base + Vec3::new((t * 3.0).cos(), 0.3 * (t * 5.0).sin(), (t * 3.0).sin()) * 2.0;
    }
    if t > 2.5 && !st.shot {
        let live: Vec<usize> = st.spawned.iter().filter_map(|&e| fxs.get(e).ok()).map(|f| f.live()).collect();
        println!("  t={t:.1} live particles per effect: {live:?}");
        if live.len() != st.spawned.len() || live.contains(&0) {
            fail("an effect has no live particles");
        }
        st.shot = true;
        if let Some(p) = args.shot.clone() {
            commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(p));
        }
    }
    if t > 3.5 {
        println!("TEST fx PASS");
        exit.write(AppExit::Success);
    }
}
