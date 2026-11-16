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

/// Additive binding revision: CPU target snapshots with per-animation epochs.
pub const BINDING_VERSION: u32 = 2;

/// One experimental namespace bound to one actual renderer generation. A new
/// binding cannot make an old production snapshot current.
pub struct Binding {
    generation: horizon_client_render::gfx::material_snapshot::Generation,
    epoch: Epoch,
    revision: u64,
}
impl Binding {
    pub fn new(renderer: &Renderer) -> Result<Self, String> {
        Ok(Self {
            generation: renderer.material_state().generation(),
            epoch: Epoch::new()?,
            revision: renderer.material_state().animation_revision(),
        })
    }
    /// CPU target content for the encoded animation revision. This never claims
    /// GPU submission/completion. Historical tickets must be checked via State.
    /// Validated synchronous consumption; both renderer and experimental epoch
    /// remain borrowed. No bank pixel copies or heap descriptor allocation.
    pub fn consume<R>(
        &mut self,
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
        &mut self,
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
        if snapshot.revision() != self.revision {
            self.epoch.invalidate()?;
            self.revision = snapshot.revision();
        }
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
        let mut binding = Binding {
            generation: state.generation(),
            epoch,
            revision: state.animation_revision(),
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

/// G05: owns the authentic immutable snapshot, not a borrowed byte pointer.
/// Authority is always the LIVE production renderer; historical owners alone
/// cannot authorize an upload or receipt after animation/reload/release.
pub struct BankSource {
    snapshot: std::sync::Arc<horizon_client_render::gfx::material_snapshot::Snapshot>,
    ticket: horizon_client_render::gfx::material_snapshot::Ticket,
    bank: u32,
}
impl Binding {
    pub fn upload_source(&self, renderer: &Renderer, bank: u32) -> Result<BankSource, String> {
        renderer
            .validate_material_snapshot_mode()
            .map_err(str::to_owned)?;
        let state = renderer.material_state();
        if state.generation() != self.generation {
            return Err("obsolete upload binding".into());
        }
        let snapshot = state.snapshot().map_err(str::to_owned)?;
        state.current(snapshot).map_err(str::to_owned)?;
        if snapshot.banks().get(bank as usize).is_none() {
            return Err("missing production bank".into());
        }
        Ok(BankSource {
            snapshot: snapshot.clone(),
            ticket: state.ticket().map_err(str::to_owned)?,
            bank,
        })
    }
}
impl BankSource {
    /// Last production job set as a contiguous span within this bank. The uploader
    /// ALSO verifies all bytes outside the span against its completed source, so
    /// skipped revisions or omitted changes cannot silently become resident.
    pub fn changed_span(
        &self,
        renderer: &Renderer,
    ) -> Result<helium_metal_prototype::upload::Update, String> {
        use helium_metal_prototype::upload::Source;
        self.check_current(renderer)?;
        let d = self.description();
        let mut span: Option<(u32, u32)> = None;
        for &global in renderer.changed_material_layers() {
            if let Ok(local) = d.slice(global) {
                span = Some(span.map_or((local, local), |(lo, hi)| (lo.min(local), hi.max(local))));
            }
        }
        let (lo, hi) = span.ok_or("no changed layers in this bank")?;
        Ok(helium_metal_prototype::upload::Update::Layers {
            first: lo,
            count: hi - lo + 1,
        })
    }
}
impl helium_metal_prototype::upload::Source for BankSource {
    type Generation = horizon_client_render::gfx::material_snapshot::Generation;
    type Authority = Renderer;
    fn description(&self) -> helium_metal_prototype::upload::Description<Self::Generation> {
        let bank = &self.snapshot.banks()[self.bank as usize];
        helium_metal_prototype::upload::Description {
            version: helium_metal_prototype::upload::VERSION,
            identity: helium_metal_prototype::upload::Identity {
                generation: self.snapshot.generation(),
                revision: self.snapshot.revision(),
            },
            bank: self.bank,
            split: self.snapshot.split(),
            sprites: self.snapshot.sprites(),
            used: bank.used(),
            allocated: bank.allocated(),
            size: self.snapshot.size(),
            levels: bank.levels(),
            format: material::Format::Rgba8Unorm,
            layout: helium_metal_prototype::upload::Layout::LayerMajorMipMajor,
        }
    }
    fn pixels(&self) -> &[u8] {
        self.snapshot.banks()[self.bank as usize].pixels()
    }
    fn retained_bytes(&self) -> usize {
        self.snapshot.retained_bytes()
    }
    fn retention_key(&self) -> Option<(usize, usize)> {
        Some((std::sync::Arc::as_ptr(&self.snapshot) as usize, 0))
    }
    fn check_current(&self, renderer: &Renderer) -> Result<(), String> {
        renderer
            .validate_material_snapshot_mode()
            .map_err(str::to_owned)?;
        let state = renderer.material_state();
        state.check_ticket(self.ticket).map_err(str::to_owned)?;
        state.current(&self.snapshot).map_err(str::to_owned)?;
        Ok(())
    }
}

#[cfg(test)]
mod upload_source_tests {
    use super::*;
    use helium_metal_prototype::upload::Source;
    #[test]
    fn authentic_prepared_owner_preserves_upload_identity_and_bytes() {
        // Handcrafted base pixels through actual production mip preparation.
        let prepared = horizon_client_render::gfx::PreparedTextures::new(
            &[17; 3 * 16],
            2,
            3,
            &[],
            (&[255; 16], 2),
        )
        .retain_material_snapshot(4096);
        let state = prepared.capture_materials(2, 3, false);
        let snapshot = state.snapshot().unwrap().clone();
        let weak = std::sync::Arc::downgrade(&snapshot);
        let source = BankSource {
            snapshot,
            ticket: state.ticket().unwrap(),
            bank: 0,
        };
        fn requires_send_sync_static<T: Send + Sync + 'static>(_: &T) {}
        requires_send_sync_static(&source); // G05-B completion lease bound, no authority captured
        let d = source.description();
        assert_eq!(d.identity.generation, state.generation());
        assert_eq!(d.identity.revision, state.animation_revision());
        assert_eq!(
            (d.bank, d.used, d.allocated, d.size, d.levels),
            (0, 3, 3, 2, 2)
        );
        assert_eq!(d.slice(2).unwrap(), 2);
        assert!(d.slice(3).is_err());
        assert_eq!(
            source.pixels(),
            state.snapshot().unwrap().banks()[0].pixels()
        );
        assert_eq!(
            source.pixels().as_ptr(),
            state.snapshot().unwrap().banks()[0].pixels().as_ptr()
        );
        drop(state);
        assert!(weak.upgrade().is_some());
        drop(source);
        assert!(weak.upgrade().is_none());
    }
}

/// Test-only owned snapshot, handcrafted base pixels through production mip code.
#[cfg(test)]
pub(crate) fn handcrafted_bridge_bank() -> BankSource {
    let prepared = horizon_client_render::gfx::PreparedTextures::new(
        &[255; 3 * 16],
        2,
        3,
        &[],
        (&[255; 16], 2),
    )
    .retain_material_snapshot(4096);
    let state = prepared.capture_materials(2, 3, false);
    BankSource {
        snapshot: state.snapshot().unwrap().clone(),
        ticket: state.ticket().unwrap(),
        bank: 0,
    }
}
