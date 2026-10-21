//! Mesh instances per actor class (dynamic flag as the loader decides it).
use std::collections::BTreeMap;
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let p = Package::open(std::env::args().nth(1).expect("map"))?;
    let lvl = upk::level::load(&p)?;
    let mut c: BTreeMap<(String, bool), usize> = BTreeMap::new();
    for i in &lvl.instances { *c.entry((i.actor_class.clone(), i.dynamic)).or_default() += 1; }
    for ((k, d), n) in c { println!("{n:6} {k} dynamic={d}"); }
    Ok(())
}
