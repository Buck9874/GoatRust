//! Print the paths of object references: obj_name <package> <ref>...
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    for r in &a[2..] {
        let r: i32 = r.parse()?;
        println!("{r}: {} ({})", p.obj_path(r), p.class_name(r));
    }
    Ok(())
}
