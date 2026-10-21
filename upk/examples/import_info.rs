use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    let i: i32 = a[2].parse()?;
    println!("{} class {}", p.obj_path(i), p.class_name(i));
    Ok(())
}
