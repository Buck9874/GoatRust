//! List exports of a class with a few properties (actor placement, mesh).
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    let mut n = 0;
    for i in 0..p.exports.len() {
        if p.class_name(i as i32 + 1) != a[2] { continue; }
        n += 1;
        if n > 8 { continue; }
        let ps = upk::level::merged_props(&p, i);
        let mesh = ps.iter().find(|x| x.name == "StaticMeshComponent").and_then(|x| match x.value { upk::props::Value::Object(o) if o > 0 => Some(o), _ => None })
            .and_then(|c| upk::level::merged_props(&p, c as usize - 1).into_iter().find(|x| x.name == "StaticMesh")).map(|x| format!("{:?}", x.value));
        println!("{} {:?} mesh {:?}", p.obj_path(i as i32 + 1), ps.iter().filter(|x| x.name.starts_with('m')).map(|x| format!("{}={:?}", x.name, x.value)).collect::<Vec<_>>(), mesh);
    }
    println!("{n} total");
    Ok(())
}
