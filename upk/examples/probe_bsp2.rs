//! Where is a map's BSP and which materials does it use?
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("map path");
    let p = Package::open(&path)?;
    let lvl = upk::level::load(&p)?;
    for (i, st) in lvl.player_starts.iter().enumerate() { println!("start {i}: {:?}", st.location); }
    // pool tiles centre
    if let Some(b) = &lvl.bsp {
        let mut c = [0f32; 3]; let mut n = 0.0;
        for (s, pts) in &b.polys { if p.obj_path(b.surfs[*s].material).contains("Pool_Tiles_Mat_01") { for q in pts { for k in 0..3 { c[k] += q[k]; } n += 1.0; } } }
        println!("pool centre {:?}", [c[0] / n, c[1] / n, c[2] / n]);
    }
    let Some(b) = lvl.bsp else { println!("no bsp"); return Ok(()) };
    let mut mats = std::collections::BTreeMap::<String, (usize, f32)>::new();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (s, pts) in &b.polys {
        let m = b.surfs[*s].material;
        let name = if m == 0 { "None".into() } else { p.obj_path(m) };
        let e = mats.entry(name).or_default();
        e.0 += 1;
        for q in pts {
            for k in 0..3 { lo[k] = lo[k].min(q[k]); hi[k] = hi[k].max(q[k]); }
        }
        // rough area
        for k in 1..pts.len() - 1 {
            let a = [pts[k][0] - pts[0][0], pts[k][1] - pts[0][1], pts[k][2] - pts[0][2]];
            let c = [pts[k + 1][0] - pts[0][0], pts[k + 1][1] - pts[0][1], pts[k + 1][2] - pts[0][2]];
            let x = [a[1] * c[2] - a[2] * c[1], a[2] * c[0] - a[0] * c[2], a[0] * c[1] - a[1] * c[0]];
            e.1 += (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt() * 0.5 / 1e4;
        }
    }
    println!("bounds {lo:?} .. {hi:?}");
    let mut v: Vec<_> = mats.into_iter().collect();
    v.sort_by(|a, b| b.1 .1.total_cmp(&a.1 .1));
    for (n, (c, area)) in v.iter().take(15) { println!("{c:5} polys {area:9.1} m2  {n}"); }
    Ok(())
}
