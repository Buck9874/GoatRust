//! Render a dumped GFx movie (out/swf/*.gfx, see upk/examples/dump_swf.rs) to PNGs.
//! usage: cargo run -p gfxui --release --example render_gfx -- <file.gfx> <out.png> [frames]
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0])?;
    let frames: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let movie = gfxui::Movie::load(&data, "file:///menu.swf", 1280, 720)?;
    for _ in 0..frames {
        movie.run_frame();
    }
    let img = movie.capture().ok_or_else(|| anyhow::anyhow!("no frame"))?;
    img.save(&args[1])?;
    println!("saved {} ({}x{}) after {frames} frames", args[1], img.width(), img.height());
    Ok(())
}
