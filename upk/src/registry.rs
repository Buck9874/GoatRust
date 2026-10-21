//! Several loaded packages with cross-package import resolution (like UE3's linker).

use crate::package::{FName, Package};
use crate::props::{self, Prop, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

/// An object inside one of the registry's packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjRef {
    pub pkg: usize,
    /// 0-based export index.
    pub export: usize,
}

pub struct Registry {
    pub packages: Vec<Package>,
    index: Vec<OnceLock<HashMap<String, usize>>>,
}

impl Registry {
    pub fn new(packages: Vec<Package>) -> Self {
        let index = packages.iter().map(|_| OnceLock::new()).collect();
        Self { packages, index }
    }

    fn path_index(&self, pkg: usize) -> &HashMap<String, usize> {
        self.index[pkg].get_or_init(|| {
            let p = &self.packages[pkg];
            let mut m = HashMap::with_capacity(p.exports.len());
            for i in 0..p.exports.len() {
                if p.class_name(i as i32 + 1) == "Package" {
                    continue;
                }
                m.entry(p.obj_path(i as i32 + 1).to_lowercase()).or_insert(i);
            }
            m
        })
    }

    /// Find an export by full dotted path in any package (first package wins).
    pub fn find_path(&self, path: &str) -> Option<ObjRef> {
        let key = path.to_lowercase();
        (0..self.packages.len()).find_map(|pkg| self.path_index(pkg).get(&key).map(|&export| ObjRef { pkg, export }))
    }

    /// Like `find_path`, but only an export of class `class`: a path can name different objects
    /// in different packages (GoatGame.u keeps an old StaticMesh MMO_Armor.Mesh.SpikeArmorBack,
    /// Startup.upk the SkeletalMesh the MMO classes use).
    pub fn find_path_class(&self, path: &str, class: &str) -> Option<ObjRef> {
        let key = path.to_lowercase();
        (0..self.packages.len()).find_map(|pkg| {
            let &export = self.path_index(pkg).get(&key)?;
            let p = &self.packages[pkg];
            if p.class_name(export as i32 + 1).eq_ignore_ascii_case(class) {
                return Some(ObjRef { pkg, export });
            }
            // a later export with the same path in this package (the index keeps the first)
            (0..p.exports.len()).find(|&i| p.class_name(i as i32 + 1).eq_ignore_ascii_case(class) && p.obj_path(i as i32 + 1).eq_ignore_ascii_case(path)).map(|export| ObjRef { pkg, export })
        })
    }

    /// Resolve an object index (export or import) seen in package `pkg`.
    pub fn resolve(&self, pkg: usize, obj: i32) -> Option<ObjRef> {
        if obj > 0 {
            return Some(ObjRef { pkg, export: obj as usize - 1 });
        }
        if obj < 0 {
            let path = self.packages[pkg].obj_path(obj);
            // an import may also be satisfied by an export with the same path in this package
            if let Some(r) = self.find_path(&path) {
                return Some(r);
            }
            // objects of a script/package file (GoatGame.Default__GGNpc...) are imported with the
            // file name as their outermost package, which their own exports don't repeat
            let (first, rest) = path.split_once('.')?;
            let target = self.packages.iter().position(|p| p.name.eq_ignore_ascii_case(first))?;
            return self.path_index(target).get(&rest.to_lowercase()).map(|&export| ObjRef { pkg: target, export });
        }
        None
    }

    pub fn pkg(&self, r: ObjRef) -> &Package {
        &self.packages[r.pkg]
    }

    pub fn class_name(&self, r: ObjRef) -> String {
        self.packages[r.pkg].class_name(r.export as i32 + 1)
    }

    pub fn path(&self, r: ObjRef) -> String {
        self.packages[r.pkg].obj_path(r.export as i32 + 1)
    }

    pub fn props(&self, r: ObjRef) -> Vec<Prop> {
        props::export_props(&self.packages[r.pkg], r.export).map(|x| x.1).unwrap_or_default()
    }

    /// Properties merged along the archetype chain across packages (instance values win).
    /// Object values stay relative to the package that defined them, so each returned prop
    /// carries its package index.
    pub fn merged_props(&self, r: ObjRef) -> Vec<(usize, Prop)> {
        let mut out: Vec<(usize, Prop)> = self.props(r).into_iter().map(|p| (r.pkg, p)).collect();
        let mut cur = r;
        for _ in 0..8 {
            let arch = self.packages[cur.pkg].exports[cur.export].archetype;
            let Some(next) = self.resolve(cur.pkg, arch) else { break };
            for q in self.props(next) {
                if !out.iter().any(|(_, o)| o.name == q.name && o.index == q.index) {
                    out.push((next.pkg, q));
                }
            }
            cur = next;
        }
        out
    }
}

/// Look up a merged property; returns the defining package with it.
pub fn find<'a>(props: &'a [(usize, Prop)], name: &str) -> Option<(usize, &'a Value)> {
    props.iter().find(|(_, p)| p.name.eq_ignore_ascii_case(name)).map(|(pk, p)| (*pk, &p.value))
}

/// Decode an ArrayProperty of FNames.
pub fn name_array(p: &Package, v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(_, d)) => d
            .chunks_exact(8)
            .map(|c| {
                p.name_str(FName {
                    index: i32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                    number: i32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                })
            })
            .collect(),
        _ => vec![],
    }
}
