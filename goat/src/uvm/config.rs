//! Config (ini) values for `var config` properties, like UE3's class config loading:
//! Engine/Config/Base<Name>.ini -> GoatGame/Config/Common/Default<Name>.ini ->
//! GoatGame/Config/Vanilla/Default<Name>.ini, each class reading its own [Package.Class] section
//! for every config property it has (inherited ones too). Values are UE3 "import text".

use super::*;
use std::collections::HashMap;

pub const CPF_CONFIG: u64 = 0x4000;
pub const CPF_LOCALIZED: u64 = 0x8000;
pub const CPF_GLOBAL_CONFIG: u64 = 0x40000;

/// section (lower case) -> key (lower case) -> values in order.
#[derive(Default)]
pub struct Ini {
    pub sections: HashMap<String, Vec<(String, Vec<String>)>>,
}

/// Read an ini/int file as text (the game's .int files are UTF-16).
fn read_text(p: &std::path::Path) -> Option<String> {
    let b = std::fs::read(p).ok()?;
    if b.starts_with(&[0xFF, 0xFE]) {
        let u: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return Some(String::from_utf16_lossy(&u));
    }
    Some(String::from_utf8_lossy(&b).into_owned())
}

impl Ini {
    /// Localization/INT/<Package>.int (sections are bare class names).
    pub fn load_localization(package: &str) -> Ini {
        let dir = upk::game_dir().join("GoatGame").join("Localization").join("INT");
        let mut ini = Ini::default();
        // file names vary in case (GoatSpace.INT)
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if let Some((stem, ext)) = n.rsplit_once('.') {
                    if stem.eq_ignore_ascii_case(package) && ext.eq_ignore_ascii_case("int") {
                        if let Some(t) = read_text(&e.path()) {
                            ini.apply(&t);
                        }
                    }
                }
            }
        }
        ini
    }

    pub fn load(name: &str) -> Ini {
        let game = upk::game_dir();
        let mut ini = Ini::default();
        for p in [
            game.join("Engine").join("Config").join(format!("Base{name}.ini")),
            game.join("GoatGame").join("Config").join("Common").join(format!("Default{name}.ini")),
            game.join("GoatGame").join("Config").join("Vanilla").join(format!("Default{name}.ini")),
        ] {
            if let Some(t) = read_text(&p) {
                ini.apply(&t);
            }
        }
        ini
    }

    fn apply(&mut self, text: &str) {
        let mut section = String::new();
        // plain Key=Value: the first one in this file replaces what earlier files set, repeats in
        // the same file add entries (DefaultDLC.ini lists mAllMaps that way)
        let mut plain_seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].to_ascii_lowercase();
                continue;
            }
            let (op, rest) = match line.chars().next() {
                Some(c @ ('+' | '.' | '-' | '!')) => (c, &line[1..]),
                _ => ('=', line),
            };
            let (key, val) = match rest.split_once('=') {
                Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim().to_string()),
                None => (rest.trim().to_ascii_lowercase(), String::new()),
            };
            let sec = self.sections.entry(section.clone()).or_default();
            let i = match sec.iter().position(|(k, _)| *k == key) {
                Some(i) => i,
                None => {
                    sec.push((key, vec![]));
                    sec.len() - 1
                }
            };
            let first_plain = op == '=' && plain_seen.insert((section.clone(), sec[i].0.clone()));
            let vals = &mut sec[i].1;
            match op {
                '+' => {
                    if !vals.contains(&val) {
                        vals.push(val);
                    }
                }
                '.' => vals.push(val),
                '-' => vals.retain(|v| *v != val),
                '!' => vals.clear(),
                _ => {
                    if first_plain {
                        *vals = vec![val];
                    } else {
                        vals.push(val);
                    }
                }
            }
        }
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&Vec<String>> {
        let sec = self.sections.get(&section.to_ascii_lowercase())?;
        let k = key.to_ascii_lowercase();
        sec.iter().find(|(kk, _)| *kk == k).map(|(_, v)| v)
    }
}

impl Vm {
    /// Config file name of a class (inherited from the parent when the class names none).
    pub fn config_name(&self, mut c: usize) -> Option<String> {
        loop {
            let cl = &self.classes[c];
            if let Some(n) = &cl.config {
                return Some(n.clone());
            }
            c = cl.super_?;
        }
    }

    /// `var localized` values from Localization/INT/<Package>.int [Class].
    pub fn apply_localized(&mut self, c: usize, cdo: ObjId) {
        let Some(r) = self.classes[c].r else { return };
        // seek-free/Startup classes live under a Package export: use that package's name
        let p = &self.reg.packages[r.pkg];
        let outer = p.obj_outer(r.export as i32 + 1);
        let pkg = if outer > 0 { p.obj_name(outer) } else { p.name.clone() };
        let key = format!("int:{}", pkg.to_ascii_lowercase());
        if !self.inis.contains_key(&key) {
            let ini = Ini::load_localization(&pkg);
            self.inis.insert(key.clone(), ini);
        }
        let section = self.class_name(c).to_string();
        let props: Vec<PropDef> = self.classes[c].props.iter().filter(|p| p.flags & CPF_LOCALIZED != 0).cloned().collect();
        for pd in props {
            let name = self.name_of(pd.name).to_string();
            let Some(v) = self.inis[&key].get(&section, &name).and_then(|v| v.last().cloned()) else { continue };
            let val = self.import_text(&v, &pd.ty);
            self.obj_mut(cdo).props.insert(pd.name, val);
        }
    }

    /// Apply the class's [Package.Class] section to its default object.
    pub fn apply_config(&mut self, c: usize, cdo: ObjId) {
        let Some(file) = self.config_name(c) else { return };
        let Some(r) = self.classes[c].r else { return };
        let pkg = self.reg.packages[r.pkg].name.clone();
        let section = format!("{pkg}.{}", self.class_name(c));
        if !self.inis.contains_key(&file) {
            let ini = Ini::load(&file);
            self.inis.insert(file.clone(), ini);
        }
        // config properties along the chain
        let mut props = vec![];
        let mut k = Some(c);
        while let Some(ci) = k {
            props.extend(self.classes[ci].props.iter().filter(|p| p.flags & (CPF_CONFIG | CPF_GLOBAL_CONFIG) != 0).cloned());
            k = self.classes[ci].super_;
        }
        for pd in props {
            let name = self.name_of(pd.name).to_string();
            let Some(vals) = self.inis[&file].get(&section, &name).cloned() else { continue };
            let v = match &pd.ty {
                Ty::Array(inner) => {
                    let inner = (**inner).clone();
                    Value::Array(vals.iter().map(|s| self.import_text(s, &inner)).collect())
                }
                ty => match vals.last() {
                    Some(s) => self.import_text(s, ty),
                    None => continue,
                },
            };
            self.obj_mut(cdo).props.insert(pd.name, v);
        }
    }

    /// UE3 ImportText for a value of a declared type.
    pub fn import_text(&mut self, s: &str, ty: &Ty) -> Value {
        let s = s.trim();
        let unq = s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(s);
        match ty {
            Ty::Int => Value::Int(unq.parse::<f64>().map(|f| f as i32).unwrap_or(0)),
            Ty::Float => Value::Float(unq.trim_end_matches('f').parse().unwrap_or(0.0)),
            Ty::Bool => Value::Bool(matches!(unq.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on")),
            Ty::Byte(e) => match unq.parse::<i32>() {
                Ok(i) => Value::Byte(i as u8),
                Err(_) => {
                    let i = e.and_then(|e| self.enums[e].names.iter().position(|n| n.eq_ignore_ascii_case(unq))).unwrap_or(0);
                    Value::Byte(i as u8)
                }
            },
            Ty::Name => Value::Name(self.sym(unq)),
            Ty::Str => Value::Str(unq.to_string()),
            Ty::Object | Ty::Class | Ty::Interface => {
                if unq.eq_ignore_ascii_case("none") || unq.is_empty() {
                    return Value::Obj(None);
                }
                // Class'Pkg.Name' or Pkg.Name
                let path = unq.split('\'').nth(1).unwrap_or(unq);
                let found = self.reg.find_path(path).or_else(|| path.split_once('.').and_then(|(_, rest)| self.reg.find_path(rest)));
                Value::Obj(found.and_then(|r| self.obj_for(r)))
            }
            Ty::Struct(sid) => {
                let mut sv = self.struct_zero(*sid);
                let fields = self.structs[*sid].fields.clone();
                for (k, v) in split_struct(s) {
                    let y = self.sym(&k);
                    let Some(f) = fields.iter().find(|f| f.name == y) else { continue };
                    let val = self.import_text(&v, &f.ty);
                    *sv.field_mut(y) = val;
                }
                Value::Struct(Box::new(sv))
            }
            Ty::Array(inner) => {
                let inner = (**inner).clone();
                let body = s.strip_prefix('(').and_then(|x| x.strip_suffix(')')).unwrap_or(s);
                Value::Array(split_top(body, ',').iter().map(|x| self.import_text(x, &inner)).collect())
            }
            _ => self.zero(ty, 1),
        }
    }
}

/// "(A=1,B=(X=2),C="x,y")" -> [(A, 1), (B, (X=2)), (C, "x,y")]
fn split_struct(s: &str) -> Vec<(String, String)> {
    let body = s.trim().strip_prefix('(').and_then(|x| x.strip_suffix(')')).unwrap_or(s);
    split_top(body, ',')
        .into_iter()
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// Split on `sep` outside quotes and parentheses.
fn split_top(s: &str, sep: char) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let (mut depth, mut quote) = (0i32, false);
    for ch in s.chars() {
        match ch {
            '"' => quote = !quote,
            '(' if !quote => depth += 1,
            ')' if !quote => depth -= 1,
            c if c == sep && !quote && depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}
