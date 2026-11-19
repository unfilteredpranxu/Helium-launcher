use super::*;

struct Pixels(Vec<Vec<u8>>);
impl BankStorage for Pixels {
    fn pixels(&self, bank: usize) -> Option<&[u8]> {
        self.0.get(bank).map(Vec::as_slice)
    }
    fn retained_bytes(&self) -> usize {
        self.0.iter().map(Vec::capacity).sum()
    }
}

#[test]
fn paired_banks_share_the_original_snapshot_without_copying_or_aliasing_the_revision() {
    let version = MaterialVersion {
        generation: 9,
        revision: 4,
    };
    let owner = Arc::new(Pixels(vec![vec![1; 40], vec![2; 20]]));
    let descriptors = [
        BankDescriptor {
            used: 2,
            allocated: 2,
            levels: 2,
        },
        BankDescriptor {
            used: 1,
            allocated: 1,
            levels: 2,
        },
    ];
    assert!(MaterialSet::new(version, 2, 2, 3, &descriptors, owner.clone(), 59).is_err());
    let banks = MaterialSet::new(version, 2, 2, 3, &descriptors, owner.clone(), 60).unwrap();
    assert_eq!(banks.banks()[1].version(), version);
    assert_eq!(banks.banks()[0].storage_id(), banks.banks()[1].storage_id());
    assert_eq!(banks.banks()[0].pixels().as_ptr(), owner.0[0].as_ptr());
    drop(owner);
    assert_eq!(banks.banks()[1].pixels(), &[2; 20]);
    assert!(
        MaterialSet::new(
            version,
            2,
            2,
            3,
            &descriptors,
            Arc::new(Pixels(vec![vec![0; 39], vec![0; 20]])),
            60
        )
        .is_err()
    );
}

#[test]
fn owned_section_and_camera_keep_terrain_abi_and_reject_foreign_epochs() {
    let version = SectionVersion {
        epoch: NonZeroU64::new(7).unwrap(),
        generation: 9,
        key: (1_000_000, 0, 0),
        revision: 23,
    };
    let vertex = TerrainVertex {
        pos: [0.25, 0.5, 0.75],
        uv: [0, 32768],
        layer: OPAQUE_LAYER,
        light: 0x243c,
        color: 0x1234abcd,
    };
    let mut bytes = bytemuck::cast_slice(&[vertex; 3]).to_vec();
    let alloc = bytes.as_ptr();
    let indices = vec![0, 1, 2];
    let budget = bytes.capacity() + indices.capacity() * 4;
    let origin = [16_000_000, -64, 0];
    let bounds = [vertex.pos; 2];
    assert!(
        Section::new(
            version,
            origin,
            bounds,
            0,
            bytes.clone(),
            indices.clone(),
            budget - 1
        )
        .is_err()
    );
    let section = Section::new(
        version,
        origin,
        bounds,
        2,
        std::mem::take(&mut bytes),
        indices,
        budget,
    )
    .unwrap();
    assert_eq!(section.retained_bytes(), budget);
    assert_eq!(section.mesh().vertices.as_ptr(), alloc);
    assert_eq!(section.mesh().vertex(0).unwrap(), vertex);
    section.mesh().validate(1).unwrap();

    let materials = MaterialSet::new(
        MaterialVersion {
            generation: 9,
            revision: 1,
        },
        1,
        1,
        1,
        &[BankDescriptor {
            used: 1,
            allocated: 1,
            levels: 1,
        }],
        Arc::new(Pixels(vec![vec![255; 4]])),
        4,
    )
    .unwrap();
    let mut frame = Frame {
        epoch: version.epoch,
        camera: Camera {
            position: [16_000_000.25, -64., 0.],
            clip_from_relative: View::identity().clip_from_relative,
        },
        lightmap: Lightmap {
            revision: 2,
            rgba: [255; 1024],
        },
        materials: Arc::new(materials),
        sections: vec![section],
    };
    assert_eq!(frame.camera.section_view(origin).unwrap().origin[0], -0.25);
    assert!(frame.accepts(&frame.sections[0]));
    frame.epoch = NonZeroU64::new(8).unwrap();
    assert!(!frame.accepts(&frame.sections[0]));
}
