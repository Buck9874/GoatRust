//! Build the Bevy scene + rapier physics world from a cooked Goat Simulator map.

use crate::convert;
use crate::physics::{PhysBody, Physics, PropImpact, Surface};
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use rapier3d::prelude as rp;
use std::collections::HashMap;
use upk::level::{Bsp, Level, MeshInstance};
use upk::mesh::StaticMesh;
use upk::registry::{ObjRef, Registry};
use upk::{material, texture};

/// Largest texture edge we upload (the game's own mip chain is used to pick it).
pub const MAX_TEX: i32 = 1024;

pub struct LoadedLevel {
    pub level: Level,
    pub spawn: Transform,
}

/// Generate a box-filtered mip chain. Returns concatenated data and level count.
fn with_mips(w: u32, h: u32, rgba: Vec<u8>) -> (Vec<u8>, u32) {
    let mut out = rgba.clone();
    let (mut cw, mut ch, mut cur) = (w, h, rgba);
    let mut levels = 1;
    while cw > 1 || ch > 1 {
        let (nw, nh) = ((cw / 2).max(1), (ch / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(cw - 1);
                        let sy = (y * 2 + dy).min(ch - 1);
                        sum += cur[((sy * cw + sx) * 4 + c) as usize] as u32;
                    }
                    next[((y * nw + x) * 4 + c) as usize] = (sum / 4) as u8;
                }
            }
        }
        out.extend_from_slice(&next);
        cur = next;
        cw = nw;
        ch = nh;
        levels += 1;
    }
    (out, levels)
}

pub fn make_image(img: texture::Image, srgb: bool) -> Image {
    let (w, h) = (img.width, img.height);
    let (data, levels) = with_mips(w, h, img.rgba);
    let fmt = if srgb { TextureFormat::Rgba8UnormSrgb } else { TextureFormat::Rgba8Unorm };
    let mut image = Image::new_uninit(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        fmt,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    let mut s = ImageSamplerDescriptor::linear();
    s.address_mode_u = ImageAddressMode::Repeat;
    s.address_mode_v = ImageAddressMode::Repeat;
    s.anisotropy_clamp = 8;
    image.sampler = ImageSampler::Descriptor(s);
    image
}

/// Decode textures on all cores.
/// Bake material graphs on all cores: material -> texture (masked ones get their mask's alpha).
fn bake_materials(graphs: &HashMap<ObjRef, material::Node>, infos: &HashMap<ObjRef, material::MatInfo>, texs: &HashMap<ObjRef, (texture::Image, bool)>) -> HashMap<ObjRef, texture::Image> {
    let items: Vec<(&ObjRef, &material::Node)> = graphs.iter().collect();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let chunk = items.len().div_ceil(threads).max(1);
    let mut out = HashMap::new();
    std::thread::scope(|sc| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|part| {
                sc.spawn(move || {
                    part.iter()
                        .filter_map(|&(&m, n)| {
                            let mut img = crate::matbake::bake(n, texs)?;
                            let info = &infos[&m];
                            if info.masked {
                                let (mt, ch, inv) = info.mask?;
                                crate::matbake::apply_mask(&mut img, &texs.get(&mt)?.0, ch, inv);
                            }
                            Some((m, img))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            out.extend(h.join().unwrap_or_default());
        }
    });
    out
}

fn decode_textures(reg: &Registry, objs: &[ObjRef]) -> HashMap<ObjRef, (texture::Image, bool)> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let chunk = objs.len().div_ceil(threads).max(1);
    let mut out = HashMap::new();
    std::thread::scope(|sc| {
        let handles: Vec<_> = objs
            .chunks(chunk)
            .map(|part| {
                sc.spawn(move || {
                    part.iter()
                        .filter_map(|&o| {
                            if !reg.class_name(o).contains("Texture2D") {
                                return None;
                            }
                            let (tp, e) = (reg.pkg(o), o.export);
                            let t = texture::Texture::parse(tp, e).ok()?;
                            let img = t.decode(tp, e, MAX_TEX).ok()?;
                            Some((o, (img, t.srgb)))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            out.extend(h.join().unwrap_or_default());
        }
    });
    out
}

struct SectionMesh {
    mesh: Handle<Mesh>,
    /// Index into StaticMesh::sections (empty sections are skipped).
    index: usize,
}

struct MeshAsset {
    sections: Vec<SectionMesh>,
    /// Bevy-space vertices (metres, unscaled) for collision.
    verts: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
}

/// A static mesh's render sections (mesh, material) and its vertices (metres), for things
/// spawned at run time (baseballs).
pub fn static_mesh_parts(reg: &Registry, r: ObjRef, meshes: &mut Assets<Mesh>) -> Option<(Vec<(Handle<Mesh>, Option<ObjRef>)>, Vec<Vec3>, i32)> {
    let m = StaticMesh::parse(reg.pkg(r), r.export).ok()?;
    let a = build_mesh(&m, meshes);
    let parts = a.sections.iter().map(|s| (s.mesh.clone(), reg.resolve(r.pkg, m.sections[s.index].material))).collect();
    Some((parts, a.verts, m.body_setup))
}

fn build_mesh(m: &StaticMesh, meshes: &mut Assets<Mesh>) -> MeshAsset {
    let verts: Vec<Vec3> = m.positions.iter().map(|&p| convert::pos(p)).collect();
    let normals: Vec<Vec3> = m.normals.iter().map(|&n| convert::dir(n).normalize_or_zero()).collect();
    let mut sections = Vec::new();
    for (si, s) in m.sections.iter().enumerate() {
        let lo = s.first_index as usize;
        let hi = (lo + s.num_faces as usize * 3).min(m.indices.len());
        if hi <= lo {
            continue;
        }
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let (mut p, mut n, mut uv, mut uv1, mut idx) = (vec![], vec![], vec![], vec![], vec![]);
        // lightmap UVs: the LightMapCoordinateIndex set, else the only one
        let lm_uvs = if m.uvs2.is_empty() { &m.uvs } else { &m.uvs2 };
        for &i in &m.indices[lo..hi] {
            let ni = *remap.entry(i).or_insert_with(|| {
                p.push(verts[i as usize].to_array());
                n.push(normals.get(i as usize).copied().unwrap_or(Vec3::Y).to_array());
                uv.push(m.uvs.get(i as usize).copied().unwrap_or([0.0, 0.0]));
                uv1.push(lm_uvs.get(i as usize).copied().unwrap_or([0.0, 0.0]));
                p.len() as u32 - 1
            });
            idx.push(ni);
        }
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, p);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, n);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1);
        mesh.insert_indices(Indices::U32(idx));
        sections.push(SectionMesh { mesh: meshes.add(mesh), index: si });
    }
    // per-poly collision only from sections with EnableCollision (water planes have none)
    let tris = m
        .sections
        .iter()
        .filter(|sec| sec.collision)
        .flat_map(|sec| {
            let a = sec.first_index as usize;
            let b = (a + sec.num_faces as usize * 3).min(m.indices.len());
            m.indices[a.min(b)..b].chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect::<Vec<_>>()
        })
        .collect();
    MeshAsset { sections, verts, tris }
}

/// Landscape component -> (positions, normals, uvs, indices) in Bevy space.
/// Intern a physical material's sound surface (footstep MaterialType, ImpactSound).
fn surface_index(reg: &Registry, physics: &mut Physics, cache: &mut HashMap<ObjRef, u16>, pm: ObjRef) -> u16 {
    *cache.entry(pm).or_insert_with(|| {
        let s = upk::physasset::surface_info(reg, pm);
        physics.surfaces.push(Surface {
            kind: s.material_type,
            impact: s.impact_sound.map(|c| reg.path(c)),
            impact_threshold: s.impact_threshold,
            impact_refire: s.impact_refire,
        });
        (physics.surfaces.len() - 1) as u16
    })
}

/// The physical material of a mesh instance: PhysMaterialOverride, else the mesh's BodySetup
/// material, else the first section material's PhysMaterial.
fn instance_physmat(reg: &Registry, parsed: &HashMap<ObjRef, StaticMesh>, inst: &MeshInstance) -> Option<ObjRef> {
    let mr = reg.resolve(0, inst.mesh)?;
    if inst.phys_material != 0 {
        if let Some(r) = reg.resolve(0, inst.phys_material) {
            return Some(r);
        }
    }
    let m = parsed.get(&mr)?;
    let body_mat = upk::physasset::body_setup(reg.pkg(mr), m.body_setup).map(|x| x.1).unwrap_or(0);
    if let Some(r) = (body_mat != 0).then(|| reg.resolve(mr.pkg, body_mat)).flatten() {
        return Some(r);
    }
    (0..m.sections.len()).find_map(|k| section_material(reg, mr, m, inst, k).and_then(|r| upk::physasset::material_physmat(reg, r)))
}

/// The material of a mesh instance's section: the component's override (map package), else the
/// mesh's own (its package).
fn section_material(reg: &Registry, mesh: ObjRef, m: &StaticMesh, inst: &MeshInstance, k: usize) -> Option<ObjRef> {
    match inst.materials.get(k).copied().filter(|&o| o != 0) {
        Some(o) => reg.resolve(0, o),
        None => reg.resolve(mesh.pkg, m.sections.get(k)?.material),
    }
}

/// LandscapeLayerInfoObject LayerName -> PhysMaterial.
fn landscape_layer_physmats(reg: &Registry) -> HashMap<String, ObjRef> {
    let p = &reg.packages[0];
    let mut out = HashMap::new();
    for i in 0..p.exports.len() {
        if p.class_name(i as i32 + 1) != "LandscapeLayerInfoObject" {
            continue;
        }
        let ps = reg.merged_props(ObjRef { pkg: 0, export: i });
        let name = match upk::registry::find(&ps, "LayerName") {
            Some((_, upk::props::Value::Name(n))) => n.to_lowercase(),
            _ => continue,
        };
        if let Some((pk, upk::props::Value::Object(o))) = upk::registry::find(&ps, "PhysMaterial") {
            if let Some(r) = reg.resolve(pk, *o) {
                out.insert(name, r);
            }
        }
    }
    out
}

fn landscape_mesh(ls: &upk::level::Landscape, c: &upk::level::LandscapeComp) -> (Vec<Vec3>, Vec<Vec3>, Vec<[f32; 2]>, Vec<u32>) {
    let n = (c.size_quads + 1) as usize;
    let sc = ls.xf.scale;
    let loc = ls.xf.location;
    let world = |i: usize, j: usize| -> [f32; 3] {
        let h = c.heights[j * n + i] as f32;
        [
            loc[0] + (c.base_x as f32 + i as f32) * sc[0],
            loc[1] + (c.base_y as f32 + j as f32) * sc[1],
            loc[2] + (h - 32768.0) / 128.0 * sc[2],
        ]
    };
    let mut pos = Vec::with_capacity(n * n);
    let mut uvs = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            let w = world(i, j);
            pos.push(convert::pos(w));
            // tile the ground texture every 8 quads
            // landscape quad coordinates; the layer material scales them per layer
            uvs.push([c.base_x as f32 + i as f32, c.base_y as f32 + j as f32]);
        }
    }
    let mut nrm = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            let l = pos[j * n + i.saturating_sub(1)];
            let r = pos[j * n + (i + 1).min(n - 1)];
            let d = pos[j.saturating_sub(1) * n + i];
            let u = pos[(j + 1).min(n - 1) * n + i];
            let mut nn = (u - d).cross(r - l).normalize_or_zero();
            if nn.y < 0.0 {
                nn = -nn;
            }
            nrm.push(nn);
        }
    }
    let mut idx = Vec::with_capacity((n - 1) * (n - 1) * 6);
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let a = (j * n + i) as u32;
            let b = a + 1;
            let cc = a + n as u32;
            let d = cc + 1;
            idx.extend_from_slice(&[a, cc, b, b, cc, d]);
        }
    }
    (pos, nrm, uvs, idx)
}

fn ensure_up_winding(pos: &[Vec3], idx: &mut [u32]) {
    // make the first triangle face up (+Y) so the default back-face culling keeps the top side
    if idx.len() >= 3 {
        let (a, b, c) = (pos[idx[0] as usize], pos[idx[1] as usize], pos[idx[2] as usize]);
        if (b - a).cross(c - a).y < 0.0 {
            for t in idx.chunks_exact_mut(3) {
                t.swap(1, 2);
            }
        }
    }
}

pub fn load(
    reg: &Registry,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    land_mats: &mut Assets<crate::landscape::LandscapeMaterial>,
    physics: &mut Physics,
) -> anyhow::Result<LoadedLevel> {
    let p = &reg.packages[0];
    let t0 = std::time::Instant::now();
    let level = upk::level::load(p)?;
    // GGRaceCheckpointAbstract.PostBeginPlay hides its mMeshToUse (races show their own copies)
    let script_hidden: std::collections::HashSet<String> = (0..p.exports.len())
        .filter(|&i| p.class_name(i as i32 + 1).contains("RaceCheckpoint") && p.obj_path(i as i32 + 1).contains(".PersistentLevel."))
        .filter_map(|i| match upk::registry::find(&reg.merged_props(ObjRef { pkg: 0, export: i }), "mMeshToUse") {
            Some((pk, upk::props::Value::Object(o))) => reg.resolve(pk, *o).map(|r| reg.pkg(r).obj_name(r.export as i32 + 1)),
            _ => None,
        })
        .collect();
    let visible: Vec<&MeshInstance> = level.instances.iter().filter(|i| !i.hidden).collect();

    // --- materials & textures ---------------------------------------------------------
    let mut mesh_assets: HashMap<ObjRef, MeshAsset> = HashMap::new();
    let mut parsed: HashMap<ObjRef, StaticMesh> = HashMap::new();
    // meshes are exports of the map or imports (Startup.upk), so resolve through the registry
    let mesh_ref = |inst: &MeshInstance| reg.resolve(0, inst.mesh);
    for inst in &visible {
        let Some(r) = mesh_ref(inst) else { continue };
        if !parsed.contains_key(&r) {
            match StaticMesh::parse(reg.pkg(r), r.export) {
                Ok(m) => {
                    parsed.insert(r, m);
                }
                Err(e) => warn!("mesh {}: {e:#}", reg.path(r)),
            }
        }
    }
    let mut mat_objs: Vec<ObjRef> = Vec::new();
    for inst in &visible {
        let Some(r) = mesh_ref(inst) else { continue };
        if let Some(m) = parsed.get(&r) {
            mat_objs.extend((0..m.sections.len()).filter_map(|k| section_material(reg, r, m, inst, k)));
        }
    }
    for ls in &level.landscapes {
        mat_objs.extend(reg.resolve(0, ls.material));
    }
    if let Some(b) = &level.bsp {
        mat_objs.extend(b.surfs.iter().filter(|s| s.material != 0).filter_map(|s| reg.resolve(0, s.material)));
    }
    mat_objs.extend(crate::destruct::material_refs(reg, &level));
    // GGExplosiveActorWreckable (cars): the wreck mesh, its materials and the self momentum
    let mut wreck_defs: HashMap<usize, (ObjRef, Vec<ObjRef>, Vec3)> = HashMap::new();
    for inst in visible.iter().filter(|i| i.actor_class.contains("Wreckable")) {
        let Some(ex) = inst.actor_export else { continue };
        let ps = reg.merged_props(ObjRef { pkg: 0, export: ex });
        let Some(mesh) = (match upk::registry::find(&ps, "mWreckageMesh") {
            Some((pk, upk::props::Value::Object(o))) => reg.resolve(pk, *o),
            _ => None,
        }) else {
            continue;
        };
        let mats: Vec<ObjRef> = match upk::registry::find(&ps, "mWreckageMaterials") {
            Some((pk, v)) => upk::material::object_array(Some(v)).into_iter().filter_map(|o| reg.resolve(pk, o)).collect(),
            None => vec![],
        };
        let self_momentum = match upk::registry::find(&ps, "mExplosiveMomentumOnSelf") {
            Some((_, upk::props::Value::Vector(v))) => Vec3::from_array(*v),
            _ => Vec3::ZERO,
        };
        if !parsed.contains_key(&mesh) {
            match StaticMesh::parse(reg.pkg(mesh), mesh.export) {
                Ok(m) => {
                    parsed.insert(mesh, m);
                }
                Err(e) => warn!("wreck mesh {}: {e:#}", reg.path(mesh)),
            }
        }
        if let Some(m) = parsed.get(&mesh) {
            mat_objs.extend(m.sections.iter().filter_map(|sec| reg.resolve(mesh.pkg, sec.material)));
        }
        mat_objs.extend(mats.iter().copied());
        wreck_defs.insert(ex, (mesh, mats, self_momentum));
    }
    mat_objs.sort();
    mat_objs.dedup();
    let infos: HashMap<ObjRef, material::MatInfo> = mat_objs.iter().map(|&r| (r, material::resolve_in(reg, r))).collect();
    let mut tex_objs: Vec<ObjRef> = infos.values().filter_map(|i| i.diffuse).collect();
    // DiffuseColor graphs beyond one plain texture are baked (matbake); not for translucent
    // materials, and masked ones only when their OpacityMask source is known
    let graphs: HashMap<ObjRef, material::Node> = if std::env::var_os("GOAT_NO_BAKE").is_some() {
        HashMap::new()
    } else {
        infos
            .iter()
            .filter(|(_, i)| !i.translucent && (!i.masked || i.mask.is_some()))
            .filter_map(|(&r, _)| material::diffuse_graph(reg, r).filter(crate::matbake::needs_bake).map(|n| (r, n)))
            .collect()
    };
    for (r, n) in &graphs {
        n.textures(&mut tex_objs);
        tex_objs.extend(infos[r].mask.map(|m| m.0));
    }
    // normal maps (GOAT_NO_NORMALMAPS=1 leaves them out)
    let normal_maps = std::env::var_os("GOAT_NO_NORMALMAPS").is_none();
    let normal_texs: std::collections::HashSet<ObjRef> = if normal_maps { infos.values().filter_map(|i| i.normal).collect() } else { Default::default() };
    tex_objs.extend(normal_texs.iter().copied());
    // landscape paint layers: (layer name, texture, tiling) per landscape
    let land_layers: Vec<Vec<(String, ObjRef, f32)>> = level
        .landscapes
        .iter()
        .map(|ls| reg.resolve(0, ls.material).map(|m| material::landscape_layers(reg, m)).unwrap_or_default())
        .collect();
    tex_objs.extend(land_layers.iter().flatten().map(|l| l.1));
    tex_objs.sort();
    tex_objs.dedup();
    let t1 = std::time::Instant::now();
    let decoded = decode_textures(reg, &tex_objs);
    info!("decoded {}/{} textures in {:?}", decoded.len(), tex_objs.len(), t1.elapsed());
    let t2 = std::time::Instant::now();
    let baked = bake_materials(&graphs, &infos, &decoded);
    info!("baked {}/{} material graphs in {:?}", baked.len(), graphs.len(), t2.elapsed());
    let baked_handles: HashMap<ObjRef, Handle<Image>> = baked.into_iter().map(|(m, img)| (m, images.add(make_image(img, true)))).collect();
    let mut tex_handles: HashMap<ObjRef, Handle<Image>> = HashMap::new();
    for (o, (mut img, srgb)) in decoded {
        // UE3 normal maps are DirectX style (green down): Bevy wants green up
        if normal_texs.contains(&o) {
            for px in img.rgba.chunks_exact_mut(4) {
                px[1] = 255 - px[1];
            }
        }
        tex_handles.insert(o, images.add(make_image(img, srgb)));
    }
    let mut mat_handles: HashMap<ObjRef, Handle<StandardMaterial>> = HashMap::new();
    for (&o, info) in &infos {
        let tex = baked_handles.get(&o).cloned().or_else(|| info.diffuse.and_then(|t| tex_handles.get(&t).cloned()));
        let tint = match (tex.is_some(), info.color) {
            (false, Some(c)) => Color::linear_rgb(c[0].min(4.0), c[1].min(4.0), c[2].min(4.0)),
            _ => Color::WHITE,
        };
        let base = if tex.is_none() && info.color.is_none() { Color::srgb(0.6, 0.6, 0.6) } else { tint };
        let alpha_mode = if info.masked { AlphaMode::Mask(0.33) } else if info.translucent { AlphaMode::Blend } else { AlphaMode::Opaque };
        // translucent (glass, water): the Opacity input isn't evaluated; use a glass-like alpha
        // SpecularColor / SpecularPower (Phong) as reflectance and roughness, when connected
        let (spec_rough, spec_refl) = match info.spec {
            Some(c) => {
                let power = info.spec_power.unwrap_or(15.0).max(1.0);
                let lum = (c[0] * 0.3 + c[1] * 0.59 + c[2] * 0.11).clamp(0.0, 1.0);
                ((2.0 / (power + 2.0)).sqrt().clamp(0.25, 0.95), (lum * 0.5).clamp(0.05, 0.5))
            }
            None => (0.85, 0.2),
        };
        let (base, roughness, reflectance) = if info.translucent && !info.masked { (base.with_alpha(0.3), 0.08, 0.5) } else { (base, spec_rough, spec_refl) };
        let normal_map = info.normal.filter(|n| normal_texs.contains(n)).and_then(|n| tex_handles.get(&n).cloned());
        mat_handles.insert(
            o,
            materials.add(StandardMaterial {
                base_color: base,
                base_color_texture: tex,
                perceptual_roughness: roughness,
                reflectance,
                normal_map_texture: normal_map,
                alpha_mode,
                double_sided: info.two_sided || info.masked,
                cull_mode: if info.two_sided || info.masked { None } else { Some(bevy::render::render_resource::Face::Back) },
                lightmap_exposure: crate::lightmap::exposure(),
                ..default()
            }),
        );
    }
    let fallback = materials.add(StandardMaterial {
        base_color: Color::srgb(0.6, 0.6, 0.6),
        perceptual_roughness: 0.9,
        lightmap_exposure: crate::lightmap::exposure(),
        ..default()
    });

    // --- baked lighting (static components' FLightMap2D) -----------------------------------
    let lightmaps = if std::env::var("GOAT_LIGHTMAPS").is_ok_and(|v| v == "0") {
        crate::lightmap::Lightmaps::default()
    } else {
        let t = std::time::Instant::now();
        let mut lms: Vec<upk::level::LightMap2D> = visible.iter().filter(|i| !i.dynamic).filter_map(|i| i.lightmap).collect();
        lms.extend(level.landscapes.iter().flat_map(|l| l.comps.iter().filter_map(|c| c.lightmap)));
        lms.extend(level.bsp.iter().flat_map(|b| b.node_lightmaps.values().copied()));
        let l = crate::lightmap::bake(p, &lms, images);
        info!("lightmaps: {} components, {:.1} Mtexels on {} pages in {:?}", lms.len(), l.texels as f64 / 1e6, l.pages(), t.elapsed());
        l
    };

    let tm = std::time::Instant::now();
    for (&k, m) in &parsed {
        mesh_assets.insert(k, build_mesh(m, meshes));
    }
    info!("built {} meshes in {:?}", mesh_assets.len(), tm.elapsed());
    let tm = std::time::Instant::now();

    // --- landscape ---------------------------------------------------------------------
    let mut surface_cache: HashMap<ObjRef, u16> = HashMap::new();
    let layer_physmats = landscape_layer_physmats(reg);
    for (li, ls) in level.landscapes.iter().enumerate() {
        // footstep surface of each paint layer (weight channel order)
        let layer_surface: Vec<Option<u16>> = ls
            .layers
            .iter()
            .map(|n| layer_physmats.get(&n.to_lowercase()).map(|&pm| surface_index(reg, physics, &mut surface_cache, pm)))
            .collect();
        let mat = reg.resolve(0, ls.material).and_then(|r| mat_handles.get(&r)).cloned().unwrap_or(fallback.clone());
        // layer textures in weight-channel order
        let white = images.add(Image::default());
        let mut tiling = [1.0f32; 4];
        let mut layer_tex: Vec<Handle<Image>> = vec![white.clone(); 4];
        let mut have_layers = 0;
        for (k, name) in ls.layers.iter().enumerate().take(4) {
            if let Some((_, t, tile)) = land_layers[li].iter().find(|(n, _, _)| n.eq_ignore_ascii_case(name)) {
                if let Some(h) = tex_handles.get(t) {
                    layer_tex[k] = h.clone();
                    tiling[k] = *tile;
                    have_layers += 1;
                }
            }
        }
        let land_mat = (have_layers > 0).then(|| {
            land_mats.add(crate::landscape::LandscapeMaterial {
                base: StandardMaterial { perceptual_roughness: 0.92, reflectance: 0.15, lightmap_exposure: crate::lightmap::exposure(), ..default() },
                extension: crate::landscape::LandscapeLayers {
                    tiling: Vec4::from_array(tiling),
                    layer0: layer_tex[0].clone(),
                    layer1: layer_tex[1].clone(),
                    layer2: layer_tex[2].clone(),
                    layer3: layer_tex[3].clone(),
                },
            })
        });
        for c in &ls.comps {
            let (pos, nrm, uvs, mut idx) = landscape_mesh(ls, c);
            ensure_up_winding(&pos, &mut idx);
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos.iter().map(|v| v.to_array()).collect::<Vec<_>>());
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm.iter().map(|v| v.to_array()).collect::<Vec<_>>());
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            // lightmap UVs: vertex (x, y) of the component through its patch-expanded lightmap
            let landscape_lm = c.lightmap.as_ref().and_then(|lm| lightmaps.get(lm));
            if landscape_lm.is_some() {
                let (s, b) = upk::level::landscape_lightmap_scale_bias(c.size_quads, ls.lighting_res);
                let n = (c.size_quads + 1) as usize;
                let uv1: Vec<[f32; 2]> = (0..n * n).map(|k| [(k % n) as f32 * s + b, (k / n) as f32 * s + b]).collect();
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1);
            }
            if c.weights.len() == pos.len() {
                let w: Vec<[f32; 4]> = c.weights.iter().map(|w| [w[0] as f32 / 255.0, w[1] as f32 / 255.0, w[2] as f32 / 255.0, w[3] as f32 / 255.0]).collect();
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, w);
            }
            mesh.insert_indices(Indices::U32(idx.clone()));
            let mut e = commands.spawn((Mesh3d(meshes.add(mesh)), Transform::default(), Name::new("Landscape")));
            if let Some(lm) = landscape_lm {
                e.insert(lm);
            }
            match (&land_mat, c.weights.len() == pos.len()) {
                (Some(lm), true) => e.insert(MeshMaterial3d(lm.clone())),
                _ => e.insert(MeshMaterial3d(mat.clone())),
            };
            let tris: Vec<[u32; 3]> = idx.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
            let v: Vec<rp::Vector> = pos.iter().map(|&v| convert::to_rapier(v)).collect();
            // the dominant paint layer of each triangle decides its surface
            let tri_surface: Vec<u16> = tris
                .iter()
                .map(|t| {
                    let mut sum = [0u32; 4];
                    if c.weights.len() == pos.len() {
                        for &vi in t {
                            for (k, w) in c.weights[vi as usize].iter().enumerate() {
                                sum[k] += *w as u32;
                            }
                        }
                    }
                    let best = (0..4).max_by_key(|&k| sum[k]).unwrap_or(0);
                    layer_surface.get(best).copied().flatten().unwrap_or(u16::MAX)
                })
                .collect();
            if let Ok(col) = rp::ColliderBuilder::trimesh(v, tris) {
                let ch = physics.world.insert_collider(col.friction(0.8), None);
                if layer_surface.iter().any(|s| s.is_some()) {
                    physics.landscape_surface.insert(ch, tri_surface);
                }
            }
        }
    }

    info!("landscape in {:?}", tm.elapsed());
    let tm = std::time::Instant::now();
    // --- volumes (GGTrampolineVolume family) -----------------------------------------------
    let mut vols = crate::physics::volumes::Volumes::default();
    // class default objects cooked into the map (Default__X or pkg.Default__X), by object name
    let map_defaults: HashMap<String, usize> = {
        let map = &reg.packages[0];
        let mut m = HashMap::new();
        for i in 0..map.exports.len() {
            let path = map.obj_path(i as i32 + 1);
            let name = path.rsplit('.').next().unwrap_or(&path);
            if name.starts_with("Default__") {
                m.entry(name.to_string()).or_insert(i);
            }
        }
        m
    };
    for v in &level.volumes {
        // the actor, then its content class's default object (cooked into the map with a package
        // prefix, so the archetype merge misses it), then GGTrampolineVolume's defaults
        let mut layers = vec![reg.merged_props(ObjRef { pkg: 0, export: v.export })];
        if let Some(&d) = map_defaults.get(&format!("Default__{}", v.class)) {
            layers.push(reg.merged_props(ObjRef { pkg: 0, export: d }));
        }
        if let Some(d) = reg.find_path("Default__GGTrampolineVolume") {
            layers.push(reg.merged_props(d));
        }
        let lookup = |k: &str| layers.iter().find_map(|l| upk::registry::find(l, k).map(|(pk, v)| (pk, v.clone())));
        let props = &layers[0];
        let float = |k: &str, d: f32| match lookup(k) {
            Some((_, upk::props::Value::Float(f))) => f,
            _ => d,
        };
        // BlockingVolume (invisible walls), PhysicsVolume (ZoneVelocity currents), GravityVolume
        // (GravityZ), LadderVolume (WallDir)
        let kind = if v.class.contains("Trampoline") || upk::registry::find(props, "mBounceFactor").is_some() {
            None
        } else if v.class == "BlockingVolume" || v.class.ends_with("BlockingVolume") {
            Some(crate::physics::volumes::Kind::Blocking)
        } else if v.class.contains("WaterVolume") || v.class.contains("CoinVolume") {
            Some(crate::physics::volumes::Kind::Water)
        } else if v.class == "PostProcessVolume" {
            if matches!(lookup("bEnabled"), Some((_, upk::props::Value::Bool(false)))) {
                continue;
            }
            let priority = match lookup("Priority") {
                Some((_, upk::props::Value::Int(p))) => p,
                Some((_, upk::props::Value::Float(p))) => p as i32,
                _ => 0,
            };
            Some(crate::physics::volumes::Kind::PostProcess { priority })
        } else if v.class.contains("Ladder") {
            let dir = match lookup("WallDir") {
                Some((_, upk::props::Value::Rotator(r))) => r,
                _ => [0, 0, 0],
            };
            Some(crate::physics::volumes::Kind::Ladder { wall_dir: convert::rotator_dir(dir) })
        } else if v.class.contains("GravityVolume") || v.class.contains("PhysicsVolume") {
            let gz = match lookup("GravityZ") {
                Some((_, upk::props::Value::Float(f))) => Some(f),
                _ => None,
            };
            let zv = match lookup("ZoneVelocity") {
                Some((_, upk::props::Value::Vector(z))) => convert::dir(z) * convert::S,
                _ => Vec3::ZERO,
            };
            let enabled = !matches!(lookup("bEnabled"), Some((_, upk::props::Value::Bool(false))));
            // GravityVolume's GravityZ only counts when set; plain PhysicsVolumes keep gravity
            let gz = if v.class.contains("Gravity") { gz } else { None };
            if gz.is_none() && zv == Vec3::ZERO && float("RigidBodyDamping", 0.0) <= 0.0 {
                continue;
            }
            let priority = match lookup("Priority") {
                Some((_, upk::props::Value::Int(p))) => p,
                _ => 0,
            };
            let damping = float("RigidBodyDamping", 0.0);
            Some(crate::physics::volumes::Kind::Physics { gravity_z: gz, zone_velocity: zv, enabled, priority, damping })
        } else {
            continue;
        };
        if let Some(kind) = kind {
            let xf = convert::xform(&v.xf);
            let pose = rp::Pose::from_parts(convert::to_rapier(xf.translation), convert::quat_to_rapier(xf.rotation));
            let hulls: Vec<(rp::Pose, rp::SharedShape)> =
                crate::shapes::colliders(&v.shapes, xf.scale).into_iter().map(|(local, s)| (pose * local, s)).collect();
            if hulls.is_empty() {
                continue;
            }
            if matches!(kind, crate::physics::volumes::Kind::Blocking) {
                for (p, s) in &hulls {
                    physics.world.insert_collider(rp::ColliderBuilder::new(s.clone()).position(*p).friction(0.6), None);
                }
            }
            vols.others.push(crate::physics::volumes::Volume { name: v.name.clone(), kind, hulls });
            continue;
        }
        let dir = match lookup("mBounceDirection") {
            Some((_, upk::props::Value::Rotator(r))) => r,
            _ => [16384, 0, 0],
        };
        let sound = match lookup("mBounceSound") {
            Some((pk, upk::props::Value::Object(o))) => reg.resolve(pk, o),
            _ => None,
        };
        let name = match lookup("mDisplayName") {
            Some((_, upk::props::Value::Str(s))) if s != "N/A" => s,
            _ => v.class.clone(),
        };
        let xf = convert::xform(&v.xf);
        let pose = rp::Pose::from_parts(convert::to_rapier(xf.translation), convert::quat_to_rapier(xf.rotation));
        let hulls: Vec<(rp::Pose, rp::SharedShape)> =
            crate::shapes::colliders(&v.shapes, xf.scale).into_iter().map(|(local, s)| (pose * local, s)).collect();
        if hulls.is_empty() {
            continue;
        }
        let cue = sound.map(|c| {
            let key = reg.path(c);
            if !vols.sounds.contains_key(&key) {
                let waves = upk::sound::cue_waves(reg, c);
                let ogg: Vec<Vec<u8>> = waves.into_iter().filter_map(|w| upk::sound::wave(reg, w)).map(|w| w.ogg).collect();
                vols.sounds.insert(key.clone(), ogg);
            }
            key
        });
        vols.trampolines.push(crate::physics::volumes::Trampoline {
            name: v.name.clone(),
            display: name,
            normal: convert::rotator_dir(dir),
            bounce_factor: float("mBounceFactor", 300.0),
            max_bounce: float("mMaxBounceVelocity", 2500.0),
            hulls,
            sound: cue,
            is_trampoline: v.class.contains("VolumeTrampoline"),
        });
    }
    {
        let mut kinds: HashMap<&str, usize> = HashMap::new();
        for t in &vols.trampolines {
            *kinds.entry(t.display.as_str()).or_default() += 1;
            if std::env::var_os("GOAT_TRACE").is_some() {
                println!("    trampoline {} sound {:?} factor {} max {} normal {:.2?}", t.name, t.sound, t.bounce_factor, t.max_bounce, t.normal);
            }
        }
        info!("volumes: {} in the level, {} trampolines {:?}, {} others (blocking/physics/gravity/ladder)", level.volumes.len(), vols.trampolines.len(), kinds, vols.others.len());
    }
    commands.insert_resource(vols);
    info!("volumes in {:?}", tm.elapsed());

    // --- BSP ---------------------------------------------------------------------------------
    if let Some(bsp) = &level.bsp {
        // one mesh per material and lightmap atlas; texture coordinates are
        // ((P - base) . U|V) / 128 like UModel, lightmap UVs ShadowTexCoord * scale + bias
        type Group = (Vec<Vec3>, Vec<Vec3>, Vec<[f32; 2]>, Vec<[f32; 2]>, Vec<u32>);
        let mut groups: HashMap<(i32, Option<(i32, i32)>), Group> = HashMap::new();
        // the atlas-wide lightmap of each group (UVs already include the element's scale/bias)
        let mut group_lm: HashMap<(i32, i32), upk::level::LightMap2D> = HashMap::new();
        let mut col_pts: Vec<rp::Vector> = Vec::new();
        let mut col_tris: Vec<[u32; 3]> = Vec::new();
        for (pi, (si, pts)) in bsp.polys.iter().enumerate() {
            let s = &bsp.surfs[*si];
            let n = convert::pos(s.normal).normalize_or_zero();
            let ps: Vec<Vec3> = pts.iter().map(|&q| convert::pos(q)).collect();
            // fan triangles, wound so they face along the surface normal
            let tri_n = (ps[1] - ps[0]).cross(ps[2] - ps[0]);
            let flip = tri_n.dot(n) < 0.0;
            if Bsp::visible(s) {
                let lm = bsp.poly_nodes.get(pi).and_then(|nd| bsp.node_lightmaps.get(nd)).filter(|lm| lightmaps.get(lm).is_some());
                let atlas = lm.map(|lm| (lm.textures[0], lm.textures[1]));
                if let (Some(a), Some(lm)) = (atlas, lm) {
                    group_lm.entry(a).or_insert(upk::level::LightMap2D { coord_scale: [1.0, 1.0], coord_bias: [0.0, 0.0], ..*lm });
                }
                let g = groups.entry((s.material, atlas)).or_default();
                let b0 = g.0.len() as u32;
                for (k, q) in pts.iter().enumerate() {
                    let rel = [q[0] - s.base[0], q[1] - s.base[1], q[2] - s.base[2]];
                    let dot = |a: [f32; 3]| rel[0] * a[0] + rel[1] * a[1] + rel[2] * a[2];
                    g.0.push(ps[k]);
                    g.1.push(n);
                    g.2.push([dot(s.texture_u) / 128.0, dot(s.texture_v) / 128.0]);
                    let st = bsp.poly_lm_uvs.get(pi).and_then(|v| v.get(k)).copied().unwrap_or([0.0, 0.0]);
                    g.3.push(match lm {
                        Some(lm) => [st[0] * lm.coord_scale[0] + lm.coord_bias[0], st[1] * lm.coord_scale[1] + lm.coord_bias[1]],
                        None => [0.0, 0.0],
                    });
                }
                for k in 1..pts.len() as u32 - 1 {
                    if flip { g.4.extend([b0, b0 + k + 1, b0 + k]) } else { g.4.extend([b0, b0 + k, b0 + k + 1]) }
                }
            }
            if Bsp::solid(s) {
                let c0 = col_pts.len() as u32;
                col_pts.extend(ps.iter().map(|&q| convert::to_rapier(q)));
                for k in 1..ps.len() as u32 - 1 {
                    col_tris.push([c0, c0 + k, c0 + k + 1]);
                }
            }
        }
        let mut n_tris = 0;
        let mut n_lit = 0;
        for ((mat, atlas), (pos, nrm, uvs, uv1, idx)) in groups {
            n_tris += idx.len() / 3;
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos.iter().map(|v| v.to_array()).collect::<Vec<_>>());
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm.iter().map(|v| v.to_array()).collect::<Vec<_>>());
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1);
            mesh.insert_indices(Indices::U32(idx));
            let m = reg.resolve(0, mat).and_then(|r| mat_handles.get(&r)).cloned().unwrap_or(fallback.clone());
            let mut e = commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(m), Transform::default(), Name::new("BSP")));
            if let Some(lm) = atlas.and_then(|a| group_lm.get(&a)).and_then(|lm| lightmaps.get(lm)) {
                e.insert(lm);
                n_lit += 1;
            }
        }
        info!("BSP: {n_lit} lightmapped meshes, {} nodes with lightmaps", bsp.node_lightmaps.len());
        if !col_tris.is_empty() {
            if let Ok(col) = rp::ColliderBuilder::trimesh(col_pts, col_tris) {
                physics.world.insert_collider(col.friction(0.8), None);
            }
        }
        info!("BSP: {} polygons, {} surfaces, {n_tris} triangles", bsp.polys.len(), bsp.surfs.len());
    }

    // --- static and dynamic mesh actors ---------------------------------------------------
    // Collision shapes are shared between instances with the same mesh and scale.
    let mut static_shapes: HashMap<(ObjRef, [i32; 3]), Option<rp::SharedShape>> = HashMap::new();
    let mut hull_shapes: HashMap<(ObjRef, [i32; 3]), Option<Vec<(rp::Pose, rp::SharedShape)>>> = HashMap::new();
    let (mut n_static, mut n_dyn) = (0, 0);
    // section meshes under normal-mapped materials get tangents (once each)
    let mut tangent_meshes: std::collections::HashSet<AssetId<Mesh>> = Default::default();
    // prop bodies with their actor (destructible KActors, explosives)
    let mut actor_bodies: Vec<(rp::RigidBodyHandle, usize)> = Vec::new();
    let mut wrecks = HashMap::new();
    for inst in &visible {
        let Some(mr) = mesh_ref(inst) else { continue };
        let Some(asset) = mesh_assets.get(&mr) else { continue };
        let actor = convert::xform(&inst.actor_xf);
        let comp = convert::local_xform(&inst.comp_xf);
        let world = match &inst.matrix {
            Some(m) => convert::matrix(m),
            None => actor.mul_transform(comp),
        };
        let scale_key = [(world.scale.x * 1000.0) as i32, (world.scale.y * 1000.0) as i32, (world.scale.z * 1000.0) as i32];
        let key = (mr, scale_key);
        let pose = rp::Pose::from_parts(convert::to_rapier(world.translation), convert::quat_to_rapier(world.rotation));
        let mut body = None;
        if inst.dynamic {
            // the mesh's authored RB_BodySetup collision, else a hull of the render mesh
            let parts = hull_shapes.entry(key).or_insert_with(|| {
                let authored = parsed
                    .get(&mr)
                    .and_then(|m| upk::physasset::body_setup(reg.pkg(mr), m.body_setup))
                    .map(|(shapes, _, _)| crate::shapes::colliders(&shapes, world.scale))
                    .unwrap_or_default();
                if !authored.is_empty() {
                    return Some(authored);
                }
                let pts: Vec<rp::Vector> = asset.verts.iter().map(|&v| convert::to_rapier(v * world.scale)).collect();
                rp::SharedShape::convex_hull(&pts).map(|s| vec![(rp::Pose::IDENTITY, s)])
            });
            if let Some(parts) = parts.clone() {
                // PhysMaterialOverride (map), else the BodySetup's material (the mesh's package)
                let mat_ref = if inst.phys_material != 0 {
                    reg.resolve(0, inst.phys_material)
                } else {
                    parsed
                        .get(&mr)
                        .and_then(|m| upk::physasset::body_setup(reg.pkg(mr), m.body_setup))
                        .and_then(|x| (x.1 != 0).then(|| reg.resolve(mr.pkg, x.1)).flatten())
                };
                // most Goat Simulator physical materials use Density 65; use it for props without one
                let (density, friction, restitution) = match mat_ref {
                    Some(r) => upk::physasset::phys_material(reg.pkg(r), r.export as i32 + 1),
                    None => (65.0, 0.7, 0.3),
                };
                // dormant until touched (see Physics::wake)
                let rb = rp::RigidBodyBuilder::fixed().pose(pose).linear_damping(0.05).angular_damping(0.2);
                let h = physics.world.insert_body(rb);
                physics.dormant.insert(h);
                let surface = instance_physmat(reg, &parsed, inst).map(|pm| surface_index(reg, physics, &mut surface_cache, pm));
                for (local, shape) in parts {
                    let ch = physics.world.insert_collider(
                        rp::ColliderBuilder::new(shape)
                            .position(local)
                            // UE3 densities are per PhysX cubic metre; keep tiny props from weighing nothing
                            .density(density.max(10.0))
                            .friction(friction.min(1.5))
                            .restitution(restitution.min(0.8)),
                        Some(h),
                    );
                    if let Some(s) = surface {
                        physics.collider_surface.insert(ch, s);
                    }
                }
                if let Some(s) = surface.filter(|&s| physics.surfaces[s as usize].impact.is_some()) {
                    physics.impacts.insert(h, PropImpact { surface: s, last: -10.0, prev_vel: Vec3::ZERO });
                }
                physics.names.insert(h, inst.actor.clone());
                // score + name for tricks: physical material property, else the material's PhysMaterial
                let mut pm = mat_ref;
                let mut info = pm.and_then(|r| upk::physasset::score_info(reg, r));
                if info.is_none() {
                    if let Some(m) = parsed.get(&mr) {
                        for k in 0..m.sections.len() {
                            pm = section_material(reg, mr, m, inst, k).and_then(|r| upk::physasset::material_physmat(reg, r));
                            info = pm.and_then(|p| upk::physasset::score_info(reg, p));
                            if info.is_some() {
                                break;
                            }
                        }
                    }
                }
                if let Some(r) = mat_ref.or(pm) {
                    physics.physmat.insert(h, r);
                }
                if let Some(info) = info {
                    physics.prop_info.insert(h, info);
                    // GGGoat.CanCauseGoatToTrip
                    if pm.is_some_and(|p| upk::physasset::trips_goat(reg, p)) {
                        physics.trip.insert(h);
                    }
                }
                body = Some(h);
                n_dyn += 1;
            }
        } else if inst.collide && (!asset.tris.is_empty() || parsed.get(&mr).is_some_and(|m| m.body_setup != 0)) {
            let shape = static_shapes.entry(key).or_insert_with(|| {
                if asset.tris.is_empty() {
                    // no per-poly collision: the mesh's simple (RB_BodySetup) collision
                    let parts = parsed
                        .get(&mr)
                        .and_then(|m| upk::physasset::body_setup(reg.pkg(mr), m.body_setup))
                        .map(|(shapes, _, _)| crate::shapes::colliders(&shapes, world.scale))
                        .unwrap_or_default();
                    return (!parts.is_empty()).then(|| rp::SharedShape::compound(parts));
                }
                let pts: Vec<rp::Vector> = asset.verts.iter().map(|&v| convert::to_rapier(v * world.scale)).collect();
                rp::SharedShape::trimesh(pts, asset.tris.clone()).ok()
            });
            if let Some(shape) = shape.clone() {
                let ch = physics.world.insert_collider(rp::ColliderBuilder::new(shape).position(pose).friction(0.8), None);
                if inst.actor_class.contains("InterpActor") {
                    physics.movers.entry(inst.actor.to_ascii_lowercase()).or_default().push((ch, world.translation, world.rotation));
                }
                if let Some(pm) = instance_physmat(reg, &parsed, inst) {
                    let s = surface_index(reg, physics, &mut surface_cache, pm);
                    physics.collider_surface.insert(ch, s);
                }
                n_static += 1;
            }
        }
        let hidden_by_script = script_hidden.contains(&inst.actor);
        let mut e = commands.spawn((world, if hidden_by_script { Visibility::Hidden } else { Visibility::default() }, Name::new(inst.actor.clone())));
        if hidden_by_script {
            e.insert(crate::race::CheckpointMesh(inst.actor.clone()));
        }
        if let Some(h) = body {
            e.insert(PhysBody { handle: h, scale: world.scale });
            if let Some(ex) = inst.actor_export {
                physics.actors.insert(h, (ex, e.id()));
                actor_bodies.push((h, ex));
                if let Some((wm, wmats, mom)) = wreck_defs.get(&ex) {
                    if let (Some(wa), Some(wp)) = (mesh_assets.get(wm), parsed.get(wm)) {
                        let parts = wa
                            .sections
                            .iter()
                            .map(|sec| {
                                let mat = wmats.get(sec.index).copied().or_else(|| reg.resolve(wm.pkg, wp.sections[sec.index].material));
                                (sec.mesh.clone(), mat.and_then(|m| mat_handles.get(&m)).cloned().unwrap_or(fallback.clone()))
                            })
                            .collect();
                        wrecks.insert(h, (parts, *mom));
                    }
                }
            }
        }
        let lightmap = if inst.dynamic { None } else { inst.lightmap.as_ref().and_then(|lm| lightmaps.get(lm)) };
        e.with_children(|c| {
            for (k, s) in asset.sections.iter().enumerate() {
                let o = parsed.get(&mr).and_then(|m| section_material(reg, mr, m, inst, s.index));
                let mat = o.and_then(|o| mat_handles.get(&o)).cloned().unwrap_or(fallback.clone());
                if o.and_then(|o| infos.get(&o)).is_some_and(|i| i.normal.is_some_and(|n| normal_texs.contains(&n)) || (i.spec.is_some() && lightmap.is_some())) {
                    tangent_meshes.insert(s.mesh.id());
                }
                let mut sec = c.spawn((Mesh3d(s.mesh.clone()), MeshMaterial3d(mat)));
                if let Some(lm) = &lightmap {
                    sec.insert(lm.clone());
                }
                // glass and water don't cast the sun's shadow (they don't in the baked shadows either)
                if o.and_then(|o| infos.get(&o)).is_some_and(|i| i.translucent && !i.masked) {
                    sec.insert(bevy::light::NotShadowCaster);
                }
            }
        });
    }
    info!(
        "level: {} instances ({} static colliders, {} dynamic), {} meshes, {} landscapes in {:?}",
        visible.len(),
        n_static,
        n_dyn,
        mesh_assets.len(),
        level.landscapes.len(),
        t0.elapsed()
    );
    let destructibles = crate::destruct::load(reg, &level, &actor_bodies, commands, meshes, &mat_handles, &fallback, physics);
    commands.insert_resource(destructibles);
    let tn = std::time::Instant::now();
    let mut n_tan = 0;
    for id in &tangent_meshes {
        if let Some(mut m) = meshes.get_mut(*id) {
            if m.generate_tangents().is_ok() {
                n_tan += 1;
            }
        }
    }
    info!("tangents for {n_tan} normal-mapped meshes in {:?}", tn.elapsed());
    commands.insert_resource(crate::look::load(reg));
    info!("wreckable explosives: {}", wrecks.len());
    commands.insert_resource(crate::explosive::Wrecks(wrecks));
    let mut spawn = level.player_starts.first().map(convert::xform).unwrap_or_default();
    // GOAT_SPAWN=x,y,z (Unreal units) overrides the spawn point, for looking at a spot
    if let Some(v) = std::env::var("GOAT_SPAWN").ok().and_then(|s| {
        let f: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (f.len() == 3).then(|| [f[0], f[1], f[2]])
    }) {
        spawn.translation = convert::pos(v);
    }
    Ok(LoadedLevel { level, spawn })
}
