//! G05 serialized worker-thread uploads on the existing G02 device and queue.
use super::*;
use crate::upload::{
    Budget, Counters, Description, Id, Phase, Plan, Record, Source, Ticket, Update,
};

/// One persistent texture. Uploads either explicitly block (G05-A) or occupy
/// the nonblocking slot (G05-B). Pending residency cannot be sampled/read back.
/// No raw texture handle or borrowed callback escapes; Drop never waits.
mod nonblocking;
use std::sync::Arc;
pub struct Material<S: Source> {
    pending: Option<nonblocking::Pending<S>>,
    members: Vec<Material<S>>,
    banked: bool,
    texture: Option<Texture>,
    _device: Retained<Device>,
    device_id: Id,
    owner: Id,
    description: Description<S::Generation>,
    source: Option<Arc<S>>,
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
        if self.upload_slot.busy() {
            return Err(
                "native upload slot busy; cannot create another material while work retires".into(),
            );
        }
        let plan = Plan::prepare(
            source,
            authority,
            None,
            Update::Full,
            &self.capabilities,
            budget,
        )?;
        let description = plan.description();
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
                members: Vec::new(),
                banked: false,
                pending: None,
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
    /// Narrow internal bridge: only exact completed, live bank-zero residency.
    pub(super) fn resident(
        &self,
        renderer: &TerrainRenderer,
        authority: &S::Authority,
    ) -> Result<(crate::upload::Stamp<S::Generation>, Arc<S>, Texture), String> {
        if self.pending.is_some() || self.device_id != renderer.upload_device {
            return Err("pending upload or wrong/recreated material device".into());
        }
        if self.banked {
            crate::banks::validate(&self.descriptions())?;
            for m in &self.members {
                m.resident(renderer, authority)?;
            }
        }
        let stamp = self.record.residency()?;
        let source = self.source.as_ref().ok_or("material source released")?;
        source.check_current(authority)?;
        if source.description() != self.description
            || (self.description.bank != 0
                && self.description.version != crate::upload::BANKED_VERSION)
        {
            return Err("changed metadata or unsupported nonzero bank; no sprite remapping".into());
        }
        Ok((
            stamp,
            source.clone(),
            self.texture.as_ref().ok_or("material released")?.clone(),
        ))
    }
    pub fn bank_mut(&mut self, bank: u32) -> Result<&mut Self, String> {
        if self.description.bank == bank {
            Ok(self)
        } else {
            self.members
                .iter_mut()
                .find(|m| m.description.bank == bank)
                .ok_or_else(|| "missing resident bank".into())
        }
    }
    pub(super) fn is_banked(&self) -> bool {
        self.banked
    }
    pub(super) fn descriptions(&self) -> Vec<Description<S::Generation>> {
        std::iter::once(self)
            .chain(self.members.iter())
            .map(|m| m.description)
            .collect()
    }
    pub(super) fn sources(&self) -> Vec<Arc<S>> {
        std::iter::once(self)
            .chain(self.members.iter())
            .filter_map(|m| m.source.clone())
            .collect()
    }
    pub(super) fn source_payload(&self) -> Result<usize, String> {
        crate::banks::source_bytes(self.sources().iter().map(|s| &**s))
    }
    pub(super) fn bindings(&self) -> Result<Vec<(u32, Texture)>, String> {
        std::iter::once(self)
            .chain(self.members.iter())
            .map(|m| {
                Ok((
                    m.description.bank,
                    m.texture.as_ref().ok_or("bank released")?.clone(),
                ))
            })
            .collect()
    }
    pub fn phase(&self) -> Phase {
        self.record.phase()
    }
    pub fn counters(&self) -> Counters {
        let mut c = self.counters;
        for m in &self.members {
            c.texture_payload = c.texture_payload.saturating_add(m.counters.texture_payload);
        }
        c
    }
    pub fn description(&self) -> Description<S::Generation> {
        self.description
    }
    pub fn release(&mut self) {
        for m in &mut self.members {
            m.release();
        }
        if self.record.phase() != Phase::Invalidated {
            self.counters.invalidations = self.counters.invalidations.saturating_add(1);
        }
        self.record.invalidate();
        self.texture = None;
        self.source = None;
        if self.pending.is_none() {
            self.counters.texture_payload = 0;
            self.counters.source_retained = 0;
        }
        // Pending fields and Metal's copied completion keepalive are NOT cancelled.
        // Standard Drop releases handles without waiting; the retaining command
        // keeps GPU resources and the completion lease keeps CPU owners alive.
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
        if self.pending.is_some() {
            return Err("material upload already pending".into());
        }
        let permit = renderer.upload_slot.acquire()?;
        if self.device_id != renderer.upload_device || self.texture.is_none() {
            return Err("released material or recreated/wrong Metal device owner".into());
        }
        let plan = Plan::prepare(
            &source,
            authority,
            self.source.as_deref(),
            update,
            &renderer.capabilities,
            budget,
        )
        .inspect_err(|e| {
            if e.contains("budget") {
                self.counters.budget_rejections = self.counters.budget_rejections.saturating_add(1);
            }
        })?;
        let d = plan.description();
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
        self.record = Record::prepared(d.identity, self.device_id, self.owner)?;
        self.counters.pending = 1;
        self.counters.staging_capacity = plan.staging_bytes();
        self.counters.source_retained = plan.source_bytes();
        permit.account(
            plan.texture_bytes(),
            plan.staging_bytes(),
            plan.source_bytes(),
        );
        self.counters.staging_peak = self.counters.staging_peak.max(plan.staging_bytes());
        self.counters.source_peak = self.counters.source_peak.max(plan.source_bytes());
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
            self.counters.committed(plan.upload_bytes());
            let submitted = self.record.advance(Phase::Encoded, Phase::Submitted);
            command.waitUntilCompleted();
            submitted?;
            let status = command.status();
            let error = command.error();
            // Blocking completion requires this exact command, after wait,
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
        self.counters.staging_capacity = 0;
        match result {
            Ok(ticket) => {
                self.counters.source_retained = source.retained_bytes();
                self.source = Some(Arc::new(source));
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
                self.counters.source_retained = 0;
                self.counters.failed_operations = self.counters.failed_operations.saturating_add(1);
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
        if self.pending.is_some() {
            return Err("material upload pending; residency unavailable".into());
        }
        let source = self
            .source
            .as_deref()
            .ok_or("no completed source residency")?;
        source.check_current(authority)?;
        if self.texture.is_none() {
            return Err("material released".into());
        }
        if source.description() != self.description {
            return Err("resident source metadata changed".into());
        }
        self.record.consume(
            ticket,
            self.description.identity,
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
        if self.pending.is_some() {
            return Err("material upload pending; readback unavailable".into());
        }
        let _permit = renderer.upload_slot.acquire()?;
        let source = self
            .source
            .as_deref()
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
        if plan.description() != self.description {
            return Err("resident source metadata changed".into());
        }
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
        if material.pending.is_some() {
            return Err("material upload pending; drawing unavailable".into());
        }
        let _permit = self.upload_slot.acquire()?;
        let source = material
            .source
            .as_deref()
            .ok_or("no completed material upload")?;
        source.check_current(authority)?;
        if material.device_id != self.upload_device || material.record.phase() != Phase::Completed {
            return Err("wrong device or incomplete material upload".into());
        }
        let d = material.description;
        if source.description() != d {
            return Err("resident source metadata changed".into());
        }
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

impl TerrainRenderer {
    /// V2 bounded set. All source/bank budgets validated before any native allocation.
    pub fn create_material_set<S: Source>(
        &self,
        sources: &[S],
        authority: &S::Authority,
        budget: Budget,
    ) -> Result<Material<S>, String> {
        let ds: Vec<_> = sources.iter().map(Source::description).collect();
        crate::banks::validate(&ds)?;
        let mut total = crate::banks::source_bytes(sources.iter())?;
        let mut staging = 0;
        let mut metadata = 0;
        for s in sources {
            let p = Plan::prepare(s, authority, None, Update::Full, &self.capabilities, budget)?;
            total = total
                .checked_add(p.texture_bytes())
                .ok_or("set texture overflow")?;
            staging = staging.max(p.staging_bytes());
            metadata = metadata.max(p.regions().len() * size_of::<crate::upload::CopyRegion>());
        }
        total = total
            .checked_add(staging.checked_mul(2).ok_or("set staging overflow")?)
            .and_then(|n| n.checked_add(metadata))
            .ok_or("set payload overflow")?;
        if total > budget.payload_bytes {
            return Err("bank set budget exceeded".into());
        }
        let mut materials = Vec::new();
        for s in sources {
            materials.push(self.create_material(s, authority, budget)?);
        }
        let mut primary = materials.remove(0);
        primary.members = materials;
        primary.banked = true;
        Ok(primary)
    }
}
