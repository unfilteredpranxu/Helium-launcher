//! No-world executable for qualifying the owned CPU scene and S2B Metal path.
//! MIT; Copyright (c) 2026 Horizon contributors.

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
fn main() -> std::process::ExitCode {
    eprintln!("Native scene qualification requires Apple Metal.");
    std::process::ExitCode::FAILURE
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn main() -> std::process::ExitCode {
    // CAMetalLayer acquisition stays on an owned worker.
    match std::thread::spawn(apple::render).join() {
        Ok(Ok(())) => std::process::ExitCode::SUCCESS,
        Ok(Err(error)) => {
            eprintln!("Native scene qualification failed: {error}");
            std::process::ExitCode::FAILURE
        }
        Err(_) => {
            eprintln!("Native scene worker panicked.");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple {
    use helium_metal_prototype::{
        terrain::{LiveScene, LiveSection, LiveSubmission, TerrainRenderer},
        upload::{Budget, Poll, Update},
    };
    use helium_render_scene::{
        BankDescriptor, BankStorage, Camera, Frame, Lightmap, MaterialSet, MaterialVersion, Mesh,
        OPAQUE_LAYER, Section, SectionVersion, TerrainVertex, View,
    };
    use std::{
        num::NonZeroU64,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };

    struct Pixels([[u8; 8]; 1]);
    impl BankStorage for Pixels {
        fn pixels(&self, bank: usize) -> Option<&[u8]> {
            self.0.get(bank).map(|bytes| bytes.as_slice())
        }
        fn retained_bytes(&self) -> usize {
            size_of_val(&self.0)
        }
    }

    fn section(version: SectionVersion, layer: u16) -> Result<Section, String> {
        let vertices = [
            [-0.7, -0.7, 0.5],
            [0.7, -0.7, 0.5],
            [0.7, 0.7, 0.5],
            [-0.7, 0.7, 0.5],
        ]
        .map(|pos| TerrainVertex {
            pos,
            uv: [0, 0],
            layer: OPAQUE_LAYER | layer,
            light: 0,
            color: 0xffffff,
        });
        let indices = vec![0, 1, 2, 0, 2, 3];
        let bytes = Mesh::from_vertices(&vertices, &indices).vertices.to_vec();
        Section::new(
            version,
            [0; 3],
            [[-0.7, -0.7, 0.5], [0.7, 0.7, 0.5]],
            0,
            bytes,
            indices,
            4096,
        )
    }

    fn until<T>(mut poll: impl FnMut() -> Result<Option<T>, String>) -> Result<T, String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = poll()? {
                return Ok(result);
            }
            if Instant::now() >= deadline {
                return Err("timed out waiting for native Metal ownership to retire".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn render() -> Result<(), String> {
        let epoch = NonZeroU64::MIN;
        let version = MaterialVersion {
            generation: 1,
            revision: 0,
        };
        let section_version = SectionVersion {
            epoch,
            generation: version.generation,
            key: (0, 0, 0),
            revision: 1,
        };
        let second = SectionVersion {
            key: (1, 0, 0),
            ..section_version
        };
        let materials = MaterialSet::new(
            version,
            1,
            2,
            2,
            &[BankDescriptor {
                used: 2,
                allocated: 2,
                levels: 1,
            }],
            Arc::new(Pixels([[255, 16, 24, 255, 16, 180, 240, 255]])),
            8,
        )?;
        let mut frame = Frame {
            epoch,
            camera: Camera {
                position: [0.; 3],
                clip_from_relative: View::identity().clip_from_relative,
            },
            lightmap: Lightmap {
                revision: 1,
                rgba: [255; 1024],
            },
            materials: Arc::new(materials),
            sections: vec![section(section_version, 0)?, section(second, 1)?],
        };
        let budget = Budget {
            payload_bytes: 1 << 20,
            staging_bytes: 1 << 20,
        };
        let renderer = TerrainRenderer::new()?;
        println!(
            "SOURCE=synthetic-production-format NATIVE_MTLDEVICE_CREATED=1 NATIVE_COMMAND_QUEUE_CREATED=1"
        );
        let display = renderer.detached_diagnostic_display(64, 64)?;
        let banks = frame.materials.banks();
        let cancel = AtomicBool::new(false);
        let mut material = renderer.create_material_set(banks, &cancel, budget)?;
        material.begin_upload(&renderer, banks[0].clone(), &cancel, Update::Full, budget)?;
        until(|| match material.poll_upload(&renderer, &cancel) {
            Poll::Completed(_) => Ok(Some(())),
            Poll::Pending => Ok(None),
            Poll::Failed(err) => Err(err),
            _ => Err("missing material completion receipt".into()),
        })?;
        until(|| Ok((!renderer.upload_retention().busy).then_some(())))?;

        let mut scene = LiveScene::new(&renderer, [64, 64])?;
        let mut resident = frame
            .sections
            .iter()
            .map(|s| LiveSection::from_scene(&renderer, s, &frame, 4096))
            .collect::<Result<Vec<_>, _>>()?;
        let geometry_peak = resident.iter().map(LiveSection::bytes).sum::<usize>();
        let mut uploads = resident.len();
        let mut completed = 0;
        let mut presentations = 0;
        let mut encode_us = Vec::new();
        let mut gpu_us = Vec::new();
        for i in 0..12 {
            frame.camera.position[0] = f64::from(i) / 48.;
            if i == 4 {
                frame.sections[0] = section(
                    SectionVersion {
                        revision: 2,
                        ..section_version
                    },
                    1,
                )?;
                resident[0] = LiveSection::from_scene(&renderer, &frame.sections[0], &frame, 4096)?;
                uploads += 1;
            }
            if i == 8 {
                frame.sections.pop();
                resident.pop();
            }
            let draw = resident
                .iter()
                .zip(&frame.sections)
                .map(|(r, s)| Ok((r, frame.camera.section_view(s.origin)?)))
                .collect::<Result<Vec<_>, String>>()?;
            let target = (i < 4).then_some(&display);
            if !matches!(
                scene.submit_scene(&renderer, &material, target, &frame, &draw, &cancel)?,
                LiveSubmission::Submitted
            ) {
                return Err("expected native frame submission".into());
            }
            let receipt = until(|| scene.poll(target, &cancel))?;
            if receipt.draws != resident.len() as u64 {
                return Err("native draw count does not match section selection".into());
            }
            completed += 1;
            presentations += usize::from(target.is_some());
            encode_us.push(receipt.encode_time.as_micros());
            gpu_us.extend(receipt.gpu_time.map(|v| v.as_micros()));
            until(|| Ok((!renderer.upload_retention().busy).then_some(())))?;
        }
        let draw = [(
            &resident[0],
            frame.camera.section_view(frame.sections[0].origin)?,
        )];
        frame.epoch = NonZeroU64::new(2).unwrap();
        if scene
            .submit_scene(&renderer, &material, None, &frame, &draw, &cancel)
            .is_ok()
        {
            return Err("foreign epoch accepted".into());
        }
        frame.epoch = epoch;
        if !matches!(
            scene.submit_scene(&renderer, &material, None, &frame, &draw, &cancel)?,
            LiveSubmission::Submitted
        ) {
            return Err("expected cancellation-test submission".into());
        }
        cancel.store(true, Ordering::Release);
        drop((scene, resident, material, display));
        until(|| Ok((!renderer.upload_retention().busy).then_some(())))?;
        encode_us.sort_unstable();
        gpu_us.sort_unstable();
        println!(
            "S2C SYNTHETIC NATIVE_FRAME_SUBMITTED={} NATIVE_FRAME_COMPLETED={completed} NATIVE_PRESENT_COMPLETED={presentations} geometry_uploads={uploads} gpu_section_bytes_peak={geometry_peak} encode_us_p50={} gpu_us_p50={:?} GPU_PENDING_AFTER_RETIREMENT=0 PHYSICAL_SCANOUT_UNVERIFIED",
            completed + 1,
            encode_us[encode_us.len() / 2],
            gpu_us.get(gpu_us.len() / 2),
        );
        Ok(())
    }
}
