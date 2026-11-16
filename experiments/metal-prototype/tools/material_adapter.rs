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

/// One experimental namespace bound to one actual renderer generation. A new
/// binding cannot make an old production snapshot current.
pub struct Binding {
    generation: horizon_client_render::gfx::material_snapshot::Generation,
    epoch: Epoch,
}
impl Binding {
    pub fn new(renderer: &Renderer) -> Result<Self, String> {
        Ok(Self {
            generation: renderer.material_state().generation(),
            epoch: Epoch::new()?,
        })
    }
    /// Validated synchronous consumption; both renderer and experimental epoch
    /// remain borrowed. No bank pixel copies or heap descriptor allocation.
    pub fn consume<R>(
        &self,
        renderer: &Renderer,
        limits: material::Limits,
        consume: impl FnOnce(&material::Resources<'_>) -> R,
    ) -> Result<R, String> {
        renderer
            .validate_material_snapshot_mode()
            .map_err(str::to_owned)?;
        self.consume_state(
            renderer.material_state(),
            &renderer.lightmap_pixels,
            Provenance::ProductionGenerated,
            limits,
            consume,
        )
    }
    fn consume_state<R>(
        &self,
        state: &horizon_client_render::gfx::material_snapshot::State,
        pixels: &[u8; 1024],
        provenance: Provenance,
        limits: material::Limits,
        consume: impl FnOnce(&material::Resources<'_>) -> R,
    ) -> Result<R, String> {
        if state.generation() != self.generation {
            return Err("obsolete production binding".into());
        }
        let snapshot = state
            .current(state.snapshot().map_err(str::to_owned)?)
            .map_err(str::to_owned)?;
        let revision = state.light_revision().map_err(str::to_owned)?;
        let light = Lightmap {
            pixels,
            provenance,
            stamp: self.epoch.stamp(),
            revision,
        };
        translate(snapshot, &self.epoch, light, limits, consume)
    }
}
fn translate<R>(
    snapshot: &horizon_client_render::gfx::material_snapshot::Snapshot,
    epoch: &Epoch,
    light: Lightmap<'_>,
    limits: material::Limits,
    consume: impl FnOnce(&material::Resources<'_>) -> R,
) -> Result<R, String> {
    let first = snapshot.banks().first().ok_or("missing production banks")?;
    fn descriptor<'a>(
        bank: &'a horizon_client_render::gfx::material_snapshot::Bank,
        size: u32,
        stamp: material::Stamp,
    ) -> material::Bank<'a> {
        material::Bank {
            pixels: bank.pixels(),
            size,
            layers: bank.used(),
            allocated_layers: bank.allocated(),
            levels: bank.levels(),
            format: material::Format::Rgba8Unorm,
            stamp,
        }
    }
    let mut banks = [descriptor(first, snapshot.size(), epoch.stamp()); 8];
    for (target, bank) in banks.iter_mut().zip(snapshot.banks()) {
        *target = descriptor(bank, snapshot.size(), epoch.stamp());
    }
    let resources = material::Resources::new(
        material::Input {
            version: material::VERSION,
            epoch,
            split: snapshot.split(),
            sprites: snapshot.sprites(),
            required_levels: first.levels(),
            banks: &banks[..snapshot.banks().len()],
            lightmap: light,
            source: helium_metal_prototype::section::Format::BakedSectionSolid,
            hdr_or_pbr: false,
        },
        limits,
    )?;
    Ok(consume(&resources))
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    #[test]
    fn production_mips_flow_into_g04_with_labeled_fixture_light() {
        let prepared = horizon_client_render::gfx::PreparedTextures::new(
            &[255; 16],
            2,
            1,
            &[],
            (&[255; 16], 2),
        )
        .retain_material_snapshot(4096);
        let state = prepared.capture_materials(2, 1, false);
        let snapshot = state.current(state.snapshot().unwrap()).unwrap();
        let epoch = Epoch::new().unwrap();
        let pixels = [255; 1024];
        let light = Lightmap {
            pixels: &pixels,
            provenance: Provenance::SyntheticFixture,
            stamp: epoch.stamp(),
            revision: 1,
        };
        translate(
            snapshot,
            &epoch,
            light,
            material::Limits {
                max_dimension: 2,
                max_array_layers: 2048,
                payload_bytes: 1044,
            },
            |resources| {
                assert_eq!(resources.sprite(0).unwrap(), (0, 0));
                assert_eq!(
                    resources.bank(0).unwrap().pixels,
                    snapshot.banks()[0].pixels()
                );
                assert_eq!(
                    resources.bank(0).unwrap().pixels.as_ptr(),
                    snapshot.banks()[0].pixels().as_ptr()
                );
                assert_eq!(
                    resources.lightmap().provenance,
                    Provenance::SyntheticFixture
                );
            },
        )
        .unwrap();
        let binding = Binding {
            generation: state.generation(),
            epoch,
        };
        assert!(
            binding
                .consume_state(
                    &state,
                    &pixels,
                    Provenance::SyntheticFixture,
                    material::Limits {
                        max_dimension: 2,
                        max_array_layers: 2048,
                        payload_bytes: 1044
                    },
                    |_| ()
                )
                .unwrap_err()
                .contains("lightmap")
        );
    }
}
