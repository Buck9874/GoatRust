//! UE3 Font: glyph rectangles on texture pages (Characters, Textures properties) and the
//! character -> glyph remap table serialized after the properties when IsRemapped.

use crate::props::{self, Value};
use crate::reader::Reader;
use crate::Package;
use anyhow::Result;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default)]
pub struct Glyph {
    /// Texels on its page.
    pub u: i32,
    pub v: i32,
    pub w: i32,
    pub h: i32,
    pub page: u8,
    pub voff: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Font {
    pub glyphs: Vec<Glyph>,
    /// Texture page object paths.
    pub pages: Vec<String>,
    pub remap: HashMap<u16, u16>,
    pub remapped: bool,
    /// Tallest glyph (texels): Canvas.StrLen's YL.
    pub max_height: i32,
}

impl Font {
    pub fn parse(p: &Package, export: usize) -> Result<Font> {
        let (end, ps) = props::export_props(p, export)?;
        let mut f = Font::default();
        for q in &ps {
            match (q.name.as_str(), &q.value) {
                // FontCharacter: StartU, StartV, USize, VSize (int), TextureIndex (byte), VerticalOffset (int)
                ("Characters", Value::Array(n, raw)) => {
                    let mut r = Reader::new(raw);
                    for _ in 0..*n {
                        let g = Glyph { u: r.i32()?, v: r.i32()?, w: r.i32()?, h: r.i32()?, page: r.u8()?, voff: r.i32()? };
                        f.glyphs.push(g);
                    }
                }
                ("Textures", Value::Array(n, raw)) => {
                    let mut r = Reader::new(raw);
                    for _ in 0..*n {
                        let o = r.i32()?;
                        f.pages.push(p.obj_path(o));
                    }
                }
                ("IsRemapped", v) => f.remapped = matches!(v, Value::Int(1) | Value::Bool(true) | Value::Byte(1)),
                _ => {}
            }
        }
        // Font::Serialize: CharRemap (TMap<WORD, WORD>) after the properties
        let data = p.export_data(export);
        if let Some(rest) = data.get(end..) {
            let mut r = Reader::new(rest);
            if let Ok(n) = r.i32() {
                if (0..=65536).contains(&n) {
                    for _ in 0..n {
                        let (Ok(k), Ok(v)) = (r.u16(), r.u16()) else { break };
                        f.remap.insert(k, v);
                    }
                }
            }
        }
        f.max_height = f.glyphs.iter().map(|g| g.h).max().unwrap_or(0);
        Ok(f)
    }

    /// The glyph for a character (UFont::RemapChar), if the font has one.
    pub fn glyph(&self, c: char) -> Option<&Glyph> {
        let code = c as u32;
        let idx = if self.remapped {
            if code > 0xFFFF {
                return None;
            }
            *self.remap.get(&(code as u16))? as usize
        } else {
            code as usize
        };
        self.glyphs.get(idx)
    }

    /// Width and height of a line in texels (UCanvas::StrLen without scaling).
    pub fn measure(&self, s: &str) -> (f32, f32) {
        let w: i32 = s.chars().filter_map(|c| self.glyph(c)).map(|g| g.w).sum();
        (w as f32, self.max_height as f32)
    }
}
