//! Native compositor for the production CPU HUD. Lives on the Metal worker.
//! MIT; Copyright (c) 2026 Horizon contributors.
use super::*;
use helium_render_scene::hud::{Blend, Frame, VERTEX_BYTES};

pub(super) struct Hud {
    atlas: Texture,
    revision: u64,
    serial: u64,
    vertices: Option<Buffer>,
    pipelines: [Retained<ProtocolObject<dyn MTLRenderPipelineState>>; 5],
    depth: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
}

impl Hud {
    pub(super) fn new(device: &Device, frame: &Frame) -> Result<Self, String> {
        let library = device
            .newLibraryWithSource_options_error(
                &NSString::from_str(include_str!("hud.metal")),
                None,
            )
            .map_err(|err| format!("native HUD shader: {err}"))?;
        let function = |name| {
            library
                .newFunctionWithName(&NSString::from_str(name))
                .ok_or_else(|| format!("missing native HUD function {name}"))
        };
        let vertex = function("hud_vertex")?;
        let normal = function("hud_fragment")?;
        let vignette = function("hud_vignette")?;
        use MTLBlendFactor as B;
        let pipeline = |depth, src, dst, fragment: &_| {
            let descriptor = MTLRenderPipelineDescriptor::new();
            descriptor.setVertexFunction(Some(&vertex));
            descriptor.setFragmentFunction(Some(fragment));
            if depth {
                descriptor.setDepthAttachmentPixelFormat(MTLPixelFormat::Depth32Float);
            }
            // SAFETY: the single color target is always attachment zero.
            let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
            color.setPixelFormat(MTLPixelFormat::RGBA8Unorm);
            color.setBlendingEnabled(true);
            color.setSourceRGBBlendFactor(src);
            color.setDestinationRGBBlendFactor(dst);
            color.setSourceAlphaBlendFactor(B::One);
            color.setDestinationAlphaBlendFactor(B::OneMinusSourceAlpha);
            device
                .newRenderPipelineStateWithDescriptor_error(&descriptor)
                .map_err(|err| format!("native HUD pipeline: {err}"))
        };
        let pipelines = [
            pipeline(false, B::SourceAlpha, B::OneMinusSourceAlpha, &normal)?,
            pipeline(
                false,
                B::OneMinusDestinationColor,
                B::OneMinusSourceColor,
                &normal,
            )?,
            pipeline(false, B::One, B::One, &normal)?,
            pipeline(true, B::SourceAlpha, B::OneMinusSourceAlpha, &normal)?,
            pipeline(false, B::Zero, B::OneMinusSourceColor, &vignette)?,
        ];
        let depth = MTLDepthStencilDescriptor::new();
        depth.setDepthWriteEnabled(true);
        depth.setDepthCompareFunction(MTLCompareFunction::LessEqual);
        let depth = device
            .newDepthStencilStateWithDescriptor(&depth)
            .ok_or("native HUD depth state unavailable")?;
        let [w, h] = frame.atlas().size();
        let descriptor = MTLTextureDescriptor::new();
        descriptor.setPixelFormat(MTLPixelFormat::RGBA8Unorm);
        descriptor.setStorageMode(MTLStorageMode::Shared);
        descriptor.setUsage(MTLTextureUsage::ShaderRead);
        // SAFETY: Atlas checked the nonzero dimensions and the byte budget.
        unsafe {
            descriptor.setWidth(w as usize);
            descriptor.setHeight(h as usize);
        }
        let atlas = device
            .newTextureWithDescriptor(&descriptor)
            .ok_or("native HUD atlas unavailable")?;
        Self::replace(&atlas, [0, 0, w, h], frame.atlas().pixels());
        Ok(Self {
            atlas,
            revision: frame.atlas().revision(),
            serial: 0,
            vertices: None,
            pipelines,
            depth,
        })
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn vertex_capacity(frame: &Frame) -> usize {
        std::mem::size_of_val(frame.vertices()).next_power_of_two()
    }

    pub(super) fn budget(existing: Option<&Self>, frame: &Frame) -> usize {
        let cached = existing
            .and_then(|hud| hud.vertices.as_ref())
            .map_or(0, |b| b.length());
        frame.atlas().pixels().len() + Self::vertex_capacity(frame).max(cached)
    }

    fn replace(texture: &Texture, rect: [u32; 4], rgba: &[u8]) {
        let [x, y, w, h] = rect.map(|n| n as usize);
        let area = MTLRegion {
            origin: MTLOrigin { x, y, z: 0 },
            size: MTLSize {
                width: w,
                height: h,
                depth: 1,
            },
        };
        // SAFETY: validated atlas rectangles and exact RGBA slices. Metal
        // copies the data during the call. No pending command samples it.
        unsafe {
            texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                area,
                0,
                NonNull::from(rgba).cast(),
                w * 4,
            );
        }
    }

    pub(super) fn encode(
        &mut self,
        device: &Device,
        command: &ProtocolObject<dyn MTLCommandBuffer>,
        color_texture: &Texture,
        retired_terrain_depth: &Texture,
        frame: &Frame,
    ) -> Result<usize, String> {
        if frame.vertices().is_empty() {
            return Ok(0);
        }
        let mut dirty_bytes = 0;
        let mut serial = self.serial;
        for tile in frame.dirty_tiles(serial) {
            Self::replace(&self.atlas, tile.rect, &tile.rgba);
            dirty_bytes += tile.rgba.len();
            serial = serial.max(tile.serial);
        }
        self.serial = serial;

        let bytes = frame.vertex_bytes();
        let capacity = Self::vertex_capacity(frame);
        if capacity > VERTEX_BYTES {
            return Err("native HUD vertex allocation exceeds budget".into());
        }
        if self.vertices.as_ref().is_none_or(|b| b.length() < capacity) {
            self.vertices = Some(
                device
                    .newBufferWithLength_options(capacity, MTLResourceOptions::StorageModeShared)
                    .ok_or("native HUD vertex buffer unavailable")?,
            );
        }
        let buffer = self.vertices.as_ref().unwrap();
        // SAFETY: shared initialized buffer range, exclusively writable between
        // commands. The caller checked terminal GPU completion before encoding.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                buffer.contents().as_ptr().cast(),
                bytes.len(),
            )
        };
        let [w, h] = frame.size();
        let [left, top, width, height] = frame.viewport().safe_rect();
        let transform = [
            2.0 * frame.scale() / w as f32,
            -2.0 * frame.scale() / h as f32,
            2.0 * left as f32 / w as f32 - 1.0,
            1.0 - 2.0 * top as f32 / h as f32,
        ];
        let mut remaining = frame.runs();
        while let Some((_, first)) = remaining.first() {
            let clip = match first {
                Blend::Depth(rect) => Some(*rect),
                _ => None,
            };
            let n = if clip.is_some() {
                1
            } else {
                remaining
                    .iter()
                    .position(|(_, b)| matches!(b, Blend::Depth(_)))
                    .unwrap_or(remaining.len())
            };
            let (runs, rest) = remaining.split_at(n);
            remaining = rest;
            let [x, y, sw, sh] = clip.unwrap_or([0, 0, width, height]);
            let (x, y) = (x.min(width), y.min(height));
            let scissor = MTLScissorRect {
                x: (x + left) as usize,
                y: (y + top) as usize,
                width: sw.min(width - x) as usize,
                height: sh.min(height - y) as usize,
            };
            if scissor.width == 0 || scissor.height == 0 {
                continue;
            }
            let pass = MTLRenderPassDescriptor::new();
            // SAFETY: the same-size, same-device color target is attachment zero.
            let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
            color.setTexture(Some(color_texture));
            color.setLoadAction(MTLLoadAction::Load);
            color.setStoreAction(MTLStoreAction::Store);
            if clip.is_some() {
                // Terrain has finished with its depth attachment (store = DontCare).
                // Each UI model run starts with independently cleared depth.
                let depth = pass.depthAttachment();
                depth.setTexture(Some(retired_terrain_depth));
                depth.setLoadAction(MTLLoadAction::Clear);
                depth.setStoreAction(MTLStoreAction::DontCare);
                depth.setClearDepth(1.0);
            }
            let encoder = command
                .renderCommandEncoderWithDescriptor(&pass)
                .ok_or("native HUD encoder unavailable")?;
            encoder.setScissorRect(scissor);
            if clip.is_some() {
                encoder.setDepthStencilState(Some(&self.depth));
            }
            // SAFETY: validated contiguous triangle ranges and an immutable
            // texture and vertex buffer through command completion.
            unsafe {
                encoder.setVertexBuffer_offset_atIndex(Some(buffer), 0, 0);
                encoder.setVertexBytes_length_atIndex(
                    NonNull::from(&transform).cast(),
                    size_of_val(&transform),
                    1,
                );
                encoder.setFragmentTexture_atIndex(Some(&self.atlas), 0);
                for (range, blend) in runs {
                    let index = match blend {
                        Blend::Alpha => 0,
                        Blend::Invert => 1,
                        Blend::Additive => 2,
                        Blend::Depth(_) => 3,
                        Blend::Vignette => 4,
                        Blend::Skip => continue,
                    };
                    encoder.setRenderPipelineState(&self.pipelines[index]);
                    encoder.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        range.start as usize,
                        range.len(),
                    );
                }
            }
            encoder.endEncoding();
        }
        Ok(dirty_bytes)
    }
}
