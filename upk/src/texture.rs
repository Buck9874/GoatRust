//! UTexture2D decoding: mip chain from the package or the texture file cache (.tfc).

use crate::package::{Package, decompress_chunk};
use crate::props::{self, Value};
use crate::reader::Reader;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::sync::Mutex;

pub const BULK_SEPARATE_FILE: u32 = 0x01;
pub const BULK_ZLIB: u32 = 0x02;
pub const BULK_LZO: u32 = 0x10;
pub const BULK_UNUSED: u32 = 0x20;

#[derive(Debug, Clone)]
pub struct BulkData {
    pub flags: u32,
    pub element_count: i32,
    pub size_on_disk: i32,
    pub offset: i32,
    /// Position of inline payload in the package data (when not in a separate file).
    pub inline_pos: usize,
}

impl BulkData {
    pub fn read(r: &mut Reader) -> Result<Self> {
        let flags = r.u32()?;
        let element_count = r.i32()?;
        let size_on_disk = r.i32()?;
        let offset = r.i32()?;
        let inline_pos = r.pos;
        if flags & (BULK_SEPARATE_FILE | BULK_UNUSED) == 0 && size_on_disk > 0 {
            r.skip(size_on_disk as usize)?;
        }
        Ok(Self { flags, element_count, size_on_disk, offset, inline_pos })
    }
    pub fn is_empty(&self) -> bool {
        self.flags & BULK_UNUSED != 0 || self.element_count <= 0
    }
    fn compression(&self) -> u32 {
        if self.flags & BULK_LZO != 0 {
            crate::package::COMPRESS_LZO
        } else if self.flags & BULK_ZLIB != 0 {
            crate::package::COMPRESS_ZLIB
        } else {
            0
        }
    }
    /// Load payload; `export_data` is the export slice the header was read from.
    pub fn load(&self, export_data: &[u8], tfc: Option<&str>) -> Result<Vec<u8>> {
        if self.is_empty() {
            bail!("bulk data unused");
        }
        let raw: Vec<u8> = if self.flags & BULK_SEPARATE_FILE != 0 {
            let name = tfc.context("separate bulk data without TextureFileCacheName")?;
            read_tfc(name, self.offset as u64, self.size_on_disk as usize)?
        } else {
            export_data[self.inline_pos..self.inline_pos + self.size_on_disk as usize].to_vec()
        };
        let c = self.compression();
        if c != 0 { decompress_chunk(&raw, c) } else { Ok(raw) }
    }
}

static TFC_PATHS: Mutex<Option<HashMap<String, std::path::PathBuf>>> = Mutex::new(None);

fn find_tfc(name: &str) -> Option<std::path::PathBuf> {
    let mut g = TFC_PATHS.lock().unwrap();
    let map = g.get_or_insert_with(|| {
        let mut m = HashMap::new();
        fn walk(d: &std::path::Path, m: &mut HashMap<String, std::path::PathBuf>) {
            if let Ok(rd) = std::fs::read_dir(d) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        walk(&p, m);
                    } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("tfc")) {
                        let k = p.file_stem().unwrap().to_string_lossy().to_lowercase();
                        m.entry(k).or_insert(p);
                    }
                }
            }
        }
        walk(&crate::game_dir().join("GoatGame"), &mut m);
        m
    });
    map.get(&name.to_lowercase()).cloned()
}

/// Also look for texture caches under `dir` (a Workshop map item ships its own Textures_*.tfc);
/// these win over the game's caches of the same name.
pub fn add_tfc_dir(dir: &std::path::Path) {
    let _ = find_tfc("");
    let mut g = TFC_PATHS.lock().unwrap();
    let Some(map) = g.as_mut() else { return };
    fn walk(d: &std::path::Path, m: &mut HashMap<String, std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, m);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("tfc")) {
                    let k = p.file_stem().unwrap().to_string_lossy().to_lowercase();
                    m.insert(k, p);
                }
            }
        }
    }
    walk(dir, map);
}

fn read_tfc(name: &str, offset: u64, len: usize) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let path = find_tfc(name).with_context(|| format!("texture cache {name}.tfc not found"))?;
    let mut f = std::fs::File::open(&path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut v = vec![0u8; len];
    f.read_exact(&mut v)?;
    Ok(v)
}

#[derive(Debug, Clone)]
pub struct Mip {
    pub bulk: BulkData,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone)]
pub struct Texture {
    pub name: String,
    pub format: String,
    pub width: i32,
    pub height: i32,
    pub tfc: Option<String>,
    pub mips: Vec<Mip>,
    pub srgb: bool,
    pub address_x: String,
    pub address_y: String,
}

/// Decoded RGBA8 image.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Texture {
    pub fn parse(p: &Package, export: usize) -> Result<Self> {
        let data = p.export_data(export);
        let (end, ps) = props::export_props(p, export)?;
        let int = |n: &str| match props::find(&ps, n) {
            Some(Value::Int(i)) => *i,
            _ => 0,
        };
        let format = match props::find(&ps, "Format") {
            Some(Value::Enum(s)) => s.clone(),
            _ => "PF_Unknown".into(),
        };
        let tfc = match props::find(&ps, "TextureFileCacheName") {
            Some(Value::Name(s)) if s != "None" => Some(s.clone()),
            _ => None,
        };
        let srgb = !matches!(props::find(&ps, "SRGB"), Some(Value::Bool(false)));
        let addr = |n: &str| match props::find(&ps, n) {
            Some(Value::Enum(s)) => s.clone(),
            _ => "TA_Wrap".into(),
        };
        let mut r = Reader::at(data, end);
        BulkData::read(&mut r)?; // SourceArt
        let n = r.i32()?;
        let mut mips = Vec::new();
        for _ in 0..n.clamp(0, 16) {
            let bulk = BulkData::read(&mut r)?;
            let width = r.i32()?;
            let height = r.i32()?;
            mips.push(Mip { bulk, width, height });
        }
        Ok(Texture {
            name: p.obj_name(export as i32 + 1),
            format,
            width: int("SizeX"),
            height: int("SizeY"),
            tfc,
            mips,
            srgb,
            address_x: addr("AddressX"),
            address_y: addr("AddressY"),
        })
    }

    /// Decode the largest available mip not wider than `max_size`.
    pub fn decode(&self, p: &Package, export: usize, max_size: i32) -> Result<Image> {
        let data = p.export_data(export);
        let mut last = None;
        for m in &self.mips {
            if m.bulk.is_empty() || (m.width > max_size && m.height > max_size) {
                continue;
            }
            match m.bulk.load(data, self.tfc.as_deref()).and_then(|bytes| decode_pixels(&self.format, m.width, m.height, &bytes)) {
                Ok(img) => return Ok(img),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| anyhow::anyhow!("{}: no loadable mip", self.name)))
    }
}

fn decode_blocks(format: &str, src: &[u8], w: usize, h: usize, bytes: usize, f: impl Fn(&[u8], &mut [u8; 64])) -> Result<Vec<u8>> {
    let bw = w.div_ceil(4);
    let bh = h.div_ceil(4);
    if src.len() < bw * bh * bytes {
        bail!("{format}: {w}x{h} needs {} bytes, have {}", bw * bh * bytes, src.len());
    }
    let mut out = vec![255u8; w * h * 4];
    let mut tmp = [0u8; 64];
    for by in 0..bh {
        for bx in 0..bw {
            let o = (by * bw + bx) * bytes;
            f(&src[o..o + bytes], &mut tmp);
            for y in 0..4 {
                for x in 0..4 {
                    let (px, py) = (bx * 4 + x, by * 4 + y);
                    if px < w && py < h {
                        let d = (py * w + px) * 4;
                        out[d..d + 4].copy_from_slice(&tmp[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4]);
                    }
                }
            }
        }
    }
    Ok(out)
}

pub fn decode_pixels(format: &str, w: i32, h: i32, src: &[u8]) -> Result<Image> {
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let mut out = vec![255u8; w * h * 4];
    match format {
        "PF_DXT1" => out = decode_blocks(format, src, w, h, 8, |b, t| bcdec_rs::bc1(b, t, 16))?,
        "PF_DXT3" => out = decode_blocks(format, src, w, h, 16, |b, t| bcdec_rs::bc2(b, t, 16))?,
        "PF_DXT5" => out = decode_blocks(format, src, w, h, 16, |b, t| bcdec_rs::bc3(b, t, 16))?,
        "PF_BC5" => out = decode_blocks(format, src, w, h, 16, |b, t| {
            let mut rg = [0u8; 32];
            bcdec_rs::bc5(b, &mut rg, 8, false);
            for i in 0..16 {
                let x = rg[i * 2] as f32 / 127.5 - 1.0;
                let y = rg[i * 2 + 1] as f32 / 127.5 - 1.0;
                let z = (1.0 - x * x - y * y).max(0.0).sqrt();
                t[i * 4] = rg[i * 2];
                t[i * 4 + 1] = rg[i * 2 + 1];
                t[i * 4 + 2] = ((z * 0.5 + 0.5) * 255.0) as u8;
                t[i * 4 + 3] = 255;
            }
        })?,
        "PF_A8R8G8B8" => {
            if src.len() < w * h * 4 {
                bail!("A8R8G8B8 short");
            }
            for i in 0..w * h {
                out[i * 4] = src[i * 4 + 2];
                out[i * 4 + 1] = src[i * 4 + 1];
                out[i * 4 + 2] = src[i * 4];
                out[i * 4 + 3] = src[i * 4 + 3];
            }
        }
        "PF_G8" => {
            if src.len() < w * h {
                bail!("G8 short");
            }
            for i in 0..w * h {
                out[i * 4..i * 4 + 3].fill(src[i]);
            }
        }
        _ => bail!("unsupported pixel format {format}"),
    }
    Ok(Image { width: w as u32, height: h as u32, rgba: out })
}
