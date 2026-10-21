//! Where opening a movie spends its time (device, parse, fonts, first frames).
//! usage: load_timing <movie folder (e.g. %TEMP%/goatrust_gfx)> <rel path> <fonts swf>
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::Path::new(&args[0]);
    for round in 0..2 {
        let t = std::time::Instant::now();
        let m = gfxui::Movie::load_file(dir, &args[1], 1280, 720)?;
        let t_load = t.elapsed();
        let fonts = std::fs::read(&args[2])?;
        m.register_fonts(&fonts, &[("$NormalFont", "Bebas Neue")])?;
        let t_fonts = t.elapsed();
        for _ in 0..30 {
            m.run_frame();
            if m.root().is_ok() {
                break;
            }
        }
        let t_root = t.elapsed();
        let _ = m.capture();
        let t_first = t.elapsed();
        println!("round {round}: load {t_load:?}, +fonts {:?}, +root {:?}, +first render {:?}", t_fonts - t_load, t_root - t_fonts, t_first - t_root);
    }
    Ok(())
}
