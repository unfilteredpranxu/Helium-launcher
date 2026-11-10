//! Deterministic synthetic material/depth chart and independent expected-image oracle.
//! CPU reference images exercise the oracle only; the exported GPU test never returns one.
use super::*;

pub struct Fixture {
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u32>,
    pub sprites: Vec<u8>,
    pub lightmap: Vec<u8>,
}
impl Fixture {
    pub fn mesh(&self) -> Mesh<'_> {
        Mesh::from_vertices(&self.vertices, &self.indices)
    }
    pub fn atlas(&self) -> Atlas<'_> {
        Atlas {
            width: 2,
            height: 2,
            layers: 3,
            rgba: &self.sprites,
        }
    }
}

pub fn fixture() -> Fixture {
    let mut f = Fixture {
        vertices: Vec::new(),
        indices: Vec::new(),
        sprites: vec![
            64, 128, 240, 255, 240, 160, 32, 255, 32, 224, 96, 255, 192, 64, 160, 255, 240, 64, 32,
            255, 32, 240, 64, 255, 64, 32, 240, 255, 192, 128, 64, 255, 80, 160, 240, 0, 240, 80,
            160, 127, 160, 240, 80, 128, 80, 240, 160, 255,
        ],
        lightmap: Vec::new(),
    };
    // Synthetic analytic lightmap, NOT a claim of vanilla light-generation parity.
    // R=block*16, G=sky*16, B=128. Fractional levels exercise linear sampling.
    for sky in 0..16 {
        for block in 0..16 {
            f.lightmap.extend([block * 16, sky * 16, 128, 255]);
        }
    }
    let mut quad = |rect: [f32; 4], z, layer, color, light| {
        let [x0, y0, x1, y1] = rect;
        let base = f.vertices.len() as u32; // fixed four-quad fixture only.
        for (pos, uv) in [
            ([x0, y0, z], [1, 32767]),
            ([x1, y0, z], [32767, 32767]),
            ([x1, y1, z], [32767, 1]),
            ([x0, y1, z], [1, 1]),
        ] {
            f.vertices.push(TerrainVertex {
                pos,
                uv,
                layer,
                color,
                light,
            });
        }
        // Exact production quad topology; no diagonal change or corner reordering.
        f.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    };
    quad([-0.75, -0.75, 0.75, 0.75], 0.25, 0, 0x00ff_ffff, 0xf0f0);
    quad([-0.5, -0.5, 0.0, 0.5], 0.75, 1, 0x9140_ff80, 0x1878);
    quad(
        [0.0, 0.0, 0.5, 0.5],
        0.625,
        OPAQUE_LAYER | 2,
        0xffff_80ff,
        0xf040,
    );
    quad([0.0, -0.5, 0.5, 0.0], 0.625, 2, 0xffff_80ff, 0xf040);
    // UVs above one must wrap inside the sprite, retaining the low fixed-point bits.
    for v in &mut f.vertices[4..8] {
        v.uv = v.uv.map(|uv| uv.checked_add(32768).unwrap());
    }
    // Nonzero section/camera origin plus a nonidentity column-major clip matrix.
    // The chart's final clip-space rectangles remain the independent oracle above.
    for v in &mut f.vertices {
        for (p, offset) in v.pos.iter_mut().zip([0.375, -0.1875, 0.03125]) {
            *p -= offset;
        }
    }
    f
}

pub fn fixture_view() -> View {
    let mut view = View::identity();
    view.origin = [0.125, -0.0625, 0.03125];
    view.clip_from_relative[3][0] = 0.25;
    view.clip_from_relative[3][1] = -0.125;
    view
}

// Independent golden UNorm bytes, calculated from the fixture's known constant
// texel*tint*light values. No shader emulator or renderer code generates this table.
const FAR: [[u8; 4]; 4] = [
    [60, 120, 120, 255],
    [226, 151, 16, 255],
    [30, 211, 48, 255],
    [181, 60, 80, 255],
];
const LEFT: [[u8; 4]; 4] = [
    [11, 30, 4, 255],
    [2, 113, 8, 255],
    [3, 15, 30, 255],
    [9, 60, 8, 255],
];
const RIGHT: [[u8; 4]; 4] = [
    [75, 20, 120, 255],
    [226, 10, 80, 255],
    [151, 30, 40, 255],
    [75, 30, 80, 255],
];

/// Full 256x256 image, pixel-center rectangles map to exact integral boundaries.
/// The far plane must remain visible through the lower right quad's alpha holes.
/// Tolerance is at most 2 RGB UNorm codes; alpha must match exactly everywhere.
pub fn check_image(image: &Image) -> Result<(), String> {
    if image.rgba.len() != crate::SIDE * crate::SIDE * 4 {
        return Err("oracle image extent mismatch".into());
    }
    for y in 0..crate::SIDE {
        for x in 0..crate::SIDE {
            let want = expected_pixel(x, y);
            let got = &image.rgba[(y * crate::SIDE + x) * 4..][..4];
            if (0..3).any(|c| want[c].abs_diff(got[c]) > 2) || want[3] != got[3] {
                return Err(format!(
                    "terrain oracle mismatch at ({x},{y}): expected {want:?}, got {got:?}"
                ));
            }
        }
    }
    Ok(())
}
fn expected_pixel(x: usize, y: usize) -> [u8; 4] {
    let mut color = [0, 0, 0, 255];
    if (32..224).contains(&x) && (32..224).contains(&y) {
        color = FAR[usize::from(x >= 128) + 2 * usize::from(y >= 128)];
    }
    if (64..128).contains(&x) && (64..192).contains(&y) {
        color = LEFT[usize::from(x >= 96) + 2 * usize::from(y >= 128)];
    }
    if (128..192).contains(&x) && (64..128).contains(&y) {
        color = RIGHT[usize::from(x >= 160) + 2 * usize::from(y >= 96)];
    }
    if (128..192).contains(&x) && (160..192).contains(&y) {
        color = RIGHT[2 + usize::from(x >= 160)];
    }
    color
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(super) fn run_gpu() -> Result<Image, String> {
    let renderer = TerrainRenderer::new()?;
    let f = fixture();
    let options = Options::default();
    let image = renderer.render(&f.mesh(), f.atlas(), &f.lightmap, fixture_view(), options)?;
    check_image(&image)?;
    let reversed: Vec<u32> = f.indices.chunks_exact(3).rev().flatten().copied().collect();
    let mesh = Mesh::from_vertices(&f.vertices, &reversed);
    let other = renderer.render(&mesh, f.atlas(), &f.lightmap, fixture_view(), options)?;
    check_image(&other)?;
    if image.rgba != other.rgba {
        return Err("terrain triangle order changed output".into());
    }
    // Forward depth with z'=w-z must produce the same physical occlusion.
    let mut view = fixture_view();
    view.clip_from_relative[2][2] = -1.;
    view.clip_from_relative[3][2] = 1.;
    check_image(&renderer.render(
        &f.mesh(),
        f.atlas(),
        &f.lightmap,
        view,
        Options {
            depth: Depth::Forward,
            ..options
        },
    )?)?;
    Ok(image)
}

#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
#[test]
#[ignore = "requires Apple Metal GPU; G02 oracle not executed on Linux"]
fn actual_gpu_full_image_and_depth() {
    run_gpu().expect("production-format terrain image oracle");
}

#[cfg(test)]
mod tests {
    use super::*;
    fn simulated_caps() -> Capabilities {
        Capabilities {
            apple_families: 1 << 4,
            common_families: 0,
            mac_families: 0,
            max_buffer_bytes: 64 * 1024 * 1024,
            unified_memory: None,
            recommended_working_set_bytes: None,
            allocated_bytes_at_creation: None,
            sample_counts: 1 << 1,
            argument_buffer_tier: None,
        }
    }
    fn oracle_reference() -> Image {
        // Independent rectangle paint, not expected_pixel or the renderer. This
        // pins the chart's known pixel contract and detects mistakes in the oracle.
        let mut rgba = vec![0; 256 * 256 * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        for (rect, rgb) in [
            ([32, 32, 128, 128], [60, 120, 120]),
            ([128, 32, 224, 128], [226, 151, 16]),
            ([32, 128, 128, 224], [30, 211, 48]),
            ([128, 128, 224, 224], [181, 60, 80]),
            ([64, 64, 96, 128], [11, 30, 4]),
            ([96, 64, 128, 128], [2, 113, 8]),
            ([64, 128, 96, 192], [3, 15, 30]),
            ([96, 128, 128, 192], [9, 60, 8]),
            ([128, 64, 160, 96], [75, 20, 120]),
            ([160, 64, 192, 96], [226, 10, 80]),
            ([128, 96, 160, 128], [151, 30, 40]),
            ([160, 96, 192, 128], [75, 30, 80]),
            ([128, 160, 160, 192], [151, 30, 40]),
            ([160, 160, 192, 192], [75, 30, 80]),
        ] {
            for y in rect[1]..rect[3] {
                for x in rect[0]..rect[2] {
                    rgba[(y * 256 + x) * 4..][..3].copy_from_slice(&rgb);
                }
            }
        }
        Image {
            rgba,
            gpu_seconds: None,
        }
    }
    #[test]
    fn layout_and_bytes_match_production_without_padding_or_rounding() {
        let v = [TerrainVertex {
            pos: [1.5, -0.25, 2.125],
            uv: [0x1234, 0xabcd],
            layer: 0x8001,
            light: 0x1878,
            color: 0x91234567,
        }];
        let indices = [0, 0, 0];
        let mesh = Mesh::from_vertices(&v, &indices);
        assert_eq!(
            mesh.vertices,
            &[
                0, 0, 192, 63, 0, 0, 128, 190, 0, 0, 8, 64, 52, 18, 205, 171, 1, 128, 120, 24, 103,
                69, 35, 145
            ]
        );
        assert_eq!(mesh.vertex(0).unwrap(), v[0]);
        assert_eq!(size_of::<TerrainVertex>(), 24);
        assert_eq!(align_of::<TerrainVertex>(), 4);
        let unaligned = [&[0u8][..], mesh.vertices].concat();
        assert_eq!(
            Mesh {
                vertices: &unaligned[1..],
                indices: &indices
            }
            .vertex(0)
            .unwrap(),
            v[0]
        );
        assert!(mesh.vertex(usize::MAX).is_err());
    }
    #[test]
    fn fixture_exercises_fractional_positions_layers_metadata_light_and_quad_order() {
        let f = fixture();
        let mesh = f.mesh();
        mesh.validate(3).unwrap();
        assert_eq!((f.vertices.len(), f.indices.len()), (16, 24));
        assert_eq!(&f.indices[..6], &[0, 1, 2, 0, 2, 3]);
        assert_eq!(f.vertices[4].light, 0x1878);
        assert_eq!(f.vertices[4].color >> 24, 0x91);
        assert_eq!(f.vertices[8].layer, 0x8002);
        assert_eq!(f.vertices[12].layer, 2);
        assert_eq!(f.vertices[4].uv, [32769, 65535]);
        assert_eq!(f.vertices[0].pos, [-1.125, -0.5625, 0.21875]);
        let view = fixture_view();
        let p = [
            f.vertices[0].pos[0] + view.origin[0],
            f.vertices[0].pos[1] + view.origin[1],
            f.vertices[0].pos[2] + view.origin[2],
            1.,
        ];
        let clip: [f32; 4] =
            std::array::from_fn(|r| (0..4).map(|c| view.clip_from_relative[c][r] * p[c]).sum());
        assert_eq!(clip, [-0.75, -0.75, 0.25, 1.]);
        let a = f.vertices[0].pos;
        let b = f.vertices[1].pos;
        let c = f.vertices[2].pos;
        assert!((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.);
    }
    #[test]
    fn invalid_streams_never_reach_metal() {
        let mut f = fixture();
        f.indices[0] = u32::MAX;
        assert!(f.mesh().validate(3).is_err());
        f.indices[0] = 0;
        f.vertices[0].pos[0] = f32::NAN;
        assert!(f.mesh().validate(3).is_err());
        f.vertices[0].pos[0] = 0.;
        f.vertices[0].layer = 3;
        assert!(f.mesh().validate(3).is_err());
        f.vertices[0].layer = OPAQUE_LAYER;
        assert!(f.mesh().validate(3).is_err()); // inconsistent flat flags
        let bytes = f.mesh().vertices;
        assert!(
            Mesh {
                vertices: &bytes[..bytes.len() - 1],
                indices: &f.indices
            }
            .validate(3)
            .is_err()
        );
        assert!(
            Mesh {
                vertices: bytes,
                indices: &f.indices[..2]
            }
            .validate(3)
            .is_err()
        );
    }
    #[test]
    fn budgets_limits_padding_and_arithmetic_are_checked() {
        let f = fixture();
        let caps = simulated_caps();
        let view = View::identity();
        let options = Options::default();
        let good = validate(&f.mesh(), f.atlas(), &f.lightmap, view, options, &caps).unwrap();
        assert_eq!(
            (
                good.atlas_row,
                good.atlas_slice,
                good.atlas_staging,
                good.staging_bytes
            ),
            (256, 512, 1536, 5632)
        );
        assert_eq!(
            (good.index_bytes, good.readback_row, good.readback_bytes),
            (96, 1024, 262144)
        );
        assert!(product(&[usize::MAX, 2]).is_err());
        assert!(aligned_row(usize::MAX).is_err());
        let validate_with =
            |o, c: &Capabilities| validate(&f.mesh(), f.atlas(), &f.lightmap, view, o, c);
        let mut small = simulated_caps();
        small.max_buffer_bytes = 16;
        assert!(validate_with(options, &small).is_err());
        small.max_buffer_bytes = caps.max_buffer_bytes;
        small.apple_families = 0;
        assert!(validate_with(options, &small).is_err());
        for width in [0, 4097, usize::MAX] {
            assert!(validate_with(Options { width, ..options }, &caps).is_err());
        }
        assert!(
            validate_with(
                Options {
                    budget: Budget {
                        payload_bytes: 1,
                        target_pixels: usize::MAX
                    },
                    ..options
                },
                &caps
            )
            .is_err()
        );
        assert!(
            validate_with(
                Options {
                    budget: Budget {
                        payload_bytes: usize::MAX,
                        target_pixels: 1
                    },
                    ..options
                },
                &caps
            )
            .is_err()
        );
        assert!(
            validate(
                &f.mesh(),
                Atlas {
                    layers: 257,
                    ..f.atlas()
                },
                &f.lightmap,
                view,
                options,
                &caps
            )
            .is_err()
        );
        assert!(validate(&f.mesh(), f.atlas(), &f.lightmap[..3], view, options, &caps).is_err());
        assert_eq!(
            validate_with(
                Options {
                    width: 257,
                    height: 1,
                    ..options
                },
                &caps
            )
            .unwrap()
            .readback_row,
            1280
        );
    }
    #[test]
    fn full_image_oracle_rejects_blank_sparse_wrong_material_and_depth() {
        let reference = oracle_reference();
        check_image(&reference).unwrap();
        let mut image = Image {
            rgba: vec![0; 256 * 256 * 4],
            gpu_seconds: None,
        };
        for p in image.rgba.chunks_exact_mut(4) {
            p[3] = 255;
        }
        assert!(check_image(&image).is_err());
        for p in image.rgba.chunks_exact_mut(4).take(101) {
            p.copy_from_slice(&[255, 0, 255, 255]);
        }
        assert!(check_image(&image).is_err());
        for wrong in [[255, 0, 255, 255], FAR[0], [11, 30, 4, 0]] {
            image.rgba.copy_from_slice(&reference.rgba);
            let at = (80 * 256 + 80) * 4;
            image.rgba[at..at + 4].copy_from_slice(&wrong);
            assert!(check_image(&image).is_err());
        }
        image.rgba.truncate(16);
        assert!(check_image(&image).is_err());
    }

    #[test]
    fn readback_copies_pixels_without_reading_uninitialized_padding() {
        use std::mem::MaybeUninit;
        let mut allocation = vec![MaybeUninit::<u8>::uninit(); 512];
        for row in 0..2 {
            for x in 0..12 {
                allocation[row * 256 + x].write((row * 12 + x) as u8);
            }
        }
        // SAFETY: allocation has two rows; only each 12-byte pixel prefix is initialized.
        let packed = unsafe { copy_readback(allocation.as_ptr().cast(), 3, 2, 256) };
        assert_eq!(packed, (0..24u8).collect::<Vec<_>>());
    }
    #[test]
    fn g01_fixture_is_compatible_via_explicit_fixture_only_conversion() {
        let old = crate::fixture();
        let vertices: Vec<_> = old
            .vertices
            .iter()
            .map(|v| TerrainVertex {
                pos: [
                    f32::from(v.position[0]),
                    f32::from(v.position[1]),
                    f32::from(v.position[2]),
                ],
                uv: v
                    .uv
                    .map(|uv| (f32::from(uv) * 32768.0 / 65535.0).round() as u16),
                layer: 0,
                light: 0xf0f0,
                color: u32::from_le_bytes(v.tint),
            })
            .collect();
        let mesh = Mesh::from_vertices(&vertices, &old.indices);
        mesh.validate(1).unwrap();
        assert_eq!((vertices.len(), old.indices.len()), (184, 276));
        assert_eq!(size_of::<crate::Vertex>(), 16);
        assert_eq!(mesh.vertices.len(), 184 * 24);
        assert_eq!(vertices[0].uv, [0, 0]);
    }
}
