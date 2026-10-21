//! ApexDestructibleAsset: the cooked APEX destructible (a binary NvParameterized "APB" blob after
//! the UObject properties) -> chunks and their render mesh parts.
//!
//! The blob is `[u32 1][u32 n][n-byte name][u32 length]` then the APB file. Its data section is a
//! memory image of the parameter classes for the platform that wrote it (Goat Simulator's were
//! cooked on x86-64 MSVC: 8-byte pointers stored as file offsets): every object is an ObjHeader
//! `{u32 rel, ptr class, ptr name, bool included, u32 version, u32 checksum size, ptr checksum}`
//! whose `rel` leads to the C++ object, an NvParameters base (0x48 bytes) followed by the class's
//! ParametersStruct. Layouts come from APEX 1.4's autogen headers (reference/PhysX-3.4):
//! DestructibleAssetParameters 0.12, RenderMeshAssetParameters 0.0, SubmeshParameters 0.0,
//! VertexBufferParameters 0.1, VertexFormatParameters 0.0, Buffer* 0.0.

use crate::package::Package;
use anyhow::{Result, bail, ensure};

/// Size of the NvParameters base class on x86-64.
const NVPARAMETERS_SIZE: usize = 0x48;

/// One chunk of the fracture hierarchy (depth 0 = the whole object).
#[derive(Debug, Clone, Copy)]
pub struct Chunk {
    pub depth: u16,
    pub parent: u16,
    pub first_child: u16,
    pub children: u16,
    /// Render mesh part (one per chunk unless instanced).
    pub part: u16,
    pub flags: u16,
}

/// One render mesh submesh (one material): vertices and triangles, split by part.
#[derive(Debug, Clone, Default)]
pub struct Submesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Index range of part p: index_partition[p]..index_partition[p + 1].
    pub index_partition: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct Destructible {
    pub name: String,
    pub depth_count: u32,
    /// Unreal units, asset space.
    pub bounds: [[f32; 3]; 2],
    pub chunks: Vec<Chunk>,
    pub submeshes: Vec<Submesh>,
    pub damage_threshold: f32,
    pub damage_to_radius: f32,
    pub debris_lifetime: [f32; 2],
    /// DestructibleParameters as stored (for inspection).
    pub params_raw: Vec<u8>,
    /// Material object indices per submesh (ApexDestructibleAsset.Materials).
    pub materials: Vec<i32>,
}

impl Destructible {
    /// Triangles (as vertex index triples into the submesh) of a chunk's part.
    pub fn part_tris(&self, sm: usize, part: u16) -> &[u32] {
        let s = &self.submeshes[sm];
        let p = part as usize;
        if p + 1 >= s.index_partition.len() {
            return &[];
        }
        let (a, b) = (s.index_partition[p] as usize, s.index_partition[p + 1] as usize);
        &s.indices[a.min(s.indices.len())..b.min(s.indices.len())]
    }

    /// The deepest chunks (what a full fracture leaves).
    pub fn leaf_chunks(&self) -> Vec<usize> {
        let deepest = self.chunks.iter().map(|c| c.depth).max().unwrap_or(0);
        (0..self.chunks.len()).filter(|&i| self.chunks[i].depth == deepest).collect()
    }
}

struct Apb<'a> {
    d: &'a [u8],
}

impl<'a> Apb<'a> {
    fn u16(&self, o: usize) -> Result<u16> {
        let b = self.d.get(o..o + 2).ok_or_else(|| anyhow::anyhow!("APB read past end at {o}"))?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&self, o: usize) -> Result<u32> {
        let b = self.d.get(o..o + 4).ok_or_else(|| anyhow::anyhow!("APB read past end at {o}"))?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&self, o: usize) -> Result<f32> {
        Ok(f32::from_bits(self.u32(o)?))
    }
    fn ptr(&self, o: usize) -> Result<usize> {
        Ok(self.u32(o)? as usize | ((self.u32(o + 4)? as usize) << 32))
    }
    fn cstr(&self, o: usize) -> String {
        let end = self.d[o.min(self.d.len())..].iter().position(|&c| c == 0).map(|e| o + e).unwrap_or(self.d.len());
        String::from_utf8_lossy(&self.d[o.min(end)..end]).into_owned()
    }
    /// A referenced object (pointer to its ObjHeader): (class, version, ParametersStruct offset).
    fn object(&self, header: usize) -> Result<(String, u32, usize)> {
        ensure!(header != 0 && header < self.d.len(), "null object");
        let rel = self.u32(header)? as usize;
        let class = self.cstr(self.ptr(header + 8)?);
        let version = self.u32(header + 0x1c)?;
        Ok((class, version, header + rel + NVPARAMETERS_SIZE))
    }
    fn expect(&self, header: usize, class: &str, version: u32) -> Result<usize> {
        let (c, v, s) = self.object(header)?;
        ensure!(c == class, "expected {class}, found {c}");
        ensure!(v == version, "{class} version {}.{} not supported", v >> 16, v & 0xffff);
        Ok(s)
    }
    /// DynamicArray1D {ptr buf; bool isAllocated; i32 elementSize; i32 size} -> (buf, elem, count).
    fn array(&self, o: usize) -> Result<(usize, usize, usize)> {
        let buf = self.ptr(o)?;
        let elem = self.u32(o + 12)? as usize;
        let n = self.u32(o + 16)? as usize;
        ensure!(n < 1 << 24, "bad array size {n}");
        ensure!(n == 0 || buf + elem * n <= self.d.len(), "array out of range");
        Ok((buf, elem, n))
    }
}

/// Parse the destructible in an ApexDestructibleAsset export.
pub fn parse(p: &Package, export: usize) -> Result<Destructible> {
    let (end, props) = crate::props::export_props(p, export)?;
    let data = p.export_data(export);
    let blob = &data[end..];
    ensure!(blob.len() > 16, "no APEX data");
    let n = u32::from_le_bytes(blob[4..8].try_into()?) as usize;
    let name = String::from_utf8_lossy(&blob[8..8 + n.saturating_sub(1)]).into_owned();
    let d = &blob[8 + n + 4..];
    ensure!(d.len() > 64 && u32::from_be_bytes(d[0..4].try_into()?) == 0x5A5B5C5D, "not an APB file");
    let arch = u32::from_be_bytes(d[36..40].try_into()?);
    if arch != 2 {
        bail!("APB written for arch {arch}, only x86-64 layouts are known");
    }
    let data_off = u32::from_be_bytes(d[24..28].try_into()?) as usize;
    let a = Apb { d };
    // object table: {ptr obj, ptr className, ptr name, ptr filename}
    let root = a.expect(a.ptr(data_off)?, "DestructibleAssetParameters", 12)?;

    let mut out = Destructible { name, ..Default::default() };
    out.depth_count = a.u32(root + 16)?;
    for k in 0..6 {
        out.bounds[k / 3][k % 3] = a.f32(root + 24 + k * 4)?;
    }
    let (cb, ce, cn) = a.array(root + 48)?;
    ensure!(cn == 0 || ce == 24, "chunk size {ce}");
    for i in 0..cn {
        let o = cb + i * 24;
        out.chunks.push(Chunk {
            depth: a.u16(o)?,
            parent: a.u16(o + 2)?,
            first_child: a.u16(o + 4)?,
            children: a.u16(o + 6)?,
            part: a.u16(o + 8)?,
            flags: a.u16(o + 10)?,
        });
    }
    // DestructibleParameters_Type at +120
    out.damage_threshold = a.f32(root + 120)?;
    out.damage_to_radius = a.f32(root + 124)?;
    out.debris_lifetime = [a.f32(root + 160)?, a.f32(root + 164)?];
    out.params_raw = a.d.get(root + 120..(root + 248).min(a.d.len())).map(|b| b.to_vec()).unwrap_or_default();
    // renderMeshAsset: Interface* after depthParameters (248), crumble/dust names (272, 288),
    // collisionData (304)
    let rma = a.expect(a.ptr(root + 312)?, "RenderMeshAssetParameters", 0)?;
    let (sb, _, sn) = a.array(rma)?;
    for s in 0..sn {
        let sm = a.expect(a.ptr(sb + s * 8)?, "SubmeshParameters", 0)?;
        let vb = a.expect(a.ptr(sm)?, "VertexBufferParameters", 1)?;
        let vcount = a.u32(vb)? as usize;
        let vf = a.expect(a.ptr(vb + 8)?, "VertexFormatParameters", 0)?;
        let (fb, fe, fnum) = a.array(vf + 8)?;
        let (bb, _, bn) = a.array(vb + 16)?;
        let mut sub = Submesh::default();
        for k in 0..fnum.min(bn) {
            let fmt_name = a.cstr(a.ptr(fb + k * fe)?);
            let (class, _, bs) = a.object(a.ptr(bb + k * 8)?)?;
            let (db, de, dn) = a.array(bs)?;
            let n = dn.min(vcount);
            match (fmt_name.as_str(), class.as_str()) {
                ("SEMANTIC_POSITION", "BufferF32x3") if de == 12 => {
                    sub.positions = (0..n).map(|i| Ok([a.f32(db + i * 12)?, a.f32(db + i * 12 + 4)?, a.f32(db + i * 12 + 8)?])).collect::<Result<_>>()?;
                }
                ("SEMANTIC_NORMAL", "BufferF32x3") if de == 12 => {
                    sub.normals = (0..n).map(|i| Ok([a.f32(db + i * 12)?, a.f32(db + i * 12 + 4)?, a.f32(db + i * 12 + 8)?])).collect::<Result<_>>()?;
                }
                ("SEMANTIC_TEXCOORD0", "BufferF32x2") if de == 8 => {
                    sub.uvs = (0..n).map(|i| Ok([a.f32(db + i * 8)?, a.f32(db + i * 8 + 4)?])).collect::<Result<_>>()?;
                }
                _ => {}
            }
        }
        ensure!(!sub.positions.is_empty(), "submesh {s} has no positions");
        let (ib, ie, inum) = a.array(sm + 8)?;
        ensure!(inum == 0 || ie == 4, "index size {ie}");
        sub.indices = (0..inum).map(|i| a.u32(ib + i * 4)).collect::<Result<_>>()?;
        let (pb, _, pn) = a.array(sm + 56)?;
        sub.index_partition = (0..pn).map(|i| a.u32(pb + i * 4)).collect::<Result<_>>()?;
        out.submeshes.push(sub);
    }
    out.materials = crate::material::object_array(crate::props::find(&props, "Materials"));
    Ok(out)
}
