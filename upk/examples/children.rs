//! List exports whose path starts with a prefix: children <package> <prefix>
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    for i in 0..p.exports.len() {
        let path = p.obj_path(i as i32 + 1);
        if path.starts_with(&a[2]) {
            println!("{path} ({}) {} bytes", p.class_name(i as i32 + 1), p.export_data(i).len());
        }
    }
    Ok(())
}
