//! MIT; Copyright (c) 2026 Horizon contributors.
//! Storage policy for the standalone native gameplay qualification build.
use horizon_client::{Result, save::SessionLock};
#[cfg(unix)]
use rustix::fs::{Mode, OFlags, mkdirat, open, openat};
use std::{
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

pub(super) const SLOT: &str = "disposable-world";
const ROOT: &str = "Helium-Disposable-v1";
const ADMISSION: &str = "world-admission-v1";
static ACTIVE: OnceLock<Namespace> = OnceLock::new();

pub(crate) fn active() -> Option<&'static Namespace> {
    ACTIVE.get()
}

#[cfg(all(target_os = "ios", feature = "helium-native-metal-gameplay"))]
pub(crate) fn activate(home: &Path) -> Result<()> {
    horizon_ios_launcher::standalone_test_image(home)
        .ok_or("disposable gameplay requires the standalone test host")?;
    let namespace = Namespace::open(home)?;
    std::env::set_current_dir(&namespace.root)?;
    ACTIVE
        .set(namespace)
        .map_err(|_| "disposable sandbox already active")?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity(u64, u64);

fn identity(metadata: &Metadata) -> Result<Identity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Identity(metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err("disposable storage requires Unix filesystem identity".into())
    }
}

fn regular(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.is_file() && metadata.nlink() == 1
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn optional(path: &Path) -> io::Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Verify the actual directory name as well as its canonical path (APFS can
/// return a match for a different spelling on a case-insensitive volume).
fn directory(path: &Path) -> Result<Identity> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || fs::canonicalize(path)? != path {
        return Err("disposable storage directory alias rejected".into());
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && !fs::read_dir(parent)?.any(|entry| entry.is_ok_and(|entry| entry.file_name() == name))
    {
        return Err("disposable storage directory spelling changed".into());
    }
    identity(&meta)
}

fn ensure_directory(path: &Path) -> Result<Identity> {
    if optional(path)?.is_none() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path)?;
    }
    directory(path)
}

fn validate_tree(path: &Path, volume: u64) -> Result<()> {
    let mut stack = vec![path.to_owned()];
    let mut directories = std::collections::HashSet::new();
    while let Some(path) = stack.pop() {
        let meta = fs::symlink_metadata(&path)?;
        let id = identity(&meta)?;
        if id.0 != volume {
            return Err("disposable storage mount alias rejected".into());
        }
        if meta.is_dir() {
            if !directories.insert(id.1) {
                return Err("disposable storage directory alias rejected".into());
            }
            for entry in fs::read_dir(path)? {
                stack.push(entry?.path());
            }
        } else if !regular(&meta) {
            return Err("disposable storage link or special file rejected".into());
        }
    }
    Ok(())
}

pub(crate) struct Namespace {
    root: PathBuf,
    documents: PathBuf,
    root_id: Identity,
    saves_id: Identity,
    backups_id: Identity,
    #[cfg(unix)]
    root_dir: File,
    #[cfg(unix)]
    saves_dir: File,
    _lock: File,
    resource_import: Mutex<()>,
}

impl Namespace {
    #[cfg(all(
        unix,
        any(test, all(target_os = "ios", feature = "helium-native-metal-gameplay"))
    ))]
    fn open(home: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(home)?;
        if !metadata.is_dir() || fs::canonicalize(home)? != home {
            return Err("iOS data container directory alias rejected".into());
        }
        let home_id = identity(&metadata)?;
        let library = home.join("Library");
        ensure_directory(&library)?;
        let support = library.join("Application Support");
        ensure_directory(&support)?;
        let root = support.join(ROOT);
        let root_id = ensure_directory(&root)?;
        let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let root_dir = File::from(open(&root, dir_flags, Mode::empty())?);
        let documents = home.join("Documents");
        ensure_directory(&documents)?;
        ensure_directory(&documents.join("logs"))?;
        validate_tree(&documents.join("logs"), home_id.0)?;
        if optional(&documents.join("horizon-boot.log"))?.is_some_and(|meta| !regular(&meta)) {
            return Err("disposable boot log alias rejected".into());
        }
        if optional(&documents.join("helium-metal-test.txt"))?.is_some() {
            return Err("disposable gameplay does not accept a mode file".into());
        }
        let saves_id = ensure_directory(&root.join("saves"))?;
        let backups_id = ensure_directory(&root.join("backups"))?;
        let saves_dir = File::from(openat(&root_dir, "saves", dir_flags, Mode::empty())?);
        if [root_id, saves_id, backups_id]
            .iter()
            .any(|id| id.0 != home_id.0)
            || identity(&root_dir.metadata()?)? != root_id
            || identity(&saves_dir.metadata()?)? != saves_id
        {
            return Err("disposable storage is outside the data container volume".into());
        }
        let lock = File::from(openat(
            &root_dir,
            "session.lock",
            OFlags::WRONLY | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        if !regular(&lock.metadata()?) {
            return Err("disposable namespace lock alias rejected".into());
        }
        lock.try_lock()
            .map_err(|_| "disposable namespace is already in use")?;
        let namespace = Self {
            _lock: lock,
            root,
            documents,
            root_id,
            saves_id,
            backups_id,
            root_dir,
            saves_dir,
            resource_import: Mutex::new(()),
        };
        namespace.inspect()?;
        // This directory is private and all legacy relative paths are rooted
        // here. Reject existing aliases before the loader uses any of them.
        for entry in fs::read_dir(&namespace.root)? {
            let entry = entry?;
            if !matches!(entry.file_name().to_str(), Some("saves" | "backups")) {
                let meta = fs::symlink_metadata(entry.path())?;
                if identity(&meta)?.0 != home_id.0 || !(meta.is_dir() || regular(&meta)) {
                    return Err("disposable private storage contains an alias".into());
                }
            }
        }
        Ok(namespace)
    }

    pub(super) fn saves(&self) -> PathBuf {
        self.root.join("saves")
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn documents(&self) -> &Path {
        &self.documents
    }

    fn inspect(&self) -> Result<Option<PathBuf>> {
        let saves = self.saves();
        if directory(&self.root)? != self.root_id
            || directory(&saves)? != self.saves_id
            || directory(&self.root.join("backups"))? != self.backups_id
        {
            return Err("disposable namespace was replaced".into());
        }
        for entry in fs::read_dir(&saves)? {
            if entry?.file_name() != SLOT {
                return Err("unexpected entry in disposable saves".into());
            }
        }
        let world = saves.join(SLOT);
        let ledger = self.root.join(ADMISSION);
        match (optional(&world)?, optional(&ledger)?) {
            (None, None) => Ok(None),
            (Some(_), Some(meta)) if regular(&meta) => {
                self.checked_world_handle(&world)?;
                Ok(Some(world))
            }
            _ => Err("disposable world has no complete creation receipt".into()),
        }
    }

    pub(super) fn check_selector(&self, folder: &str, create: bool) -> Result<()> {
        if folder != SLOT {
            return Err("only the disposable world slot is allowed".into());
        }
        if self.inspect()?.is_some() == create {
            return Err("disposable world must be freshly created or explicitly reopened".into());
        }
        Ok(())
    }

    /// Worker-only path before level metadata, upgrades or data packs are read.
    pub(super) fn prepare_world(&self, folder: &str, create: bool) -> Result<PathBuf> {
        self.check_selector(folder, create)?;
        if create {
            #[cfg(unix)]
            {
                mkdirat(&self.saves_dir, SLOT, Mode::RWXU)?;
                self.saves_dir.sync_all()?;
                let world = self.open_world_handle(&self.saves().join(SLOT))?;
                let mut file = File::from(openat(
                    &self.root_dir,
                    ADMISSION,
                    OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::WRONLY
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                )?);
                file.write_all(receipt(&world.metadata()?)?.as_bytes())?;
                file.sync_all()?;
                self.root_dir.sync_all()?;
            }
            #[cfg(not(unix))]
            return Err("disposable creation requires anchored filesystem operations".into());
        }
        self.authorized_world()?
            .ok_or_else(|| "disposable world missing".into())
    }

    pub(super) fn authorized_world(&self) -> Result<Option<PathBuf>> {
        let Some(world) = self.inspect()? else {
            return Ok(None);
        };
        validate_tree(&world, self.saves_id.0)?;
        Ok(Some(world))
    }

    #[cfg(unix)]
    fn open_world_handle(&self, world: &Path) -> Result<File> {
        let slot = File::from(openat(
            &self.saves_dir,
            SLOT,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let id = identity(&slot.metadata()?)?;
        if id.0 != self.saves_id.0 || id != directory(world)? {
            return Err("disposable slot path and descriptor differ".into());
        }
        Ok(slot)
    }

    fn checked_world_handle(&self, world: &Path) -> Result<File> {
        #[cfg(unix)]
        {
            let slot = self.open_world_handle(world)?;
            let ledger = File::from(openat(
                &self.root_dir,
                ADMISSION,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            if !regular(&ledger.metadata()?) {
                return Err("disposable receipt alias rejected".into());
            }
            let mut contents = String::new();
            ledger.take(128).read_to_string(&mut contents)?;
            if contents != receipt(&slot.metadata()?)? {
                return Err("disposable world does not match its creation receipt".into());
            }
            Ok(slot)
        }
        #[cfg(not(unix))]
        {
            let _ = world;
            Err("disposable receipt verification requires anchored filesystem operations".into())
        }
    }

    pub(super) fn world_lock(&self, world: &Path) -> Result<SessionLock> {
        #[cfg(unix)]
        {
            if world != self.saves().join(SLOT) {
                return Err("disposable lock requires the admitted world slot".into());
            }
            let slot = self.checked_world_handle(world)?;
            SessionLock::acquire_anchored(world, &slot)
        }
        #[cfg(not(unix))]
        {
            let _ = world;
            Err("disposable lock requires anchored filesystem operations".into())
        }
    }

    pub(super) fn list(&self) -> Result<Vec<horizon_client::title::WorldEntry>> {
        let Some(world) = self.authorized_world()? else {
            return Ok(Vec::new());
        };
        // An interruption can leave only the durably admitted level record.
        // Recover through the same exclusive session lock before listing it.
        if !has_level_data(&world) {
            let _lock = self.world_lock(&world)?;
            require_level_data(&world)?;
        }
        Ok(horizon_client::title::list_worlds(&self.saves()))
    }

    pub(super) fn backup(&self, folder: &str) -> Result<u64> {
        self.check_selector(folder, false)?;
        let world = self.authorized_world()?.ok_or("disposable world missing")?;
        let _world_lock = self.world_lock(&world)?;
        require_level_data(&world)?;
        validate_tree(&self.root.join("backups"), self.root_id.0)?;
        horizon_client::title::world_management::backup(&self.saves(), folder)
    }

    pub(super) fn import_resources(
        &self,
        path: &Path,
    ) -> Result<horizon_client::import::Installed> {
        #[cfg(unix)]
        {
            // Only a direct Documents child. Accept the system's /var spelling
            // of this sandbox but never canonicalize the untrusted leaf.
            let parent = path
                .parent()
                .ok_or("prepared archive must be in Documents")?;
            let name = path.file_name().ok_or("missing prepared archive name")?;
            if path.extension().and_then(|s| s.to_str()) != Some("zip")
                || parent
                    .components()
                    .any(|p| matches!(p, std::path::Component::ParentDir))
                || fs::canonicalize(parent)? != self.documents
            {
                return Err("prepared archive must be a .zip directly in Documents".into());
            }
            let dir = open(
                &self.documents,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
                Mode::empty(),
            )?;
            let file = File::from(openat(
                &dir,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )?);
            if !regular(&file.metadata()?) {
                return Err("prepared archive link or special file rejected".into());
            }
            self.inspect()?;
            let _import = self
                .resource_import
                .try_lock()
                .map_err(|_| "a prepared resource import is already running")?;
            horizon_client::import::wait_for_boot_import();
            horizon_client::import::Pack::install_prepared_copy(path, file, &self.root)
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err("disposable import requires Unix filesystem checks".into())
        }
    }
}

fn has_level_data(world: &Path) -> bool {
    ["level.dat", "level.dat_old"]
        .iter()
        .any(|name| world.join(name).is_file())
}

pub(super) fn require_level_data(world: &Path) -> Result<()> {
    if !has_level_data(world) {
        return Err("disposable world creation did not finish".into());
    }
    Ok(())
}

#[cfg(unix)]
fn receipt(metadata: &Metadata) -> Result<String> {
    // The device number is checked against this launch's private volume.
    // The inode and birth time identify the created directory across launches.
    let created = metadata.created()?.duration_since(std::time::UNIX_EPOCH)?;
    Ok(format!(
        "{}:{}:{}\n",
        identity(metadata)?.1,
        created.as_secs(),
        created.subsec_nanos()
    ))
}

/// The native host bridge exposes a closed set of game commands. In-game
/// UIKit/winit input continues through the normal key and touch handlers.
pub(super) fn allows_control(command: &str) -> bool {
    matches!(command, "worlds" | "world-create" | "state")
}

#[cfg(all(test, unix))]
mod tests;
