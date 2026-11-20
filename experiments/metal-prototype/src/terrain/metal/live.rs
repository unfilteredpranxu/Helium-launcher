//! Worker-owned resident terrain. Metal retains encoded resources; a copied
//! completion block holds the process-wide permit even if the worker exits.
use super::{hud::Hud, *};
use crate::{
    display::Display,
    upload::{self, Source},
};
use block2::RcBlock;
use helium_render_scene::hud::Frame as HudFrame;
use helium_render_scene::{Frame as CpuFrame, MaterialBank, Section};
use objc2_quartz_core::CAMetalDrawable;
use std::num::NonZeroU64;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const LIGHT_PITCH: usize = 256;
const LIGHT_STAGING: usize = 16 * LIGHT_PITCH;
const MAX_SECTIONS: usize = 64;
const MAX_SECTION_BYTES: usize = 2 * 1024 * 1024;

pub struct LiveSection<G> {
    owner: upload::Id,
    generation: G,
    epoch: Option<NonZeroU64>,
    split: u32,
    sprites: u32,
    vertices: Buffer,
    indices: Buffer,
    // Byte offsets and counts in the unchanged index stream. Calculated once.
    runs: Vec<(usize, usize, u32)>,
    opaque_runs: usize,
    mask: u8,
    payload: usize,
}
impl<G: Copy + Eq> LiveSection<G> {
    pub fn new(
        renderer: &TerrainRenderer,
        mesh: &Mesh<'_>,
        identity: upload::Identity<G>,
        split: u32,
        sprites: u32,
        budget: usize,
    ) -> Result<Self, String> {
        Self::with_passes(
            renderer,
            mesh,
            identity,
            split,
            sprites,
            budget,
            mesh.indices.len(),
        )
    }

    fn with_passes(
        renderer: &TerrainRenderer,
        mesh: &Mesh<'_>,
        identity: upload::Identity<G>,
        split: u32,
        sprites: u32,
        budget: usize,
        opaque_indices: usize,
    ) -> Result<Self, String> {
        if opaque_indices > mesh.indices.len() || !opaque_indices.is_multiple_of(3) {
            return Err("invalid opaque/translucent pass boundary".into());
        }
        let index_bytes = std::mem::size_of_val(mesh.indices);
        let metadata = (mesh.indices.len() / 3)
            .checked_mul(size_of::<(usize, usize, u32)>())
            .ok_or("live draw metadata overflow")?;
        let payload = mesh
            .vertices
            .len()
            .checked_add(index_bytes)
            .and_then(|n| n.checked_add(metadata))
            .ok_or("live mesh byte count overflow")?;
        if payload > budget.min(MAX_SECTION_BYTES)
            || mesh.indices.is_empty()
            || split == 0
            || sprites.div_ceil(split) > 8
        {
            return Err("live mesh outside section or bank budget".into());
        }
        mesh.validate(sprites as usize)?;
        let mut runs: Vec<(usize, usize, u32)> = Vec::new();
        runs.try_reserve_exact(mesh.indices.len() / 3)
            .map_err(|_| "live draw metadata allocation failed")?;
        let mut mask = 0u8;
        for (tri, indices) in mesh.indices.chunks_exact(3).enumerate() {
            let bank = u32::from(mesh.vertex(indices[0] as usize)?.layer & !OPAQUE_LAYER) / split;
            mask |= 1 << bank;
            if let Some(last) = runs
                .last_mut()
                .filter(|last| last.2 == bank && tri * 3 != opaque_indices)
            {
                last.1 += 3;
            } else {
                runs.push((tri * 3 * size_of::<u32>(), 3, bank));
            }
        }
        let opaque_runs = runs.partition_point(|run| run.0 < opaque_indices * size_of::<u32>());
        // SAFETY: initialized u32 values, exact byte length, borrowed for copying.
        let indices =
            unsafe { std::slice::from_raw_parts(mesh.indices.as_ptr().cast(), index_bytes) };
        Ok(Self {
            owner: renderer.upload_device,
            generation: identity.generation,
            epoch: None,
            split,
            sprites,
            vertices: upload(&renderer.device, mesh.vertices)?,
            indices: upload(&renderer.device, indices)?,
            runs,
            opaque_runs,
            mask,
            payload,
        })
    }
    pub fn draw_count(&self) -> usize {
        self.runs.len()
    }
    pub fn bytes(&self) -> usize {
        self.payload
    }
}

impl LiveSection<u64> {
    pub fn from_scene<S>(
        renderer: &TerrainRenderer,
        section: &Section,
        frame: &CpuFrame<S>,
        budget: usize,
    ) -> Result<Self, String> {
        if !frame.accepts(section) {
            return Err("section belongs to a different scene epoch or material generation".into());
        }
        let bank = frame.materials.banks()[0].description();
        let mut resident = Self::with_passes(
            renderer,
            &section.mesh(),
            bank.identity,
            bank.split,
            bank.sprites,
            budget,
            section.opaque_indices(),
        )?;
        resident.epoch = Some(frame.epoch);
        Ok(resident)
    }
}

pub struct LiveFrame<'a, G> {
    pub sections: &'a [(&'a LiveSection<G>, View)],
    pub identity: upload::Identity<G>,
    pub lightmap: &'a [u8; 1024],
    pub clear_rgb: [f32; 3],
}
pub enum LiveSubmission {
    Submitted,
    Pending,
    DrawableUnavailable,
}
#[derive(Debug)]
pub struct LiveCompletion {
    pub draws: u64,
    pub encode_time: Duration,
    pub completion_latency: Duration,
    pub gpu_time: Option<Duration>,
    pub hud_bytes: usize,
    pub hud_uploaded_bytes: usize,
}

struct Pending {
    command: Retained<ProtocolObject<dyn MTLCommandBuffer>>,
    _drawable: Option<Retained<ProtocolObject<dyn CAMetalDrawable>>>,
    display_owner: Option<upload::Id>,
    submitted: Instant,
    encode_time: Duration,
    draws: u64,
    hud_bytes: usize,
    hud_uploaded_bytes: usize,
}

pub struct LiveScene {
    device: upload::Id,
    size: [usize; 2],
    color: Texture,
    depth: Texture,
    light: Texture,
    staging: Buffer,
    hud: Option<Hud>,
    pending: Option<Pending>,
}
impl LiveScene {
    pub fn new(renderer: &TerrainRenderer, size: [usize; 2]) -> Result<Self, String> {
        let [w, h] = size;
        if w == 0 || h == 0 || w > 4096 || h > 4096 || w * h > 6 * 1024 * 1024 {
            return Err("live output target outside pixel budget".into());
        }
        let depth = texture(
            &renderer.device,
            w,
            h,
            1,
            false,
            MTLPixelFormat::Depth32Float,
            MTLTextureUsage::RenderTarget,
        )?;
        let color = texture(
            &renderer.device,
            w,
            h,
            1,
            false,
            MTLPixelFormat::RGBA8Unorm,
            MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead,
        )?;
        let light = texture(
            &renderer.device,
            16,
            16,
            1,
            false,
            MTLPixelFormat::RGBA8Unorm,
            MTLTextureUsage::ShaderRead,
        )?;
        Ok(Self {
            device: renderer.upload_device,
            size,
            color,
            depth,
            light,
            staging: upload(&renderer.device, &[0; LIGHT_STAGING])?,
            hud: None,
            pending: None,
        })
    }

    pub fn bytes(&self) -> usize {
        self.size[0] * self.size[1] * 8 + LIGHT_STAGING + 1024
    }

    pub fn submit_scene<S>(
        &mut self,
        renderer: &TerrainRenderer,
        material: &upload::Material<MaterialBank>,
        display: Option<&Display>,
        frame: &CpuFrame<S>,
        sections: &[(&LiveSection<u64>, View)],
        cancelled: &AtomicBool,
    ) -> Result<LiveSubmission, String> {
        self.submit_scene_with_hud(
            renderer, material, display, frame, sections, 0, None, cancelled,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn submit_scene_with_hud<S>(
        &mut self,
        renderer: &TerrainRenderer,
        material: &upload::Material<MaterialBank>,
        display: Option<&Display>,
        frame: &CpuFrame<S>,
        sections: &[(&LiveSection<u64>, View)],
        retained_geometry_bytes: usize,
        hud: Option<&HudFrame>,
        cancelled: &AtomicBool,
    ) -> Result<LiveSubmission, String> {
        if frame.lightmap.revision == 0
            || sections.iter().any(|(s, _)| s.epoch != Some(frame.epoch))
        {
            return Err("scene draw has a stale epoch or missing lightmap revision".into());
        }
        autoreleasepool(|_| {
            self.encode_and_submit(
                renderer,
                material,
                display,
                LiveFrame {
                    sections,
                    identity: frame.materials.banks()[0].description().identity,
                    lightmap: &frame.lightmap.rgba,
                    clear_rgb: frame.clear_rgb,
                },
                hud,
                retained_geometry_bytes,
                cancelled,
            )
        })
    }

    pub fn submit<S: Source<Authority = AtomicBool> + Send + Sync + 'static>(
        &mut self,
        renderer: &TerrainRenderer,
        material: &upload::Material<S>,
        display: Option<&Display>,
        scene: LiveFrame<'_, S::Generation>,
        cancelled: &AtomicBool,
    ) -> Result<LiveSubmission, String> {
        autoreleasepool(|_| {
            self.encode_and_submit(renderer, material, display, scene, None, 0, cancelled)
        })
    }
    fn encode_and_submit<S: Source<Authority = AtomicBool> + Send + Sync + 'static>(
        &mut self,
        renderer: &TerrainRenderer,
        material: &upload::Material<S>,
        display: Option<&Display>,
        scene: LiveFrame<'_, S::Generation>,
        hud: Option<&HudFrame>,
        retained_geometry_bytes: usize,
        cancelled: &AtomicBool,
    ) -> Result<LiveSubmission, String> {
        let started = Instant::now();
        if self.pending.is_some() || renderer.upload_retention().busy {
            return Ok(LiveSubmission::Pending);
        }
        let LiveFrame {
            sections,
            identity,
            lightmap,
            clear_rgb,
        } = scene;
        if self.device != renderer.upload_device
            || cancelled.load(Ordering::Acquire)
            || (sections.is_empty() && hud.is_none_or(|f| f.vertices().is_empty()))
            || sections.len() > MAX_SECTIONS
            || !clear_rgb.iter().all(|c| (0.0..=1.0).contains(c))
        {
            return Err("invalid/cancelled live frame".into());
        }
        if hud.is_some_and(|f| f.size().map(|n| n as usize) != self.size) {
            return Ok(LiveSubmission::DrawableUnavailable);
        }
        material.resident(renderer, cancelled)?;
        let banks = material.bindings()?;
        let d = material.description();
        if !material.is_banked() || d.identity != identity {
            return Err("live frame material revision not resident".into());
        }
        let mask = banks.iter().fold(0u8, |mask, (id, _)| mask | (1 << id));
        let mut bytes = 0usize;
        for (section, view) in sections {
            if section.owner != self.device
                || section.generation != identity.generation
                || section.split != d.split
                || section.sprites != d.sprites
                || section.mask & !mask != 0
                || view
                    .origin
                    .iter()
                    .chain(view.clip_from_relative.iter().flatten())
                    .any(|v| !v.is_finite())
            {
                return Err("live section generation, view or bank mismatch".into());
            }
            bytes = bytes
                .checked_add(section.bytes())
                .ok_or("live geometry overflow")?;
        }
        // The production worker may retain offscreen resident geometry too.
        let bytes = bytes.max(retained_geometry_bytes);
        let hud_bytes = hud.map_or(0, |frame| Hud::budget(self.hud.as_ref(), frame));
        let live = bytes
            .checked_add(self.bytes())
            .and_then(|n| n.checked_add(hud_bytes))
            .and_then(|n| n.checked_add(material.counters().texture_payload))
            .and_then(|n| n.checked_add(material.source_payload().ok()?))
            .ok_or("live frame accounting overflow")?;
        const BUDGET: usize = 192 * 1024 * 1024;
        if live > BUDGET {
            return Err("live frame payload budget".into());
        }
        let permit = renderer.upload_slot.acquire()?;
        let drawable = if let Some(display) = display {
            let [w, h] = display.dimensions();
            let [sw, sh] = self.size;
            let viewport = crate::display::fit((sw, sh), (w, h), live, BUDGET)?;
            let Some(target) = display.live_drawable(renderer)? else {
                return Ok(LiveSubmission::DrawableUnavailable);
            };
            Some((target, viewport))
        } else {
            None
        };
        if cancelled.load(Ordering::Acquire) {
            return Err("live frame cancelled during drawable acquisition".into());
        }
        let command = renderer
            .queue
            .commandBuffer()
            .ok_or("live command allocation failed")?;
        let blit = command
            .blitCommandEncoder()
            .ok_or("live lightmap encoder unavailable")?;
        for row in 0..16 {
            // SAFETY: a unique command flight protects this shared staging buffer.
            // Every row's 64 initialized bytes are in the checked 4096-byte range.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    lightmap.as_ptr().add(row * 64),
                    self.staging
                        .contents()
                        .as_ptr()
                        .cast::<u8>()
                        .add(row * LIGHT_PITCH),
                    64,
                );
            }
        }
        unsafe {
            blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                &self.staging, 0, LIGHT_PITCH, LIGHT_STAGING, MTLSize { width:16,height:16,depth:1 },
                &self.light, 0, 0, MTLOrigin{x:0,y:0,z:0});
        }
        blit.endEncoding();

        let pass = MTLRenderPassDescriptor::new();
        // SAFETY: supported color attachment zero; textures are same-device.
        let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(&self.color));
        color.setLoadAction(MTLLoadAction::Clear);
        color.setStoreAction(MTLStoreAction::Store);
        color.setClearColor(MTLClearColor {
            red: f64::from(clear_rgb[0]),
            green: f64::from(clear_rgb[1]),
            blue: f64::from(clear_rgb[2]),
            alpha: 1.,
        });
        let depth = pass.depthAttachment();
        depth.setTexture(Some(&self.depth));
        depth.setLoadAction(MTLLoadAction::Clear);
        depth.setStoreAction(MTLStoreAction::DontCare);
        depth.setClearDepth(0.);
        let encoder = command
            .renderCommandEncoderWithDescriptor(&pass)
            .ok_or("live terrain encoder unavailable")?;
        encoder.setCullMode(MTLCullMode::None);
        // SAFETY: the lightmap is retained by this command and texture slot zero
        // matches both terrain vertex pipelines.
        unsafe { encoder.setVertexTexture_atIndex(Some(&self.light), 0) };
        let mut draws = 0;
        for blended in [false, true] {
            if blended {
                if !sections.iter().any(|(s, _)| s.opaque_runs < s.runs.len()) {
                    break;
                }
                encoder.setRenderPipelineState(&renderer.blended_pipeline);
                encoder.setDepthStencilState(Some(&renderer.blended_reversed));
            } else {
                encoder.setRenderPipelineState(&renderer.banked_pipeline);
                encoder.setDepthStencilState(Some(&renderer.reversed));
            }
            let mut bound = None;
            for i in 0..sections.len() {
                // The game worker supplies nearest-first sections. Keep opaque
                // early depth rejection and blend farther sections first.
                let (section, view) = &sections[if blended { sections.len() - 1 - i } else { i }];
                let runs = if blended {
                    &section.runs[section.opaque_runs..]
                } else {
                    &section.runs[..section.opaque_runs]
                };
                if runs.is_empty() {
                    continue;
                }
                let mut uniforms = [0f32; 20];
                for (dst, src) in uniforms
                    .iter_mut()
                    .zip(view.clip_from_relative.iter().flatten())
                {
                    *dst = *src;
                }
                uniforms[16..19].copy_from_slice(&view.origin);
                uniforms[19] = UV_STEP;
                // SAFETY: Metal copies all uniform bytes immediately. Encoded buffers,
                // textures and index ranges are validated and retained by the command.
                unsafe {
                    encoder.setVertexBuffer_offset_atIndex(Some(&section.vertices), 0, 0);
                    encoder.setVertexBytes_length_atIndex(
                        NonNull::from(&uniforms).cast(),
                        size_of_val(&uniforms),
                        1,
                    );
                    for &(offset, count, bank) in runs {
                        if bound != Some(bank) {
                            let texture = &banks
                                .iter()
                                .find(|(id, _)| *id == bank)
                                .ok_or("missing validated live bank")?
                                .1;
                            let first = bank * d.split;
                            encoder.setFragmentTexture_atIndex(Some(texture), 0);
                            encoder.setFragmentBytes_length_atIndex(
                                NonNull::from(&first).cast(),
                                4,
                                0,
                            );
                            bound = Some(bank);
                        }
                        encoder
                            .drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset(
                                MTLPrimitiveType::Triangle,
                                count,
                                MTLIndexType::UInt32,
                                &section.indices,
                                offset,
                            );
                        draws += 1;
                    }
                }
            }
        }
        encoder.endEncoding();
        let mut hud_uploaded_bytes = 0;
        if let Some(frame) = hud {
            if self
                .hud
                .as_ref()
                .is_none_or(|h| h.revision() != frame.atlas().revision())
            {
                self.hud = None;
                self.hud = Some(Hud::new(&renderer.device, frame)?);
                hud_uploaded_bytes = frame.atlas().pixels().len();
            }
            hud_uploaded_bytes += self.hud.as_mut().unwrap().encode(
                &renderer.device,
                &command,
                &self.color,
                &self.depth,
                frame,
            )?;
        }
        if let (Some(display), Some((drawable, viewport))) = (display, &drawable) {
            display.encode(&command, &self.color, &drawable.texture(), *viewport)?;
            command.presentDrawable(ProtocolObject::from_ref(&**drawable));
            display.retain_for_live_command(&command);
        }
        if cancelled.load(Ordering::Acquire) {
            return Err("live frame cancelled before commit".into());
        }
        permit.account(
            material.counters().texture_payload,
            LIGHT_STAGING,
            material.source_payload()?,
        );
        permit.account_frame(
            bytes + self.bytes() + hud_bytes - LIGHT_STAGING
                + display.map_or(0, |d| {
                    let [w, h] = d.dimensions();
                    w * h * 12
                }),
        );
        let permit = Arc::new(permit);
        let terminal = move |_: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            let _keep = &permit;
        };
        fn require_send_sync_static<T: Send + Sync + 'static>(_: &T) {}
        require_send_sync_static(&terminal);
        let block = RcBlock::new(terminal);
        // SAFETY: a copied, thread-safe block with only the owned reservation.
        // Standard retaining command buffers own native resources through completion.
        unsafe { command.addCompletedHandler(RcBlock::as_ptr(&block)) };
        self.pending = Some(Pending {
            command,
            _drawable: drawable.map(|(target, _)| target),
            display_owner: display.map(Display::live_owner),
            submitted: Instant::now(),
            encode_time: started.elapsed(),
            draws,
            hud_bytes,
            hud_uploaded_bytes,
        });
        self.pending.as_ref().unwrap().command.commit();
        Ok(LiveSubmission::Submitted)
    }

    pub fn poll(
        &mut self,
        display: Option<&Display>,
        cancelled: &AtomicBool,
    ) -> Result<Option<LiveCompletion>, String> {
        let Some(pending) = self.pending.as_ref() else {
            return Ok(None);
        };
        let status = pending.command.status();
        if status != MTLCommandBufferStatus::Completed && status != MTLCommandBufferStatus::Error {
            return Ok(None);
        }
        let pending = self.pending.take().unwrap();
        if status != MTLCommandBufferStatus::Completed
            || pending.command.error().is_some()
            || cancelled.load(Ordering::Acquire)
            || pending.display_owner != display.map(Display::live_owner)
            || display.is_some_and(|d| !d.live_layout_valid())
        {
            return Err("live command failed or frame revoked before terminal completion".into());
        }
        if let (Some(display), Some(_)) = (display, &pending._drawable) {
            display.confirm_live();
        }
        let start = pending.command.GPUStartTime();
        let end = pending.command.GPUEndTime();
        Ok(Some(LiveCompletion {
            draws: pending.draws,
            hud_bytes: pending.hud_bytes,
            hud_uploaded_bytes: pending.hud_uploaded_bytes,
            encode_time: pending.encode_time,
            completion_latency: pending.submitted.elapsed(),
            gpu_time: (start > 0. && end.is_finite() && end >= start)
                .then(|| Duration::from_secs_f64(end - start)),
        }))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use upload::tests::{Fixture, budget, fixture};

    #[derive(Clone)]
    struct Synthetic(Fixture);
    impl Source for Synthetic {
        type Generation = u64;
        type Authority = AtomicBool;
        fn description(&self) -> upload::Description<u64> {
            self.0.description
        }
        fn pixels(&self) -> &[u8] {
            self.0.pixels()
        }
        fn retained_bytes(&self) -> usize {
            self.0.retained_bytes()
        }
        fn check_current(&self, cancelled: &AtomicBool) -> Result<(), String> {
            if cancelled.load(Ordering::Acquire) {
                Err("cancelled fixture".into())
            } else {
                Ok(())
            }
        }
    }

    fn readback(renderer: &TerrainRenderer, scene: &LiveScene) -> Vec<u8> {
        let [w, h] = scene.size;
        let row = w * 4;
        assert_eq!(row % 256, 0);
        let output = renderer
            .device
            .newBufferWithLength_options(row * h, MTLResourceOptions::StorageModeShared)
            .unwrap();
        let command = renderer.queue.commandBuffer().unwrap();
        let blit = command.blitCommandEncoder().unwrap();
        // SAFETY: exact initialized 64x64 RGBA target and aligned buffer.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                &scene.color,0,0,MTLOrigin{x:0,y:0,z:0},MTLSize{width:w,height:h,depth:1},&output,0,row,row*h);
        }
        blit.endEncoding();
        command.commit();
        command.waitUntilCompleted();
        assert_eq!(command.status(), MTLCommandBufferStatus::Completed);
        unsafe { crate::terrain::copy_readback(output.contents().as_ptr().cast(), w, h, row) }
    }

    fn retire(renderer: &TerrainRenderer) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while renderer.upload_retention().busy {
            assert!(Instant::now() < deadline, "GPU permit did not retire");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    #[ignore = "requires Apple Metal; CI executes the ignored GPU suite serially"]
    fn native_hud_blends_over_live_color_with_safe_scissor_and_keeps_dirty_glyphs() {
        use helium_render_scene::hud::{Atlas, Blend, Updates, Vertex, Viewport};
        autoreleasepool(|_| {
            let renderer = TerrainRenderer::new().unwrap();
            let cancel = AtomicBool::new(false);
            let (mut fixture, _) = fixture();
            fixture.description.version = upload::BANKED_VERSION;
            fixture.pixels = Arc::new([0, 0, 255, 255].repeat(15));
            let bank = Synthetic(fixture);
            let d = bank.description();
            let identity = d.identity;
            let mut material = renderer
                .create_material_set(&[bank.clone()], &cancel, budget())
                .unwrap();
            let target = material.bank_mut(0).unwrap();
            target
                .begin_upload(&renderer, bank, &cancel, upload::Update::Full, budget())
                .unwrap();
            let limit = Instant::now() + Duration::from_secs(3);
            loop {
                match target.poll_upload(&renderer, &cancel) {
                    upload::Poll::Completed(_) => break,
                    upload::Poll::Pending => assert!(Instant::now() < limit),
                    _ => panic!("fixture atlas upload failed"),
                }
            }
            retire(&renderer);
            let terrain = [
                [-1.0, -1.0, 0.5],
                [1.0, -1.0, 0.5],
                [1.0, 1.0, 0.5],
                [-1.0, 1.0, 0.5],
            ]
            .map(|pos| TerrainVertex {
                pos,
                uv: [0; 2],
                layer: OPAQUE_LAYER,
                light: 0,
                color: 0xffffff,
            });
            let terrain = LiveSection::new(
                &renderer,
                &Mesh::from_vertices(&terrain, &[0, 1, 2, 0, 2, 3]),
                identity,
                d.split,
                d.sprites,
                4096,
            )
            .unwrap();
            let sections = [(&terrain, View::identity())];
            let live = || LiveFrame {
                sections: &sections,
                identity,
                lightmap: &[255; 1024],
                clear_rgb: [0.0; 3],
            };
            let mut scene = LiveScene::new(&renderer, [64, 64]).unwrap();
            let viewport = Viewport::new([64, 64], [4, 8, 4, 4]);
            let mut updates = Updates::new(Atlas::new(1, [1, 1], Arc::new(vec![255; 4])).unwrap());
            let quad = |color| {
                let positions = [[0.0, 0.0], [16.0, 0.0], [16.0, 16.0], [0.0, 16.0]];
                [0, 1, 2, 0, 2, 3].map(|i| Vertex {
                    pos: positions[i],
                    uv: [0.5; 2],
                    color,
                    depth: 0.5,
                    glint_uv: [0.0; 2],
                    text: 0,
                })
            };
            let vertices = [quad(0x800000ff), quad(0xff00ff00)].concat();
            let runs = vec![(0..6, Blend::Alpha), (6..12, Blend::Depth([8, 8, 8, 8]))];
            let mut encode_times = Vec::new();
            for frame in 0..3 {
                if frame == 1 {
                    updates.patch([0, 0, 1, 1], &[255, 0, 0, 255]).unwrap();
                }
                let hud = updates
                    .frame(viewport, 2.0, vertices.clone(), runs.clone())
                    .unwrap();
                assert!(matches!(
                    scene
                        .encode_and_submit(
                            &renderer,
                            &material,
                            None,
                            live(),
                            Some(&hud),
                            terrain.bytes(),
                            &cancel
                        )
                        .unwrap(),
                    LiveSubmission::Submitted
                ));
                let limit = Instant::now() + Duration::from_secs(3);
                loop {
                    if let Some(done) = scene.poll(None, &cancel).unwrap() {
                        assert_eq!(done.hud_bytes, 516);
                        assert_eq!(done.hud_uploaded_bytes, if frame < 2 { 4 } else { 0 });
                        encode_times.push(done.encode_time.as_micros());
                        break;
                    }
                    assert!(Instant::now() < limit);
                }
                retire(&renderer);
                let bytes = readback(&renderer, &scene);
                let at = |x: usize, y: usize| &bytes[(y * 64 + x) * 4..][..4];
                assert_eq!(at(1, 1), [0, 0, 255, 255]);
                assert!(
                    at(6, 10)
                        .iter()
                        .zip([128u8, 0, 127, 255])
                        .all(|(&a, b)| a.abs_diff(b) <= 1)
                );
                assert_eq!(
                    at(13, 17),
                    if frame > 0 {
                        [0, 0, 0, 255]
                    } else {
                        [0, 255, 0, 255]
                    }
                );
            }
            let hud = updates.frame(viewport, 2.0, vertices, runs).unwrap();
            assert!(matches!(
                scene
                    .encode_and_submit(
                        &renderer,
                        &material,
                        None,
                        live(),
                        Some(&hud),
                        terrain.bytes(),
                        &cancel
                    )
                    .unwrap(),
                LiveSubmission::Submitted
            ));
            assert!(matches!(
                scene
                    .encode_and_submit(
                        &renderer,
                        &material,
                        None,
                        live(),
                        Some(&hud),
                        terrain.bytes(),
                        &cancel
                    )
                    .unwrap(),
                LiveSubmission::Pending
            ));
            cancel.store(true, Ordering::Release);
            drop(hud);
            drop(scene);
            retire(&renderer);
            eprintln!("S2E SYNTHETIC macOS Metal HUD frame encode_us={encode_times:?}");
        });
    }

    #[test]
    #[ignore = "requires Apple Metal; CI executes the ignored GPU suite serially"]
    fn native_translucency_blends_in_depth_order_over_terrain_and_sky() {
        autoreleasepool(|_| {
            let renderer = TerrainRenderer::new().unwrap();
            let cancel = AtomicBool::new(false);
            let (mut fixture, _) = fixture();
            fixture.description.version = upload::BANKED_VERSION;
            fixture.pixels = Arc::new(
                [
                    [255, 0, 0, 255].repeat(5),
                    [0, 0, 255, 128].repeat(5),
                    [0, 255, 0, 128].repeat(5),
                ]
                .concat(),
            );
            let bank = Synthetic(fixture);
            let d = bank.description();
            let mut material = renderer
                .create_material_set(&[bank.clone()], &cancel, budget())
                .unwrap();
            let target = material.bank_mut(0).unwrap();
            target
                .begin_upload(&renderer, bank, &cancel, upload::Update::Full, budget())
                .unwrap();
            let started = Instant::now();
            loop {
                match target.poll_upload(&renderer, &cancel) {
                    upload::Poll::Completed(_) => break,
                    upload::Poll::Pending => assert!(started.elapsed() < Duration::from_secs(3)),
                    _ => panic!("translucency fixture upload failed"),
                }
            }
            retire(&renderer);
            let section = |layer: u16, z, opaque: bool| {
                let vertices = [
                    [-0.5, -0.5, z],
                    [0.5, -0.5, z],
                    [0.5, 0.5, z],
                    [-0.5, 0.5, z],
                ]
                .map(|pos| TerrainVertex {
                    pos,
                    uv: [0; 2],
                    layer,
                    light: 0,
                    color: 0xffffff,
                });
                LiveSection::with_passes(
                    &renderer,
                    &Mesh::from_vertices(&vertices, &[0, 1, 2, 0, 2, 3]),
                    d.identity,
                    d.split,
                    d.sprites,
                    4096,
                    usize::from(opaque) * 6,
                )
                .unwrap()
            };
            let solid = section(OPAQUE_LAYER, 0.50, true);
            let front = section(1, 0.75, false);
            let back = section(2, 0.25, false);
            let view = View::identity();
            let mut scene = LiveScene::new(&renderer, [64, 64]).unwrap();
            let both_passes = [(&front, view), (&solid, view), (&back, view)];
            let over_sky = [(&front, view), (&back, view)];
            let channels = |a: &[u8], b: [u8; 4]| a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 2);
            for (sections, expected) in [
                (&both_passes[..], [127, 0, 128, 255]),
                (&over_sky[..], [16, 64, 192, 255]),
            ] {
                assert!(matches!(
                    scene
                        .submit(
                            &renderer,
                            &material,
                            None,
                            LiveFrame {
                                sections,
                                identity: d.identity,
                                lightmap: &[255; 1024],
                                clear_rgb: [0.25, 0.0, 1.0],
                            },
                            &cancel
                        )
                        .unwrap(),
                    LiveSubmission::Submitted
                ));
                let started = Instant::now();
                loop {
                    if let Some(result) = scene.poll(None, &cancel).unwrap() {
                        assert_eq!(result.draws, sections.len() as u64);
                        break;
                    }
                    assert!(started.elapsed() < Duration::from_secs(3));
                }
                retire(&renderer);
                let pixels = readback(&renderer, &scene);
                let center = (32 * 64 + 32) * 4;
                assert!(
                    channels(&pixels[center..center + 4], expected),
                    "{:?}",
                    &pixels[center..center + 4]
                );
                assert!(channels(&pixels[..4], [64, 0, 255, 255]));
            }
            eprintln!(
                "S2D synthetic Apple Metal readback: ordered translucent terrain, reverse-Z opaque rejection and production-format clear passed"
            );
        });
    }

    #[test]
    #[ignore = "requires Apple Metal; CI executes the ignored GPU suite serially"]
    fn live_resident_sections_render_consecutive_camera_frames_and_present() {
        autoreleasepool(|_| {
            let renderer = TerrainRenderer::new().unwrap();
            let cancel = AtomicBool::new(false);
            let (mut fixture, _) = fixture();
            fixture.description.version = upload::BANKED_VERSION;
            fixture.pixels = Arc::new(
                [
                    [255, 0, 0, 255].repeat(5),
                    [0, 0, 255, 255].repeat(5),
                    [0, 255, 0, 255].repeat(5),
                ]
                .concat(),
            );
            let bank = Synthetic(fixture);
            let d = bank.description();
            let mut material = renderer
                .create_material_set(std::slice::from_ref(&bank), &cancel, budget())
                .unwrap();
            let make_vertices = |layer, z| {
                [
                    [-0.5, -0.5, z],
                    [0.5, -0.5, z],
                    [0.5, 0.5, z],
                    [-0.5, 0.5, z],
                ]
                .map(|pos| TerrainVertex {
                    pos,
                    uv: [0; 2],
                    layer: OPAQUE_LAYER | layer,
                    light: 0,
                    color: 0xffffff,
                })
            };
            let indices = [0, 1, 2, 0, 2, 3];
            let section = |layer, z, id| {
                LiveSection::new(
                    &renderer,
                    &Mesh::from_vertices(&make_vertices(layer, z), &indices),
                    id,
                    d.split,
                    d.sprites,
                    4096,
                )
                .unwrap()
            };
            let front = section(0, 0.75, d.identity);
            let back = section(1, 0.25, d.identity);
            let replacement = section(2, 0.75, d.identity);
            let obsolete = section(
                0,
                0.75,
                upload::Identity {
                    generation: 0,
                    revision: 0,
                },
            );
            let mut scene = LiveScene::new(&renderer, [64, 64]).unwrap();
            let display = renderer.detached_diagnostic_display(64, 64).unwrap();
            let light = [255; 1024];
            let identity = View::identity();
            let initial = [(&front, identity), (&back, identity)];
            let f = || LiveFrame {
                sections: &initial,
                identity: d.identity,
                lightmap: &light,
                clear_rgb: [0.0; 3],
            };
            assert!(
                scene
                    .submit(&renderer, &material, None, f(), &cancel)
                    .is_err(),
                "unready texture"
            );
            let target = material.bank_mut(0).unwrap();
            target
                .begin_upload(
                    &renderer,
                    bank.clone(),
                    &cancel,
                    upload::Update::Full,
                    budget(),
                )
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match target.poll_upload(&renderer, &cancel) {
                    upload::Poll::Completed(_) => break,
                    upload::Poll::Pending => {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    _ => panic!("synthetic upload failed"),
                }
            }
            retire(&renderer);
            assert!(
                scene
                    .submit(
                        &renderer,
                        &material,
                        None,
                        LiveFrame {
                            sections: &[(&obsolete, identity)],
                            identity: d.identity,
                            lightmap: &light,
                            clear_rgb: [0.0; 3],
                        },
                        &cancel
                    )
                    .is_err(),
                "wrong world/renderer generation"
            );

            let color_owner = scene.color.clone();
            let depth_owner = scene.depth.clone();
            let mut gpu_us = Vec::new();
            let mut enc_us = Vec::new();
            for frame in 0u32..120 {
                if frame == 60 {
                    let mut changed = bank.clone();
                    changed.0.description.identity.revision += 1;
                    Arc::make_mut(&mut changed.0.pixels)[0] = 128;
                    let target = material.bank_mut(0).unwrap();
                    let update = target.changed_layers(&changed);
                    assert_eq!(update, upload::Update::Layers { first: 0, count: 1 });
                    target
                        .begin_upload(&renderer, changed, &cancel, update, budget())
                        .unwrap();
                    let deadline = Instant::now() + Duration::from_secs(3);
                    loop {
                        match target.poll_upload(&renderer, &cancel) {
                            upload::Poll::Completed(_) => break,
                            upload::Poll::Pending => {
                                assert!(Instant::now() < deadline);
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            _ => panic!("partial animated update failed"),
                        }
                    }
                    retire(&renderer);
                    assert!(
                        scene
                            .submit(&renderer, &material, None, f(), &cancel)
                            .is_err(),
                        "stale material revision"
                    );
                }
                let front = if frame < 90 { &front } else { &replacement };
                let x = if frame.is_multiple_of(2) {
                    -0.25
                } else {
                    -0.75
                };
                let both = [
                    (
                        front,
                        View {
                            origin: [x, 0., 0.],
                            ..identity
                        },
                    ),
                    (
                        &back,
                        View {
                            origin: [0.25, 0., 0.],
                            ..identity
                        },
                    ),
                ];
                let sections = if frame < 119 { &both[..] } else { &both[1..] };
                let target = (frame < 4).then_some(&display);
                assert!(matches!(
                    scene
                        .submit(
                            &renderer,
                            &material,
                            target,
                            LiveFrame {
                                sections,
                                identity: material.description().identity,
                                lightmap: &light,
                                clear_rgb: [0.0; 3],
                            },
                            &cancel
                        )
                        .unwrap(),
                    LiveSubmission::Submitted
                ));
                assert!(matches!(
                    scene
                        .submit(&renderer, &material, target, f(), &cancel)
                        .unwrap(),
                    LiveSubmission::Pending
                ));
                let deadline = Instant::now() + Duration::from_secs(3);
                let terminal = loop {
                    if let Some(result) = scene.poll(target, &cancel).unwrap() {
                        break result;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                };
                retire(&renderer);
                assert_eq!(terminal.draws, sections.len() as u64);
                assert_eq!(scene.color, color_owner);
                assert_eq!(scene.depth, depth_owner);
                enc_us.push(terminal.encode_time.as_micros());
                gpu_us.extend(terminal.gpu_time.map(|time| time.as_micros()));
                if [0, 1, 90, 119].contains(&frame) {
                    let pixels = readback(&renderer, &scene);
                    let pixel = &pixels[(32 * 64 + 31) * 4..][..4];
                    let expected = match frame {
                        0 => [255, 0, 0, 255],
                        90 => [0, 255, 0, 255],
                        _ => [0, 0, 255, 255],
                    };
                    // x=31 is inside the front at x=-.25, outside at x=-.75.
                    assert_eq!(pixel, expected, "fixture pixel for frame {frame}");
                }
            }
            enc_us.sort_unstable();
            gpu_us.sort_unstable();
            eprintln!(
                "S2B SYNTHETIC macOS Metal: 120 consecutive frames, 4 detached presentations, resident targets reused; encode_us_p50={} gpu_us_p50={:?}",
                enc_us[60],
                gpu_us.get(gpu_us.len() / 2)
            );
            let last = [(&back, identity)];
            assert!(matches!(
                scene
                    .submit(
                        &renderer,
                        &material,
                        None,
                        LiveFrame {
                            sections: &last,
                            identity: material.description().identity,
                            lightmap: &light,
                            clear_rgb: [0.0; 3],
                        },
                        &cancel
                    )
                    .unwrap(),
                LiveSubmission::Submitted
            ));
            cancel.store(true, Ordering::Release);
            drop((scene, material, display, front, back, replacement, obsolete));
            retire(&renderer);
        });
    }
}
