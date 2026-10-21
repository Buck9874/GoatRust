//! Paths of all exports of a class (optionally filtered by a substring).
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = upk::package::Package::open(&a[1])?;
    for i in 0..p.exports.len() {
        if p.class_name(i as i32 + 1) != a[2] { continue; }
        let path = p.obj_path(i as i32 + 1);
        if a.get(3).is_none_or(|f| path.to_lowercase().contains(&f.to_lowercase())) { println!("{path}"); }
    }
    Ok(())
}
