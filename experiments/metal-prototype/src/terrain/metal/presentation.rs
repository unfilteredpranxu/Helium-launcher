//! One worker-owned CAMetalLayer. Acquisition may wait for Apple's timeout.
use super::*;
use crate::{
    display,
    frame::CompletedFrame,
    upload::{Ownership, Source},
};
use block2::RcBlock;
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use std::sync::Arc;
struct KeepAlive<S> {
    owners: Ownership<S>,
    _device: Retained<Device>,
    _queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
}
// An attached UI owner may disappear while the GPU still owns the drawable.
// Bind the layer to the exact native command through Objective-C ownership;
// never transfer a !Send Rust CAMetalLayer into a Rust completion closure.
static LAYER_OWNER_KEY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
fn retain_layer_for_command(command: &ProtocolObject<dyn MTLCommandBuffer>, layer: &CAMetalLayer) {
    use objc2::ffi::{OBJC_ASSOCIATION_RETAIN_NONATOMIC, objc_setAssociatedObject};
    let key = std::ptr::from_ref(&LAYER_OWNER_KEY).cast();
    // SAFETY: live native objects, unique process-static key, retain policy. No
    // borrowed Rust data is stored. The layer has no reference back to command.
    unsafe {
        objc_setAssociatedObject(
            std::ptr::from_ref(command).cast_mut().cast(),
            key,
            std::ptr::from_ref(layer).cast_mut().cast(),
            OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }
    let callback = |completed: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
        // Metal invokes this only at terminal execution, including errors. The
        // command argument is live. Clearing its native reference does not touch
        // UIKit, a Rust Display/session, or any borrowed stack state.
        unsafe {
            objc_setAssociatedObject(
                completed.as_ptr().cast(),
                std::ptr::from_ref(&LAYER_OWNER_KEY).cast(),
                std::ptr::null_mut(),
                OBJC_ASSOCIATION_RETAIN_NONATOMIC,
            );
        }
    };
    fn checked<T: Send + Sync + 'static>(_: &T) {}
    checked(&callback);
    let block = RcBlock::new(callback);
    // Metal copies the block. It captures no layer, command or session; no cycle.
    unsafe {
        command.addCompletedHandler(RcBlock::as_ptr(&block));
    }
}
pub struct Display {
    completion: Option<Arc<crate::attached_protocol::Gate>>,
    layer: Retained<CAMetalLayer>,
    device: crate::upload::Id,
    live_owner: crate::upload::Id,
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    size: (usize, usize),
    used: bool,
    progress: display::Progress,
}
impl TerrainRenderer {
    /// Worker-only, UNATTACHED diagnostic target. This is not a game surface or
    /// evidence of visible display; never competes with wgpu's drawable.
    pub fn detached_diagnostic_display(
        &self,
        width: usize,
        height: usize,
    ) -> Result<Display, String> {
        display::fit((width, height), (width, height), 0, 192 * 1024 * 1024)?;
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&self.device));
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        layer.setAllowsNextDrawableTimeout(true);
        layer.setPresentsWithTransaction(false);
        layer.setMaximumDrawableCount(3);
        let mut size = layer.drawableSize();
        size.width = width as f64;
        size.height = height as f64;
        layer.setDrawableSize(size);
        let space = objc2_core_graphics::CGColorSpace::with_name(Some(unsafe {
            objc2_core_graphics::kCGColorSpaceLinearSRGB
        }))
        .ok_or("linear-sRGB unavailable")?;
        layer.setColorspace(Some(&space));
        self.display(layer)
    }

    /// Layer must be configured by its UI owner for SDR linear-sRGB BGRA8Unorm,
    /// with stable physical drawableSize until this one-shot Display is retired.
    /// Retains the layer; never stores a borrowed Swift pointer.
    pub fn display(&self, layer: Retained<CAMetalLayer>) -> Result<Display, String> {
        if objc2_foundation::NSThread::isMainThread_class() {
            return Err("display initialization requires worker".into());
        }
        if layer.device().as_deref() != Some(&*self.device)
            || layer.pixelFormat() != MTLPixelFormat::BGRA8Unorm
            || layer.presentsWithTransaction()
            || !layer.allowsNextDrawableTimeout()
        {
            return Err("wrong device/format or unsafe layer acquisition configuration".into());
        }
        let space = layer
            .colorspace()
            .ok_or("display requires explicit linear-sRGB colorspace")?;
        let name = objc2_core_graphics::CGColorSpace::name(Some(&space))
            .ok_or("unnamed display colorspace")?;
        if &*name != unsafe { objc2_core_graphics::kCGColorSpaceLinearSRGB } {
            return Err("unsupported display colorspace".into());
        }
        let s = layer.drawableSize();
        if !s.width.is_finite()
            || !s.height.is_finite()
            || s.width.fract() != 0.
            || s.height.fract() != 0.
            || s.width < 1.
            || s.height < 1.
        {
            return Err("nonphysical drawable dimensions".into());
        }
        let size = (s.width as usize, s.height as usize);
        display::fit((256, 256), size, 0, 64 * 1024 * 1024)?;
        let library = self
            .device
            .newLibraryWithSource_options_error(
                &NSString::from_str(include_str!("presentation.metal")),
                None,
            )
            .map_err(|e| format!("display shader: {e}"))?;
        let desc = MTLRenderPipelineDescriptor::new();
        desc.setVertexFunction(
            library
                .newFunctionWithName(&NSString::from_str("display_vertex"))
                .as_deref(),
        );
        desc.setFragmentFunction(
            library
                .newFunctionWithName(&NSString::from_str("display_fragment"))
                .as_deref(),
        );
        unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) }
            .setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        let pipeline = self
            .device
            .newRenderPipelineStateWithDescriptor_error(&desc)
            .map_err(|e| format!("display pipeline: {e}"))?;
        Ok(Display {
            completion: None,
            layer,
            device: self.upload_device,
            live_owner: crate::upload::Id::new()?,
            pipeline,
            size,
            used: false,
            progress: display::Progress::created(),
        })
    }
}
impl Display {
    pub(crate) fn with_attachment(mut self, gate: Arc<crate::attached_protocol::Gate>) -> Self {
        self.completion = Some(gate);
        self
    }
    pub fn counters(&self) -> display::Counters {
        self.progress.counters
    }

    pub(super) fn encode(
        &self,
        command: &ProtocolObject<dyn MTLCommandBuffer>,
        source: &Texture,
        target: &Texture,
        viewport: [f64; 4],
    ) -> Result<(), String> {
        let pass = MTLRenderPassDescriptor::new();
        let attachment = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        attachment.setTexture(Some(&target));
        attachment.setLoadAction(MTLLoadAction::Clear);
        attachment.setStoreAction(MTLStoreAction::Store);
        attachment.setClearColor(MTLClearColor {
            red: 0.,
            green: 0.,
            blue: 0.,
            alpha: 1.,
        });
        let encoder = command
            .renderCommandEncoderWithDescriptor(&pass)
            .ok_or("display encoder allocation failed")?;
        encoder.setRenderPipelineState(&self.pipeline);
        encoder.setViewport(MTLViewport {
            originX: viewport[0],
            originY: viewport[1],
            width: viewport[2],
            height: viewport[3],
            znear: 0.,
            zfar: 1.,
        });
        unsafe {
            encoder.setFragmentTexture_atIndex(Some(&source), 0);
            encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
        }
        encoder.endEncoding();
        Ok(())
    }
    pub(super) fn live_drawable(
        &self,
        renderer: &TerrainRenderer,
    ) -> Result<Option<Retained<ProtocolObject<dyn CAMetalDrawable>>>, String> {
        if self.device != renderer.upload_device || !self.live_layout_valid() {
            return Err("live display layout/device changed".into());
        }
        let Some(drawable) = self.layer.nextDrawable() else {
            return Ok(None);
        };
        let target = drawable.texture();
        if !self.live_layout_valid()
            || target.pixelFormat() != MTLPixelFormat::BGRA8Unorm
            || (target.width(), target.height()) != self.size
        {
            return Err("live drawable layout changed".into());
        }
        Ok(Some(drawable))
    }
    pub(super) fn live_owner(&self) -> crate::upload::Id {
        self.live_owner
    }
    pub(super) fn live_layout_valid(&self) -> bool {
        let size = self.layer.drawableSize();
        size.width == self.size.0 as f64
            && size.height == self.size.1 as f64
            && self.layer.pixelFormat() == MTLPixelFormat::BGRA8Unorm
    }
    pub fn dimensions(&self) -> [usize; 2] {
        [self.size.0, self.size.1]
    }
    pub(super) fn retain_for_live_command(&self, command: &ProtocolObject<dyn MTLCommandBuffer>) {
        retain_layer_for_command(command, &self.layer);
    }
    pub(super) fn confirm_live(&self) {
        if let Some(gate) = &self.completion {
            gate.native_completed();
        }
    }

    /// One acquisition attempt, worker only. No GPU wait; nextDrawable itself may
    /// wait for the OS timeout. None means recoverable drawable unavailability.
    pub fn present<'a, S: Source + Send + Sync + 'static>(
        &'a mut self,
        output: CompletedFrame<'a, S>,
        budget: usize,
        cancelled: &'a std::sync::atomic::AtomicBool,
    ) -> Result<Option<Presentation<'a, S>>, String> {
        use std::sync::atomic::Ordering;
        if objc2_foundation::NSThread::isMainThread_class() || self.used {
            self.progress.counters.skipped = self.progress.counters.skipped.saturating_add(1);
            return Err("main-thread or repeated presentation rejected".into());
        }
        let renderer = output.presentation_renderer();
        if self.device != renderer.upload_device {
            return Err("wrong display device".into());
        }
        let (source, owner, live) = output.presentation_source()?;
        let viewport = display::fit(output.dimensions(), self.size, live, budget)?;
        let permit = renderer.upload_slot.acquire().inspect_err(|_| {
            self.progress.counters.busy = self.progress.counters.busy.saturating_add(1)
        })?;
        if cancelled.load(Ordering::Acquire) {
            return Err("display suspended/released".into());
        }
        let Some(drawable) = self.layer.nextDrawable() else {
            self.progress.counters.unavailable += 1;
            return Ok(None);
        };
        let size = self.layer.drawableSize();
        if cancelled.load(Ordering::Acquire)
            || size.width != self.size.0 as f64
            || size.height != self.size.1 as f64
        {
            return Err("display resized/released during acquisition".into());
        }
        let target = drawable.texture();
        if target.pixelFormat() != MTLPixelFormat::BGRA8Unorm
            || target.width() != self.size.0
            || target.height() != self.size.1
        {
            return Err("drawable layout mismatch".into());
        }
        output.presentation_current()?;
        let command = renderer
            .queue
            .commandBuffer()
            .ok_or("display command allocation failed")?;
        self.encode(&command, &source, &target, viewport)?;
        // Retaining command owns drawable/texture/pipeline. Callback owns ONLY CPU
        // source/permit; it captures no command, view, layer, or borrowed session.
        let lease = Arc::new(KeepAlive {
            owners: Ownership {
                source: owner,
                previous: None,
                _permit: permit,
            },
            _device: renderer.device.clone(),
            _queue: renderer.queue.clone(),
        });
        let kept = lease.clone();
        let callback = move |_: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            let _source = &kept.owners.source;
        };
        fn checked<T: Send + Sync + 'static>(_: &T) {}
        checked(&callback);
        let block = RcBlock::new(callback);
        unsafe {
            command.addCompletedHandler(RcBlock::as_ptr(&block));
        }
        command.presentDrawable(ProtocolObject::from_ref(&*drawable));
        let payload = live + self.size.0 * self.size.1 * 12; // checked by fit before allocation
        self.progress.begin(payload)?;
        self.used = true;
        retain_layer_for_command(&command, &self.layer);
        command.commit();
        Ok(Some(Presentation {
            output,
            display: self,
            command,
            _drawable: drawable,
            _lease: lease,
            cancelled,
            terminal: false,
            invalid: false,
        }))
    }
}
/// Owns the exact submitted command and borrows frame/material authority until
/// retirement. Drop never waits; Metal and the copied source lease retain owners.
pub struct Presentation<'a, S: Source> {
    output: CompletedFrame<'a, S>,
    display: &'a mut Display,
    command: Retained<ProtocolObject<dyn MTLCommandBuffer>>,
    _drawable: Retained<ProtocolObject<dyn CAMetalDrawable>>,
    _lease: Arc<KeepAlive<S>>,
    cancelled: &'a std::sync::atomic::AtomicBool,
    terminal: bool,
    invalid: bool,
}
impl<S: Source> Presentation<'_, S> {
    pub fn poll(&mut self) -> Result<bool, String> {
        if self.terminal {
            return Err("presentation receipt already consumed".into());
        }
        let size = self.display.layer.drawableSize();
        let resized =
            size.width != self.display.size.0 as f64 || size.height != self.display.size.1 as f64;
        if resized && !self.invalid {
            self.display.progress.counters.resizes += 1;
        }
        let valid_space = self
            .display
            .layer
            .colorspace()
            .and_then(|s| objc2_core_graphics::CGColorSpace::name(Some(&s)))
            .is_some_and(|name| &*name == unsafe { objc2_core_graphics::kCGColorSpaceLinearSRGB });
        self.invalid |= !valid_space;
        self.invalid |= self.display.layer.pixelFormat() != MTLPixelFormat::BGRA8Unorm
            || self.display.layer.device().as_deref()
                != Some(&*self.output.presentation_renderer().device)
            || self.cancelled.load(std::sync::atomic::Ordering::Acquire)
            || size.width != self.display.size.0 as f64
            || size.height != self.display.size.1 as f64
            || self.output.presentation_current().is_err();
        let status = self.command.status();
        let terminal = if status == MTLCommandBufferStatus::Completed
            || status == MTLCommandBufferStatus::Error
        {
            Some(status == MTLCommandBufferStatus::Completed && self.command.error().is_none())
        } else {
            None
        };
        let result = self.display.progress.observe(terminal, !self.invalid);
        self.terminal = terminal.is_some();
        if result == Ok(true) {
            if let Some(gate) = &self.display.completion {
                gate.native_completed();
            }
        }
        result.map_err(|e| format!("{e}: {status:?}, {:?}", self.command.error()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "real Metal command/layer ownership and terminal retirement; NOT attached UIKit runtime"]
    fn native_command_owns_layer_until_terminal_without_view_or_rust_session() {
        use objc2::{ffi::objc_getAssociatedObject, rc::Weak};
        use std::time::{Duration, Instant};
        let renderer = TerrainRenderer::new().unwrap();
        let baseline = autoreleasepool(|_| {
            let layer = CAMetalLayer::new();
            let weak = Weak::from_retained(&layer);
            drop(layer);
            weak
        });
        eprintln!(
            "G06-C baseline CAMetalLayer weakly observable after original owner/pool release: {}",
            baseline.load().is_some()
        );
        autoreleasepool(|_| {
            let layer = CAMetalLayer::new();
            let weak = Weak::from_retained(&layer);
            let command = renderer.queue.commandBuffer().unwrap();
            retain_layer_for_command(&command, &layer);
            let associated = || unsafe {
                objc_getAssociatedObject(
                    std::ptr::from_ref(&*command).cast(),
                    std::ptr::from_ref(&LAYER_OWNER_KEY).cast(),
                )
            };
            assert_eq!(
                associated(),
                std::ptr::from_ref(&*layer).cast(),
                "exact native command owns exact layer"
            );
            drop(layer);
            assert!(
                weak.load().is_some(),
                "unsubmitted command lease survives UI/Rust owner release"
            );
            command.commit(); // may complete before first poll
            let deadline = Instant::now() + Duration::from_secs(10);
            while !matches!(
                command.status(),
                MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error
            ) {
                assert!(Instant::now() < deadline, "native command deadline");
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(command.status(), MTLCommandBufferStatus::Completed);
            assert!(command.error().is_none());
            while !associated().is_null() {
                assert!(
                    Instant::now() < deadline,
                    "exact native layer lease must retire at real terminal execution"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            eprintln!(
                "G06-C exact native command layer association cleared at terminal success; remaining native/autorelease liveness={} is not this command lease",
                weak.load().is_some()
            );
        });
    }
    #[test]
    #[ignore = "real Metal presentation-pass channel/alpha/orientation diagnostic; no drawable or scanout claim"]
    fn actual_bgra_conversion_preserves_corners_and_alpha() {
        let r = TerrainRenderer::new().unwrap();
        let _permit = r.upload_slot.acquire().unwrap();
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&r.device));
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        layer.setAllowsNextDrawableTimeout(true);
        layer.setPresentsWithTransaction(false);
        let mut size = layer.drawableSize();
        size.width = 2.;
        size.height = 2.;
        layer.setDrawableSize(size);
        let space = objc2_core_graphics::CGColorSpace::with_name(Some(unsafe {
            objc2_core_graphics::kCGColorSpaceLinearSRGB
        }))
        .unwrap();
        layer.setColorspace(Some(&space));
        let display = r.display(layer).unwrap();
        let source = texture(
            &r.device,
            2,
            2,
            1,
            false,
            MTLPixelFormat::RGBA8Unorm,
            MTLTextureUsage::ShaderRead,
        )
        .unwrap();
        let target = texture(
            &r.device,
            2,
            2,
            1,
            false,
            MTLPixelFormat::BGRA8Unorm,
            MTLTextureUsage::RenderTarget,
        )
        .unwrap();
        let mut pixels = [0u8; 512];
        pixels[..8].copy_from_slice(&[255, 0, 0, 17, 0, 255, 0, 127]);
        pixels[256..264].copy_from_slice(&[0, 0, 255, 128, 255, 255, 255, 255]);
        let staging = upload(&r.device, &pixels).unwrap();
        let readback = r
            .device
            .newBufferWithLength_options(512, MTLResourceOptions::StorageModeShared)
            .unwrap();
        let command = r.queue.commandBuffer().unwrap();
        let blit = command.blitCommandEncoder().unwrap();
        unsafe {
            blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(&staging,0,256,512,MTLSize{width:2,height:2,depth:1},&source,0,0,MTLOrigin{x:0,y:0,z:0});
        }
        blit.endEncoding();
        display
            .encode(&command, &source, &target, [0., 0., 2., 2.])
            .unwrap();
        let blit = command.blitCommandEncoder().unwrap();
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(&target,0,0,MTLOrigin{x:0,y:0,z:0},MTLSize{width:2,height:2,depth:1},&readback,0,256,512);
        }
        blit.endEncoding();
        command.commit();
        command.waitUntilCompleted();
        assert_eq!(command.status(), MTLCommandBufferStatus::Completed);
        assert!(command.error().is_none());
        let bytes = unsafe {
            crate::terrain::copy_readback(readback.contents().as_ptr().cast(), 2, 2, 256)
        };
        assert_eq!(
            bytes,
            [
                0, 0, 255, 17, 0, 255, 0, 127, 255, 0, 0, 128, 255, 255, 255, 255
            ]
        );
        eprintln!(
            "G05-D real GPU BGRA conversion: exact 4 corners, row orientation, RGB and alpha passed"
        );
    }
}
