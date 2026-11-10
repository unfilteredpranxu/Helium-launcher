//! The phone launcher's Mods page (packaging/ios/Launcher/Mods.swift): the `.hmod` content mods in
//! `horizon-mods`, listed, switched on and off, installed and deleted. It speaks JSON lines like
//! `launcher-world` does, and everything it writes stays inside the game folder the player's Files app
//! can see.
use horizon_client::{
    Result,
    mod_list::{self, ModList},
    settings::Settings,
};
use horizon_mods::{Content, Package, Side, find, manifest::MANIFEST};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// A code mod's row on the Mods page: this build has no WebAssembly runtime, so only content loads.
const CODE: &str = "Code mods don't run on this build; only content mods do";
/// A server mod (`side = "server"`) belongs to a server, not to the player's game.
const SERVER: &str = "A server mod doesn't run in the game";

static MODS_DIRTY: AtomicBool = AtomicBool::new(false);

/// Whether a command changed `horizon-mods` or `horizon-mods.txt` since the launchers last looked:
/// the settings they hold are a snapshot, and the next Play must read the new list.
pub(super) fn take_dirty() -> bool {
    MODS_DIRTY.swap(false, Ordering::Relaxed)
}

/// `mods-list`, `mods-set {json}` and `mods-delete {json}`, which read and write files only. `None`
/// for any other line; installing is [`install`], on a worker.
pub(super) fn command(dir: &Path, line: &str) -> Result<Option<Value>> {
    let command = line.split_whitespace().next().unwrap_or_default();
    let value = match command {
        "mods-list" => list(dir)?,
        "mods-set" => {
            let args = json_args(line, command)?;
            let id = args["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or("mods-set {\"id\", \"on\"}")?;
            let on = args["on"].as_bool().ok_or("mods-set {\"id\", \"on\"}")?;
            set(dir, id, on)?
        }
        "mods-delete" => {
            let args = json_args(line, command)?;
            let file = args["file"]
                .as_str()
                .filter(|file| !file.is_empty())
                .ok_or("mods-delete {\"file\"}")?;
            delete(dir, file)?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

/// `mods-set`'s id and switch, or `mods-install`'s staged path, as JSON after the command.
fn json_args(line: &str, command: &str) -> Result<Value> {
    let rest = line
        .trim_start()
        .strip_prefix(command)
        .unwrap_or_default()
        .trim();
    if rest.is_empty() {
        return Err(format!("{command} {{json}}").into());
    }
    Ok(serde_json::from_str(rest)?)
}

/// The `path` of a `mods-install {"path": "..."}` line: the copy the launcher staged in the game
/// folder, before [`install`] reads and moves it.
pub(super) fn install_arg(line: &str) -> std::result::Result<PathBuf, String> {
    let args: Value = json_args(line, "mods-install").map_err(|e| e.to_string())?;
    args["path"]
        .as_str()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "mods-install {\"path\"}".into())
}

/// A mod as the page's rows show it: `{file, id, name, version, description, api, enabled, code,
/// problem}`, and the folder the player can see in Files.
fn list(dir: &Path) -> Result<Value> {
    let folder = dir.join(mod_list::DIR);
    fs::create_dir_all(&folder)?;
    let on = ModList::load(dir).enabled;
    let rows: Vec<Value> = find(&folder)
        .into_iter()
        .map(|path| {
            let file = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            match Package::read(&path) {
                Ok(package) => {
                    let manifest = &package.manifest;
                    let code = package.wasm.is_some();
                    let problem = if code {
                        Some(CODE)
                    } else if manifest.side == Side::Server {
                        Some(SERVER)
                    } else {
                        None
                    };
                    json!({
                        "file": file,
                        "id": manifest.id,
                        "name": manifest.name,
                        "version": manifest.version,
                        "description": manifest.description,
                        "api": manifest.api,
                        "enabled": on.contains(&manifest.id),
                        "code": code,
                        "problem": problem,
                    })
                }
                Err(e) => json!({
                    "file": file,
                    "id": "",
                    "name": file,
                    "version": "",
                    "description": "",
                    "api": "",
                    "enabled": false,
                    "code": false,
                    "problem": format!("Can't be read: {e}"),
                }),
            }
        })
        .collect();
    Ok(json!({ "mods": rows, "folder": folder.to_string_lossy() }))
}

/// `mods-set {"id", "on"}`: turns an installed content mod on or off in `horizon-mods.txt`, keeping
/// every other mod's line and the keys the player chose.
fn set(dir: &Path, id: &str, on: bool) -> Result<Value> {
    let folder = dir.join(mod_list::DIR);
    let package = package_with_id(&folder, id)?.ok_or_else(|| format!("no mod {id}"))?;
    if on && package.wasm.is_some() {
        return Err(CODE.into());
    }
    let mut list = ModList::load(dir);
    if on {
        list.enabled.insert(id.to_owned());
    } else {
        list.enabled.remove(id);
    }
    list.save(dir);
    MODS_DIRTY.store(true, Ordering::Relaxed);
    Ok(json!({ "ok": true, "id": id, "on": on }))
}

/// `mods-delete {"file"}`: removes an installed mod by the file name the page lists, then drops its
/// line and keys. A folder mod is deleted only when it has a manifest, as [`find`] lists it.
fn delete(dir: &Path, file: &str) -> Result<Value> {
    if file == "." || file == ".." || file.contains('/') || file.contains('\\') {
        return Err(format!("{file}: only a file name in {}", mod_list::DIR).into());
    }
    let folder = dir.join(mod_list::DIR);
    let path = folder.join(file);
    let meta = fs::symlink_metadata(&path)?;
    if meta.file_type().is_symlink() {
        return Err(format!("{file}: a link, not a mod").into());
    }
    let folder_mod = meta.is_dir() && path.join(MANIFEST).is_file();
    let file_mod = meta.is_file() && path.extension().is_some_and(|e| e == "hmod");
    if !folder_mod && !file_mod {
        return Err(format!("{file}: not an installed mod").into());
    }
    let id = Package::read(&path).ok().map(|package| package.manifest.id);
    if folder_mod {
        fs::remove_dir_all(&path)?;
    } else {
        fs::remove_file(&path)?;
    }
    if let Some(id) = id {
        let mut list = ModList::load(dir);
        list.enabled.remove(&id);
        list.disabled.remove(&id);
        list.keys.remove(&id);
        list.save(dir);
    }
    MODS_DIRTY.store(true, Ordering::Relaxed);
    Ok(json!({ "ok": true, "file": file }))
}

/// Installs the staged `.hmod` and answers the row it makes: validated, then moved into
/// `horizon-mods/<id>.hmod`. An update keeps the mod's switch; a fresh content mod comes on. A code
/// mod installs too, but stays off: this build can only load content.
pub(super) fn install(dir: &Path, staged: &Path) -> std::result::Result<Value, String> {
    let result = install_inner(dir, staged);
    if result.is_err() {
        // Only the launcher's own copy, inside the game folder, is ours to remove.
        if let (Ok(base), Ok(file)) = (fs::canonicalize(dir), fs::canonicalize(staged))
            && file.starts_with(&base)
        {
            let _ = fs::remove_file(file);
        }
    }
    result
}

fn install_inner(dir: &Path, staged: &Path) -> std::result::Result<Value, String> {
    let meta = fs::symlink_metadata(staged).map_err(|e| format!("{}: {e}", staged.display()))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(format!(
            "{}: a link or folder, not a mod file",
            staged.display()
        ));
    }
    if !staged.extension().is_some_and(|e| e == "hmod") {
        return Err(format!("{}: a mod is a .hmod file", staged.display()));
    }
    // The staged copy and the game folder as the system resolves them (/var and /private/var are one).
    let base = fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let staged = fs::canonicalize(staged).map_err(|e| e.to_string())?;
    if !staged.starts_with(&base) {
        return Err(format!(
            "{} is outside the game folder: install it from the Files app into Horizon's own folder",
            staged.display()
        ));
    }
    let package = Package::read(&staged)?;
    if package.manifest.side == Side::Server {
        return Err(SERVER.into());
    }
    Content::read(&staged, &package.manifest)?;
    let folder = dir.join(mod_list::DIR);
    fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let id = package.manifest.id.clone();
    let target = folder.join(format!("{id}.hmod"));
    // An update keeps the switch it had; a fresh content mod comes on by itself.
    let mut replacing = target.exists();
    // Another file with this id is the same mod under an old name: the update replaces it. A folder
    // is the player's own mod, not ours to touch.
    for path in find(&folder) {
        if path == target {
            continue;
        }
        let Ok(other) = Package::read(&path) else {
            continue;
        };
        if other.manifest.id != id {
            continue;
        }
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.is_dir() {
            return Err(format!(
                "{} is a folder mod with the id {id}: remove it first",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
        if meta.file_type().is_symlink() {
            return Err(format!("{}: a link, not a mod", path.display()));
        }
        fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        replacing = true;
    }
    fs::rename(&staged, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    let mut list = ModList::load(dir);
    let was_on = list.enabled.contains(&id);
    let on = if replacing {
        was_on
    } else {
        package.wasm.is_none()
    };
    if on && !was_on {
        list.enabled.insert(id.clone());
        list.save(dir);
    }
    MODS_DIRTY.store(true, Ordering::Relaxed);
    Ok(json!({
        "ok": true,
        "id": id,
        "file": format!("{id}.hmod"),
        "enabled": on,
    }))
}

/// The package in `folder` whose manifest is `id`.
fn package_with_id(folder: &Path, id: &str) -> std::result::Result<Option<Package>, String> {
    for path in find(folder) {
        if let Ok(package) = Package::read(&path)
            && package.manifest.id == id
        {
            return Ok(Some(package));
        }
    }
    Ok(None)
}

/// The settings a launcher holds, with its mod list read again: the rows it shows stay, the on/off
/// state and keys are the files'. Called after a mods command, before the next Play loads.
pub(super) fn reload(settings: &mut Settings, dir: &Path) {
    let mut list = ModList::load(dir);
    list.installed = std::mem::take(&mut settings.mods.installed);
    settings.mods = list;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "horizon-mods-control-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn content_mod(id: &str) -> Vec<u8> {
        let manifest = format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"1.0\"\napi = \"0.2\"\ncapabilities = [\"registries\"]\n"
        );
        Package::write_files(&manifest, None, &[(&format!("assets/{id}/x.txt"), b"x")]).unwrap()
    }

    fn stage(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let folder = dir.join(".horizon-import");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn installs_lists_switches_and_deletes() {
        let dir = temp("roundtrip");
        let staged = stage(&dir, "one.hmod", &content_mod("one"));
        let installed = install(&dir, &staged).unwrap();
        assert_eq!(installed["id"], "one");
        assert_eq!(installed["enabled"], true);
        assert!(dir.join("horizon-mods/one.hmod").is_file());
        assert!(!staged.exists());

        let listed = command(&dir, "mods-list").unwrap().unwrap();
        assert_eq!(listed["mods"][0]["id"], "one");
        assert_eq!(listed["mods"][0]["enabled"], true);
        assert_eq!(listed["mods"][0]["problem"], Value::Null);
        assert!(listed["folder"].as_str().unwrap().ends_with("horizon-mods"));

        command(&dir, r#"mods-set {"id":"one","on":false}"#).unwrap();
        let listed = command(&dir, "mods-list").unwrap().unwrap();
        assert_eq!(listed["mods"][0]["enabled"], false);
        command(&dir, r#"mods-set {"id":"one","on":true}"#).unwrap();

        // Another mod's switch and this one's keys stay when this one goes.
        let mut list = ModList::load(&dir);
        list.enabled.insert("other".into());
        list.keys
            .entry("one".into())
            .or_default()
            .insert("pick".into(), "key.keyboard.x".into());
        list.keys
            .entry("other".into())
            .or_default()
            .insert("pick".into(), "key.keyboard.y".into());
        list.save(&dir);
        command(&dir, r#"mods-delete {"file":"one.hmod"}"#).unwrap();
        assert!(!dir.join("horizon-mods/one.hmod").exists());
        let text = fs::read_to_string(dir.join("horizon-mods.txt")).unwrap();
        assert!(text.contains("mod.other:true"));
        assert!(text.contains("key.other.pick:key.keyboard.y"));
        assert!(!text.contains("mod.one"));
        assert!(!text.contains("key.one."));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_paths_outside_the_game_folder_and_files_that_are_not_hmods() {
        let dir = temp("refuse");
        let outside = std::env::temp_dir().join(format!(
            "horizon-outside-{}-{}.hmod",
            std::process::id(),
            "path"
        ));
        fs::write(&outside, content_mod("out")).unwrap();
        let error = install(&dir, &outside).unwrap_err();
        assert!(error.contains("outside the game folder"), "{error}");
        // Not ours to delete: the player's file is still there.
        assert!(outside.exists());
        fs::remove_file(&outside).unwrap();

        let staged = stage(&dir, "one.zip", &content_mod("zip"));
        let error = install(&dir, &staged).unwrap_err();
        assert!(error.contains(".hmod"), "{error}");
        assert!(!staged.exists());

        // A server mod belongs to a server, not to this game.
        let server =
            "id = \"server\"\nname = \"server\"\nversion = \"1.0\"\napi = \"0.3\"\nside = \"server\"\n";
        let bytes = Package::write_files(server, Some(b"\0"), &[]).unwrap();
        let staged = stage(&dir, "server.hmod", &bytes);
        let error = install(&dir, &staged).unwrap_err();
        assert_eq!(error, SERVER);

        // Code installs, but comes on only if it was on: this build can't run the code.
        let code = "id = \"code\"\nname = \"code\"\nversion = \"1.0\"\napi = \"0.1\"\ncapabilities = [\"hud\"]\n";
        let bytes = Package::write_files(code, Some(b"\0"), &[]).unwrap();
        let staged = stage(&dir, "code.hmod", &bytes);
        let installed = install(&dir, &staged).unwrap();
        assert_eq!(installed["enabled"], false);
        let listed = command(&dir, "mods-list").unwrap().unwrap();
        assert_eq!(listed["mods"][0]["code"], true);
        assert_eq!(listed["mods"][0]["problem"], CODE);
        assert!(command(&dir, r#"mods-set {"id":"code","on":true}"#).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_update_replaces_a_file_of_the_id_and_a_folder_duplicate_is_refused() {
        let dir = temp("duplicate");
        let staged = stage(&dir, "one.hmod", &content_mod("dupe"));
        install(&dir, &staged).unwrap();
        // Turned off, an update stays off: only a fresh install comes on by itself.
        command(&dir, r#"mods-set {"id":"dupe","on":false}"#).unwrap();
        // The same id under another file name: an update replaces it.
        fs::write(dir.join("horizon-mods/other.hmod"), content_mod("dupe")).unwrap();
        let staged = stage(&dir, "new.hmod", &content_mod("dupe"));
        let installed = install(&dir, &staged).unwrap();
        assert_eq!(installed["file"], "dupe.hmod");
        assert_eq!(installed["enabled"], false);
        assert!(!dir.join("horizon-mods/other.hmod").exists());
        assert!(dir.join("horizon-mods/dupe.hmod").is_file());

        // A folder is the player's own mod: the install says which one.
        let folder = dir.join("horizon-mods/handmade");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join(MANIFEST),
            "id = \"dupe\"\nname = \"dupe\"\nversion = \"1.0\"\napi = \"0.2\"\ncapabilities = [\"registries\"]\n",
        )
        .unwrap();
        let staged = stage(&dir, "again.hmod", &content_mod("dupe"));
        let error = install(&dir, &staged).unwrap_err();
        assert!(error.contains("folder mod"), "{error}");
        assert!(!staged.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn delete_takes_only_installed_mods_inside_the_folder() {
        let dir = temp("delete");
        let staged = stage(&dir, "one.hmod", &content_mod("one"));
        install(&dir, &staged).unwrap();
        fs::write(dir.join("horizon-mods/notes.txt"), "not a mod").unwrap();
        for line in [
            r#"mods-delete {"file":"notes.txt"}"#,
            r#"mods-delete {"file":"../../horizon-mods.txt"}"#,
            r#"mods-delete {"file":"missing.hmod"}"#,
        ] {
            assert!(command(&dir, line).is_err(), "{line}");
        }
        fs::create_dir_all(dir.join("horizon-mods/not-a-mod")).unwrap();
        assert!(command(&dir, r#"mods-delete {"file":"not-a-mod"}"#).is_err());
        assert!(dir.join("horizon-mods/not-a-mod").is_dir());
        assert!(dir.join("horizon-mods/one.hmod").is_file());
        fs::remove_dir_all(&dir).unwrap();
    }
}
