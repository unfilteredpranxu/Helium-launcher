//! MIT; Copyright (c) 2026 Horizon contributors.
//! Per-record redo log. Published handoffs are fsynced before writer admission;
//! replay requires the world's exclusive session lock, before any world reads.
use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

const MAX_RECORD: u64 = 64 * 1024 * 1024;
const SUFFIX: &str = ".helium-redo-v3";
// Serialize publication/acknowledgement, including different SaveDir owners.
// Never held by a UI status probe or across a worker join.
static SERIAL: Mutex<()> = Mutex::new(());
pub(super) fn directory(path: &Path) -> Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        directory(parent)?;
    }
    match fs::create_dir(path) {
        Ok(()) => sync_folder(path)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
pub(super) struct Ticket {
    path: PathBuf,
    digest: [u8; 32],
}
fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn publish(path: PathBuf, bytes: &[u8]) -> Result<Ticket> {
    let _serial = SERIAL.lock().map_err(|_| "save redo lock poisoned")?;
    publish_locked(path, bytes)
}
fn publish_locked(path: PathBuf, bytes: &[u8]) -> Result<Ticket> {
    if bytes.len() as u64 > MAX_RECORD {
        return Err("save redo record exceeds bounded record budget".into());
    }
    let parent = path.parent().ok_or("save redo parent missing")?;
    directory(parent)?;
    let hash = digest(bytes);
    let temporary = path.with_extension("helium-redo-building");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(b"HLJ3")?;
    file.write_all(&(bytes.len() as u64).to_le_bytes())?;
    file.write_all(&hash)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(test)]
    tests::pause_before_publication(&path, bytes)?;
    fs::rename(&temporary, &path)?;
    sync_folder(&path)?;
    Ok(Ticket { path, digest: hash })
}
fn contents(path: &Path) -> Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    let mut header = [0; 44];
    file.read_exact(&mut header)?;
    if &header[..4] != b"HLJ3" {
        return Err("invalid save redo version".into());
    }
    let length = u64::from_le_bytes(header[4..12].try_into().unwrap());
    if length > MAX_RECORD || file.metadata()?.len() != 44 + length {
        return Err("invalid save redo length".into());
    }
    let mut bytes = vec![0; length as usize];
    file.read_exact(&mut bytes)?;
    if digest(&bytes) != header[12..44] {
        return Err("save redo checksum mismatch; world opening refused".into());
    }
    Ok(bytes)
}
pub(super) fn acknowledge(ticket: Ticket) -> Result<()> {
    let _serial = SERIAL.lock().map_err(|_| "save redo lock poisoned")?;
    acknowledge_locked(ticket)
}
pub(super) fn acknowledge_locked(ticket: Ticket) -> Result<()> {
    match contents(&ticket.path) {
        Ok(bytes) if digest(&bytes) == ticket.digest => {
            fs::remove_file(&ticket.path)?;
            sync_folder(&ticket.path)?;
        }
        Ok(_) => {} // A newer handoff owns this pathname.
        Err(_) if !ticket.path.exists() => {}
        Err(error) => return Err(error),
    }
    Ok(())
}
pub(super) fn file(path: &Path, bytes: &[u8]) -> Result<Ticket> {
    let name = path
        .file_name()
        .ok_or("save filename missing")?
        .to_os_string();
    let mut redo = name;
    redo.push(SUFFIX);
    publish(path.with_file_name(redo), bytes)
}
pub(super) fn replace_file(
    path: &Path,
    bytes: &[u8],
    apply: impl FnOnce() -> Result<()>,
) -> Result<()> {
    // Publication, destination replacement and acknowledgement for one file
    // cannot overtake each other even when two storage owners target it.
    let _serial = SERIAL.lock().map_err(|_| "save redo lock poisoned")?;
    let mut name = path
        .file_name()
        .ok_or("save filename missing")?
        .to_os_string();
    name.push(SUFFIX);
    let ticket = publish_locked(path.with_file_name(name), bytes)?;
    apply()?;
    acknowledge_locked(ticket)
}
pub(super) fn commit_file(ticket: Ticket, apply: impl FnOnce() -> Result<()>) -> Result<bool> {
    let _serial = SERIAL.lock().map_err(|_| "save redo lock poisoned")?;
    match contents(&ticket.path) {
        Ok(bytes) if digest(&bytes) == ticket.digest => {
            apply()?;
            acknowledge_locked(ticket)?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(_) if !ticket.path.exists() => Ok(false),
        Err(error) => Err(error),
    }
}
pub(super) fn locked<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    let _serial = SERIAL.lock().map_err(|_| "save redo lock poisoned")?;
    f()
}
pub(super) fn current(ticket: &Ticket) -> Result<bool> {
    match contents(&ticket.path) {
        Ok(bytes) => Ok(digest(&bytes) == ticket.digest),
        Err(_) if !ticket.path.exists() => Ok(false),
        Err(error) => Err(error),
    }
}
pub(super) fn apply_region(
    root: &Path,
    cache: &RegionCache,
    folder: &str,
    records: &[((i32, i32), &Record)],
    tickets: Vec<((i32, i32), Ticket)>,
) -> Result<()> {
    write_region_records_guarded(root, cache, folder, records, Some(tickets))
}
pub(super) fn region(
    root: &Path,
    folder: Folder,
    key: (i32, i32),
    record: &Record,
) -> Result<Ticket> {
    let mut bytes = vec![0];
    if let Some(tag) = record.tag() {
        bytes[0] = 1;
        bytes.extend(nbt::write_named(&tag, Limits::saved()).map_err(|e| e.to_string())?);
    }
    let kind = match folder {
        Folder::Region => 0,
        Folder::Entities => 1,
        Folder::Poi => 2,
    };
    publish(
        root.join(".helium-redo-v3")
            .join(format!("{kind}_{}_{}{SUFFIX}", key.0, key.1)),
        &bytes,
    )
}

/// Called only by SessionLock::acquire while its OS lock is held. Invalid
/// committed records fail closed and stay intact. Unpublished build files are
/// ignored; a writer never acknowledges admission until publication is synced.
pub(super) fn recover_locked(root: &Path) -> Result<()> {
    fn walk(root: &Path, depth: usize, count: &mut usize) -> Result<()> {
        if depth > 32 {
            return Err("save redo directory depth exceeded".into());
        }
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            *count += 1;
            if *count > 100_000 {
                return Err("save redo scan budget exceeded".into());
            }
            let path = entry.path();
            if kind.is_dir() {
                walk(&path, depth + 1, count)?;
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(SUFFIX))
            {
                replay(&path)?;
            }
        }
        Ok(())
    }
    walk(root, 0, &mut 0)
}
fn replay(path: &Path) -> Result<()> {
    let bytes = contents(path)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("invalid redo filename")?;
    let stem = name.strip_suffix(SUFFIX).ok_or("invalid redo suffix")?;
    let parent = path.parent().ok_or("invalid redo parent")?;
    if parent.file_name().is_some_and(|p| p == ".helium-redo-v3") {
        let parts: Vec<_> = stem.split('_').collect();
        if parts.len() != 3 {
            return Err("invalid region redo key".into());
        }
        let folder = match parts[0] {
            "0" => Folder::Region,
            "1" => Folder::Entities,
            "2" => Folder::Poi,
            _ => return Err("invalid region redo kind".into()),
        };
        let key = (parts[1].parse::<i32>()?, parts[2].parse::<i32>()?);
        let record = match bytes.first() {
            Some(0) if bytes.len() == 1 => Record::Delete,
            Some(1) => {
                let (tag, used) =
                    nbt::read_named(&bytes[1..], Limits::saved()).map_err(|e| e.to_string())?;
                if used != bytes.len() - 1 {
                    return Err("trailing save redo data".into());
                }
                Record::Tag(Arc::new(tag))
            }
            _ => return Err("invalid region redo payload".into()),
        };
        let cache = RegionCache::default();
        let root = parent.parent().ok_or("invalid redo root")?;
        write_region_records(root, &cache, folder.name(), &[(key, &record)])?;
    } else {
        if stem.is_empty() {
            return Err("invalid file redo destination".into());
        }
        let destination = parent.join(stem);
        let temporary = destination.with_extension("helium-replaying");
        write_synced(&temporary, &bytes)?;
        fs::rename(&temporary, &destination)?;
        sync_folder(&destination)?;
    }
    #[cfg(test)]
    if let Some(root) = std::env::var_os("HORIZON_REPLAY_CHILD_ROOT") {
        // Test binary only: interrupt replay after real destination sync and
        // before cleanup, without weakening the production commit boundary.
        fs::write(
            PathBuf::from(root).join("replay-synced-before-ack"),
            b"synthetic replay fixture",
        )?;
        loop {
            std::thread::park();
        }
    }
    acknowledge(Ticket {
        path: path.to_owned(),
        digest: digest(&bytes),
    })
}

#[cfg(test)]
mod tests;
