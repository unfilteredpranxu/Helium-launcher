fn main() -> Result<(), Box<dyn std::error::Error>> {
    let image = helium_metal_prototype::smoke_test().map_err(std::io::Error::other)?;
    println!(
        "Metal terrain/depth smoke test passed; GPU render+blit seconds: {:?}",
        image.gpu_seconds
    );
    if let Some(path) = std::env::args_os().nth(1) {
        use std::io::Write;
        let mut file = std::fs::File::create_new(path)?;
        writeln!(file, "P6\n256 256\n255")?;
        for p in image.rgba.chunks_exact(4) {
            file.write_all(&p[..3])?;
        }
    }
    Ok(())
}
