//! Kismet ops whose OutputLinks mention an object: links_to <package> <object path>
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    let target = (0..p.exports.len()).find(|&i| p.obj_path(i as i32 + 1) == a[2]).expect("not found") as i32 + 1;
    let needle = target.to_le_bytes();
    for i in 0..p.exports.len() {
        let Ok((_, ps)) = upk::props::export_props(&p, i) else { continue };
        for x in ps.iter().filter(|x| x.name == "OutputLinks" || x.name == "Targets" || x.name == "VariableLinks") {
            if let upk::props::Value::Array(_, d) = &x.value {
                if d.windows(4).any(|w| w == needle) {
                    println!("{} ({}) via {}", p.obj_path(i as i32 + 1), p.class_name(i as i32 + 1), x.name);
                }
            }
        }
    }
    Ok(())
}
