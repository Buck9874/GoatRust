//! USkeletalMesh (UE3 v868): reference skeleton + LOD 0 GPU-skin vertex buffer.

use crate::package::{FName, Package};
use crate::props;
use crate::reader::Reader;
use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Bone {
    pub name: String,
    /// Local rotation (X, Y, Z, W) and translation relative to the parent (Unreal space).
    pub rotation: [f32; 4],
    pub position: [f32; 3],
    pub parent: i32,
}

#[derive(Debug, Clone)]
pub struct SkelSection {
    pub material: usize,
    pub first_index: u32,
    pub num_triangles: u32,
}

#[derive(Debug, Clone, Default)]
pub struct SkeletalMesh {
    pub name: String,
    pub bones: Vec<Bone>,
    pub materials: Vec<i32>,
    /// Mesh-space origin offset and rotation applied before the component transform.
    pub origin: [f32; 3],
    pub rot_origin: [i32; 3],
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Global bone indices / weights (0..255) per vertex.
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[u8; 4]>,
    pub indices: Vec<u32>,
    pub sections: Vec<SkelSection>,
}

fn bulk<'a>(r: &mut Reader<'a>) -> Result<(usize, usize, &'a [u8])> {
    let elem = r.i32()?;
    let count = r.i32()?;
    if elem < 0 || count < 0 || elem as usize * count as usize > r.remaining() {
        bail!("bad bulk array {elem}x{count} at {}", r.pos - 8);
    }
    Ok((elem as usize, count as usize, r.bytes(elem as usize * count as usize)?))
}

impl SkeletalMesh {
    pub fn parse(p: &Package, export: usize) -> Result<Self> {
        let data = p.export_data(export);
        let (end, _) = props::export_props(p, export)?;
        let mut r = Reader::at(data, end);
        r.skip(28)?; // bounds
        let materials = r.array(|r| r.i32())?;
        let origin = r.vec3()?;
        let rot_origin = [r.i32()?, r.i32()?, r.i32()?];
        let nb = r.i32()?;
        if !(1..=1024).contains(&nb) {
            bail!("bad bone count {nb}");
        }
        let mut bones = Vec::with_capacity(nb as usize);
        for _ in 0..nb {
            let name = p.name_str(FName { index: r.i32()?, number: r.i32()? });
            r.u32()?; // flags
            let rotation = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
            let position = r.vec3()?;
            r.i32()?; // NumChildren
            let parent = r.i32()?;
            r.u32()?; // BoneColor
            bones.push(Bone { name, rotation, position, parent });
        }
        r.i32()?; // SkeletalDepth
        let nlods = r.i32()?;
        if nlods < 1 {
            bail!("no LODs");
        }
        let mut m = Self::parse_lod(&mut r).context("LOD 0")?;
        m.name = p.obj_name(export as i32 + 1);
        m.bones = bones;
        m.materials = materials;
        m.origin = origin;
        m.rot_origin = rot_origin;
        Ok(m)
    }

    fn parse_lod(r: &mut Reader) -> Result<Self> {
        let nsec = r.i32()?;
        if !(0..=64).contains(&nsec) {
            bail!("bad section count {nsec}");
        }
        let mut sec_raw = Vec::new();
        for _ in 0..nsec {
            let mat = r.i16()?;
            let chunk = r.i16()?;
            let first = r.i32()?;
            let ntri = r.i32()?;
            r.u8()?;
            sec_raw.push((mat, chunk, first, ntri));
        }
        // FSkelIndexBuffer3 (ver >= 806): i32 + u8 item size + bulk array
        r.i32()?;
        let item = r.u8()?;
        let (elem, _n, idx) = bulk(r).context("indices")?;
        let indices: Vec<u32> = match (item, elem) {
            (2, 2) => idx.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32).collect(),
            (4, 4) => idx.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
            _ => bail!("index item size {item}/{elem}"),
        };
        let _used_bones = r.array(|r| r.i16())?;
        // chunks
        let nchunks = r.i32()?;
        if !(0..=256).contains(&nchunks) {
            bail!("bad chunk count {nchunks}");
        }
        let mut chunks = Vec::new();
        for _ in 0..nchunks {
            let first_vertex = r.i32()?;
            // FRigidVertex3 = pos, 3 packed normals, 4 UV sets, colour, bone (61 bytes);
            // FSoftVertex3 adds 4 bone indices + 4 weights instead of one bone (68 bytes).
            // The GPU-skin buffer below holds the same vertices, so these are skipped.
            let nrigid = r.i32()?;
            r.skip(nrigid.max(0) as usize * 61)?;
            let nsoft = r.i32()?;
            r.skip(nsoft.max(0) as usize * 68)?;
            let bones = r.array(|r| r.i16())?;
            let num_rigid = r.i32()?;
            let num_soft = r.i32()?;
            let _max_inf = r.i32()?;
            chunks.push((first_vertex, bones, num_rigid + num_soft));
        }
        r.i32()?; // size
        let num_vertices = r.i32()? as usize;
        let _required = r.array(|r| r.u8())?;
        crate::texture::BulkData::read(r)?; // raw point indices
        let _lod_uv_sets = r.i32()?;
        // GPU skin vertex buffer
        let num_uv = r.i32()? as usize;
        let full = r.i32()? != 0;
        // bUsePackedPosition is only honoured on consoles; PC data is always full floats
        let _packed = r.i32()?;
        r.vec3()?;
        r.vec3()?;
        let (elem, count, vd) = bulk(r).context("gpu verts")?;
        let uv_size = if full { 8 } else { 4 };
        if elem != 8 + 8 + 12 + num_uv * uv_size || count != num_vertices {
            bail!("unexpected vertex layout elem {elem} count {count}/{num_vertices} uv {num_uv} full {full}");
        }
        let mut m = SkeletalMesh::default();
        let mut vr = Reader::new(vd);
        let mut local_joints = Vec::with_capacity(count);
        for _ in 0..count {
            vr.u32()?;
            let n = vr.u32()?;
            let bi = [vr.u8()?, vr.u8()?, vr.u8()?, vr.u8()?];
            let bw = [vr.u8()?, vr.u8()?, vr.u8()?, vr.u8()?];
            let pos = vr.vec3()?;
            let mut uv0 = [0.0, 0.0];
            for s in 0..num_uv {
                let uv = if full { [vr.f32()?, vr.f32()?] } else { [vr.f16()?, vr.f16()?] };
                if s == 0 {
                    uv0 = uv;
                }
            }
            m.positions.push(pos);
            m.normals.push(crate::mesh::unpack_normal(n));
            m.uvs.push(uv0);
            local_joints.push(bi);
            m.weights.push(bw);
        }
        // chunk-local bone indices -> skeleton indices
        m.joints = vec![[0; 4]; count];
        for (first, bones, n) in &chunks {
            for v in *first as usize..(*first as usize + *n as usize).min(count) {
                for k in 0..4 {
                    m.joints[v][k] = bones.get(local_joints[v][k] as usize).copied().unwrap_or(0) as u16;
                }
            }
        }
        m.indices = indices;
        m.sections = sec_raw
            .iter()
            .map(|&(mat, _c, first, ntri)| SkelSection { material: mat.max(0) as usize, first_index: first as u32, num_triangles: ntri as u32 })
            .collect();
        Ok(m)
    }
}
