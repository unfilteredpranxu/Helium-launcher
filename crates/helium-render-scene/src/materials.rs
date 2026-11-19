// MIT; Copyright (c) 2026 Horizon contributors.
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaterialVersion {
    pub generation: u64,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BankDescriptor {
    pub used: u32,
    pub allocated: u32,
    pub levels: u32,
}

/// Retains one immutable snapshot for all its banks. While alive, every bank
/// must continue to expose the exact stored bytes and capacity reported at capture.
pub trait BankStorage: Send + Sync {
    fn pixels(&self, bank: usize) -> &[u8];
    fn retained_bytes(&self) -> usize;
}

#[derive(Clone)]
pub struct MaterialBank {
    version: MaterialVersion,
    bank: u32,
    edge: u32,
    split: u32,
    sprites: u32,
    descriptor: BankDescriptor,
    storage: Arc<dyn BankStorage>,
}

impl MaterialBank {
    pub fn version(&self) -> MaterialVersion {
        self.version
    }
    pub fn index(&self) -> u32 {
        self.bank
    }
    pub fn dimensions(&self) -> (u32, u32, u32) {
        (self.edge, self.split, self.sprites)
    }
    pub fn descriptor(&self) -> BankDescriptor {
        self.descriptor
    }

    pub fn pixels(&self) -> &[u8] {
        self.storage.pixels(self.bank as usize)
    }
    pub fn retained_bytes(&self) -> usize {
        self.storage.retained_bytes()
    }
    pub fn storage_id(&self) -> usize {
        Arc::as_ptr(&self.storage).cast::<()>() as usize
    }
}

/// Bank pixels are RGBA8 in layer-major order, with each layer's complete
/// mip chain stored largest first. One owner groups all banks of a revision.
pub struct MaterialSet {
    version: MaterialVersion,
    banks: Vec<MaterialBank>,
}

impl MaterialSet {
    pub fn new(
        version: MaterialVersion,
        edge: u32,
        split: u32,
        sprites: u32,
        descriptors: &[BankDescriptor],
        storage: Arc<dyn BankStorage>,
        budget: usize,
    ) -> Result<Self, String> {
        if version.generation == 0
            || !edge.is_power_of_two()
            || split == 0
            || split > 2048
            || sprites == 0
            || sprites > 32768
            || descriptors.len() > 8
            || descriptors.len() != sprites.div_ceil(split) as usize
        {
            return Err("invalid scene material namespace".into());
        }
        let levels = edge.ilog2() + 1;
        let mut layer_bytes = 0usize;
        for level in 0..levels {
            let n = (edge >> level) as usize;
            layer_bytes = n
                .checked_mul(n)
                .and_then(|n| n.checked_mul(4))
                .and_then(|n| layer_bytes.checked_add(n))
                .ok_or("scene mip chain overflow")?;
        }
        let mut payload = 0usize;
        for (index, d) in descriptors.iter().enumerate() {
            let used = (sprites - index as u32 * split).min(split);
            let bytes = layer_bytes
                .checked_mul(d.allocated as usize)
                .ok_or("scene bank size overflow")?;
            if d.used != used
                || d.allocated < used
                || d.allocated > 512
                || d.levels != levels
                || storage.pixels(index).len() != bytes
            {
                return Err("invalid scene bank layout".into());
            }
            payload = payload
                .checked_add(bytes)
                .ok_or("scene material payload overflow")?;
        }
        if storage.retained_bytes() < payload || storage.retained_bytes() > budget {
            return Err("scene material storage outside budget".into());
        }
        let banks = descriptors
            .iter()
            .copied()
            .enumerate()
            .map(|(index, descriptor)| MaterialBank {
                version,
                bank: index as u32,
                edge,
                split,
                sprites,
                descriptor,
                storage: storage.clone(),
            })
            .collect();
        Ok(Self { version, banks })
    }

    pub fn version(&self) -> MaterialVersion {
        self.version
    }
    pub fn banks(&self) -> &[MaterialBank] {
        &self.banks
    }
}
