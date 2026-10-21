//! StaticMeshComponents whose mesh path contains a substring, with their collision flags:
//! mesh_comps <package> <substring>
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    let want = a[2].to_lowercase();
    for i in 0..p.exports.len() {
        let class = p.class_name(i as i32 + 1);
        if !class.contains("StaticMeshComponent") {
            continue;
        }
        let Ok((_, ps)) = upk::props::export_props(&p, i) else { continue };
        let mesh = ps.iter().find(|x| x.name == "StaticMesh").and_then(|x| match &x.value {
            upk::props::Value::Object(o) => Some(p.obj_path(*o)),
            _ => None,
        });
        let Some(mesh) = mesh else { continue };
        if !mesh.to_lowercase().contains(&want) {
            continue;
        }
        let flags: Vec<String> = ps
            .iter()
            .filter(|x| x.name.contains("Collide") || x.name.contains("Block") || x.name.contains("RBChannel") || x.name.contains("Collision"))
            .map(|x| format!("{}={:?}", x.name, x.value))
            .collect();
        println!("{} ({class}) mesh {mesh} {flags:?}", p.obj_path(i as i32 + 1));
    }
    Ok(())
}
