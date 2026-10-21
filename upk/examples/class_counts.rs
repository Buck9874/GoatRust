//! All export classes of a package with counts (no cap).
use std::collections::BTreeMap;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = upk::package::Package::open(&a[1])?;
    let mut c: BTreeMap<String, usize> = BTreeMap::new();
    for i in 0..p.exports.len() { *c.entry(p.class_name(i as i32 + 1)).or_default() += 1; }
    for (k, n) in c { if a.get(2).is_none_or(|f| k.to_lowercase().contains(&f.to_lowercase())) { println!("{n:6} {k}"); } }
    Ok(())
}
