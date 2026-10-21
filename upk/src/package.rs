//! UE3 package (.u/.upk/.goat) reader for Goat Simulator (file version 868, licensee 0).

use crate::lzo;
use crate::reader::Reader;
use anyhow::{Context, Result, bail};
use std::path::Path;

pub const TAG: u32 = 0x9E2A83C1;

pub const COMPRESS_ZLIB: u32 = 1;
pub const COMPRESS_LZO: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FName {
    pub index: i32,
    pub number: i32,
}

#[derive(Debug, Clone)]
pub struct Import {
    pub class_package: FName,
    pub class_name: FName,
    pub outer: i32,
    pub name: FName,
}

#[derive(Debug, Clone)]
pub struct Export {
    pub class: i32,
    pub super_: i32,
    pub outer: i32,
    pub name: FName,
    pub archetype: i32,
    pub object_flags: u64,
    pub serial_size: i32,
    pub serial_offset: i32,
    pub export_flags: u32,
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub version: u16,
    pub licensee: u16,
    pub header_size: i32,
    pub folder: String,
    pub package_flags: u32,
    pub name_count: i32,
    pub name_offset: i32,
    pub export_count: i32,
    pub export_offset: i32,
    pub import_count: i32,
    pub import_offset: i32,
    pub engine_version: u32,
    pub cooker_version: u32,
    pub compression_flags: u32,
    pub chunks: Vec<[i32; 4]>,
}

pub struct Package {
    pub path: String,
    pub name: String,
    pub summary: Summary,
    pub data: Vec<u8>,
    pub names: Vec<String>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
}

fn read_summary(r: &mut Reader) -> Result<Summary> {
    let tag = r.u32()?;
    if tag != TAG {
        bail!("not a UE3 package (tag {tag:08x})");
    }
    let version = r.u16()?;
    let licensee = r.u16()?;
    let header_size = r.i32()?;
    let folder = r.fstring()?;
    let package_flags = r.u32()?;
    let name_count = r.i32()?;
    let name_offset = r.i32()?;
    let export_count = r.i32()?;
    let export_offset = r.i32()?;
    let import_count = r.i32()?;
    let import_offset = r.i32()?;
    let _depends_offset = r.i32()?;
    if version >= 623 {
        r.skip(12)?; // import/export guids offset + counts
    }
    if version >= 584 {
        r.skip(4)?; // thumbnail table offset
    }
    r.guid()?;
    let gens = r.i32()?;
    for _ in 0..gens {
        r.skip(12)?;
    }
    let engine_version = r.u32()?;
    let cooker_version = r.u32()?;
    let compression_flags = r.u32()?;
    let chunks = r.array(|r| Ok([r.i32()?, r.i32()?, r.i32()?, r.i32()?]))?;
    Ok(Summary {
        version,
        licensee,
        header_size,
        folder,
        package_flags,
        name_count,
        name_offset,
        export_count,
        export_offset,
        import_count,
        import_offset,
        engine_version,
        cooker_version,
        compression_flags,
        chunks,
    })
}

/// Decompress one UE3 compressed chunk (FCompressedChunkInfo header + blocks).
pub fn decompress_chunk(src: &[u8], flags: u32) -> Result<Vec<u8>> {
    let mut r = Reader::new(src);
    let tag = r.u32()?;
    if tag != TAG {
        bail!("bad chunk tag {tag:08x}");
    }
    let block_size = r.i32()? as usize;
    let _sum_comp = r.i32()?;
    let sum_uncomp = r.i32()? as usize;
    let nblocks = sum_uncomp.div_ceil(block_size.max(1));
    let mut sizes = Vec::with_capacity(nblocks);
    for _ in 0..nblocks {
        sizes.push((r.i32()? as usize, r.i32()? as usize));
    }
    let mut out = Vec::with_capacity(sum_uncomp);
    for (c, u) in sizes {
        let block = r.bytes(c)?;
        let dec = decompress_block(block, u, flags)?;
        out.extend_from_slice(&dec);
    }
    Ok(out)
}

pub fn decompress_block(block: &[u8], out_len: usize, flags: u32) -> Result<Vec<u8>> {
    if flags & COMPRESS_LZO != 0 {
        lzo::decompress(block, out_len)
    } else if flags & COMPRESS_ZLIB != 0 {
        use std::io::Read;
        let mut d = flate2::read::ZlibDecoder::new(block);
        let mut v = Vec::with_capacity(out_len);
        d.read_to_end(&mut v)?;
        Ok(v)
    } else {
        bail!("unsupported compression flags {flags}")
    }
}

impl Package {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let summary = read_summary(&mut Reader::new(&raw))?;
        let data = if summary.compression_flags != 0 && !summary.chunks.is_empty() {
            let total = summary
                .chunks
                .iter()
                .map(|c| (c[0] + c[1]) as usize)
                .max()
                .unwrap_or(0)
                .max(summary.header_size as usize);
            let mut buf = vec![0u8; total];
            let first = summary.chunks.iter().map(|c| c[0] as usize).min().unwrap_or(0);
            buf[..first].copy_from_slice(&raw[..first]);
            for c in &summary.chunks {
                let src = &raw[c[2] as usize..(c[2] + c[3]) as usize];
                let dec = decompress_chunk(src, summary.compression_flags)
                    .with_context(|| format!("chunk at {}", c[2]))?;
                buf[c[0] as usize..c[0] as usize + dec.len()].copy_from_slice(&dec);
            }
            buf
        } else {
            raw
        };
        let mut r = Reader::at(&data, summary.name_offset as usize);
        let mut names = Vec::with_capacity(summary.name_count as usize);
        for _ in 0..summary.name_count {
            names.push(r.fstring()?);
            r.u64()?; // flags
        }
        let mut r = Reader::at(&data, summary.import_offset as usize);
        let mut imports = Vec::with_capacity(summary.import_count as usize);
        let fname = |r: &mut Reader| -> Result<FName> { Ok(FName { index: r.i32()?, number: r.i32()? }) };
        for _ in 0..summary.import_count {
            imports.push(Import {
                class_package: fname(&mut r)?,
                class_name: fname(&mut r)?,
                outer: r.i32()?,
                name: fname(&mut r)?,
            });
        }
        let mut r = Reader::at(&data, summary.export_offset as usize);
        let mut exports = Vec::with_capacity(summary.export_count as usize);
        for _ in 0..summary.export_count {
            let class = r.i32()?;
            let super_ = r.i32()?;
            let outer = r.i32()?;
            let name = fname(&mut r)?;
            let archetype = r.i32()?;
            let object_flags = r.u64()?;
            let serial_size = r.i32()?;
            let serial_offset = r.i32()?;
            let export_flags = r.u32()?;
            let net = r.i32()?;
            r.skip(net.max(0) as usize * 4)?;
            r.guid()?;
            r.u32()?; // package flags
            exports.push(Export {
                class,
                super_,
                outer,
                name,
                archetype,
                object_flags,
                serial_size,
                serial_offset,
                export_flags,
            });
        }
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(Package { path: path.display().to_string(), name, summary, data, names, imports, exports })
    }

    pub fn name_str(&self, n: FName) -> String {
        let base = self.names.get(n.index as usize).cloned().unwrap_or_else(|| format!("<name{}>", n.index));
        if n.number > 0 { format!("{}_{}", base, n.number - 1) } else { base }
    }

    /// Bare object name for an object index (+export / -import / 0 = None).
    pub fn obj_name(&self, idx: i32) -> String {
        if idx > 0 {
            self.exports.get(idx as usize - 1).map(|e| self.name_str(e.name)).unwrap_or_default()
        } else if idx < 0 {
            self.imports.get((-idx) as usize - 1).map(|i| self.name_str(i.name)).unwrap_or_default()
        } else {
            "None".into()
        }
    }

    pub fn obj_outer(&self, idx: i32) -> i32 {
        if idx > 0 {
            self.exports.get(idx as usize - 1).map(|e| e.outer).unwrap_or(0)
        } else if idx < 0 {
            self.imports.get((-idx) as usize - 1).map(|i| i.outer).unwrap_or(0)
        } else {
            0
        }
    }

    /// Full dotted path, e.g. "GoatGame.GGGoat.Default__GGGoat".
    pub fn obj_path(&self, idx: i32) -> String {
        let mut parts = vec![self.obj_name(idx)];
        let mut o = self.obj_outer(idx);
        let mut guard = 0;
        while o != 0 && guard < 64 {
            parts.push(self.obj_name(o));
            o = self.obj_outer(o);
            guard += 1;
        }
        parts.reverse();
        parts.join(".")
    }

    /// Class name of an object index.
    pub fn class_name(&self, idx: i32) -> String {
        if idx > 0 {
            let e = &self.exports[idx as usize - 1];
            if e.class == 0 { "Class".into() } else { self.obj_name(e.class) }
        } else if idx < 0 {
            self.name_str(self.imports[(-idx) as usize - 1].class_name)
        } else {
            "None".into()
        }
    }

    pub fn export_data(&self, i: usize) -> &[u8] {
        let e = &self.exports[i];
        let s = e.serial_offset.max(0) as usize;
        let n = e.serial_size.max(0) as usize;
        &self.data[s.min(self.data.len())..(s + n).min(self.data.len())]
    }

    pub fn find_name(&self, s: &str) -> Option<i32> {
        self.names.iter().position(|n| n.eq_ignore_ascii_case(s)).map(|i| i as i32)
    }

    /// Export index (0-based) by bare name and class.
    pub fn find_export(&self, name: &str, class: Option<&str>) -> Option<usize> {
        (0..self.exports.len()).find(|&i| {
            self.name_str(self.exports[i].name).eq_ignore_ascii_case(name)
                && class.is_none_or(|c| self.class_name(i as i32 + 1).eq_ignore_ascii_case(c))
        })
    }
}
