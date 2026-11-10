//! Only macOS/iOS compile this file. No surface, display link, or gameplay ownership.
use crate::{Image, Mesh, SIDE};
use objc2::{
    rc::{Retained, autoreleasepool},
    runtime::ProtocolObject,
};
use objc2_foundation::NSString;
use objc2_metal::*;
use std::{ffi::c_void, ptr::NonNull};

fn pointer(bytes: &[u8]) -> NonNull<c_void> {
    NonNull::new(bytes.as_ptr().cast_mut().cast()).expect("nonempty upload")
}

fn upload(
    device: &ProtocolObject<dyn MTLDevice>,
    bytes: &[u8],
) -> Result<Retained<ProtocolObject<dyn MTLBuffer>>, String> {
    // SAFETY: Metal copies exactly bytes.len() initialized bytes before returning;
    // no no-copy buffer or borrowed CPU storage escapes this call.
    unsafe {
        device.newBufferWithBytes_length_options(
            pointer(bytes),
            bytes.len(),
            MTLResourceOptions::StorageModeShared,
        )
    }
    .ok_or_else(|| "Metal buffer allocation failed".into())
}

fn target(
    device: &ProtocolObject<dyn MTLDevice>,
    format: MTLPixelFormat,
) -> Result<Retained<ProtocolObject<dyn MTLTexture>>, String> {
    let desc = MTLTextureDescriptor::new();
    desc.setTextureType(MTLTextureType::Type2D);
    desc.setPixelFormat(format);
    // SAFETY: fixed 256-square, single-sample 2D targets, within baseline Metal limits.
    unsafe {
        desc.setWidth(SIDE);
        desc.setHeight(SIDE);
    }
    desc.setStorageMode(MTLStorageMode::Private);
    desc.setUsage(MTLTextureUsage::RenderTarget);
    device
        .newTextureWithDescriptor(&desc)
        .ok_or_else(|| "Metal target allocation failed".into())
}

pub(super) fn render(mesh: &Mesh, camera: [[f32; 4]; 4]) -> Result<Image, String> {
    autoreleasepool(|_| render_in_pool(mesh, camera))
}

fn render_in_pool(mesh: &Mesh, camera: [[f32; 4]; 4]) -> Result<Image, String> {
    let device = MTLCreateSystemDefaultDevice().ok_or("no Metal device")?;
    let queue = device
        .newCommandQueue()
        .ok_or("Metal queue allocation failed")?;
    eprintln!("metal-prototype: device and command queue acquired");
    // Explicit serialization keeps the Rust/MSL ABI independent of Rust struct padding.
    let mut vertices = Vec::with_capacity(mesh.vertices.len() * 16);
    for v in &mesh.vertices {
        for p in v.position {
            vertices.extend(p.to_ne_bytes());
        }
        for uv in v.uv {
            vertices.extend(uv.to_ne_bytes());
        }
        vertices.extend(v.tint);
    }
    let indices: Vec<u8> = mesh.indices.iter().flat_map(|i| i.to_ne_bytes()).collect();
    let uniforms: Vec<u8> = camera
        .iter()
        .flatten()
        .flat_map(|f| f.to_ne_bytes())
        .collect();
    let vertex_buffer = upload(&device, &vertices)?;
    let index_buffer = upload(&device, &indices)?;
    let camera_buffer = upload(&device, &uniforms)?;
    let color = target(&device, MTLPixelFormat::RGBA8Unorm)?;
    let depth = target(&device, MTLPixelFormat::Depth32Float)?;

    // Generated RGBA8 checker texels; no Minecraft asset files are read or distributed.
    let texels = [
        255, 255, 255, 255, 170, 170, 170, 255, 170, 170, 170, 255, 255, 255, 255, 255,
    ];
    let desc = MTLTextureDescriptor::new();
    desc.setTextureType(MTLTextureType::Type2D);
    desc.setPixelFormat(MTLPixelFormat::RGBA8Unorm);
    // SAFETY: a 2x2 RGBA8 texture is within baseline Metal limits.
    unsafe {
        desc.setWidth(2);
        desc.setHeight(2);
    }
    desc.setStorageMode(MTLStorageMode::Shared);
    desc.setUsage(MTLTextureUsage::ShaderRead);
    let albedo = device
        .newTextureWithDescriptor(&desc)
        .ok_or("Metal albedo allocation failed")?;
    // SAFETY: 2 rows of 2 RGBA8 texels, copied before any GPU use, exactly 16 bytes.
    unsafe {
        albedo.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
            MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: 2,
                    height: 2,
                    depth: 1,
                },
            },
            0,
            pointer(&texels),
            8,
        );
    }
    let library = device
        .newLibraryWithSource_options_error(
            &NSString::from_str(include_str!("terrain.metal")),
            None,
        )
        .map_err(|e| format!("MSL compilation failed: {e}"))?;
    let vertex = library
        .newFunctionWithName(&NSString::from_str("terrain_vertex"))
        .ok_or("missing vertex function")?;
    let fragment = library
        .newFunctionWithName(&NSString::from_str("terrain_fragment"))
        .ok_or("missing fragment function")?;
    let pipeline_desc = MTLRenderPipelineDescriptor::new();
    pipeline_desc.setVertexFunction(Some(&vertex));
    pipeline_desc.setFragmentFunction(Some(&fragment));
    pipeline_desc.setDepthAttachmentPixelFormat(MTLPixelFormat::Depth32Float);
    // SAFETY: attachment 0 exists (Metal supports at least one color attachment).
    unsafe { pipeline_desc.colorAttachments().objectAtIndexedSubscript(0) }
        .setPixelFormat(MTLPixelFormat::RGBA8Unorm);
    let pipeline = device
        .newRenderPipelineStateWithDescriptor_error(&pipeline_desc)
        .map_err(|e| format!("Metal pipeline creation failed: {e}"))?;
    let depth_desc = MTLDepthStencilDescriptor::new();
    depth_desc.setDepthCompareFunction(MTLCompareFunction::Less);
    depth_desc.setDepthWriteEnabled(true);
    let depth_state = device
        .newDepthStencilStateWithDescriptor(&depth_desc)
        .ok_or("Metal depth state allocation failed")?;
    eprintln!("metal-prototype: indexed terrain pipeline compiled");

    let pass = MTLRenderPassDescriptor::new();
    // SAFETY: valid color attachment index, retained descriptor and textures.
    let attachment = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
    attachment.setTexture(Some(&color));
    attachment.setLoadAction(MTLLoadAction::Clear);
    attachment.setStoreAction(MTLStoreAction::Store);
    attachment.setClearColor(MTLClearColor {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        alpha: 1.0,
    });
    let depth_attachment = pass.depthAttachment();
    depth_attachment.setTexture(Some(&depth));
    depth_attachment.setLoadAction(MTLLoadAction::Clear);
    depth_attachment.setStoreAction(MTLStoreAction::DontCare);
    depth_attachment.setClearDepth(1.0);
    let byte_count = SIDE * SIDE * 4; // 1024-byte rows satisfy Metal's 256-byte blit alignment.
    let readback = device
        .newBufferWithLength_options(byte_count, MTLResourceOptions::StorageModeShared)
        .ok_or("Metal readback allocation failed")?;
    // Normal commandBuffer retains its encoded resources; never use unretainedReferences.
    let command = queue
        .commandBuffer()
        .ok_or("Metal command buffer allocation failed")?;
    let encoder = command
        .renderCommandEncoderWithDescriptor(&pass)
        .ok_or("Metal render encoder allocation failed")?;
    encoder.setRenderPipelineState(&pipeline);
    encoder.setDepthStencilState(Some(&depth_state));
    encoder.setCullMode(MTLCullMode::None);
    encoder.setViewport(MTLViewport {
        originX: 0.0,
        originY: 0.0,
        width: SIDE as f64,
        height: SIDE as f64,
        znear: 0.0,
        zfar: 1.0,
    });
    // SAFETY: all buffers/textures belong to this device and remain retained through completion.
    // Mesh::validate checked every uint32 index; serialized vertices are exactly 16 bytes each,
    // and buffer(1) contains exactly one 64-byte column-major matrix. All offsets are zero.
    unsafe {
        encoder.setVertexBuffer_offset_atIndex(Some(&vertex_buffer), 0, 0);
        encoder.setVertexBuffer_offset_atIndex(Some(&camera_buffer), 0, 1);
        encoder.setFragmentTexture_atIndex(Some(&albedo), 0);
        encoder.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset(
            MTLPrimitiveType::Triangle,
            mesh.indices.len(),
            MTLIndexType::UInt32,
            &index_buffer,
            0,
        );
    }
    encoder.endEncoding();
    let blit = command
        .blitCommandEncoder()
        .ok_or("Metal blit encoder allocation failed")?;
    // SAFETY: source rectangle fits the 256-square color target. Destination holds exactly
    // SIDE rows of SIDE*4 bytes. The blit follows rendering in this command buffer.
    unsafe {
        blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
            &color, 0, 0, MTLOrigin {x:0,y:0,z:0}, MTLSize {width:SIDE,height:SIDE,depth:1},
            &readback, 0, SIDE * 4, byte_count);
    }
    blit.endEncoding();
    command.commit();
    eprintln!("metal-prototype: command buffer submitted; waiting offscreen");
    // This intentionally blocking probe must never be invoked from the production frame loop.
    // No early return after commit: local Retained owners stay alive until the GPU is finished.
    command.waitUntilCompleted();
    if command.status() != MTLCommandBufferStatus::Completed {
        return Err(format!(
            "Metal command failed: status={:?}, error={:?}",
            command.status(),
            command.error()
        ));
    }
    let start = command.GPUStartTime();
    let end = command.GPUEndTime();
    let gpu_seconds = (start > 0.0 && end > start && end.is_finite()).then_some(end - start);
    // SAFETY: shared buffer is CPU-accessible and the GPU completed all writes above.
    // Copy owned bytes before releasing the Metal buffer / autorelease pool.
    let rgba = unsafe {
        std::slice::from_raw_parts(readback.contents().as_ptr().cast::<u8>(), byte_count)
    }
    .to_vec();
    eprintln!("metal-prototype: command completed and pixels read back");
    Ok(Image { rgba, gpu_seconds })
}
