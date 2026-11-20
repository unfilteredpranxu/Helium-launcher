//! Direct Metal input from the real game, without a reference graphics device.
use super::{Game, PackResources};
use horizon_client::{
    Result,
    environment::Sky,
    gfx::{self, NativeTerrainMaterials},
};

pub(super) fn prepare(pack: &PackResources) -> Result<NativeTerrainMaterials> {
    let assets = &pack.render;
    let materials = gfx::prepare_native_terrain(
        &assets.pixels.get(),
        assets.sprite_size,
        assets.texture_layers,
        &assets.sprite_mips,
        horizon_metal_bridge::runtime_snapshot_budget(),
    )?;
    // Validate the exact production namespace before the game can open a world.
    materials
        .materials()
        .scene_materials(horizon_metal_bridge::runtime_snapshot_budget())?;
    Ok(materials)
}

impl Game<'_> {
    /// Pressure may drop all pinned geometry while the streamer still owns
    /// the chunks. Requeue a bounded neighborhood once the native pause ends.
    pub(super) fn remesh_native_spawn_area(&mut self) {
        let Some(player) = self.player.as_ref() else {
            return;
        };
        if !self
            .direct_metal
            .as_mut()
            .is_some_and(|native| native.native_restream_requested())
        {
            return;
        }
        let (cx, cz) = self.streaming.center;
        let layout = self.world.layout();
        let cy = ((player.position.y.floor() as i32 - layout.min_y()) >> 4)
            .clamp(0, layout.height() / 16 - 1);
        for x in (cx - 2)..=(cx + 2) {
            for z in (cz - 2)..=(cz + 2) {
                if self.world.chunks.contains_key(&(x, z)) {
                    for y in (cy - 2).max(0)..=(cy + 2).min(layout.height() / 16 - 1) {
                        self.world.dirty_sections.insert((x, y, z));
                    }
                }
            }
        }
    }

    pub(super) fn render_native_terrain(
        &mut self,
        view: Option<(glam::DVec3, f32, f32)>,
        sky: &Sky,
        size: [u32; 2],
        alpha: f32,
        camera_alpha: f32,
    ) -> Result<()> {
        if horizon_ios_launcher::covered() || self.shows_panorama() {
            if let Some(native) = &mut self.direct_metal {
                native.invalidate("launcher covered");
            }
            return Ok(());
        }
        let Some((camera, yaw, pitch)) = view else {
            return Ok(());
        };
        if size.contains(&0) {
            return Ok(());
        }
        let clip = gfx::terrain_clip(
            yaw,
            pitch,
            self.fov_degrees(camera_alpha, camera.to_array()),
            size,
            horizon_client::camera::depth_far(self.terrain_distance(), self.settings.cloud_range),
            self.bob_matrix(camera_alpha) * self.spinning_matrix(camera_alpha),
        );
        let (gamma_darken, darkness) = self.darkness_lighting(alpha);
        let lightmap = gfx::terrain_lightmap(
            sky,
            darkness,
            (self.settings.gamma - gamma_darken).max(0.0),
            self.night_vision_intensity(alpha),
            self.fog.block_light_flicker,
            self.boss_overlay.darken_amount(alpha),
            self.settings.extras.fullbright,
        );
        let clear_rgb = if self.settings.horizon.sky {
            sky.sky_color
        } else {
            sky.fog_color
        };
        let materials = self
            .native_terrain
            .as_mut()
            .ok_or("missing direct Metal terrain data")?;
        materials.set_lightmap(lightmap);
        let native = self
            .direct_metal
            .as_mut()
            .ok_or("missing direct Metal controller")?;
        native.set_window(self.window.clone());
        native
            .native_frame(
                materials,
                horizon_metal_bridge::Camera {
                    position: camera.to_array(),
                    clip_from_relative: clip,
                },
                size,
                clear_rgb.map(|c| c.clamp(0.0, 1.0)),
            )
            .map_err(|error| format!("direct Metal gameplay frame: {error}").into())
    }
}
