//! G05-D SDR presentation contract. No command completion implies scanout.
pub const VERSION: u32 = 1;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use crate::terrain::metal::presentation::Display;
/// Aspect-fit physical pixel rectangle, top-left origin. Black outside.
pub fn fit(
    source: (usize, usize),
    target: (usize, usize),
    live: usize,
    budget: usize,
) -> Result<[f64; 4], String> {
    if [source.0, source.1, target.0, target.1]
        .iter()
        .any(|&n| n == 0 || n > 4096)
    {
        return Err("invalid display dimensions".into());
    }
    // Charge all three possible Core Animation buffers, not only the acquired one.
    let bytes = target
        .0
        .checked_mul(target.1)
        .and_then(|n| n.checked_mul(12))
        .and_then(|n| n.checked_add(live))
        .ok_or("display payload overflow")?;
    if bytes > budget {
        return Err("display budget exceeded".into());
    }
    let scale = (target.0 as f64 / source.0 as f64).min(target.1 as f64 / source.1 as f64);
    let w = source.0 as f64 * scale;
    let h = source.1 as f64 * scale;
    Ok([(target.0 as f64 - w) / 2., (target.1 as f64 - h) / 2., w, h])
}
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod host;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aspect_orientation_and_budget() {
        assert_eq!(
            fit((256, 256), (512, 256), 0, 2_000_000).unwrap(),
            [128., 0., 256., 256.]
        );
        assert_eq!(
            fit((256, 256), (256, 512), 0, 2_000_000).unwrap(),
            [0., 128., 256., 256.]
        );
        for dims in [(0, 1), (4097, 1), (usize::MAX, usize::MAX)] {
            assert!(fit((256, 256), dims, 0, usize::MAX).is_err());
        }
        assert!(fit((256, 256), (256, 256), usize::MAX, usize::MAX).is_err());
        assert!(fit((256, 256), (256, 256), 0, 1).is_err());
    }
}
