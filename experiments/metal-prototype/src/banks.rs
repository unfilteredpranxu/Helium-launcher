//! V2 bank namespace. Vertices and triangle order never change.
use crate::{
    terrain::{Capabilities, Mesh},
    upload::{BANKED_VERSION, Description},
};
pub fn supports_512(c: &Capabilities) -> bool {
    c.apple_families & 0x7f0 != 0 || c.mac_families & 6 != 0
}
pub fn validate<G: Copy + Eq>(descriptions: &[Description<G>]) -> Result<(), String> {
    let first = descriptions.first().ok_or("empty bank set")?;
    if descriptions.len() > 8
        || first.split == 0
        || first.split > 2048
        || first.sprites == 0
        || first.sprites > 32768
        || first.sprites.div_ceil(first.split) > 8
    {
        return Err("invalid bank namespace/split".into());
    }
    let mut seen = 0u8;
    for d in descriptions {
        let mut expected = *first;
        expected.bank = d.bank;
        expected.used = d.used;
        expected.allocated = d.allocated;
        if *d != expected || d.version != BANKED_VERSION || d.bank >= 8 || seen & (1 << d.bank) != 0
        {
            return Err("mixed or duplicate bank identity/layout/revision".into());
        }
        if d.size == 0
            || !d.size.is_power_of_two()
            || d.levels != d.size.ilog2() + 1
            || d.format != crate::material::Format::Rgba8Unorm
            || d.layout != crate::upload::Layout::LayerMajorMipMajor
        {
            return Err("unsupported bank format/mips".into());
        }
        seen |= 1 << d.bank;
        let start = d.bank.checked_mul(d.split).ok_or("bank overflow")?;
        let remaining = d
            .sprites
            .checked_sub(start)
            .ok_or("bank outside namespace")?;
        if d.used == 0
            || d.used != remaining.min(d.split)
            || d.allocated < d.used
            || d.allocated > 512
        {
            return Err("used/allocated bank mismatch".into());
        }
    }
    Ok(())
}
pub fn required(mesh: &Mesh<'_>, split: u32, sprites: u32) -> Result<Vec<u32>, String> {
    if split == 0 || split > 2048 || sprites == 0 || sprites.div_ceil(split) > 8 {
        return Err("invalid sprite namespace".into());
    }
    mesh.validate(sprites as usize)?;
    let mut used = [false; 8];
    for &index in mesh.indices {
        let id = u32::from(mesh.vertex(index as usize)?.layer & 0x7fff);
        used[(id / split) as usize] = true;
    }
    Ok(used
        .iter()
        .enumerate()
        .filter_map(|(i, &v)| v.then_some(i as u32))
        .collect())
}
pub fn check_mesh<G: Copy + Eq>(mesh: &Mesh<'_>, ds: &[Description<G>]) -> Result<(), String> {
    validate(ds)?;
    for bank in required(mesh, ds[0].split, ds[0].sprites)? {
        if !ds.iter().any(|d| d.bank == bank) {
            return Err(format!("missing bank {bank}"));
        }
    }
    for &index in mesh.indices {
        let id = u32::from(mesh.vertex(index as usize)?.layer & 0x7fff);
        let d = ds.iter().find(|d| d.bank == id / ds[0].split).unwrap();
        d.slice(id)?;
    }
    Ok(())
}

pub fn source_bytes<'a, S: crate::upload::Source + 'a>(
    sources: impl Iterator<Item = &'a S>,
) -> Result<usize, String> {
    let mut keys = Vec::new();
    let mut total = 0usize;
    for s in sources {
        let key = s.retention_key();
        if key.is_some_and(|k| keys.contains(&k)) {
            continue;
        }
        if let Some(k) = key {
            keys.push(k);
        }
        total = total
            .checked_add(s.retained_bytes())
            .ok_or("source retention overflow")?;
    }
    Ok(total)
}
