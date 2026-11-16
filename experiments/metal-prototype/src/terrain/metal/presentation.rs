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
pub struct Display {
    layer: Retained<CAMetalLayer>,
    device: crate::upload::Id,
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    size: (usize, usize),
    used: bool,
    progress: display::Progress,
}
impl TerrainRenderer {
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
            layer,
            device: self.upload_device,
            pipeline,
            size,
            used: false,
            progress: display::Progress::default(),
        })
    }
}
impl Display {
    pub fn counters(&self) -> display::Counters {
        self.progress.counters
    }

    fn encode(
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
        let lease = Arc::new(Ownership {
            source: owner,
            previous: None,
            _permit: permit,
        });
        let kept = lease.clone();
        let callback = move |_: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            let _source = &kept.source;
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
        command.commit();
        Ok(Some(Presentation {
            output,
            display: self,
            command,
            _drawable: drawable,
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
        result.map_err(|e| format!("{e}: {status:?}, {:?}", self.command.error()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
