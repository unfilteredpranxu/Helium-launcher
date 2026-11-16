//! Nonblocking resident draws using the original G02 encoder/pipeline.
//! Completion blocks retain CPU ownership/permit only; they never publish success.
use super::*;
use crate::{
    frame::{self, Counters, Poll, Protocol, Receipt},
    upload::{Id, Material, Ownership, Phase, Source, Stamp},
};
use block2::RcBlock;
use std::sync::Arc;

type Command = Retained<ProtocolObject<dyn MTLCommandBuffer>>;
struct Targets {
    color: Texture,
    depth: Texture,
    width: usize,
    height: usize,
}
struct Lease<S> {
    owners: Ownership<S>,
    _sources: Vec<Arc<S>>,
    _device: Retained<Device>,
    _queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
}
struct Resources {
    _vertices: Buffer,
    _indices: Buffer,
    _uniforms: Buffer,
    _staging: Buffer,
    _lightmap: Texture,
    _sprites: Texture,
    _pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    _depth_state: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
}
struct Pending<S> {
    command: Command,
    _resources: Resources,
    lease: Arc<Lease<S>>,
}
/// Owner-thread controller. One pending draw; shares the process-wide upload
/// permit. No Send/Sync overrides, blocking Drop, implicit readback or GPU waits.
/// Retains one reusable color/depth pair, never a completed-frame history.
pub struct Frames<S: Source> {
    device: Id,
    owner: Id,
    released: bool,
    protocol: Option<Protocol<S::Generation>>,
    pending: Option<Pending<S>>,
    targets: Option<Targets>,
    source: Option<Arc<S>>,
    counters: Counters,
}
/// Borrowed GPU output after one-use receipt consumption. The borrow prevents
/// target reuse/release while this view exists. No raw texture escapes to an
/// untracked external command; presentation is deliberately a later milestone.
///
/// ```compile_fail,E0499
/// use helium_metal_prototype::{frame::{Frames, Receipt}, terrain::TerrainRenderer, upload::{Source, Material}};
/// fn cannot_reuse<S: Source>(f: &mut Frames<S>, r: &TerrainRenderer,
///     m: &Material<S>, a: &S::Authority, receipt: &mut Receipt<S::Generation>) {
///     let output = f.take_completed(r, m, a, receipt).unwrap();
///     f.release();
///     output.dimensions();
/// }
/// ```
pub struct CompletedFrame<'a, S: Source> {
    frames: &'a mut Frames<S>,
    renderer: &'a TerrainRenderer,
    material: &'a Material<S>,
    authority: &'a S::Authority,
}
impl TerrainRenderer {
    pub fn create_frames<S: Source>(&self) -> Result<Frames<S>, String> {
        Ok(Frames {
            device: self.upload_device,
            owner: Id::new()?,
            released: false,
            protocol: None,
            pending: None,
            targets: None,
            source: None,
            counters: Counters::default(),
        })
    }
}
impl<S: Source> Frames<S> {
    pub fn counters(&self) -> Counters {
        self.counters
    }
    pub fn phase(&self) -> Option<Phase> {
        self.protocol.as_ref().map(Protocol::phase)
    }
    fn invalidate(&mut self, reason: String) {
        if let Some(p) = self.protocol.as_mut() {
            if p.phase() != Phase::Invalidated {
                self.counters.invalidated = self.counters.invalidated.saturating_add(1);
            }
            p.invalidate(reason);
        }
    }
    /// Permanent release. A pending retaining command and its copied CPU lease
    /// retire asynchronously; invalid output cannot be exposed by subsequent polls.
    pub fn release(&mut self) {
        self.released = true;
        self.invalidate("frame owner released".into());
        self.source = None;
        // Keep targets/accounting until terminal observation while owner survives.
        if self.pending.is_none() {
            self.targets = None;
            self.counters.allocated_payload = 0;
        }
    }
    fn authorized(
        &self,
        renderer: &TerrainRenderer,
        material: &Material<S>,
        authority: &S::Authority,
    ) -> Result<Stamp<S::Generation>, String> {
        if self.released || self.device != renderer.upload_device {
            return Err("released/wrong frame device".into());
        }
        let (stamp, _, _) = material.resident(renderer, authority)?;
        self.protocol
            .as_ref()
            .ok_or("no submitted frame")?
            .check(stamp, renderer.upload_device)?;
        Ok(stamp)
    }
    pub fn take_completed<'a>(
        &'a mut self,
        renderer: &'a TerrainRenderer,
        material: &'a Material<S>,
        authority: &'a S::Authority,
        receipt: &mut Receipt<S::Generation>,
    ) -> Result<CompletedFrame<'a, S>, String> {
        if self.pending.is_some() || self.targets.is_none() {
            return Err("frame has no completed output".into());
        }
        let stamp = self.authorized(renderer, material, authority)?;
        self.protocol
            .as_ref()
            .unwrap()
            .consume(receipt, stamp, self.device, self.owner)?;
        Ok(CompletedFrame {
            frames: self,
            renderer,
            material,
            authority,
        })
    }
}
impl<S: Source + Send + Sync + 'static> Frames<S> {
    /// CPU encoding/allocation may take time. There is NO scheduling/completion
    /// wait, sleep, polling loop or CPU readback in this method.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &mut self,
        renderer: &TerrainRenderer,
        material: &Material<S>,
        authority: &S::Authority,
        mesh: &Mesh<'_>,
        lightmap: &[u8],
        view: View,
        options: Options,
    ) -> Result<(), String> {
        if self.pending.is_some() {
            self.counters.busy_rejections = self.counters.busy_rejections.saturating_add(1);
            return Err("frame slot busy".into());
        }
        if self.released || self.device != renderer.upload_device {
            return Err("released/wrong frame device".into());
        }
        let permit = renderer.upload_slot.acquire().inspect_err(|_| {
            self.counters.busy_rejections = self.counters.busy_rejections.saturating_add(1);
        })?;
        let (stamp, source, sprites) = material.resident(renderer, authority)?;
        let d = source.description();
        let bindings = material.bindings()?;
        let source_payload = material.source_payload()?;
        let layout = if material.is_banked() {
            let ds = material.descriptions();
            let metadata = ds
                .len()
                .checked_mul(
                    size_of::<crate::upload::Description<S::Generation>>()
                        + size_of::<(u32, Texture)>()
                        + 2 * size_of::<Arc<S>>()
                        + 32,
                )
                .ok_or("banked frame metadata overflow")?;
            crate::banks::check_mesh(mesh, &ds)?;
            let mut layout = super::super::resource_layout_ceiling(
                mesh,
                (d.size as usize, d.size as usize, d.sprites as usize),
                material.counters().texture_payload,
                false,
                false,
                lightmap,
                view,
                options,
                &renderer.capabilities,
                32768,
            )?;
            layout.payload_bytes = layout
                .payload_bytes
                .checked_add(source_payload)
                .and_then(|n| n.checked_add(metadata))
                .ok_or("frame group overflow")?;
            if layout.payload_bytes > options.budget.payload_bytes {
                return Err("banked frame budget exceeded".into());
            }
            layout
        } else {
            frame::validate(
                mesh,
                d,
                material.counters().texture_payload,
                source_payload,
                lightmap,
                view,
                options,
                &renderer.capabilities,
            )?
        };
        let mut protocol = Protocol::prepare(stamp, self.device, self.owner)?;
        // A new accepted frame supersedes old completion even if allocation fails.
        self.invalidate("frame superseded".into());
        self.protocol = None;
        self.source = None;
        let reusable = self
            .targets
            .as_ref()
            .is_some_and(|t| t.width == options.width && t.height == options.height);
        if !reusable {
            self.targets = None;
        }
        self.counters.allocated_payload =
            self.targets.as_ref().map_or(0, |t| t.width * t.height * 8);
        let prepared = autoreleasepool(|_| {
            if self.targets.is_none() {
                self.targets = Some(Targets {
                    color: texture(
                        &renderer.device,
                        options.width,
                        options.height,
                        1,
                        false,
                        MTLPixelFormat::RGBA8Unorm,
                        MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead,
                    )?,
                    depth: texture(
                        &renderer.device,
                        options.width,
                        options.height,
                        1,
                        false,
                        MTLPixelFormat::Depth32Float,
                        MTLTextureUsage::RenderTarget,
                    )?,
                    width: options.width,
                    height: options.height,
                });
            }
            let targets = self.targets.as_ref().unwrap();
            let vertices = upload(&renderer.device, mesh.vertices)?;
            // SAFETY: validated u32 slice length, no padding, synchronous copy.
            let bytes = unsafe {
                std::slice::from_raw_parts(mesh.indices.as_ptr().cast(), layout.index_bytes)
            };
            let indices = upload(&renderer.device, bytes)?;
            let mut uniforms = [0f32; 20];
            for (to, from) in uniforms[..16]
                .iter_mut()
                .zip(view.clip_from_relative.iter().flatten())
            {
                *to = *from;
            }
            uniforms[16..19].copy_from_slice(&view.origin);
            uniforms[19] = UV_STEP;
            // SAFETY: fully initialized same 80-byte G02 uniform ABI.
            let uniforms = upload(&renderer.device, unsafe {
                std::slice::from_raw_parts(uniforms.as_ptr().cast(), 80)
            })?;
            // Fixed stack scratch: no frame-sized CPU allocation or atlas copy.
            let mut pixels = [0u8; 4096];
            for row in 0..16 {
                pixels[row * 256..row * 256 + 64]
                    .copy_from_slice(&lightmap[row * 64..row * 64 + 64]);
            }
            let staging = upload(&renderer.device, &pixels)?;
            let lm = texture(
                &renderer.device,
                16,
                16,
                1,
                false,
                MTLPixelFormat::RGBA8Unorm,
                MTLTextureUsage::ShaderRead,
            )?;
            // Retaining command buffer owns native resources even if Frames drops.
            let command = renderer
                .queue
                .commandBuffer()
                .ok_or("frame command allocation failed")?;
            let blit = command
                .blitCommandEncoder()
                .ok_or("frame lightmap encoder allocation failed")?;
            // SAFETY: 4096 initialized bytes, 16 aligned rows, same-device texture.
            unsafe {
                blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                &staging, 0, 256, 4096, MTLSize {width:16,height:16,depth:1}, &lm,0,0,MTLOrigin{x:0,y:0,z:0});
            }
            blit.endEncoding();
            renderer.encode_terrain(
                &command,
                &vertices,
                &indices,
                &uniforms,
                &lm,
                &sprites,
                &targets.color,
                &targets.depth,
                mesh.indices.len(),
                options,
                material
                    .is_banked()
                    .then_some((mesh, bindings.as_slice(), d.split)),
            )?;
            protocol.encoded()?;
            // Full frame payload conservatively occupies this shared slot until
            // terminal cleanup; not an assertion about physical allocator bytes.
            permit.account(material.counters().texture_payload, 4096, source_payload);
            permit.account_frame(
                layout.payload_bytes - source_payload - material.counters().texture_payload - 8192,
            );
            let lease = Arc::new(Lease {
                _sources: material.sources(),
                owners: Ownership {
                    source,
                    previous: None,
                    _permit: permit,
                },
                _device: renderer.device.clone(),
                _queue: renderer.queue.clone(),
            });
            let retained = lease.clone();
            let keepalive = move |_command: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                let _source = (&retained.owners.source, &retained._sources);
            };
            fn require_send_sync_static<T: Send + Sync + 'static>(_: &T) {}
            require_send_sync_static(&keepalive);
            let block = RcBlock::new(keepalive);
            // SAFETY: Metal copies this owned Send+Sync+'static block. No command,
            // GPU object, borrowed authority, self pointer or reference cycle captured.
            unsafe {
                command.addCompletedHandler(RcBlock::as_ptr(&block));
            }
            Ok::<_, String>(Pending {
                command,
                lease,
                _resources: Resources {
                    _vertices: vertices,
                    _indices: indices,
                    _uniforms: uniforms,
                    _staging: staging,
                    _lightmap: lm,
                    _sprites: sprites,
                    _pipeline: renderer.pipeline.clone(),
                    _depth_state: if options.depth == Depth::Reversed {
                        renderer.reversed.clone()
                    } else {
                        renderer.forward.clone()
                    },
                },
            })
        });
        let pending = match prepared {
            Ok(p) => p,
            Err(e) => {
                self.targets = None;
                self.counters.allocated_payload = 0;
                self.counters.failed = self.counters.failed.saturating_add(1);
                return Err(e);
            }
        };
        if reusable {
            self.counters.reused_targets = self.counters.reused_targets.saturating_add(1);
        }
        self.counters.allocated_payload = layout.payload_bytes - 4096; // stack staging has returned
        self.counters.pending = 1;
        self.counters.peak_payload = self.counters.peak_payload.max(layout.payload_bytes);
        self.protocol = Some(protocol);
        self.pending = Some(pending); // install BEFORE commit
        self.pending.as_ref().unwrap().command.commit();
        self.counters.submitted = self.counters.submitted.saturating_add(1);
        self.protocol
            .as_mut()
            .unwrap()
            .submitted()
            .inspect_err(|_| self.invalidate("submission protocol invalidated".into()))
    }
    /// Observe once. A logically invalidated command remains Pending until Metal
    /// reports Completed/Error. No receipt is published by the completion block.
    pub fn poll(
        &mut self,
        renderer: &TerrainRenderer,
        material: &Material<S>,
        authority: &S::Authority,
    ) -> Poll<S::Generation> {
        if self.pending.is_none() {
            return Poll::Idle;
        }
        self.counters.pending_polls = self.counters.pending_polls.saturating_add(1);
        if let Err(e) = self.authorized(renderer, material, authority) {
            self.invalidate(e);
        }
        let pending = self.pending.as_ref().unwrap();
        let status = pending.command.status();
        let terminal = if status == MTLCommandBufferStatus::Completed
            || status == MTLCommandBufferStatus::Error
        {
            Some(status == MTLCommandBufferStatus::Completed && pending.command.error().is_none())
        } else {
            None
        };
        let mut result = self.protocol.as_mut().unwrap().observe(terminal);
        if terminal == Some(false) && self.protocol.as_ref().unwrap().phase() == Phase::Failed {
            result = Poll::Failed(format!(
                "Metal frame command failed: {status:?}, {:?}",
                pending.command.error()
            ));
        }
        if terminal.is_some() {
            let pending = self.pending.take().unwrap();
            self.counters.pending = 0;
            match &result {
                Poll::Completed(_) => {
                    self.source = Some(pending.lease.owners.source.clone());
                    self.counters.completed = self.counters.completed.saturating_add(1);
                    let t = self.targets.as_ref().unwrap();
                    self.counters.allocated_payload = t.width * t.height * 8
                        + material.source_payload().unwrap_or(usize::MAX)
                        + material.counters().texture_payload;
                }
                _ => {
                    self.targets = None;
                    self.source = None;
                    self.counters.allocated_payload = 0;
                    self.counters.failed = self.counters.failed.saturating_add(1);
                }
            }
        }
        result
    }
}
impl<S: Source> CompletedFrame<'_, S> {
    pub(super) fn presentation_source(&self) -> Result<(Texture, Arc<S>, usize), String> {
        self.frames
            .authorized(self.renderer, self.material, self.authority)?;
        Ok((
            self.frames.targets.as_ref().unwrap().color.clone(),
            self.frames.source.as_ref().unwrap().clone(),
            self.frames.counters.allocated_payload,
        ))
    }
    pub(super) fn presentation_renderer(&self) -> &TerrainRenderer {
        self.renderer
    }
    pub(super) fn presentation_current(&self) -> Result<(), String> {
        self.frames
            .authorized(self.renderer, self.material, self.authority)
            .map(|_| ())
    }
}
impl<S: Source> CompletedFrame<'_, S> {
    pub fn dimensions(&self) -> (usize, usize) {
        let t = self.frames.targets.as_ref().unwrap();
        (t.width, t.height)
    }
    /// Explicit diagnostic worker operation. Never called by begin/poll. Budget
    /// includes live frame payload, shared readback and returned tight CPU pixels.
    pub fn readback_for_validation(&mut self, budget: usize) -> Result<Image, String> {
        self.frames
            .authorized(self.renderer, self.material, self.authority)?;
        let _permit = self.renderer.upload_slot.acquire()?;
        let t = self.frames.targets.as_ref().unwrap();
        let row = aligned_row(t.width)?;
        let bytes = product(&[row, t.height])?;
        let tight = product(&[t.width, t.height, 4])?;
        let peak = self
            .frames
            .counters
            .allocated_payload
            .checked_add(bytes)
            .and_then(|n| n.checked_add(tight))
            .ok_or("diagnostic payload overflow")?;
        if peak > budget
            || bytes > self.renderer.capabilities.max_buffer_bytes
            || bytes > isize::MAX as usize
        {
            return Err("diagnostic readback budget/buffer limit exceeded".into());
        }
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(tight)
            .map_err(|_| "diagnostic CPU allocation failed")?;
        if rgba.capacity() != tight {
            return Err("unexpected diagnostic CPU capacity".into());
        }
        autoreleasepool(|_| {
            let buffer = self
                .renderer
                .device
                .newBufferWithLength_options(bytes, MTLResourceOptions::StorageModeShared)
                .ok_or("frame readback allocation failed")?;
            let command = self
                .renderer
                .queue
                .commandBuffer()
                .ok_or("frame readback command allocation failed")?;
            let blit = command
                .blitCommandEncoder()
                .ok_or("frame readback encoder allocation failed")?;
            // SAFETY: completed stored color target and checked buffer/pitches.
            unsafe {
                blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                &t.color,0,0,MTLOrigin{x:0,y:0,z:0},MTLSize{width:t.width,height:t.height,depth:1},&buffer,0,row,bytes);
            }
            blit.endEncoding();
            command.commit();
            command.waitUntilCompleted();
            if command.status() != MTLCommandBufferStatus::Completed || command.error().is_some() {
                return Err(format!(
                    "diagnostic frame readback failed: {:?}, {:?}",
                    command.status(),
                    command.error()
                ));
            }
            for y in 0..t.height {
                // SAFETY: only initialized row prefixes after successful completion.
                let pixels = unsafe {
                    std::slice::from_raw_parts(
                        buffer.contents().as_ptr().cast::<u8>().add(y * row),
                        t.width * 4,
                    )
                };
                rgba.extend_from_slice(pixels);
            }
            self.frames.counters.peak_payload = self.frames.counters.peak_payload.max(peak);
            self.frames.counters.diagnostic_readback_bytes = self
                .frames
                .counters
                .diagnostic_readback_bytes
                .saturating_add(tight as u64);
            Ok(Image {
                rgba,
                gpu_seconds: None,
            })
        })
    }
}
