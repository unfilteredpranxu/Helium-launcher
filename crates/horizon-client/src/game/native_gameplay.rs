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
            )
            .map_err(|error| format!("direct Metal gameplay frame: {error}").into())
    }
}
