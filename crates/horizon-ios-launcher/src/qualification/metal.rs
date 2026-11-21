//! A bounded synthetic scene driven through the same resident renderer as the game.
use helium_metal_prototype::{
    attached::Layer,
    terrain::{LiveScene, LiveSection, LiveSubmission, TerrainRenderer},
    upload::{Budget, Poll, Update},
};
use helium_render_scene::{
    BankDescriptor, BankStorage, Camera, Frame, Lightmap, MaterialSet, MaterialVersion, Mesh,
    OPAQUE_LAYER, Section, SectionVersion, TerrainVertex, View,
};
use horizon_ios_launcher::qualification::{Event, event};
use std::{
    num::NonZeroU64,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct Palette([u8; 16]);

impl BankStorage for Palette {
    fn pixels(&self, bank: usize) -> Option<&[u8]> {
        (bank == 0).then_some(self.0.as_slice())
    }

    fn retained_bytes(&self) -> usize {
        self.0.len()
    }
}

fn fixture(id: u64) -> Result<Frame<Section>, String> {
    let epoch = NonZeroU64::new(id).ok_or("missing diagnostic attachment generation")?;
    let version = MaterialVersion {
        generation: 1,
        revision: 0,
    };
    let materials = MaterialSet::new(
        version,
        1,
        4,
        4,
        &[BankDescriptor {
            used: 4,
            allocated: 4,
            levels: 1,
        }],
        Arc::new(Palette([
            255, 60, 20, 255, 10, 195, 235, 255, 225, 35, 160, 255, 70, 235, 65, 255,
        ])),
        16,
    )?;
    let mut vertices = Vec::with_capacity(16);
    let mut indices = Vec::with_capacity(24);
    for tile in 0..4u16 {
        let x = if tile % 2 == 0 { -0.85 } else { 0.04 };
        let y = if tile < 2 { -0.85 } else { 0.04 };
        let start = u32::from(tile) * 4;
        vertices.extend(
            [[x, y], [x + 0.81, y], [x + 0.81, y + 0.81], [x, y + 0.81]].map(|[x, y]| {
                TerrainVertex {
                    pos: [x, y, 0.5],
                    uv: [0; 2],
                    layer: OPAQUE_LAYER | ((tile + (id & 1) as u16) % 4),
                    light: 0,
                    color: 0xffffff,
                }
            }),
        );
        indices.extend([start, start + 1, start + 2, start, start + 2, start + 3]);
    }
    let bytes = Mesh::from_vertices(&vertices, &indices).vertices.to_vec();
    let section = Section::new(
        SectionVersion {
            epoch,
            generation: 1,
            key: (0, 0, 0),
            revision: 1,
        },
        [0; 3],
        [[-0.85, -0.85, 0.5], [0.85, 0.85, 0.5]],
        0,
        bytes,
        indices,
        4096,
    )?;
    Ok(Frame {
        epoch,
        camera: Camera {
            position: [0.; 3],
            clip_from_relative: View::identity().clip_from_relative,
        },
        lightmap: Lightmap {
            revision: 1,
            rgba: [255; 1024],
        },
        clear_rgb: [0.025, 0.04, 0.1],
        materials: Arc::new(materials),
        sections: vec![section],
    })
}

pub(super) fn render(layer: Layer, cancel: &AtomicBool) -> Result<(), String> {
    match present(&layer, cancel) {
        Err(_) if cancel.load(Ordering::Acquire) => return Ok(()),
        result => result?,
    }
    // The completed frame remains visible until host cancellation. GPU
    // resources from present() are already retired or completion-owned.
    while !cancel.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn present(layer: &Layer, cancel: &AtomicBool) -> Result<(), String> {
    let id = layer.generation();
    let renderer = TerrainRenderer::new()?;
    event(Event::RendererReady, id);
    let display = layer.display(&renderer)?;
    let frame = fixture(id)?;
    let banks = frame.materials.banks();
    let budget = Budget {
        payload_bytes: 1 << 20,
        staging_bytes: 1 << 20,
    };
    let mut materials = renderer.create_material_set(banks, cancel, budget)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    materials.begin_upload(&renderer, banks[0].clone(), cancel, Update::Full, budget)?;
    loop {
        match materials.poll_upload(&renderer, cancel) {
            Poll::Completed(_) => break,
            Poll::Pending => wait(cancel, deadline)?,
            Poll::Failed(error) => return Err(error),
            _ => return Err("missing qualification upload receipt".into()),
        }
    }
    while renderer.upload_retention().busy {
        wait(cancel, deadline)?;
    }
    let mut scene = LiveScene::new(&renderer, display.dimensions())?;
    let section = LiveSection::from_scene(&renderer, &frame.sections[0], &frame, 4096)?;
    let draws = [(&section, frame.camera.section_view([0; 3])?)];
    loop {
        match scene.submit_scene(
            &renderer,
            &materials,
            Some(&display),
            &frame,
            &draws,
            cancel,
        )? {
            LiveSubmission::Submitted => break,
            LiveSubmission::DrawableUnavailable | LiveSubmission::Pending => {
                wait(cancel, deadline)?
            }
        }
    }
    event(Event::Submitted, id);
    loop {
        if let Some(receipt) = scene.poll(Some(&display), cancel)? {
            if receipt.draws == 0 {
                return Err("qualification scene did not draw".into());
            }
            event(Event::PresentCompleted, id);
            layer.reveal_live()?;
            return Ok(());
        }
        wait(cancel, deadline)?;
    }
}

fn wait(cancel: &AtomicBool, deadline: Instant) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
        return Err("qualification cancelled or timed out; terminal ownership retained".into());
    }
    std::thread::sleep(Duration::from_millis(8));
    Ok(())
}
