//! Separate G02 runner; never launches the game or reads game assets/worlds.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let image = helium_metal_prototype::terrain::smoke_test().map_err(std::io::Error::other)?;
    println!(
        "G02 full-image oracle PASS; single upload+render+readback command seconds: {:?}",
        image.gpu_seconds
    );
    if let Some(path) = std::env::args_os().nth(1) {
        use std::io::Write;
        let mut file = std::fs::File::create_new(path)?;
        writeln!(file, "P6\n256 256\n255")?;
        for pixel in image.rgba.chunks_exact(4) {
            file.write_all(&pixel[..3])?;
        }
    }
    Ok(())
}
