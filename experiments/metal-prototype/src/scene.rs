/// A separate experimental layout: 16 bytes, no implicit padding.
/// It does not reinterpret production TerrainVertex or packed mesher records.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vertex {
    pub position: [i16; 4],
    pub uv: [u16; 2],
    pub tint: [u8; 4],
}

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn validate(&self) -> Result<(), String> {
        if self.vertices.is_empty() || self.indices.is_empty() || self.indices.len() % 3 != 0 {
            return Err("expected nonempty indexed triangles".into());
        }
        if self
            .indices
            .iter()
            .any(|&i| i as usize >= self.vertices.len())
        {
            return Err("terrain index outside vertex buffer".into());
        }
        Ok(())
    }
}

pub fn fixture_camera() -> [[f32; 4]; 4] {
    // Orthographic isometric view towards the stepped hill. Nearer blocks have smaller depth.
    [
        [0.22, -0.11, -0.075, 0.0],
        [0.0, 0.22, -0.075, 0.0],
        [-0.22, -0.11, -0.075, 0.0],
        [0.0, -0.15, 0.75, 1.0],
    ]
}

pub fn fixture() -> Mesh {
    let height = |x: i16, z: i16| {
        if (0..3).contains(&x) && (0..3).contains(&z) {
            1 + i16::from(x == 1) + i16::from(z == 1)
        } else {
            0
        }
    };
    let faces = [
        ([1, 0, 0], [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]]),
        ([-1, 0, 0], [[0, 0, 1], [0, 1, 1], [0, 1, 0], [0, 0, 0]]),
        ([0, 1, 0], [[0, 1, 1], [1, 1, 1], [1, 1, 0], [0, 1, 0]]),
        ([0, -1, 0], [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]]),
        ([0, 0, 1], [[1, 0, 1], [1, 1, 1], [0, 1, 1], [0, 0, 1]]),
        ([0, 0, -1], [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]]),
    ];
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    for x in 0..3 {
        for z in 0..3 {
            for y in 0..height(x, z) {
                for (normal, corners) in faces {
                    let neighbor_y = y + normal[1];
                    if neighbor_y >= 0 && neighbor_y < height(x + normal[0], z + normal[2]) {
                        continue;
                    }
                    let base = mesh.vertices.len() as u32;
                    let tint = if normal[1] > 0 {
                        [90, 180, 65, 255]
                    } else {
                        [130, 90, 55, 255]
                    };
                    for (corner, uv) in
                        corners
                            .into_iter()
                            .zip([[0, 0], [0, 65535], [65535, 65535], [65535, 0]])
                    {
                        mesh.vertices.push(Vertex {
                            position: [x + corner[0], y + corner[1], z + corner[2], 1],
                            uv,
                            tint,
                        });
                    }
                    mesh.indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
                }
            }
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_has_only_exposed_indexed_block_faces() {
        let m = fixture();
        m.validate().unwrap();
        // 9 top and 9 bottom faces; each side profile is 1,2,1: 16 boundary faces.
        // Internal height steps add another 12 faces: total 46.
        assert_eq!(m.vertices.len(), 46 * 4);
        assert_eq!(m.indices.len(), 46 * 6);
        assert_eq!(m.vertices, fixture().vertices);
        assert_eq!(std::mem::size_of::<Vertex>(), 16);
        assert_eq!(std::mem::offset_of!(Vertex, uv), 8);
        assert_eq!(std::mem::offset_of!(Vertex, tint), 12);
        for v in m.vertices {
            let p = v.position.map(f32::from);
            let camera = fixture_camera();
            let clip: [f32; 4] = std::array::from_fn(|r| (0..4).map(|c| camera[c][r] * p[c]).sum());
            assert!((-1.0..=1.0).contains(&clip[0]) && (-1.0..=1.0).contains(&clip[1]));
            assert!((0.0..=1.0).contains(&clip[2]));
        }
    }
    #[test]
    fn invalid_meshes_are_rejected_before_ffi() {
        let mut m = fixture();
        m.indices[0] = m.vertices.len() as u32;
        assert!(m.validate().is_err());
        m.indices.clear();
        assert!(m.validate().is_err());
        m.indices.push(0);
        assert!(m.validate().is_err());
    }
}
