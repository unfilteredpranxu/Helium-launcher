//! Owned CPU HUD data shared by the game and native renderer.
//! MIT; Copyright (c) 2026 Horizon contributors.
use std::{collections::BTreeMap, ops::Range, sync::Arc};

pub const ATLAS_BYTES: usize = 96 * 1024 * 1024;
pub const VERTEX_BYTES: usize = 4 * 1024 * 1024;
pub const PATCH_BYTES: usize = 12 * 1024 * 1024;
const EDGE: u32 = 64;
const TILE_BYTES: usize = (EDGE * EDGE * 4) as usize;
const MAX_RUNS: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: u32,
    pub depth: f32,
    pub glint_uv: [f32; 2],
    pub text: u32,
}
const _: () = assert!(size_of::<Vertex>() == 36);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Alpha,
    Invert,
    Additive,
    Depth([u32; 4]),
    Vignette,
    Skip,
}

/// The full drawable and the safe rectangle within it, in physical pixels.
#[derive(Clone, Copy)]
pub struct Viewport {
    drawable: [u32; 2],
    safe: [u32; 4],
}

impl Viewport {
    pub fn new(drawable: [u32; 2], [l, t, r, b]: [u32; 4]) -> Self {
        let [w, h] = drawable;
        let safe_size = [
            w.saturating_sub(l.saturating_add(r)),
            h.saturating_sub(t.saturating_add(b)),
        ];
        let safe = if safe_size.contains(&0) {
            [0, 0, w, h]
        } else {
            [l, t, safe_size[0], safe_size[1]]
        };
        Self { drawable, safe }
    }

    pub fn drawable(self) -> [u32; 2] {
        self.drawable
    }
    pub fn safe_rect(self) -> [u32; 4] {
        self.safe
    }
    pub fn size(self) -> [u32; 2] {
        [self.safe[2], self.safe[3]]
    }

    pub fn contains(self, [x, y]: [f64; 2]) -> bool {
        let [left, top, w, h] = self.safe;
        (f64::from(left)..f64::from(left + w)).contains(&x)
            && (f64::from(top)..f64::from(top + h)).contains(&y)
    }

    pub fn to_gui(self, [x, y]: [f64; 2], scale: f32) -> (f32, f32) {
        (
            ((x - f64::from(self.safe[0])) / f64::from(scale)) as f32,
            ((y - f64::from(self.safe[1])) / f64::from(scale)) as f32,
        )
    }

    pub fn from_gui(self, (x, y): (f32, f32), scale: f32) -> (f64, f64) {
        (
            f64::from(x * scale) + f64::from(self.safe[0]),
            f64::from(y * scale) + f64::from(self.safe[1]),
        )
    }
}

pub struct Atlas {
    revision: u64,
    size: [u32; 2],
    pixels: Arc<Vec<u8>>,
}

impl Atlas {
    pub fn new(revision: u64, size: [u32; 2], pixels: Arc<Vec<u8>>) -> Result<Self, &'static str> {
        let [w, h] = size;
        if revision == 0
            || w == 0
            || h == 0
            || w > 8192
            || h > 8192
            || w as usize * h as usize * 4 != pixels.len()
            || pixels.capacity() > ATLAS_BYTES
        {
            return Err("native HUD atlas exceeds layout or memory limits");
        }
        Ok(Self {
            revision,
            size,
            pixels,
        })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

type Tiles = BTreeMap<(u32, u32), Arc<Tile>>;

#[derive(Clone)]
pub struct Tile {
    pub serial: u64,
    pub rect: [u32; 4],
    pub rgba: Box<[u8]>,
}

/// Persistent dirty pixels. A latest-only frame pins its complete tile view,
/// so dropping an intermediate frame cannot lose newly rasterized glyphs.
pub struct Updates {
    atlas: Arc<Atlas>,
    tiles: Arc<Tiles>,
    serial: u64,
}

impl Updates {
    pub fn new(atlas: Atlas) -> Self {
        Self {
            atlas: Arc::new(atlas),
            tiles: Arc::default(),
            serial: 0,
        }
    }

    pub fn revision(&self) -> u64 {
        self.atlas.revision()
    }

    pub fn patch(&mut self, rect: [u32; 4], rgba: &[u8]) -> Result<(), &'static str> {
        let [x, y, w, h] = rect;
        let [aw, ah] = self.atlas.size;
        if w == 0
            || h == 0
            || x.checked_add(w).is_none_or(|n| n > aw)
            || y.checked_add(h).is_none_or(|n| n > ah)
            || rgba.len() != w as usize * h as usize * 4
        {
            return Err("invalid native HUD atlas patch");
        }
        let cols = x / EDGE..=(x + w - 1) / EDGE;
        let rows = y / EDGE..=(y + h - 1) / EDGE;
        let extra = rows
            .clone()
            .flat_map(|r| cols.clone().map(move |c| (c, r)))
            .filter(|k| !self.tiles.contains_key(k))
            .count();
        if (self.tiles.len() + extra) * TILE_BYTES > PATCH_BYTES {
            return Err("native HUD dirty tile memory limit exceeded");
        }
        self.serial = self.serial.checked_add(1).ok_or("HUD revision exhausted")?;
        let tiles = Arc::make_mut(&mut self.tiles);
        for r in rows {
            for c in cols.clone() {
                let tx = c * EDGE;
                let ty = r * EDGE;
                let tw = EDGE.min(aw - tx);
                let th = EDGE.min(ah - ty);
                let tile = Arc::make_mut(tiles.entry((c, r)).or_insert_with(|| {
                    let rgba = (ty..ty + th)
                        .flat_map(|row| {
                            let at = ((row * aw + tx) * 4) as usize;
                            self.atlas.pixels[at..at + (tw * 4) as usize]
                                .iter()
                                .copied()
                        })
                        .collect();
                    Arc::new(Tile {
                        serial: 0,
                        rect: [tx, ty, tw, th],
                        rgba,
                    })
                }));
                tile.serial = self.serial;
                let first = tx.max(x);
                let n = ((tx + tw).min(x + w) - first) as usize * 4;
                for row in ty.max(y)..(ty + th).min(y + h) {
                    let from = (((row - y) * w + first - x) * 4) as usize;
                    let to = (((row - ty) * tw + first - tx) * 4) as usize;
                    tile.rgba[to..to + n].copy_from_slice(&rgba[from..from + n]);
                }
            }
        }
        Ok(())
    }

    pub fn frame(
        &self,
        viewport: Viewport,
        scale: f32,
        vertices: Vec<Vertex>,
        runs: Vec<(Range<u32>, Blend)>,
    ) -> Result<Frame, &'static str> {
        if viewport.size().contains(&0)
            || !scale.is_finite()
            || scale < 1.0
            || vertices.capacity() > VERTEX_BYTES / size_of::<Vertex>()
            || runs.capacity() > MAX_RUNS
            || vertices
                .iter()
                .any(|v| v.pos.iter().chain(&v.uv).any(|n| !n.is_finite()) || !v.depth.is_finite())
        {
            return Err("invalid native HUD geometry or memory budget");
        }
        let mut next = 0;
        for (r, _) in &runs {
            if r.start != next
                || r.end <= r.start
                || !r.len().is_multiple_of(3)
                || r.end as usize > vertices.len()
            {
                return Err("invalid native HUD draw order");
            }
            next = r.end;
        }
        if next as usize != vertices.len() {
            return Err("native HUD draw runs do not cover vertices");
        }
        Ok(Frame {
            atlas: self.atlas.clone(),
            tiles: self.tiles.clone(),
            viewport,
            scale,
            vertices,
            runs,
        })
    }
}

pub struct Frame {
    atlas: Arc<Atlas>,
    tiles: Arc<Tiles>,
    viewport: Viewport,
    scale: f32,
    vertices: Vec<Vertex>,
    runs: Vec<(Range<u32>, Blend)>,
}

impl Frame {
    pub fn atlas(&self) -> &Atlas {
        &self.atlas
    }
    pub fn dirty_tiles(&self, since: u64) -> impl Iterator<Item = &Tile> {
        self.tiles
            .values()
            .filter(move |tile| tile.serial > since)
            .map(Arc::as_ref)
    }
    pub fn size(&self) -> [u32; 2] {
        self.viewport.drawable()
    }
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }
    pub fn scale(&self) -> f32 {
        self.scale
    }
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }
    pub fn vertex_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.vertices)
    }
    pub fn runs(&self) -> &[(Range<u32>, Blend)] {
        &self.runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas() -> Updates {
        Updates::new(Atlas::new(17, [96, 80], Arc::new(vec![255; 96 * 80 * 4])).unwrap())
    }

    fn quad() -> Vec<Vertex> {
        let positions = [[0.0, 0.0], [8.0, 0.0], [8.0, 8.0], [0.0, 8.0]];
        [0, 1, 2, 0, 2, 3]
            .into_iter()
            .map(|i| Vertex {
                pos: positions[i],
                uv: [0.5, 0.5],
                color: u32::MAX,
                depth: 0.0,
                glint_uv: [0.0; 2],
                text: 0,
            })
            .collect()
    }

    #[test]
    fn dirty_tiles_keep_overlapping_pixels_when_an_intermediate_frame_is_dropped() {
        let viewport = Viewport::new([2360, 1640], [24, 0, 24, 20]);
        let mut updates = atlas();
        updates.patch([62, 62, 4, 4], &[9; 64]).unwrap();
        let first = updates
            .frame(viewport, 4.0, quad(), vec![(0..6, Blend::Alpha)])
            .unwrap();
        updates.patch([63, 63, 1, 1], &[99; 4]).unwrap();
        let mut later = Vec::new();
        for tile in first.dirty_tiles(0) {
            assert!(tile.rgba.contains(&9));
            later.push(tile.serial);
        }
        assert_eq!(later, [1; 4]);
        drop(first);
        let latest = updates
            .frame(viewport, 4.0, quad(), vec![(0..6, Blend::Alpha)])
            .unwrap();
        let dirty: Vec<_> = latest.dirty_tiles(0).collect();
        assert_eq!(dirty.len(), 4);
        assert_eq!(latest.dirty_tiles(1).count(), 1);
        assert_eq!(&dirty[0].rgba[(63 * 64 + 63) * 4..][..4], &[99; 4]);
        assert_eq!(dirty[0].rgba[(62 * 64 + 62) * 4], 9);
        assert_eq!(dirty[0].rgba[0], 255);
    }

    #[test]
    fn rejects_malformed_atlases_patches_and_runs() {
        assert!(Atlas::new(1, [2, 2], Arc::new(vec![0; 15])).is_err());
        let mut updates = atlas();
        assert!(updates.patch([u32::MAX, 0, 2, 1], &[0; 8]).is_err());
        assert!(updates.patch([94, 0, 3, 1], &[0; 12]).is_err());
        assert!(updates.patch([0, 0, 2, 2], &[0; 15]).is_err());
        let viewport = Viewport::new([640, 480], [0; 4]);
        for runs in [
            vec![(0..2, Blend::Alpha), (2..6, Blend::Invert)],
            vec![(0..3, Blend::Alpha)],
            vec![(3..6, Blend::Alpha)],
            vec![(0..6, Blend::Alpha), (0..6, Blend::Alpha)],
        ] {
            assert!(updates.frame(viewport, 2.0, quad(), runs).is_err());
        }
        let frame = updates
            .frame(
                viewport,
                2.0,
                quad(),
                vec![(0..6, Blend::Depth([0, 0, 16, 16]))],
            )
            .unwrap();
        assert_eq!(frame.vertex_bytes().len(), 216);
    }

    #[test]
    fn safe_area_pointer_round_trip_and_resize() {
        for (size, insets) in [
            ([2360, 1640], [24, 22, 48, 20]),
            ([1640, 2360], [0, 48, 0, 24]),
        ] {
            let viewport = Viewport::new(size, insets);
            assert_eq!(viewport.drawable(), size);
            assert_eq!(
                viewport.size(),
                [
                    size[0] - insets[0] - insets[2],
                    size[1] - insets[1] - insets[3]
                ]
            );
            let gui = (84.25, 110.5);
            let (x, y) = viewport.from_gui(gui, 4.0);
            assert_eq!(viewport.to_gui([x, y], 4.0), gui);
            assert_eq!(
                viewport.to_gui([f64::from(insets[0]), f64::from(insets[1])], 4.0),
                (0.0, 0.0)
            );
        }
        assert_eq!(
            Viewport::new([100, 80], [1000; 4]).safe_rect(),
            [0, 0, 100, 80]
        );
    }
}
