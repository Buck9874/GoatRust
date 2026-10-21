//! Extract renderable/physical content from a cooked map package.

use crate::material::object_array;
use crate::package::Package;
use crate::props::{self, Prop, Value};
use crate::reader::Reader;
use anyhow::Result;

/// UE3 actor/component transform (Unreal units, left-handed, Z up).
#[derive(Debug, Clone, Copy)]
pub struct Xform {
    pub location: [f32; 3],
    /// Pitch, Yaw, Roll in Unreal rotation units (65536 = 360 degrees).
    pub rotation: [i32; 3],
    pub scale: [f32; 3],
}

impl Default for Xform {
    fn default() -> Self {
        Self { location: [0.0; 3], rotation: [0; 3], scale: [1.0; 3] }
    }
}

#[derive(Debug, Clone)]
pub struct MeshInstance {
    pub actor: String,
    pub actor_class: String,
    /// StaticMesh object index in the package (> 0 export, < 0 import from another package,
    /// e.g. Startup.upk; resolve it through the registry).
    pub mesh: i32,
    pub actor_xf: Xform,
    pub comp_xf: Xform,
    /// Material overrides per section (object indices, 0 = use mesh default).
    pub materials: Vec<i32>,
    /// Simulated rigid body (KActor family).
    pub dynamic: bool,
    pub hidden: bool,
    pub collide: bool,
    /// PhysMaterialOverride on the component (0 = use the mesh's BodySetup material).
    pub phys_material: i32,
    /// World matrix (Unreal row-vector convention, row 3 = translation) for instances that are
    /// placed by a matrix instead of actor + component transforms (foliage).
    pub matrix: Option<[f32; 16]>,
    /// Baked lighting of the component's first LOD.
    pub lightmap: Option<LightMap2D>,
    /// The owning actor's export (0-based), for class-specific settings (explosives, wrecks,
    /// GGKActor.mApexActor).
    pub actor_export: Option<usize>,
}

/// An APEX destructible actor (GGApexDestructibleActor) and its ApexDestructibleAsset.
#[derive(Debug, Clone)]
pub struct ApexPlacement {
    pub export: usize,
    pub name: String,
    pub xf: Xform,
    /// ApexDestructibleAsset object index in the package.
    pub asset: i32,
}

/// FLightMap2D: a directional lightmap. Textures [0] NormalizedAverageColor and
/// [1] DirectionalMaxComponent (object indices, LightMapTexture2D atlases), [2] unused; each is
/// multiplied by its scale vector. Lightmap UV = mesh lightmap UV * coord_scale + coord_bias.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LightMap2D {
    pub textures: [i32; 3],
    pub scales: [[f32; 3]; 3],
    pub coord_scale: [f32; 2],
    pub coord_bias: [f32; 2],
    /// The component's first ShadowMap2D (the dominant light's static shadows), 0 = none.
    pub shadow: i32,
}

/// The lightmap in a (Instanced)StaticMeshComponent's native data: `[i32 LODData count]`, then
/// per LOD `[ShadowMaps: n x obj][ShadowVertexBuffers: n x obj][i32 LightMap type]`, type 2 =
/// FLightMap2D `[LightGuids: n x 16 bytes][3 x (obj texture, FVector scale)][FVector2D
/// CoordinateScale][FVector2D CoordinateBias]`.
pub fn component_lightmap(native: &[u8]) -> Option<LightMap2D> {
    let mut r = Reader::new(native);
    let read = |r: &mut Reader| -> Result<Option<LightMap2D>> {
        if r.i32()? < 1 {
            return Ok(None);
        }
        let mut shadow = 0;
        for k in 0..2 {
            let n = r.i32()?;
            if !(0..4096).contains(&n) {
                return Ok(None);
            }
            if k == 0 && n > 0 {
                shadow = r.i32()?;
                r.skip((n as usize - 1) * 4)?;
            } else {
                r.skip(n as usize * 4)?;
            }
        }
        Ok(read_lightmap(r)?.map(|mut lm| {
            lm.shadow = shadow;
            lm
        }))
    };
    read(&mut r).ok().flatten()
}

/// A serialized FLightMap pointer: `[i32 type]`, type 2 = FLightMap2D (see [`component_lightmap`]).
/// Index 2 is the simple (low quality) coefficient, whose texture the cooker strips.
pub fn read_lightmap(r: &mut Reader) -> Result<Option<LightMap2D>> {
    if r.i32()? != 2 {
        return Ok(None);
    }
    let n = r.i32()?;
    if !(0..4096).contains(&n) {
        return Ok(None);
    }
    r.skip(n as usize * 16)?;
    let mut lm = LightMap2D::default();
    for k in 0..3 {
        lm.textures[k] = r.i32()?;
        lm.scales[k] = r.vec3()?;
    }
    lm.coord_scale = [r.f32()?, r.f32()?];
    lm.coord_bias = [r.f32()?, r.f32()?];
    Ok(Some(lm).filter(|lm| lm.textures[0] > 0 && lm.textures[1] > 0))
}

/// Landscape lightmap UVs (UE's GetTerrainExpandPatchCount and the landscape scene proxy's
/// LightmapScaleBias): a component of `quads` quads at StaticLightingResolution `res` gets a
/// power-of-two lightmap with an expanded border; vertex (x, y) maps to (x, y) * scale + bias
/// before the lightmap's own coordinate scale/bias.
pub fn landscape_lightmap_scale_bias(quads: i32, res: f32) -> (f32, f32) {
    if res <= 0.0 || quads <= 0 {
        return (1.0 / (quads.max(1) as f32 + 1.0), 0.0);
    }
    let padding = 4; // DXT1 block
    let expand = if res >= 1.0 { (padding as f32 / res) as i32 } else { padding }.max(1);
    let mut desired = (((quads + 1) as f32 * res) as i32).min(4096);
    let current = (((2 * expand + quads + 1) as f32 * res) as i32).min(4096);
    if current > desired {
        let mut prior = desired;
        while desired > 0 {
            prior = desired;
            desired &= !(desired & !(desired - 1));
        }
        desired = prior << 1;
        if current * current <= (prior * prior) << 1 {
            desired = prior;
        }
    }
    let quad_texels = quads as f32 * res;
    let dest = (desired as f32 / current as f32 * quad_texels) as i32;
    let ratio = dest as f32 / quad_texels * current as f32 / desired as f32;
    let scale = ratio / ((quads + 1) + 2 * expand) as f32;
    (scale, expand as f32 * scale)
}

/// [`instanced_transforms`] plus each instance's LightmapUVBias.
#[cfg(test)]
mod lightmap_tests {
    #[test]
    fn landscape_lightmap_fits_its_atlas() {
        // Level_01: 126-quad components at resolution 6 cook to 1024^2 lightmaps; the vertices
        // span DestSize = 1000 texels after a ~8 texel border
        let (s, b) = super::landscape_lightmap_scale_bias(126, 6.0);
        assert!((b * 1024.0 - 7.94).abs() < 0.05, "bias {}", b * 1024.0);
        assert!(((s * 126.0) * 1024.0 - 1000.0).abs() < 0.5, "span {}", s * 126.0 * 1024.0);
    }
}

pub fn instanced_transforms_lm(native: &[u8]) -> Vec<([f32; 16], [f32; 2])> {
    let len = native.len();
    for o in 0..len.saturating_sub(8) {
        let elem = u32::from_le_bytes([native[o], native[o + 1], native[o + 2], native[o + 3]]);
        let n = u32::from_le_bytes([native[o + 4], native[o + 5], native[o + 6], native[o + 7]]) as usize;
        if elem == 80 && n > 0 && o + 8 + n * 80 == len {
            let f = |b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            return (0..n)
                .map(|k| {
                    let b = &native[o + 8 + k * 80..o + 8 + (k + 1) * 80];
                    let mut m = [0f32; 16];
                    for j in 0..16 {
                        m[j] = f(&b[j * 4..]);
                    }
                    (m, [f(&b[64..]), f(&b[68..])])
                })
                .collect();
        }
    }
    vec![]
}

/// UInstancedStaticMeshComponent's PerInstanceSMData: a bulk array at the very end of the
/// component's native data, `[u32 80][u32 n]` then n x (FMatrix Transform, FVector2D
/// LightmapUVBias, FVector2D ShadowmapUVBias).
pub fn instanced_transforms(native: &[u8]) -> Vec<[f32; 16]> {
    let len = native.len();
    for o in 0..len.saturating_sub(8) {
        let elem = u32::from_le_bytes([native[o], native[o + 1], native[o + 2], native[o + 3]]);
        let n = u32::from_le_bytes([native[o + 4], native[o + 5], native[o + 6], native[o + 7]]) as usize;
        if elem == 80 && n > 0 && o + 8 + n * 80 == len {
            return (0..n)
                .map(|k| {
                    let b = &native[o + 8 + k * 80..o + 8 + k * 80 + 64];
                    let mut m = [0f32; 16];
                    for (j, c) in b.chunks_exact(4).enumerate() {
                        m[j] = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                    }
                    m
                })
                .collect();
        }
    }
    vec![]
}

#[derive(Debug, Clone)]
pub struct LandscapeComp {
    pub base_x: i32,
    pub base_y: i32,
    pub size_quads: i32,
    /// (size_quads+1)^2 raw heights; world z = (h - 32768) / 128 * scale.z
    pub heights: Vec<u16>,
    /// Per-vertex weights (0..255) of the landscape's layers (see `Landscape::layers`).
    pub weights: Vec<[u8; 4]>,
    /// The matching LandscapeComponent's lightmap (UVs from `landscape_lightmap_scale_bias`).
    pub lightmap: Option<LightMap2D>,
}

#[derive(Debug, Clone)]
pub struct Landscape {
    pub xf: Xform,
    pub comps: Vec<LandscapeComp>,
    pub material: i32,
    /// Paint layer names in weight channel order (at most 4).
    pub layers: Vec<String>,
    /// StaticLightingResolution (lightmap texels per quad).
    pub lighting_res: f32,
}

#[derive(Debug, Clone)]
pub struct Level {
    pub instances: Vec<MeshInstance>,
    pub landscapes: Vec<Landscape>,
    pub player_starts: Vec<Xform>,
    /// GGPlayerStartRace: (mRaceID, placement).
    pub race_starts: Vec<(String, Xform)>,
    pub lights: Vec<(String, Xform, Vec<Prop>)>,
    /// The level's BSP (PersistentLevel.Model_0).
    pub bsp: Option<Bsp>,
    /// Volumes (trampolines, fans, ladders, gravity, triggers...) with their brush hulls.
    pub volumes: Vec<VolumeActor>,
    /// APEX destructibles.
    pub apex: Vec<ApexPlacement>,
}

/// A Volume actor and the convex hull(s) of its BrushComponent (BrushAggGeom, actor-local).
#[derive(Debug, Clone)]
pub struct VolumeActor {
    /// Actor export index (0-based) in the package.
    pub export: usize,
    pub name: String,
    pub class: String,
    pub xf: Xform,
    pub shapes: Vec<crate::physasset::Shape>,
}

/// One BSP surface (FBspSurf): material and texture mapping shared by its polygons.
#[derive(Debug, Clone)]
pub struct BspSurf {
    /// Material object index in the package (0 = none).
    pub material: i32,
    pub flags: u32,
    pub base: [f32; 3],
    pub normal: [f32; 3],
    pub texture_u: [f32; 3],
    pub texture_v: [f32; 3],
}

/// A level's BSP geometry, as convex polygons per surface (Unreal units).
#[derive(Debug, Clone, Default)]
pub struct Bsp {
    pub surfs: Vec<BspSurf>,
    /// (surface index, polygon points)
    pub polys: Vec<(usize, Vec<[f32; 3]>)>,
    /// Per polygon: its BSP node and the lightmap UVs of its points (FVert.ShadowTexCoord).
    pub poly_nodes: Vec<usize>,
    pub poly_lm_uvs: Vec<Vec<[f32; 2]>>,
    /// Lightmap of each BSP node, from the level's ModelComponents' elements.
    pub node_lightmaps: std::collections::HashMap<usize, LightMap2D>,
}

/// UModelComponent native data: `[obj Model][i32 ComponentIndex][Elements n]`, each
/// FModelElement `[FLightMap][obj Component][obj Material][TArray<WORD> Nodes]
/// [TArray<obj> ShadowMaps][TArray<FGuid> IrrelevantLights]`. Returns (model, [(lightmap, nodes)]).
pub fn model_component_elements(native: &[u8]) -> Option<(i32, Vec<(LightMap2D, Vec<u16>)>)> {
    let mut r = Reader::new(native);
    let mut read = || -> Result<(i32, Vec<(LightMap2D, Vec<u16>)>)> {
        let model = r.i32()?;
        r.i32()?;
        let n = r.i32()?;
        anyhow::ensure!((0..4096).contains(&n), "bad element count {n}");
        let mut out = Vec::new();
        for _ in 0..n {
            let pos = r.pos;
            let ty = r.i32()?;
            anyhow::ensure!(ty == 0 || ty == 2, "lightmap type {ty}");
            r.pos = pos;
            let lm = read_lightmap(&mut r)?;
            r.i32()?; // component
            r.i32()?; // material
            let nn = r.i32()?;
            anyhow::ensure!((0..1 << 20).contains(&nn), "bad node count {nn}");
            let nodes: Vec<u16> = (0..nn).map(|_| r.u16()).collect::<Result<_>>()?;
            let ns = r.i32()?;
            anyhow::ensure!((0..1 << 16).contains(&ns), "bad shadow map count {ns}");
            let shadow = if ns > 0 { r.i32()? } else { 0 };
            r.skip((ns.max(1) as usize - 1) * 4 * (ns > 0) as usize)?;
            let lm = lm.map(|mut lm| {
                lm.shadow = shadow;
                lm
            });
            let nl = r.i32()?;
            anyhow::ensure!((0..1 << 16).contains(&nl), "bad light count {nl}");
            r.skip(nl as usize * 16)?;
            if let Some(lm) = lm {
                out.push((lm, nodes));
            }
        }
        Ok((model, out))
    };
    read().ok()
}

impl Bsp {
    /// PF_Invisible
    pub fn visible(s: &BspSurf) -> bool {
        s.flags & 0x1 == 0
    }
    /// Not PF_NotSolid / PF_Semisolid.
    pub fn solid(s: &BspSurf) -> bool {
        s.flags & (0x8 | 0x20) == 0
    }
}

/// UModel::Serialize (cooked v868): Bounds; bulk Vectors, Points, Nodes (64 bytes);
/// Surfs (TTransArray: owner, count, 60 bytes each); bulk Verts (24 bytes); ...
pub fn parse_bsp(p: &Package, export: usize) -> Result<Bsp> {
    let (end, _) = props::export_props(p, export)?;
    let d = p.export_data(export);
    let u32at = |o: usize| -> Result<u32> {
        d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| anyhow::anyhow!("BSP truncated at {o}"))
    };
    let f32at = |o: usize| -> Result<f32> { Ok(f32::from_bits(u32at(o)?)) };
    let vec3 = |o: usize| -> Result<[f32; 3]> { Ok([f32at(o)?, f32at(o + 4)?, f32at(o + 8)?]) };
    let mut o = end + 28; // FBoxSphereBounds
    let bulk = |o: &mut usize, expect: usize| -> Result<(usize, usize)> {
        let es = u32at(*o)? as usize;
        let n = u32at(*o + 4)? as usize;
        if n > 0 && es != expect {
            anyhow::bail!("BSP: element size {es}, expected {expect}");
        }
        let start = *o + 8;
        *o = start + es * n;
        Ok((start, n))
    };
    let (vec_at, n_vec) = bulk(&mut o, 12)?;
    let (pts_at, n_pts) = bulk(&mut o, 12)?;
    let (nodes_at, n_nodes) = bulk(&mut o, 64)?;
    // Surfs: owner object, count, then 60-byte FBspSurf records
    let n_surfs = u32at(o + 4)? as usize;
    let surfs_at = o + 8;
    o = surfs_at + n_surfs * 60;
    let (verts_at, n_verts) = bulk(&mut o, 24)?;
    let vectors = (0..n_vec).map(|k| vec3(vec_at + k * 12)).collect::<Result<Vec<_>>>()?;
    let points = (0..n_pts).map(|k| vec3(pts_at + k * 12)).collect::<Result<Vec<_>>>()?;
    let vec_or_zero = |i: u32| vectors.get(i as usize).copied().unwrap_or([0.0; 3]);
    let mut surfs = Vec::with_capacity(n_surfs);
    for k in 0..n_surfs {
        let s = surfs_at + k * 60;
        surfs.push(BspSurf {
            material: u32at(s)? as i32,
            flags: u32at(s + 4)?,
            base: points.get(u32at(s + 8)? as usize).copied().unwrap_or([0.0; 3]),
            normal: vec_or_zero(u32at(s + 12)?),
            texture_u: vec_or_zero(u32at(s + 16)?),
            texture_v: vec_or_zero(u32at(s + 20)?),
        });
    }
    let mut polys = Vec::with_capacity(n_nodes);
    let mut poly_nodes = Vec::with_capacity(n_nodes);
    let mut poly_lm_uvs = Vec::with_capacity(n_nodes);
    for k in 0..n_nodes {
        let nd = nodes_at + k * 64;
        let pool = u32at(nd + 16)? as usize;
        let surf = u32at(nd + 20)? as usize;
        let nv = d[nd + 54] as usize;
        if nv < 3 || surf >= surfs.len() || pool + nv > n_verts {
            continue;
        }
        let mut pts = Vec::with_capacity(nv);
        let mut lm = Vec::with_capacity(nv);
        for v in 0..nv {
            // FVert: pVertex, iSide, ShadowTexCoord, BackfaceShadowTexCoord
            let va = verts_at + (pool + v) * 24;
            let pv = u32at(va)? as usize;
            if let Some(pt) = points.get(pv) {
                pts.push(*pt);
                lm.push([f32at(va + 8)?, f32at(va + 12)?]);
            }
        }
        if pts.len() >= 3 {
            polys.push((surf, pts));
            poly_nodes.push(k);
            poly_lm_uvs.push(lm);
        }
    }
    // element lightmaps of the ModelComponents built from this model
    let mut node_lightmaps = std::collections::HashMap::new();
    for i in 0..p.exports.len() {
        if p.class_name(i as i32 + 1) != "ModelComponent" {
            continue;
        }
        let Ok((cend, _)) = props::export_props(p, i) else { continue };
        let Some((model, elems)) = model_component_elements(&p.export_data(i)[cend..]) else { continue };
        if model != export as i32 + 1 {
            continue;
        }
        for (lm, nodes) in elems {
            for nd in nodes {
                node_lightmaps.insert(nd as usize, lm);
            }
        }
    }
    Ok(Bsp { surfs, polys, poly_nodes, poly_lm_uvs, node_lightmaps })
}

fn vec3(ps: &[Prop], n: &str, d: [f32; 3]) -> [f32; 3] {
    match props::find(ps, n) {
        Some(Value::Vector(v)) => *v,
        _ => d,
    }
}
fn rot(ps: &[Prop], n: &str) -> [i32; 3] {
    match props::find(ps, n) {
        Some(Value::Rotator(v)) => *v,
        _ => [0; 3],
    }
}
fn float(ps: &[Prop], n: &str, d: f32) -> f32 {
    match props::find(ps, n) {
        Some(Value::Float(v)) => *v,
        _ => d,
    }
}
fn boolean(ps: &[Prop], n: &str) -> Option<bool> {
    match props::find(ps, n) {
        Some(Value::Bool(v)) => Some(*v),
        _ => None,
    }
}

/// Properties of an export merged with its in-package archetype chain (instance wins).
pub fn merged_props(p: &Package, export: usize) -> Vec<Prop> {
    let mut out: Vec<Prop> = props::export_props(p, export).map(|x| x.1).unwrap_or_default();
    let mut arch = p.exports[export].archetype;
    for _ in 0..8 {
        if arch <= 0 {
            break;
        }
        if let Ok((_, ps)) = props::export_props(p, arch as usize - 1) {
            for q in ps {
                if !out.iter().any(|o| o.name == q.name && o.index == q.index) {
                    out.push(q);
                }
            }
        }
        arch = p.exports[arch as usize - 1].archetype;
    }
    out
}

fn actor_xform(ps: &[Prop]) -> Xform {
    let ds = float(ps, "DrawScale", 1.0);
    let s3 = vec3(ps, "DrawScale3D", [1.0; 3]);
    Xform { location: vec3(ps, "Location", [0.0; 3]), rotation: rot(ps, "Rotation"), scale: [s3[0] * ds, s3[1] * ds, s3[2] * ds] }
}

fn comp_xform(ps: &[Prop]) -> Xform {
    let s = float(ps, "Scale", 1.0);
    let s3 = vec3(ps, "Scale3D", [1.0; 3]);
    Xform { location: vec3(ps, "Translation", [0.0; 3]), rotation: rot(ps, "Rotation"), scale: [s3[0] * s, s3[1] * s, s3[2] * s] }
}

fn in_level(p: &Package, idx: i32) -> bool {
    p.obj_path(idx).contains(".PersistentLevel.")
}

/// Read the render components' weightmaps into per-vertex layer weights.
fn paint_layers(p: &Package, landscape_props: &[Prop], comps: &mut [LandscapeComp]) -> Vec<String> {
    struct RenderComp {
        base: (i32, i32),
        sub_quads: i32,
        allocs: Vec<(String, usize, usize)>,
        textures: Vec<i32>,
        bias: [f32; 4],
        lightmap: Option<LightMap2D>,
    }
    let mut rcs = Vec::new();
    for c in object_array(props::find(landscape_props, "LandscapeComponents")) {
        if c <= 0 {
            continue;
        }
        let Ok((end, cps)) = props::export_props(p, c as usize - 1) else { continue };
        // LandscapeComponent native data starts with its FLightMap
        let shadow = object_array(props::find(&cps, "ShadowMaps")).first().copied().unwrap_or(0);
        let lightmap = read_lightmap(&mut Reader::at(p.export_data(c as usize - 1), end)).ok().flatten().map(|mut lm| {
            lm.shadow = shadow;
            lm
        });
        let int = |k: &str, d: i32| match props::find(&cps, k) {
            Some(Value::Int(v)) => *v,
            _ => d,
        };
        let allocs = crate::material::struct_array(p, props::find(&cps, "WeightmapLayerAllocations"))
            .iter()
            .filter_map(|a| {
                let name = match props::find(a, "LayerName") {
                    Some(Value::Name(n)) => n.clone(),
                    _ => return None,
                };
                let byte = |k: &str| match props::find(a, k) {
                    Some(Value::Byte(b)) => *b as usize,
                    _ => 0,
                };
                Some((name, byte("WeightmapTextureIndex"), byte("WeightmapTextureChannel")))
            })
            .collect();
        let bias = match props::find(&cps, "WeightmapScaleBias") {
            Some(Value::Vector4(v)) => *v,
            _ => [1.0 / 128.0, 1.0 / 128.0, 0.5 / 128.0, 0.5 / 128.0],
        };
        rcs.push(RenderComp {
            base: (int("SectionBaseX", 0), int("SectionBaseY", 0)),
            sub_quads: int("SubsectionSizeQuads", 63),
            allocs,
            textures: object_array(props::find(&cps, "WeightmapTextures")),
            bias,
            lightmap,
        });
    }
    let mut layers: Vec<String> = rcs.iter().flat_map(|r| r.allocs.iter().map(|a| a.0.clone())).collect();
    layers.sort();
    layers.dedup();
    layers.truncate(4);
    let mut cache: std::collections::HashMap<i32, Option<crate::texture::Image>> = std::collections::HashMap::new();
    for comp in comps.iter_mut() {
        let Some(rc) = rcs.iter().find(|r| r.base == (comp.base_x, comp.base_y)) else { continue };
        comp.lightmap = rc.lightmap;
        let n = (comp.size_quads + 1) as usize;
        let mut w = vec![[0u8; 4]; n * n];
        for (name, ti, ch) in &rc.allocs {
            let Some(li) = layers.iter().position(|l| l == name) else { continue };
            let Some(&tex) = rc.textures.get(*ti) else { continue };
            let img = cache.entry(tex).or_insert_with(|| {
                if tex <= 0 {
                    return None;
                }
                let e = tex as usize - 1;
                let t = crate::texture::Texture::parse(p, e).ok()?;
                t.decode(p, e, 4096).ok()
            });
            let Some(img) = img.as_ref() else { continue };
            let (tw, th) = (img.width as f32, img.height as f32);
            for y in 0..n {
                for x in 0..n {
                    // vertices on subsection seams are duplicated in the weightmap
                    let sx = (x as i32 / rc.sub_quads.max(1)).min(1);
                    let sy = (y as i32 / rc.sub_quads.max(1)).min(1);
                    let u = ((x as i32 + sx) as f32 * rc.bias[0] + rc.bias[2]) * tw;
                    let v = ((y as i32 + sy) as f32 * rc.bias[1] + rc.bias[3]) * th;
                    let px = (u.floor() as i32).clamp(0, img.width as i32 - 1) as usize;
                    let py = (v.floor() as i32).clamp(0, img.height as i32 - 1) as usize;
                    w[y * n + x][li] = img.rgba[(py * img.width as usize + px) * 4 + ch];
                }
            }
        }
        comp.weights = w;
    }
    layers
}

pub fn load(p: &Package) -> Result<Level> {
    let mut lvl = Level { instances: vec![], landscapes: vec![], player_starts: vec![], race_starts: vec![], lights: vec![], bsp: None, volumes: vec![], apex: vec![] };
    // the level's own BSP model: the biggest Model directly inside PersistentLevel
    let level_model = (0..p.exports.len())
        .filter(|&i| p.class_name(i as i32 + 1) == "Model" && p.obj_path(i as i32 + 1).rsplit_once('.').is_some_and(|(outer, _)| outer.ends_with("PersistentLevel")))
        .max_by_key(|&i| p.exports[i].serial_size);
    if let Some(m) = level_model {
        match parse_bsp(p, m) {
            Ok(b) => lvl.bsp = Some(b),
            Err(e) => eprintln!("BSP {}: {e:#}", p.obj_path(m as i32 + 1)),
        }
    }
    for i in 0..p.exports.len() {
        let idx = i as i32 + 1;
        let class = p.class_name(idx);
        match class.as_str() {
            "StaticMeshComponent" if in_level(p, idx) => {
                let owner = p.exports[i].outer;
                if owner <= 0 {
                    continue;
                }
                let cps = merged_props(p, i);
                let Some(Value::Object(mesh)) = props::find(&cps, "StaticMesh") else { continue };
                if *mesh == 0 || p.class_name(*mesh) != "StaticMesh" {
                    continue;
                }
                let lightmap = props::export_props(p, i).ok().and_then(|(end, _)| component_lightmap(&p.export_data(i)[end..]));
                let aps = merged_props(p, owner as usize - 1);
                let actor_class = p.class_name(owner);
                let dynamic = (actor_class.contains("KActor") || actor_class.starts_with("GGExplosiveActor")) && !actor_class.contains("Spawnable");
                let hidden = boolean(&aps, "bHidden").unwrap_or(false) || boolean(&cps, "HiddenGame").unwrap_or(false);
                let collide = boolean(&cps, "CollideActors").unwrap_or(true) && boolean(&cps, "BlockRigidBody").unwrap_or(true);
                lvl.instances.push(MeshInstance {
                    actor: p.obj_name(owner),
                    actor_class,
                    mesh: *mesh,
                    actor_xf: actor_xform(&aps),
                    comp_xf: comp_xform(&cps),
                    materials: object_array(props::find(&cps, "Materials")),
                    dynamic,
                    hidden,
                    collide,
                    phys_material: match props::find(&cps, "PhysMaterialOverride") {
                        Some(Value::Object(o)) => *o,
                        _ => 0,
                    },
                    matrix: None,
                    lightmap,
                    actor_export: Some(owner as usize - 1),
                });
            }
            // InstancedFoliageActor: one component per mesh, instances placed by world matrices
            "InstancedStaticMeshComponent" if in_level(p, idx) => {
                let owner = p.exports[i].outer;
                let Ok((end, cps)) = props::export_props(p, i) else { continue };
                let Some(Value::Object(mesh)) = props::find(&cps, "StaticMesh") else { continue };
                if *mesh == 0 || p.class_name(*mesh) != "StaticMesh" {
                    continue;
                }
                let collide = boolean(&cps, "CollideActors").unwrap_or(true) && boolean(&cps, "BlockRigidBody").unwrap_or(true);
                let hidden = boolean(&cps, "HiddenGame").unwrap_or(false);
                let materials = object_array(props::find(&cps, "Materials"));
                let native = &p.export_data(i)[end..];
                // instances share the component's lightmap atlas, each at its own LightmapUVBias
                let comp_lm = component_lightmap(native);
                for (m, bias) in instanced_transforms_lm(native) {
                    lvl.instances.push(MeshInstance {
                        actor: if owner > 0 { p.obj_name(owner) } else { p.obj_name(idx) },
                        actor_class: "InstancedFoliageActor".into(),
                        mesh: *mesh,
                        actor_xf: Xform::default(),
                        comp_xf: Xform::default(),
                        materials: materials.clone(),
                        dynamic: false,
                        hidden,
                        collide,
                        phys_material: 0,
                        matrix: Some(m),
                        lightmap: comp_lm.map(|lm| LightMap2D { coord_bias: bias, ..lm }),
                        actor_export: None,
                    });
                }
            }
            "Landscape" | "LandscapeProxy" if in_level(p, idx) => {
                let ps = merged_props(p, i);
                let mut comps = Vec::new();
                for c in object_array(props::find(&ps, "CollisionComponents")) {
                    if c <= 0 {
                        continue;
                    }
                    let ci = c as usize - 1;
                    let Ok((end, cps)) = props::export_props(p, ci) else { continue };
                    let n = match props::find(&cps, "CollisionSizeQuads") {
                        Some(Value::Int(v)) => *v,
                        _ => continue,
                    };
                    let int = |k: &str| match props::find(&cps, k) {
                        Some(Value::Int(v)) => *v,
                        _ => 0,
                    };
                    let data = p.export_data(ci);
                    let mut r = Reader::at(data, end);
                    let Ok(bulk) = crate::texture::BulkData::read(&mut r) else { continue };
                    let Ok(raw) = bulk.load(data, None) else { continue };
                    let heights: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    if heights.len() != ((n + 1) * (n + 1)) as usize {
                        continue;
                    }
                    comps.push(LandscapeComp { base_x: int("SectionBaseX"), base_y: int("SectionBaseY"), size_quads: n, heights, weights: vec![], lightmap: None });
                }
                let material = match props::find(&ps, "LandscapeMaterial") {
                    Some(Value::Object(o)) => *o,
                    _ => 0,
                };
                let layers = paint_layers(p, &ps, &mut comps);
                let mut xf = actor_xform(&ps);
                // Engine's Default__LandscapeProxy: DrawScale3D (128, 128, 256) when the map keeps it
                if props::find(&ps, "DrawScale3D").is_none() {
                    let ds = xf.scale[0];
                    xf.scale = [128.0 * ds, 128.0 * ds, 256.0 * ds];
                }
                let lighting_res = match props::find(&ps, "StaticLightingResolution") {
                    Some(Value::Float(f)) => *f,
                    Some(Value::Int(i)) => *i as f32,
                    _ => 1.0,
                };
                lvl.landscapes.push(Landscape { xf, comps, material, layers, lighting_res });
            }
            // a volume's BrushComponent carries its convex hull
            "BrushComponent" if in_level(p, idx) => {
                let owner = p.exports[i].outer;
                if owner <= 0 {
                    continue;
                }
                let actor_class = p.class_name(owner);
                if actor_class == "Brush" {
                    continue; // CSG builder brushes are baked into the BSP
                }
                let Ok((_, cps)) = props::export_props(p, i) else { continue };
                let Some(Value::Struct(_, agg)) = props::find(&cps, "BrushAggGeom") else { continue };
                let shapes = crate::physasset::agg_geom(p, agg);
                if shapes.is_empty() {
                    continue;
                }
                let aps = merged_props(p, owner as usize - 1);
                lvl.volumes.push(VolumeActor { export: owner as usize - 1, name: p.obj_name(owner), class: actor_class, xf: actor_xform(&aps), shapes });
            }
            "GGApexDestructibleActor" | "ApexDestructibleActor" if in_level(p, idx) => {
                let aps = merged_props(p, i);
                let Some(Value::Object(comp)) = props::find(&aps, "StaticDestructibleComponent") else { continue };
                if *comp <= 0 {
                    continue;
                }
                let cps = merged_props(p, *comp as usize - 1);
                let Some(Value::Object(asset)) = props::find(&cps, "Asset") else { continue };
                if *asset == 0 || boolean(&aps, "bHidden").unwrap_or(false) {
                    continue;
                }
                lvl.apex.push(ApexPlacement { export: i, name: p.obj_name(idx), xf: actor_xform(&aps), asset: *asset });
            }
            c if c.starts_with("GGPlayerStartRace") && in_level(p, idx) => {
                // only used by GGGameInfoRace (ShouldIgnorePlayerStart), with its mRaceID
                let ps = merged_props(p, i);
                let id = match props::find(&ps, "mRaceID") {
                    Some(Value::Str(s)) => s.clone(),
                    _ => String::new(),
                };
                lvl.race_starts.push((id, actor_xform(&ps)));
            }
            c if (c == "PlayerStart" || c.starts_with("GGPlayerStart")) && in_level(p, idx) => {
                lvl.player_starts.push(actor_xform(&merged_props(p, i)));
            }
            c if c.ends_with("LightComponent") && in_level(p, idx) => {
                let owner = p.exports[i].outer;
                if owner > 0 {
                    let cps = merged_props(p, i);
                    let aps = merged_props(p, owner as usize - 1);
                    lvl.lights.push((c.to_string(), actor_xform(&aps), cps));
                }
            }
            _ => {}
        }
    }
    Ok(lvl)
}
