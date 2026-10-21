//! List the AS3 classes defined in a SWF's DoABC tags.
use ruffle_core::swf::avm2::types::{Multiname, Namespace};
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap();
    let filter = std::env::args().nth(2).unwrap_or_default();
    let data = std::fs::read(&path)?;
    let buf = ruffle_core::swf::decompress_swf(&data[..])?;
    let swf = ruffle_core::swf::parse_swf(&buf)?;
    for tag in &swf.tags {
        let abc_data = match tag {
            ruffle_core::swf::Tag::DoAbc(d) => *d,
            ruffle_core::swf::Tag::DoAbc2(d) => d.data,
            _ => continue,
        };
        let abc = ruffle_core::swf::avm2::read::Reader::new(abc_data).read()?;
        let cp = &abc.constant_pool;
        let s = |i: u32| if i == 0 { String::new() } else { String::from_utf8_lossy(&cp.strings[i as usize - 1]).to_string() };
        println!("{} classes", abc.instances.len());
        for inst in &abc.instances {
            if let Multiname::QName { namespace, name } = &cp.multinames[inst.name.0 as usize - 1] {
                let ns = match &cp.namespaces[namespace.0 as usize - 1] {
                    Namespace::Package(n) | Namespace::PackageInternal(n) => s(n.0),
                    _ => "?".into(),
                };
                let full = if ns.is_empty() { s(name.0) } else { format!("{ns}.{}", s(name.0)) };
                if full.contains(&filter) {
                    println!("  {full}");
                }
            }
        }
    }
    Ok(())
}
