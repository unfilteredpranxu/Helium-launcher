//! Production types at the G04 boundary. No dependency from production to the experiment.
use helium_metal_prototype::material::{self, Epoch, Lightmap, Provenance};
use horizon_client_render::{gfx::Renderer, mipmap, sprites::Sprites};

/// Borrow metadata rather than translating mip strategies or losing resource names.
/// `Sprites::pixels` is the initial base image, NOT the renderer's animated mip
/// snapshot. It must not be advertised as a current full GPU atlas.
pub struct Source<'a> {
    pub pixels: &'a [u8],
    pub settings: &'a [mipmap::Settings],
    pub opaque: &'a [bool],
    pub translucent: &'a [bool],
    pub names: &'a std::collections::BTreeMap<String, u32>,
    pub epoch: &'a Epoch,
    pub layers: u32,
    pub size: u32,
}
pub fn source<'a>(
    sprites: &'a Sprites,
    epoch: &'a Epoch,
    budget: usize,
) -> Result<Source<'a>, String> {
    let layers = u32::try_from(sprites.mips.len()).map_err(|_| "sprite count overflow")?;
    let bytes = material::chain_bytes(sprites.size, 1, layers)?;
    if layers > 32768 || bytes > budget || bytes != sprites.pixels.len() {
        return Err("invalid sprite base payload/count or source budget".into());
    }
    if sprites.opaque.len() != layers as usize
        || sprites.translucent.len() != layers as usize
        || sprites.layers.values().any(|&id| id >= layers)
        || sprites
            .mips
            .iter()
            .any(|m| !m.alpha_cutoff_bias.is_finite())
    {
        return Err("invalid sprite identity or material metadata".into());
    }
    if !sprites.normals.is_empty() || !sprites.specular.is_empty() {
        return Err("LabPBR material maps unsupported".into());
    }
    Ok(Source {
        pixels: &sprites.pixels,
        settings: &sprites.mips,
        opaque: &sprites.opaque,
        translucent: &sprites.translucent,
        names: &sprites.layers,
        epoch,
        layers,
        size: sprites.size,
    })
}
/// Call after production update_lightmap. An untouched renderer has zero alpha
/// and is rejected. Revision belongs to the integration owner and must advance
/// when these bytes change. This does not copy or synthesize light.
pub fn lightmap<'a>(
    renderer: &'a Renderer,
    epoch: &Epoch,
    revision: u64,
) -> Result<Lightmap<'a>, String> {
    if renderer
        .lightmap_pixels
        .chunks_exact(4)
        .any(|p| p[3] != 255)
    {
        return Err("production lightmap has not been generated".into());
    }
    Ok(Lightmap {
        pixels: &renderer.lightmap_pixels,
        provenance: Provenance::ProductionGenerated,
        stamp: epoch.stamp(),
        revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handcrafted_production_sprite_types_preserve_identity_and_metadata() {
        let mut sprites = Sprites::default();
        sprites.size = 1;
        sprites.pixels = vec![255; 257 * 4];
        sprites.mips = vec![mipmap::Settings::default(); 257];
        sprites.opaque = vec![true; 257];
        sprites.translucent = vec![false; 257];
        sprites.layers.insert("handcrafted:last".into(), 256);
        let epoch = Epoch::new().unwrap();
        let borrowed = source(&sprites, &epoch, 1028).unwrap();
        assert_eq!(borrowed.names["handcrafted:last"], 256);
        assert_eq!(borrowed.pixels.as_ptr(), sprites.pixels.as_ptr());
        assert_eq!(borrowed.settings.as_ptr(), sprites.mips.as_ptr());
        assert!(source(&sprites, &epoch, 1027).is_err());
        sprites.layers.insert("handcrafted:missing".into(), 257);
        assert!(source(&sprites, &epoch, 1028).is_err());
    }
}
