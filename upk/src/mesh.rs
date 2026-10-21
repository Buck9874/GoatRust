//! UStaticMesh (UE3 v868) geometry: positions, packed normals, UVs, indices, sections.

use crate::package::Package;
use crate::props;
use crate::reader::Reader;
use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Section {
    /// Material object index in the package (0 = none).
    pub material: i32,
    pub first_index: u32,
    pub num_faces: u32,
    /// FStaticMeshElement.EnableCollision (per-poly collision for this section).
    pub collision: bool,
}

#[derive(Debug, Clone, Default)]
pub struct StaticMesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// The lightmap UV set (StaticMesh.LightMapCoordinateIndex, usually 1), empty when the
    /// mesh has a single UV set.
    pub uvs2: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub sections: Vec<Section>,
    pub bounds_origin: [f32; 3],
    pub bounds_extent: [f32; 3],
    /// RB_BodySetup object index holding simplified collision (0 = none).
    pub body_setup: i32,
}

/// UE3 TArray::BulkSerialize: i32 element size, then a normal TArray.
fn bulk_array<'a>(r: &mut Reader<'a>) -> Result<(usize, usize, &'a [u8])> {
    let elem = r.i32()?;
    let count = r.i32()?;
    if elem < 0 || count < 0 || (elem as usize).saturating_mul(count as usize) > r.remaining() {
        bail!("bad bulk array elem {elem} count {count} at {}", r.pos - 8);
    }
    let d = r.bytes(elem as usize * count as usize)?;
    Ok((elem as usize, count as usize, d))
}

/// Skip FUntypedBulkData header (+ inline payload).
fn skip_bulk(r: &mut Reader) -> Result<()> {
    crate::texture::BulkData::read(r).map(|_| ())
}

pub fn unpack_normal(n: u32) -> [f32; 3] {
    let f = |b: u32| (b & 0xff) as f32 / 127.5 - 1.0;
    [f(n), f(n >> 8), f(n >> 16)]
}

impl StaticMesh {
    pub fn parse(p: &Package, export: usize) -> Result<Self> {
        let data = p.export_data(export);
        let (end, ps) = props::export_props(p, export)?;
        let lm_index = match props::find(&ps, "LightMapCoordinateIndex") {
            Some(props::Value::Int(i)) => (*i).max(0) as usize,
            _ => 1,
        };
        let mut r = Reader::at(data, end);
        let bounds_origin = r.vec3()?;
        let bounds_extent = r.vec3()?;
        r.f32()?; // sphere radius
        let body_setup = r.i32()?;
        // kDOP tree (ver >= 770): bounds + compact nodes + triangles
        r.skip(24)?;
        bulk_array(&mut r).context("kDOP nodes")?;
        bulk_array(&mut r).context("kDOP tris")?;
        let _internal_version = r.i32()?;
        // ver >= 823: optional extra LOD, ver >= 829: TArray<{i32,u8[3]}>, i32
        let has_extra = r.i32()?;
        if has_extra != 0 {
            bail!("extra LOD model not supported");
        }
        let n5 = r.i32()?;
        r.skip(n5.max(0) as usize * 7)?;
        r.i32()?;
        r.i32()?; // ver >= 859
        let nlods = r.i32()?;
        if nlods < 1 || nlods > 8 {
            bail!("bad LOD count {nlods}");
        }
        let mut m = Self::parse_lod(p, &mut r, lm_index).context("LOD 0")?;
        m.name = p.obj_name(export as i32 + 1);
        m.bounds_origin = bounds_origin;
        m.bounds_extent = bounds_extent;
        m.body_setup = body_setup;
        Ok(m)
    }

    fn parse_lod(_p: &Package, r: &mut Reader, lm_index: usize) -> Result<Self> {
        skip_bulk(r)?; // raw triangles
        let nsec = r.i32()?;
        if !(0..=256).contains(&nsec) {
            bail!("bad section count {nsec}");
        }
        let mut sections = Vec::new();
        for _ in 0..nsec {
            let material = r.i32()?;
            let collision = r.i32()? != 0; // EnableCollision
            r.i32()?; // OldEnableCollision
            r.i32()?; // shadow casting
            let first_index = r.i32()? as u32;
            let num_faces = r.i32()? as u32;
            r.i32()?; // min vertex
            r.i32()?; // max vertex
            r.i32()?; // material index
            let n = r.i32()?; // fragments: TArray<{i32,i32}>
            r.skip(n.max(0) as usize * 8)?;
            let ps3 = r.u8()?;
            if ps3 != 0 {
                bail!("PS3 section data");
            }
            sections.push(Section { material, first_index, num_faces, collision });
        }
        // position stream
        let _stride = r.i32()?;
        let nverts = r.i32()? as usize;
        let (elem, count, pos) = bulk_array(r).context("positions")?;
        if elem != 12 || count != nverts {
            bail!("unexpected position stream {elem}x{count}");
        }
        let mut pr = Reader::new(pos);
        let positions: Vec<[f32; 3]> = (0..count).map(|_| pr.vec3()).collect::<Result<_>>()?;
        // UV/tangent stream
        let num_uv = r.i32()? as usize;
        let _item = r.i32()?;
        let _n = r.i32()?;
        let full = r.i32()? != 0;
        let (elem, count, uvd) = bulk_array(r).context("uv stream")?;
        let per_uv = if full { 8 } else { 4 };
        if elem != 8 + num_uv * per_uv || count != nverts {
            bail!("unexpected uv stream elem {elem} count {count} uvsets {num_uv} full {full}");
        }
        let mut normals = Vec::with_capacity(count);
        let mut uvs = Vec::with_capacity(count);
        let mut uvs2 = Vec::new();
        let mut ur = Reader::new(uvd);
        for _ in 0..count {
            ur.u32()?; // tangent X
            let n = ur.u32()?; // tangent Z (normal)
            normals.push(unpack_normal(n));
            for s in 0..num_uv {
                let uv = if full { [ur.f32()?, ur.f32()?] } else { [ur.f16()?, ur.f16()?] };
                if s == 0 {
                    uvs.push(uv);
                }
                if s > 0 && s == lm_index.min(num_uv - 1) {
                    uvs2.push(uv);
                }
            }
        }
        // color stream
        let _item = r.i32()?;
        let ncol = r.i32()?;
        if ncol != 0 {
            bulk_array(r).context("colors")?;
        }
        let _numverts = r.i32()?;
        let (elem, _count, idx) = bulk_array(r).context("indices")?;
        if elem != 2 {
            bail!("index size {elem}");
        }
        let indices = idx.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32).collect();
        Ok(StaticMesh { positions, normals, uvs, uvs2, indices, sections, ..Default::default() })
    }
}
