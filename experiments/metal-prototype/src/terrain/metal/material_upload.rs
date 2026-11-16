//! G05 serialized worker-thread uploads on the existing G02 device and queue.
use super::*;
use crate::upload::{
    Budget, Counters, Description, Id, Phase, Plan, Record, Source, Ticket, Update,
};

/// One persistent texture. All methods that reference it wait for their exact
/// command; no raw texture handle or asynchronous borrowed callback escapes.
pub struct Material<S: Source> {
    texture: Option<Texture>,
    _device: Retained<Device>,
    device_id: Id,
    owner: Id,
    description: Description<S::Generation>,
    source: Option<S>,
    record: Record<S::Generation>,
    counters: Counters,
}
impl TerrainRenderer {
    pub fn create_material<S: Source>(
        &self,
        source: &S,
        authority: &S::Authority,
        budget: Budget,
    ) -> Result<Material<S>, String> {
        let plan = Plan::prepare(
            source,
            authority,
            None,
            Update::Full,
            &self.capabilities,
            budget,
        )?;
        let description = source.description();
        autoreleasepool(|_| {
            let desc = MTLTextureDescriptor::new();
            desc.setTextureType(MTLTextureType::Type2DArray);
            desc.setPixelFormat(MTLPixelFormat::RGBA8Unorm);
            desc.setStorageMode(MTLStorageMode::Private);
            desc.setUsage(MTLTextureUsage::ShaderRead);
            // SAFETY: complete power-of-two mips, dimensions and slice count were
            // checked against G02 policy and the queried device before allocation.
            unsafe {
                desc.setWidth(description.size as usize);
                desc.setHeight(description.size as usize);
                desc.setArrayLength(description.allocated as usize);
                desc.setMipmapLevelCount(description.levels as usize);
            }
            let texture = self
                .device
                .newTextureWithDescriptor(&desc)
                .ok_or("material texture allocation failed")?;
            let owner = Id::new()?;
            Ok(Material {
                texture: Some(texture),
                _device: self.device.clone(),
                device_id: self.upload_device,
                owner,
                description,
                source: None,
                record: Record::prepared(description.identity, self.upload_device, owner)?,
                counters: Counters {
                    texture_payload: plan.texture_bytes(),
                    ..Counters::default()
                },
            })
        })
    }
}
impl<S: Source> Material<S> {
    pub fn phase(&self) -> Phase {
        self.record.phase()
    }
    pub fn counters(&self) -> Counters {
        self.counters
    }
    pub fn description(&self) -> Description<S::Generation> {
        self.description
    }
    pub fn release(&mut self) {
        // No command can still be in flight: only blocking methods expose this owner.
        self.record.invalidate();
        self.texture = None;
        self.source = None;
        self.counters.texture_payload = 0;
    }
    /// Offscreen/validation worker use only. Serializes updates and bounds pending
    /// work to ONE operation. Failure after encoding invalidates existing residency.
    pub fn upload_blocking(
        &mut self,
        renderer: &TerrainRenderer,
        source: S,
        authority: &S::Authority,
        update: Update,
        budget: Budget,
    ) -> Result<Ticket<S::Generation>, String> {
        if self.device_id != renderer.upload_device || self.texture.is_none() {
            return Err("released material or recreated/wrong Metal device owner".into());
        }
        let d = source.description();
        let mut expected = self.description;
        expected.identity.revision = d.identity.revision;
        if expected != d
            || self
                .source
                .as_ref()
                .is_some_and(|old| old.description().identity.revision >= d.identity.revision)
        {
            return Err("incompatible generation/layout or stale upload revision".into());
        }
        let plan = Plan::prepare(
            &source,
            authority,
            self.source.as_ref(),
            update,
            &renderer.capabilities,
            budget,
        )?;
        self.record = Record::prepared(d.identity, self.device_id, self.owner)?;
        self.counters.pending = 1;
        self.counters.staging_peak = self.counters.staging_peak.max(plan.staging_bytes());
        self.counters.source_peak = self.counters.source_peak.max(
            source
                .retained_bytes()
                .checked_add(self.source.as_ref().map_or(0, Source::retained_bytes))
                .ok_or("source accounting overflow")?,
        );
        let texture = self.texture.as_ref().unwrap().clone();
        let result = autoreleasepool(|_| {
            let bytes = plan.stage(source.pixels())?;
            let staging = upload(&renderer.device, &bytes)?;
            if staging.length() < plan.staging_bytes() {
                return Err("Metal staging buffer shorter than requested".into());
            }
            let command = renderer
                .queue
                .commandBuffer()
                .ok_or("material command allocation failed")?;
            let blit = command
                .blitCommandEncoder()
                .ok_or("material blit encoder allocation failed")?;
            for r in plan.regions() {
                // SAFETY: plan proves every offset, initialized padded row/image,
                // full target mip/slice extent and actual MTLBuffer size. All owners
                // (including both source revisions) stay live through wait below.
                unsafe {
                    blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                        &staging, r.staging, r.row, r.image,
                        MTLSize { width: r.width, height: r.width, depth: 1 }, &texture, r.slice, r.level, MTLOrigin { x: 0, y: 0, z: 0 });
                }
            }
            blit.endEncoding();
            self.record.advance(Phase::Prepared, Phase::Encoded)?;
            command.commit();
            let submitted = self.record.advance(Phase::Encoded, Phase::Submitted);
            self.counters.upload_bytes = self
                .counters
                .upload_bytes
                .saturating_add(plan.upload_bytes() as u64);
            command.waitUntilCompleted();
            submitted?;
            let status = command.status();
            let error = command.error();
            // The ONLY non-test path to completion: this exact command, after wait,
            // must report Completed AND no NSError. Submission alone never suffices.
            if status != MTLCommandBufferStatus::Completed || error.is_some() {
                let _ = self.record.finish(false);
                return Err(format!(
                    "material Metal command failed: {status:?}, {error:?}"
                ));
            }
            source
                .check_current(authority)
                .inspect_err(|_| self.record.invalidate())?;
            self.record.finish(true)
        });
        self.counters.pending = 0;
        match result {
            Ok(ticket) => {
                if self.source.is_some() {
                    self.counters.reused_resources =
                        self.counters.reused_resources.saturating_add(1);
                }
                self.source = Some(source);
                self.description = d;
                self.counters.completed_uploads = self.counters.completed_uploads.saturating_add(1);
                Ok(ticket)
            }
            Err(error) => {
                if error.contains("allocation failed") {
                    self.counters.failed_allocations =
                        self.counters.failed_allocations.saturating_add(1);
                }
                if self.record.phase() != Phase::Invalidated {
                    self.record.fail();
                }
                self.source = None;
                Err(error)
            }
        }
    }
    /// Consumes a one-use receipt against the live producer, owner and exact
    /// operation. This authorizes only this bank's Metal residency, never wgpu.
    pub fn consume_ticket(
        &self,
        renderer: &TerrainRenderer,
        authority: &S::Authority,
        ticket: &mut Ticket<S::Generation>,
    ) -> Result<(), String> {
        let source = self
            .source
            .as_ref()
            .ok_or("no completed source residency")?;
        source.check_current(authority)?;
        if self.texture.is_none() {
            return Err("material released".into());
        }
        self.record.consume(
            ticket,
            source.description().identity,
            renderer.upload_device,
            self.owner,
        )
    }

    /// Explicit validation readback. Allocates bounded scratch and waits; never
    /// used by upload/update or the gameplay frame loop. Returns actual mip bytes.
    pub fn readback_for_validation(
        &self,
        renderer: &TerrainRenderer,
        authority: &S::Authority,
        budget: Budget,
    ) -> Result<Vec<u8>, String> {
        let source = self
            .source
            .as_ref()
            .ok_or("no completed material residency")?;
        source.check_current(authority)?;
        if self.device_id != renderer.upload_device || self.record.phase() != Phase::Completed {
            return Err("wrong device or incomplete residency".into());
        }
        let plan = Plan::prepare(
            source,
            authority,
            None,
            Update::Full,
            &renderer.capabilities,
            budget,
        )?;
        autoreleasepool(|_| {
            let texture = self.texture.as_ref().ok_or("released texture")?;
            let buffer = renderer
                .device
                .newBufferWithLength_options(
                    plan.staging_bytes(),
                    MTLResourceOptions::StorageModeShared,
                )
                .ok_or("material readback allocation failed")?;
            if buffer.length() < plan.staging_bytes() {
                return Err("Metal readback buffer shorter than requested".into());
            }
            let command = renderer
                .queue
                .commandBuffer()
                .ok_or("readback command allocation failed")?;
            let blit = command
                .blitCommandEncoder()
                .ok_or("readback encoder allocation failed")?;
            for r in plan.regions() {
                // SAFETY: same checked extents/pitches as upload, with roles reversed.
                unsafe {
                    blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                    texture, r.slice, r.level, MTLOrigin {x:0,y:0,z:0}, MTLSize {width:r.width,height:r.width,depth:1}, &buffer,r.staging,r.row,r.image);
                }
            }
            blit.endEncoding();
            command.commit();
            command.waitUntilCompleted();
            if command.status() != MTLCommandBufferStatus::Completed || command.error().is_some() {
                return Err(format!("material readback failed: {:?}", command.error()));
            }
            let mut result = Vec::new();
            result
                .try_reserve_exact(plan.texture_bytes())
                .map_err(|_| "readback result allocation failed")?;
            if result.capacity() != plan.texture_bytes() {
                return Err("unexpected readback capacity".into());
            }
            result.resize(plan.texture_bytes(), 0);
            for r in plan.regions() {
                for y in 0..r.width {
                    // SAFETY: read only initialized row prefixes AFTER this command
                    // completed; padding is never read as initialized Rust memory.
                    let row = unsafe {
                        std::slice::from_raw_parts(
                            buffer
                                .contents()
                                .as_ptr()
                                .cast::<u8>()
                                .add(r.staging + y * r.row),
                            r.width * 4,
                        )
                    };
                    let start = r.source + y * r.width * 4;
                    result[start..start + row.len()].copy_from_slice(row);
                }
            }
            Ok(result)
        })
    }
}

impl TerrainRenderer {
    /// Existing G02 pipeline, using the persistent texture instead of re-uploading
    /// atlas pixels. Only bank zero can draw unchanged GLOBAL vertex sprite IDs.
    /// Full production filtering/multi-bank/HDR parity is not implied.
    pub fn render_material<S: Source>(
        &self,
        material: &Material<S>,
        authority: &S::Authority,
        mesh: &Mesh<'_>,
        lightmap: &[u8],
        view: View,
        options: Options,
    ) -> Result<Image, String> {
        let source = material
            .source
            .as_ref()
            .ok_or("no completed material upload")?;
        source.check_current(authority)?;
        if material.device_id != self.upload_device || material.record.phase() != Phase::Completed {
            return Err("wrong device or incomplete material upload".into());
        }
        let d = source.description();
        if d.bank != 0 {
            return Err("G02 global-ID shader can draw only resident bank zero; no sprite remapping performed".into());
        }
        let texture = material.texture.as_ref().ok_or("material released")?;
        let mut layout = resource_layout(
            mesh,
            (d.size as usize, d.size as usize, d.used as usize),
            material.counters.texture_payload,
            false,
            lightmap,
            view,
            options,
            &self.capabilities,
        )?;
        layout.payload_bytes = layout
            .payload_bytes
            .checked_add(source.retained_bytes())
            .ok_or("resident source budget overflow")?;
        if layout.payload_bytes > options.budget.payload_bytes {
            return Err("resident render source/payload budget exceeded".into());
        }
        let atlas = Atlas {
            width: d.size as usize,
            height: d.size as usize,
            layers: d.used as usize,
            rgba: &[],
        };
        autoreleasepool(|_| self.draw(mesh, atlas, lightmap, view, options, layout, Some(texture)))
    }
}
