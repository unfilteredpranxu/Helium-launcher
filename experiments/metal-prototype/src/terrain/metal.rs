//! Blocking offscreen G02 probe. No production surface or simulation ownership.
use super::*;
use objc2::{
    rc::{Retained, autoreleasepool},
    runtime::{NSObjectProtocol, ProtocolObject},
    sel,
};
use objc2_foundation::NSString;
use objc2_metal::*;
use std::{ffi::c_void, ptr::NonNull};

type Device = ProtocolObject<dyn MTLDevice>;
type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

/// Persistent device, queue, pipeline and depth states; all uploads are per-probe.
/// Call on a worker thread. One render waits for completion before releasing data.
pub struct TerrainRenderer {
    device: Retained<Device>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    reversed: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    forward: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    capabilities: Capabilities,
}
impl TerrainRenderer {
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
    pub fn new() -> Result<Self, String> {
        autoreleasepool(|_| Self::create())
    }
    fn create() -> Result<Self, String> {
        if !objc2::available!(ios = 15.0, macos = 10.15) {
            return Err("requires iOS 15/macOS 10.15".into());
        }
        let device = MTLCreateSystemDefaultDevice().ok_or("no Metal device")?;
        if !device.respondsToSelector(sel!(supportsFamily:))
            || !device.respondsToSelector(sel!(maxBufferLength))
        {
            return Err("required Metal capability queries unavailable".into());
        }
        let mut apple_families = 0;
        for n in 1..=7 {
            if device.supportsFamily(MTLGPUFamily(1000 + n)) {
                apple_families |= 1 << n;
            }
        }
        if objc2::available!(ios = 16.0, macos = 13.0)
            && device.supportsFamily(MTLGPUFamily::Apple8)
        {
            apple_families |= 1 << 8;
        }
        if objc2::available!(ios = 17.0, macos = 14.0)
            && device.supportsFamily(MTLGPUFamily::Apple9)
        {
            apple_families |= 1 << 9;
        }
        if objc2::available!(ios = 26.0, macos = 26.0)
            && device.supportsFamily(MTLGPUFamily::Apple10)
        {
            apple_families |= 1 << 10;
        }
        let mut common_families = 0;
        for n in 1..=2 {
            if device.supportsFamily(MTLGPUFamily(3000 + n)) {
                common_families |= 1 << n;
            }
        }
        if objc2::available!(ios = 16.0, macos = 13.0)
            && device.supportsFamily(MTLGPUFamily::Common3)
        {
            common_families |= 1 << 3;
        }
        let mut mac_families = 0;
        for n in 1..=2 {
            if device.supportsFamily(MTLGPUFamily(2000 + n)) {
                mac_families |= 1 << n;
            }
        }
        let mut sample_counts = 0;
        for n in [1, 2, 4, 8] {
            if device.supportsTextureSampleCount(n) {
                sample_counts |= 1 << n;
            }
        }
        let capabilities = Capabilities {
            apple_families,
            common_families,
            mac_families,
            max_buffer_bytes: device.maxBufferLength(),
            sample_counts,
            unified_memory: device
                .respondsToSelector(sel!(hasUnifiedMemory))
                .then(|| device.hasUnifiedMemory()),
            recommended_working_set_bytes: device
                .respondsToSelector(sel!(recommendedMaxWorkingSetSize))
                .then(|| device.recommendedMaxWorkingSetSize())
                .filter(|&n| n != 0),
            allocated_bytes_at_creation: device
                .respondsToSelector(sel!(currentAllocatedSize))
                .then(|| device.currentAllocatedSize()),
            argument_buffer_tier: device
                .respondsToSelector(sel!(argumentBuffersSupport))
                .then(|| device.argumentBuffersSupport().0),
        };
        if !capabilities.baseline() {
            return Err("no verified Common1/Apple4 baseline".into());
        }
        eprintln!("helium-terrain: actual Metal capabilities: {capabilities:?}");
        let queue = device
            .newCommandQueue()
            .ok_or("command queue allocation failed")?;
        let library = device
            .newLibraryWithSource_options_error(
                &NSString::from_str(include_str!("terrain24.metal")),
                None,
            )
            .map_err(|e| format!("terrain MSL compilation failed: {e}"))?;
        let vertex = library
            .newFunctionWithName(&NSString::from_str("terrain24_vertex"))
            .ok_or("missing terrain vertex function")?;
        let fragment = library
            .newFunctionWithName(&NSString::from_str("terrain24_fragment"))
            .ok_or("missing terrain fragment function")?;
        let desc = MTLRenderPipelineDescriptor::new();
        desc.setVertexFunction(Some(&vertex));
        desc.setFragmentFunction(Some(&fragment));
        desc.setDepthAttachmentPixelFormat(MTLPixelFormat::Depth32Float);
        // SAFETY: color attachment zero is valid on every supported Metal device.
        unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) }
            .setPixelFormat(MTLPixelFormat::RGBA8Unorm);
        let pipeline = device
            .newRenderPipelineStateWithDescriptor_error(&desc)
            .map_err(|e| format!("terrain pipeline failed: {e}"))?;
        let depth = |compare| {
            let desc = MTLDepthStencilDescriptor::new();
            desc.setDepthCompareFunction(compare);
            desc.setDepthWriteEnabled(true);
            device
                .newDepthStencilStateWithDescriptor(&desc)
                .ok_or_else(|| "depth state allocation failed".to_string())
        };
        let reversed = depth(MTLCompareFunction::GreaterEqual)?;
        let forward = depth(MTLCompareFunction::LessEqual)?;
        Ok(Self {
            device,
            queue,
            pipeline,
            reversed,
            forward,
            capabilities,
        })
    }

    pub fn render(
        &self,
        mesh: &Mesh<'_>,
        atlas: Atlas<'_>,
        lightmap: &[u8],
        view: View,
        options: Options,
    ) -> Result<Image, String> {
        let layout = validate(mesh, atlas, lightmap, view, options, &self.capabilities)?;
        autoreleasepool(|_| self.draw(mesh, atlas, lightmap, view, options, layout))
    }
    fn draw(
        &self,
        mesh: &Mesh<'_>,
        atlas: Atlas<'_>,
        lightmap: &[u8],
        view: View,
        options: Options,
        layout: ResourceLayout,
    ) -> Result<Image, String> {
        let vertices = upload(&self.device, mesh.vertices)?;
        // SAFETY: a borrowed u32 slice has no padding; allocation size is checked
        // in validate, and all indices were checked against the vertex byte stream.
        let index_bytes =
            unsafe { std::slice::from_raw_parts(mesh.indices.as_ptr().cast(), layout.index_bytes) };
        let indices = upload(&self.device, index_bytes)?;
        let mut uniforms = [0f32; 20];
        for (to, from) in uniforms[..16]
            .iter_mut()
            .zip(view.clip_from_relative.iter().flatten())
        {
            *to = *from;
        }
        uniforms[16..19].copy_from_slice(&view.origin);
        uniforms[19] = UV_STEP;
        // SAFETY: fully initialized 80 bytes, no padding, Metal buffer starts at offset zero.
        let bytes = unsafe { std::slice::from_raw_parts(uniforms.as_ptr().cast(), 80) };
        let uniforms = upload(&self.device, bytes)?;
        let sprites = texture(
            &self.device,
            atlas.width,
            atlas.height,
            atlas.layers,
            true,
            MTLPixelFormat::RGBA8Unorm,
            MTLTextureUsage::ShaderRead,
        )?;
        let lm = texture(
            &self.device,
            16,
            16,
            1,
            false,
            MTLPixelFormat::RGBA8Unorm,
            MTLTextureUsage::ShaderRead,
        )?;
        let color = texture(
            &self.device,
            options.width,
            options.height,
            1,
            false,
            MTLPixelFormat::RGBA8Unorm,
            MTLTextureUsage::RenderTarget,
        )?;
        let depth = texture(
            &self.device,
            options.width,
            options.height,
            1,
            false,
            MTLPixelFormat::Depth32Float,
            MTLTextureUsage::RenderTarget,
        )?;
        // All padding initialized; shared staging works on unified AND discrete GPUs.
        let mut pixels = vec![0u8; layout.staging_bytes];
        for layer in 0..atlas.layers {
            for row in 0..atlas.height {
                let source = (layer * atlas.height + row) * atlas.width * 4;
                let target = layer * layout.atlas_slice + row * layout.atlas_row;
                pixels[target..target + atlas.width * 4]
                    .copy_from_slice(&atlas.rgba[source..source + atlas.width * 4]);
            }
        }
        for row in 0..16 {
            pixels[layout.atlas_staging + row * 256..layout.atlas_staging + row * 256 + 64]
                .copy_from_slice(&lightmap[row * 64..row * 64 + 64]);
        }
        let staging = upload(&self.device, &pixels)?;
        let readback = self
            .device
            .newBufferWithLength_options(
                layout.readback_bytes,
                MTLResourceOptions::StorageModeShared,
            )
            .ok_or("readback allocation failed")?;
        let command = self
            .queue
            .commandBuffer()
            .ok_or("command buffer allocation failed")?;
        let blit = command
            .blitCommandEncoder()
            .ok_or("upload encoder allocation failed")?;
        for layer in 0..atlas.layers {
            // SAFETY: validated complete slices, aligned offsets/rows, same-device resources.
            unsafe {
                blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                &staging, layer * layout.atlas_slice, layout.atlas_row, layout.atlas_slice,
                MTLSize { width: atlas.width, height: atlas.height, depth: 1 }, &sprites, layer, 0, MTLOrigin { x:0,y:0,z:0 });
            }
        }
        // SAFETY: final 4096-byte staging region has 16 aligned initialized rows.
        unsafe {
            blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
            &staging, layout.atlas_staging, 256, 4096, MTLSize { width:16,height:16,depth:1 }, &lm, 0, 0, MTLOrigin { x:0,y:0,z:0 });
        }
        blit.endEncoding();
        let pass = MTLRenderPassDescriptor::new();
        // SAFETY: supported color attachment zero.
        let a = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        a.setTexture(Some(&color));
        a.setLoadAction(MTLLoadAction::Clear);
        a.setStoreAction(MTLStoreAction::Store);
        a.setClearColor(MTLClearColor {
            red: 0.,
            green: 0.,
            blue: 0.,
            alpha: 1.,
        });
        let d = pass.depthAttachment();
        d.setTexture(Some(&depth));
        d.setLoadAction(MTLLoadAction::Clear);
        d.setStoreAction(MTLStoreAction::DontCare);
        d.setClearDepth(if options.depth == Depth::Reversed {
            0.
        } else {
            1.
        });
        let encoder = command
            .renderCommandEncoderWithDescriptor(&pass)
            .ok_or("render encoder allocation failed")?;
        encoder.setRenderPipelineState(&self.pipeline);
        encoder.setDepthStencilState(Some(if options.depth == Depth::Reversed {
            &self.reversed
        } else {
            &self.forward
        }));
        encoder.setCullMode(MTLCullMode::None);
        encoder.setViewport(MTLViewport {
            originX: 0.,
            originY: 0.,
            width: options.width as f64,
            height: options.height as f64,
            znear: 0.,
            zfar: 1.,
        });
        // SAFETY: every indexed vertex, flat layer, texture and uniform range was
        // validated. Upload blit precedes reads; all resources remain retained.
        unsafe {
            encoder.setVertexBuffer_offset_atIndex(Some(&vertices), 0, 0);
            encoder.setVertexBuffer_offset_atIndex(Some(&uniforms), 0, 1);
            encoder.setVertexTexture_atIndex(Some(&lm), 0);
            encoder.setFragmentTexture_atIndex(Some(&sprites), 0);
            encoder.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset(
                MTLPrimitiveType::Triangle,
                mesh.indices.len(),
                MTLIndexType::UInt32,
                &indices,
                0,
            );
        }
        encoder.endEncoding();
        let blit = command
            .blitCommandEncoder()
            .ok_or("readback encoder allocation failed")?;
        // SAFETY: full stored color image fits the checked readback allocation.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
            &color, 0, 0, MTLOrigin{x:0,y:0,z:0}, MTLSize{width:options.width,height:options.height,depth:1}, &readback, 0, layout.readback_row, layout.readback_bytes);
        }
        blit.endEncoding();
        command.commit();
        command.waitUntilCompleted();
        if command.status() != MTLCommandBufferStatus::Completed {
            return Err(format!(
                "terrain command failed: {:?}, {:?}",
                command.status(),
                command.error()
            ));
        }
        // SAFETY: Shared storage is CPU-visible after completion on both memory
        // architectures. Only initialized pixel prefixes are read, never row
        // padding. All ranges were checked; owners/pool remain live during copy.
        let rgba = unsafe {
            copy_readback(
                readback.contents().as_ptr().cast(),
                options.width,
                options.height,
                layout.readback_row,
            )
        };
        let start = command.GPUStartTime();
        let end = command.GPUEndTime();
        Ok(Image {
            rgba,
            gpu_seconds: (start > 0. && end > start && end.is_finite()).then_some(end - start),
        })
    }
}

fn upload(device: &Device, bytes: &[u8]) -> Result<Buffer, String> {
    let pointer = NonNull::new(bytes.as_ptr().cast_mut().cast::<c_void>()).ok_or("null upload")?;
    // SAFETY: copies exactly this initialized live borrow before returning.
    unsafe {
        device.newBufferWithBytes_length_options(
            pointer,
            bytes.len(),
            MTLResourceOptions::StorageModeShared,
        )
    }
    .ok_or("buffer allocation failed".into())
}
fn texture(
    device: &Device,
    width: usize,
    height: usize,
    layers: usize,
    array: bool,
    format: MTLPixelFormat,
    usage: MTLTextureUsage,
) -> Result<Texture, String> {
    let desc = MTLTextureDescriptor::new();
    desc.setTextureType(if array {
        MTLTextureType::Type2DArray
    } else {
        MTLTextureType::Type2D
    });
    desc.setPixelFormat(format);
    desc.setStorageMode(MTLStorageMode::Private);
    desc.setUsage(usage);
    // SAFETY: nonzero, policy-bounded dimensions/layers validated before allocation.
    unsafe {
        desc.setWidth(width);
        desc.setHeight(height);
        desc.setArrayLength(layers);
    }
    device
        .newTextureWithDescriptor(&desc)
        .ok_or("texture allocation failed".into())
}
