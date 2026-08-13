use std::{
    env,
    ffi::{OsStr, OsString},
    fmt,
    io::{Cursor, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, bail, Result};
use console::Style;
use indicatif::{style::TemplateError, ProgressBar, ProgressStyle};
use log::debug;
use modio::{
    types::{
        id::{FileId, ModId},
        mods::Mod,
        TargetPlatform,
    },
    util::download::{Download, DownloadAction},
    Client,
};
use tokio::{
    fs::{remove_dir_all, remove_file, symlink_metadata, try_exists},
    task::spawn_blocking,
};
use zip::{read::root_dir_common_filter, ZipArchive};

use crate::app_data::InstalledMod;

#[derive(Clone, Copy)]
pub(crate) enum ModInstallationState {
    Checking,
    Downloading,
    Installing,
    Updating,
    Installed,
    Updated,
    AlreadyInstalled,
    Failed,
}

impl fmt::Display for ModInstallationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Checking => "Checking",
                Self::Downloading => "Downloading",
                Self::Installing => "Installing",
                Self::Updating => "Updating",
                Self::Installed => "Installed",
                Self::Updated => "Updated",
                Self::AlreadyInstalled => "Already installed",
                Self::Failed => "Failed",
            }
        )
    }
}

/// What a finished task wants recorded in the app data.
///
/// Tasks report back rather than writing the app data themselves, so the
/// concurrent installs can't overwrite each other's bookkeeping.
pub(crate) enum ModInstallationOutcome {
    Installed(ModId, InstalledMod),
    Updated(ModId, InstalledMod),
    AlreadyInstalled,
    Failed,
}

impl ModInstallationOutcome {
    fn state(&self) -> ModInstallationState {
        match self {
            Self::Installed(..) => ModInstallationState::Installed,
            Self::Updated(..) => ModInstallationState::Updated,
            Self::AlreadyInstalled => ModInstallationState::AlreadyInstalled,
            Self::Failed => ModInstallationState::Failed,
        }
    }
}

struct ModInstallation {
    progress_bar: ProgressBar,
    state: ModInstallationState,
    bytes: u64,
    total_bytes: u64,
    name: String,
}

impl ModInstallation {
    pub(crate) fn new(name: String, progress_bar: ProgressBar) -> Result<Self, TemplateError> {
        let mut new_self = Self {
            progress_bar: progress_bar
                .with_message(name.clone())
                .with_style(Self::indeterminate_style()?),
            state: ModInstallationState::Checking,
            bytes: 0,
            total_bytes: 0,
            name,
        };

        new_self.update_state(new_self.state)?;

        debug!("create new `ModInstallation`");

        Ok(new_self)
    }

    fn update_state(&mut self, state: ModInstallationState) -> Result<(), TemplateError> {
        let doing_style = Style::new().bold().cyan();
        let done_style = Style::new().bold().green();
        let didnt_style = Style::new().bold().red();
        let state_string = state.to_string();

        self.state = state;
        debug!("set internal mod installation state");

        self.progress_bar.set_style(match state {
            ModInstallationState::Downloading => Self::bar_style()?,
            ModInstallationState::Failed => Self::error_style()?,
            _ => Self::indeterminate_style()?,
        });
        debug!("set mod installation style");

        self.progress_bar.set_prefix(match state {
            ModInstallationState::Checking
            | ModInstallationState::Downloading
            | ModInstallationState::Installing
            | ModInstallationState::Updating => doing_style.apply_to(state_string).to_string(),
            ModInstallationState::Installed
            | ModInstallationState::Updated
            | ModInstallationState::AlreadyInstalled => {
                done_style.apply_to(state_string).to_string()
            }
            ModInstallationState::Failed => didnt_style.apply_to(state_string).to_string(),
        });
        debug!("set mod installation prefix");

        match state {
            ModInstallationState::Installed
            | ModInstallationState::Updated
            | ModInstallationState::AlreadyInstalled
            | ModInstallationState::Failed => {
                self.progress_bar.finish();
                debug!("finished mod installation");
            }
            _ => (),
        }

        debug!("updated mod installation state");

        Ok(())
    }

    fn increment_bytes(&mut self, bytes: u64) {
        self.bytes += bytes;
        self.progress_bar.inc(bytes);
    }

    fn update_total_bytes(&mut self, total_bytes: u64) {
        self.total_bytes = total_bytes;
        self.progress_bar.set_length(total_bytes);
    }

    fn fail(&mut self, msg: impl fmt::Display) -> Result<(), TemplateError> {
        self.progress_bar
            .set_message(format!("{}: {msg}", self.name));
        self.update_state(ModInstallationState::Failed)?;
        debug!("failed mod installation");

        Ok(())
    }

    fn indeterminate_style() -> Result<ProgressStyle, TemplateError> {
        ProgressStyle::with_template("{prefix:>17} {wide_msg}")
    }

    fn error_style() -> Result<ProgressStyle, TemplateError> {
        ProgressStyle::with_template("{prefix:>17} {msg}")
    }

    fn bar_style() -> Result<ProgressStyle, TemplateError> {
        Ok(ProgressStyle::with_template(
            "{prefix:>17} [{bar:17}] {bytes:>11} / {total_bytes:>11}: {wide_msg}",
        )?
        .progress_chars("=> "))
    }
}

/// Deletes what a previous run installed for a mod.
///
/// A missing path is treated as success: the user may well have deleted it by
/// hand, and there is nothing left to do either way.
pub(crate) async fn remove_installed_mod(
    mods_dir: &Path,
    game_dir: Option<&Path>,
    installed_mod: &InstalledMod,
) -> Result<()> {
    // A pack put down several folders and all of them have to go, or
    // unsubscribing would leave most of it behind. A code mod put its files
    // beside the game instead, and leaving those would keep it loading.
    let in_mods_dir = installed_mod
        .folders
        .iter()
        .map(|folder| mods_dir.join(folder));
    let beside_the_game = installed_mod
        .game_files
        .iter()
        .filter_map(|file| game_dir.map(|dir| dir.join(file)));

    for path in in_mods_dir.chain(beside_the_game) {
        debug!("removing \"{}\"", path.display());

        let metadata = match symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                debug!("nothing to remove");

                continue;
            }
            Err(err) => return Err(err.into()),
        };

        // Content mods are directories; code mods are a single `.dll`.
        if metadata.is_dir() {
            remove_dir_all(&path).await?;
        } else {
            remove_file(&path).await?;
        }
    }

    Ok(())
}

/// Picks the mod file built for `target_platform`.
///
/// Mods are published per platform and the builds genuinely differ, so handing
/// a Quest user the Windows file would give them something Bonelab cannot load.
///
/// `modfile` is only a fallback, and a poor one: mod.io fills it in with the
/// most recent upload regardless of the target platform header, so it is quite
/// capable of naming an Android build for a Windows install. It is used anyway
/// because the alternative is refusing a mod that lists no platforms at all.
fn file_id_for(r#mod: &Mod, target_platform: TargetPlatform) -> Result<FileId> {
    r#mod
        .platforms
        .iter()
        .find(|platform| platform.target == target_platform)
        .map(|platform| platform.modfile_id)
        .or_else(|| r#mod.modfile.as_ref().map(|modfile| modfile.id))
        .ok_or(anyhow!("Mod does not have a {target_platform} mod file"))
}

/// Turns a failure into something worth reading.
///
/// Most of these need no help, but a raw HTTP status does not tell anyone what
/// went wrong or whether it is their problem. The common one is a mod that is
/// still in someone's subscriptions after its author hid or removed it:
/// subscriptions survive that, downloads do not.
fn explain(err: &anyhow::Error) -> String {
    // Downloading wraps the mod.io error in one of its own, so the interesting
    // one is somewhere down the chain rather than at the top.
    let Some(modio_err) = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<modio::Error>())
    else {
        return format!("{err:#}");
    };

    if modio_err.is_ratelimited() {
        return "mod.io is rate limiting this account. Try again in a while, or set \
                BMM_CONCURRENT_DOWNLOADS lower."
            .to_string();
    }

    match modio_err.status().map(|status| status.as_u16()) {
        Some(403 | 404) => {
            "no longer available to download. Its author has probably hidden or removed it, \
             which does not unsubscribe you. Unsubscribe on mod.io to stop it being retried."
                .to_string()
        }
        _ => format!("{err:#}"),
    }
}

pub(crate) async fn install_mod(
    r#mod: Mod,
    progress_bar: ProgressBar,
    client: Arc<Client>,
    target_platform: TargetPlatform,
    mods_dir: PathBuf,
    game_dir: Option<PathBuf>,
    installed_mod: Option<InstalledMod>,
) -> Result<ModInstallationOutcome> {
    let mut mod_installation = ModInstallation::new(r#mod.name.clone(), progress_bar)?;

    match _install_mod(
        r#mod,
        &mut mod_installation,
        client,
        target_platform,
        mods_dir,
        game_dir,
        installed_mod,
    )
    .await
    {
        Ok(outcome) => {
            mod_installation.update_state(outcome.state())?;
            debug!("mod installation was okay");

            Ok(outcome)
        }
        Err(err) => {
            let mut msg = explain(&err);

            if let Ok(backtrace) = env::var("RUST_BACKTRACE") {
                if backtrace == "1" {
                    msg += &format!("\n{}", err.backtrace());
                }
            }

            mod_installation.fail(msg)?;
            debug!("mod installation was notokay");

            Ok(ModInstallationOutcome::Failed)
        }
    }
}

async fn _install_mod(
    r#mod: Mod,
    mod_installation: &mut ModInstallation,
    client: Arc<Client>,
    target_platform: TargetPlatform,
    mods_dir: PathBuf,
    game_dir: Option<PathBuf>,
    installed_mod: Option<InstalledMod>,
) -> Result<ModInstallationOutcome> {
    let mod_id = r#mod.id;
    // mod.io reports timestamps as signed seconds since the epoch; nothing on
    // mod.io predates 1970, so anything negative is nonsense we can floor.
    let date_updated = r#mod.date_updated.as_secs().max(0) as u64;

    let updating = match &installed_mod {
        Some(installed_mod) => {
            debug!("mod has an installation on record");

            // The record is only a cache of what is on disk, so a mod that has
            // since been deleted by hand needs laying down again however recent
            // the record claims it is. A pack counts as present only if all of
            // its folders are, since a half deleted pack is still incomplete.
            let laid_down: Vec<_> = installed_mod
                .folders
                .iter()
                .map(|folder| mods_dir.join(folder))
                .chain(
                    installed_mod
                        .game_files
                        .iter()
                        .filter_map(|file| game_dir.as_ref().map(|dir| dir.join(file))),
                )
                .collect();
            let mut present = !laid_down.is_empty();

            for path in laid_down {
                if !try_exists(path).await? {
                    present = false;

                    break;
                }
            }

            if present && installed_mod.date_updated >= date_updated {
                return Ok(ModInstallationOutcome::AlreadyInstalled);
            }

            debug!("mod needs to be installed again");

            present
        }
        None => false,
    };

    let file_id = file_id_for(&r#mod, target_platform)?;

    debug!("got mod file id");

    let mut downloader = client
        .download(DownloadAction::File {
            game_id: r#mod.game_id,
            mod_id,
            file_id,
        })
        .chunked()
        .await?;

    debug!("created mod downloader");
    mod_installation.update_state(ModInstallationState::Downloading)?;

    let filesize = downloader.info().filesize;

    mod_installation.update_total_bytes(filesize);

    let mut bytes = Vec::with_capacity(filesize as usize);

    while let Some(chunk) = downloader.data().await {
        let chunk = chunk?;

        mod_installation.increment_bytes(chunk.len() as u64);
        bytes.extend_from_slice(&chunk);
    }

    debug!("received all chunks");
    mod_installation.update_state(if updating {
        ModInstallationState::Updating
    } else {
        ModInstallationState::Installing
    })?;

    // Clear the old copy first so files dropped by the new version don't
    // survive the update.
    if let Some(installed_mod) = &installed_mod {
        remove_installed_mod(&mods_dir, game_dir.as_deref(), installed_mod).await?;
    }

    let installed =
        spawn_blocking(move || extract_mod(bytes, &mods_dir, game_dir.as_deref(), target_platform))
            .await??;

    debug!(
        "extracted {} folder(s) and {} file(s) beside the game",
        installed.folders.len(),
        installed.game_files.len(),
    );

    let installed_mod = InstalledMod {
        date_updated,
        folders: installed.folders,
        game_files: installed.game_files,
    };

    Ok(if updating {
        ModInstallationOutcome::Updated(mod_id, installed_mod)
    } else {
        ModInstallationOutcome::Installed(mod_id, installed_mod)
    })
}

/// Unpacks a mod archive into `mods_dir` and reports the folders it created.
///
/// Bonelab loads a mod from an `Author.ModName` directory holding a pallet
/// manifest, named either `pallet.json` or after the mod itself. An archive may
/// hold several such directories, a pack of mods published as one download, and
/// all of them get installed. What it may not do is leave files loose at the
/// root, which would scatter them across the Mods folder; that is checked before
/// extracting anything, so a bad archive leaves the Mods folder untouched.
/// What a mod put on disk, so that removing it later can undo exactly that.
#[derive(Default, Debug)]
pub(crate) struct Installed {
    pub(crate) folders: Vec<OsString>,
    pub(crate) game_files: Vec<OsString>,
}

/// The directories a code mod archive mirrors into the game directory.
///
/// MelonLoader reads assemblies from `Mods` and `Plugins`, and mods keep their
/// settings in `UserData`. Anything else in the archive, a readme or an icon or
/// a Thunderstore manifest, is packaging rather than the mod.
const GAME_DIRS: [&str; 3] = ["Mods", "Plugins", "UserData"];

fn extract_mod(
    bytes: Vec<u8>,
    mods_dir: &Path,
    game_dir: Option<&Path>,
    target_platform: TargetPlatform,
) -> Result<Installed> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let folders = pallet_folders(&mut archive, target_platform)?;

    // Nothing holding a pallet means this is not content at all. A code mod is
    // an assembly MelonLoader loads from the game directory, which Bonelab
    // never looks at.
    if folders.is_empty() {
        return extract_code_mod(&mut archive, game_dir);
    }

    for (source, name) in &folders {
        extract_folder(&mut archive, source, &mods_dir.join(name))?;
    }

    Ok(Installed {
        folders: folders.into_iter().map(|(_, name)| name).collect(),
        game_files: Vec::new(),
    })
}

/// Every directory in an archive that holds a pallet, and the name it should
/// have once installed.
///
/// Bonelab only looks one level into its Mods directory, so where the pallet
/// sits in the archive and where it has to end up are different questions. Some
/// downloads are packs, several mods side by side. Others wrap the mod in an
/// extra folder, often to offer a PC and a Quest copy in one file, and those
/// have to be lifted out of the wrapper or the game never sees them. Keying off
/// the pallet rather than the layout covers both without caring about depth.
fn pallet_folders(
    archive: &mut ZipArchive<Cursor<Vec<u8>>>,
    target_platform: TargetPlatform,
) -> Result<Vec<(PathBuf, OsString)>> {
    let mut holding_a_pallet: Vec<PathBuf> = Vec::new();

    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let path = file
            .enclosed_name()
            .ok_or(anyhow!("Mod file archive contains an unsafe path"))?;

        // Skips the likes of `__MACOSX` and `.DS_Store`, which are not part of
        // the mod and would otherwise look like a mod of their own.
        if !root_dir_common_filter(&path) {
            continue;
        }

        let Some(name) = path.file_name().map(|name| name.to_string_lossy()) else {
            continue;
        };

        // Mods in the wild use either a plain `pallet.json` or one named after
        // the mod, like `Aubies12.Bean.pallet.json`.
        if name != "pallet.json" && !name.ends_with(".pallet.json") {
            continue;
        }

        // A pallet at the very root has no folder to become, and extracting it
        // would scatter the mod across the Mods directory.
        let directory = path
            .parent()
            .filter(|parent| parent.components().next().is_some())
            .ok_or(anyhow!(
                "Mod file archive has \"{}\" loose at its root instead of inside a mod folder,                  so it is not laid out the way Bonelab expects",
                path.display(),
            ))?
            .to_path_buf();

        if !holding_a_pallet.contains(&directory) {
            holding_a_pallet.push(directory);
        }
    }

    // Dropping the copy meant for the other platform, unless that would leave
    // nothing at all, which would mean the guess was wrong.
    let wanted: Vec<_> = holding_a_pallet
        .iter()
        .filter(|path| !names_the_other_platform(path, target_platform))
        .cloned()
        .collect();
    let holding_a_pallet = if wanted.is_empty() {
        holding_a_pallet
    } else {
        wanted
    };
    let mut folders: Vec<(PathBuf, OsString)> = Vec::new();

    for directory in holding_a_pallet {
        let Some(name) = directory.file_name().map(OsStr::to_os_string) else {
            continue;
        };

        // A PC and a Quest copy of one mod install to the same place, so only
        // the first can win however the platform guess went.
        if folders.iter().any(|(_, installed)| installed == &name) {
            continue;
        }

        folders.push((directory, name));
    }

    Ok(folders)
}

/// Whether any part of a path names the platform being installed away from.
///
/// Authors put a PC and a Quest build of the same mod in one download, in
/// folders named for each, so the names are all there is to go on. Matching
/// whole words keeps `PC` from being found inside an unrelated name.
fn names_the_other_platform(path: &Path, target_platform: TargetPlatform) -> bool {
    let other: &[&str] = if target_platform == TargetPlatform::WINDOWS {
        &["quest", "android"]
    } else {
        &["pc", "pcvr", "windows"]
    };

    path.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .split(|character: char| !character.is_alphanumeric())
            .any(|word| other.contains(&word.to_lowercase().as_str()))
    })
}

/// Unpacks everything under `source` into `destination`, flattening away
/// whatever the archive wrapped it in.
fn extract_folder(
    archive: &mut ZipArchive<Cursor<Vec<u8>>>,
    source: &Path,
    destination: &Path,
) -> Result<()> {
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;

        if file.is_dir() {
            continue;
        }

        let Some(path) = file.enclosed_name() else {
            bail!("Mod file archive contains an unsafe path");
        };
        let Ok(within) = path.strip_prefix(source) else {
            continue;
        };

        write_entry(&mut file, &destination.join(within))?;
    }

    Ok(())
}

/// Writes one archive entry, making the directories above it first.
fn write_entry(source: &mut impl std::io::Read, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::io::copy(source, &mut std::fs::File::create(destination)?)?;
    debug!("wrote \"{}\"", destination.display());

    Ok(())
}

/// Lays a code mod out beside the game the way MelonLoader expects to find it.
///
/// Archives come both flat and wrapped in a folder named after the release, so
/// placement keys off where `Mods`, `Plugins` or `UserData` appears in each path
/// rather than the depth it appears at. An assembly with none of those above it
/// is taken to belong in `Mods`, which is where a bare `.dll` download goes.
fn extract_code_mod(
    archive: &mut ZipArchive<Cursor<Vec<u8>>>,
    game_dir: Option<&Path>,
) -> Result<Installed> {
    let mut planned: Vec<(usize, PathBuf)> = Vec::new();

    for index in 0..archive.len() {
        let file = archive.by_index(index)?;

        if file.is_dir() {
            continue;
        }

        let Some(path) = file.enclosed_name() else {
            bail!("Mod file archive contains an unsafe path");
        };

        match game_relative_path(&path) {
            Some(relative) => planned.push((index, relative)),
            None => debug!(
                "skipping \"{}\", packaging rather than the mod",
                path.display()
            ),
        }
    }

    // Worked out before the game directory is asked for, so an archive with
    // nothing loadable in it is reported as that rather than as a missing
    // setting the user would then go and set for no reason.
    if planned.is_empty() {
        bail!("Mod file archive has nothing in it that Bonelab or MelonLoader could load");
    }

    let Some(game_dir) = game_dir else {
        bail!(
            "this is a code mod, which goes beside the game rather than in the mods folder.              Set BMM_GAME_DIR to the folder holding BONELAB_Steam_Windows64.exe to install it"
        );
    };
    let mut installed = Installed::default();

    for (index, relative) in planned {
        write_entry(&mut archive.by_index(index)?, &game_dir.join(&relative))?;
        installed.game_files.push(relative.into_os_string());
    }

    Ok(installed)
}

/// Where an archive entry belongs relative to the game directory, if anywhere.
fn game_relative_path(path: &Path) -> Option<PathBuf> {
    let components: Vec<_> = path.components().collect();

    for (index, component) in components.iter().enumerate() {
        if GAME_DIRS.contains(&component.as_os_str().to_string_lossy().as_ref()) {
            return Some(components[index..].iter().collect());
        }
    }

    // A loose assembly is what a mod published as a bare `.dll` looks like.
    if path.extension().is_some_and(|extension| extension == "dll") {
        return Some(Path::new("Mods").join(path.file_name()?));
    }

    None
}

#[cfg(test)]
mod tests {
    use std::{env, io::Write, sync::Arc};

    use indicatif::ProgressBar;
    use modio::{
        types::{
            id::{GameId, ModId},
            TargetPlatform,
        },
        Client,
    };
    use zip::{write::SimpleFileOptions, ZipWriter};

    use crate::BONELAB_GAME_ID;

    use super::{
        _install_mod, explain, extract_mod, file_id_for, Cursor, Download, DownloadAction, FileId,
        InstalledMod, Mod, ModInstallation, ModInstallationOutcome, OsString, Path,
    };

    /// A small mod whose Windows and Android files have different ids, so
    /// picking the wrong platform's file would fetch different bytes.
    const LIVE_MOD_ID: u64 = 6297147;

    /// A name ending in a slash becomes a directory entry, which real archives
    /// carry and which must not be mistaken for a file loose at the root.
    fn archive(entries: &[&str]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));

        for entry in entries {
            if let Some(dir) = entry.strip_suffix('/') {
                writer
                    .add_directory(dir, SimpleFileOptions::default())
                    .unwrap();

                continue;
            }

            writer
                .start_file(*entry, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"{}").unwrap();
        }

        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_a_mod_and_reports_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&[
            "Author.ModName/Author.ModName.pallet.json",
            "Author.ModName/data/thing.bin",
        ]);

        let folders = extract_mod(bytes, dir.path(), None, TargetPlatform::WINDOWS)
            .unwrap()
            .folders;

        assert_eq!(folders, vec![OsString::from("Author.ModName")]);
        assert!(dir
            .path()
            .join("Author.ModName")
            .join("Author.ModName.pallet.json")
            .exists());
    }

    /// Some downloads are packs: several complete mods in one archive, each
    /// with its own pallet. Bonelab loads them independently, so all of them
    /// are installed and all of them are recorded.
    #[test]
    fn installs_every_mod_in_a_pack() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&[
            "BamBaeYoh.ZombieWeapons/",
            "BamBaeYoh.ZombieWeapons/pallet.json",
            "Pulvox.Dempsy/",
            "Pulvox.Dempsy/pallet.json",
            "Pulvox.Nikolai/",
            "Pulvox.Nikolai/pallet.json",
        ]);

        let folders = extract_mod(bytes, dir.path(), None, TargetPlatform::WINDOWS)
            .unwrap()
            .folders;

        assert_eq!(
            folders,
            vec![
                OsString::from("BamBaeYoh.ZombieWeapons"),
                OsString::from("Pulvox.Dempsy"),
                OsString::from("Pulvox.Nikolai"),
            ],
        );

        for folder in &folders {
            assert!(
                dir.path().join(folder).join("pallet.json").exists(),
                "{folder:?} did not make it out of the archive",
            );
        }
    }

    /// Authors wrap a mod in an extra folder, often named "OpenMe" or after the
    /// platform. Bonelab only looks one level down, so a wrapped mod is invisible
    /// to it and has to be lifted out.
    #[test]
    fn lifts_a_mod_out_of_the_folder_it_was_wrapped_in() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&[
            "OpenMe/",
            "OpenMe/Seikatsu.Rebecca1/pallet.json",
            "OpenMe/Seikatsu.Rebecca1/data.bin",
            "OpenMe/Seikatsu.Rebecca2/pallet.json",
        ]);

        let installed = extract_mod(bytes, dir.path(), None, TargetPlatform::WINDOWS).unwrap();

        assert_eq!(
            installed.folders,
            vec![
                OsString::from("Seikatsu.Rebecca1"),
                OsString::from("Seikatsu.Rebecca2"),
            ],
        );
        assert!(
            !dir.path().join("OpenMe").exists(),
            "the wrapper was installed instead of being flattened away",
        );
        assert!(dir
            .path()
            .join("Seikatsu.Rebecca1")
            .join("pallet.json")
            .exists());
        assert!(dir
            .path()
            .join("Seikatsu.Rebecca1")
            .join("data.bin")
            .exists());
        assert!(dir
            .path()
            .join("Seikatsu.Rebecca2")
            .join("pallet.json")
            .exists());
    }

    /// One download often carries both builds, in folders named for each. They
    /// install to the same name, so taking both would leave whichever landed
    /// last, which is a coin toss over whether the game can load it.
    #[test]
    fn takes_only_the_build_for_the_platform_being_installed() {
        for (target, expected) in [
            (TargetPlatform::WINDOWS, "pc"),
            (TargetPlatform::ANDROID, "quest"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let bytes = archive(&[
                "PC [OPEN]/RRRyu.Kobeni/pallet.json",
                "PC [OPEN]/RRRyu.Kobeni/which.txt",
                "Quest [OPEN]/RRRyu.Kobeni/pallet.json",
                "Quest [OPEN]/RRRyu.Kobeni/which.txt",
            ]);

            let installed = extract_mod(bytes, dir.path(), None, target).unwrap();

            assert_eq!(
                installed.folders,
                vec![OsString::from("RRRyu.Kobeni")],
                "{expected}: both builds were installed under the same name",
            );
            assert!(dir.path().join("RRRyu.Kobeni").join("pallet.json").exists());
        }
    }

    /// Guessing by name is only a guess. An archive whose only copy happens to
    /// mention the other platform still has to install, since mod.io already
    /// served the file for this one.
    #[test]
    fn installs_a_lone_build_even_if_its_name_mentions_the_other_platform() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&["Havoc (PC)/HavocV2.Galaxy/pallet.json"]);

        let installed = extract_mod(bytes, dir.path(), None, TargetPlatform::ANDROID).unwrap();

        assert_eq!(installed.folders, vec![OsString::from("HavocV2.Galaxy")]);
        assert!(dir
            .path()
            .join("HavocV2.Galaxy")
            .join("pallet.json")
            .exists());
    }

    /// A code mod is loaded by MelonLoader from beside the game, so it has to be
    /// laid out there rather than in the folder Bonelab reads pallets from.
    #[test]
    fn installs_a_code_mod_beside_the_game() {
        let mods = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir().unwrap();
        // The shape Fusion ships in: a wrapper folder, packaging files that are
        // not the mod, and the assembly under `Mods`.
        let bytes = archive(&[
            "Lakatrazz-Fusion-1.14.2/manifest.json",
            "Lakatrazz-Fusion-1.14.2/README.md",
            "Lakatrazz-Fusion-1.14.2/icon.png",
            "Lakatrazz-Fusion-1.14.2/Mods/LabFusion.dll",
        ]);

        let installed = extract_mod(
            bytes,
            mods.path(),
            Some(game.path()),
            TargetPlatform::WINDOWS,
        )
        .unwrap();

        assert!(
            installed.folders.is_empty(),
            "a code mod should put nothing in the pallets folder",
        );
        assert_eq!(
            installed.game_files,
            vec![Path::new("Mods").join("LabFusion.dll").into_os_string()],
        );
        assert!(game.path().join("Mods").join("LabFusion.dll").exists());
        assert!(
            !mods.path().join("Lakatrazz-Fusion-1.14.2").exists(),
            "the packaging folder was copied into the pallets folder",
        );
        assert!(
            !game.path().join("README.md").exists(),
            "packaging files should not be strewn beside the game",
        );
    }

    /// A mod published as a bare assembly is what MelonLoader's own docs
    /// describe, and belongs in `Mods` even though the archive says nothing.
    #[test]
    fn a_loose_assembly_goes_into_the_games_mods_folder() {
        let mods = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir().unwrap();
        let bytes = archive(&["SomeCodeMod.dll"]);

        let installed = extract_mod(
            bytes,
            mods.path(),
            Some(game.path()),
            TargetPlatform::WINDOWS,
        )
        .unwrap();

        assert_eq!(
            installed.game_files,
            vec![Path::new("Mods").join("SomeCodeMod.dll").into_os_string()],
        );
        assert!(game.path().join("Mods").join("SomeCodeMod.dll").exists());
    }

    /// Without somewhere to put it, saying so beats installing it where nothing
    /// will ever load it, which is what used to happen.
    #[test]
    fn a_code_mod_with_nowhere_to_go_says_so() {
        let mods = tempfile::tempdir().unwrap();
        let err = extract_mod(
            archive(&["Whatever/Mods/Thing.dll"]),
            mods.path(),
            None,
            TargetPlatform::WINDOWS,
        )
        .expect_err("a code mod was installed with no game directory set");

        assert!(
            format!("{err:#}").contains("BMM_GAME_DIR"),
            "the error does not say how to fix it: {err:#}",
        );
    }

    /// Junk a packaging tool left behind is not a mod, and counting it as one
    /// would leave an empty folder in the Mods directory forever.
    #[test]
    fn ignores_archive_junk_when_working_out_the_folders() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&[
            "Author.ModName/pallet.json",
            "__MACOSX/Author.ModName/._pallet.json",
        ]);

        let folders = extract_mod(bytes, dir.path(), None, TargetPlatform::WINDOWS)
            .unwrap()
            .folders;

        assert_eq!(folders, vec![OsString::from("Author.ModName")]);
    }

    #[test]
    fn rejects_an_archive_with_loose_files_at_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&["pallet.json"]);

        assert!(extract_mod(bytes, dir.path(), None, TargetPlatform::WINDOWS).is_err());
        assert!(!dir.path().join("pallet.json").exists());
    }

    /// Mods in the wild use either a plain `pallet.json` or one named after the
    /// mod, like `Aubies12.Bean.pallet.json`, so accept both.
    fn has_pallet(folder: &Path) -> bool {
        folder
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();

                name == "pallet.json" || name.ends_with(".pallet.json")
            })
    }

    /// Mirrors what `authentication::builder` sets up, since both the user
    /// agent and the host are load bearing against the live API.
    fn live_client(target_platform: TargetPlatform) -> Client {
        let api_key = env::var("MODIO_API_KEY").expect("MODIO_API_KEY must be set for this test");

        Client::builder(api_key)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .game_host(GameId::new(BONELAB_GAME_ID))
            .target_platform(target_platform)
            .build()
            .unwrap()
    }

    fn total_size(dir: &Path) -> u64 {
        dir.read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| {
                let path = entry.path();

                if path.is_dir() {
                    total_size(&path)
                } else {
                    path.metadata().map(|metadata| metadata.len()).unwrap_or(0)
                }
            })
            .sum()
    }

    async fn fetch_live_mod(client: &Client) -> Mod {
        client
            .get_mod(GameId::new(BONELAB_GAME_ID), ModId::new(LIVE_MOD_ID))
            .await
            .unwrap()
            .data()
            .await
            .unwrap()
    }

    /// Fetches the mod fresh each time, the way a real run would.
    async fn install_live_mod(
        client: &Arc<Client>,
        target_platform: TargetPlatform,
        mods_dir: &Path,
        installed_mod: Option<InstalledMod>,
    ) -> ModInstallationOutcome {
        let r#mod = fetch_live_mod(client).await;
        let mut mod_installation =
            ModInstallation::new(r#mod.name.clone(), ProgressBar::hidden()).unwrap();

        _install_mod(
            r#mod,
            &mut mod_installation,
            Arc::clone(client),
            target_platform,
            mods_dir.to_path_buf(),
            None,
            installed_mod,
        )
        .await
        .expect("install failed")
    }

    /// Installs a real mod, exercising the parts that only a live mod.io can
    /// cover: platform file selection, the chunked download, and extraction of
    /// an archive we did not build ourselves.
    ///
    /// Ignored by default since it needs the network and a key. Run with
    /// `MODIO_API_KEY=... cargo test -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "hits the live mod.io API"]
    async fn installs_a_real_mod_from_modio() {
        let dir = tempfile::tempdir().unwrap();
        let client = Arc::new(live_client(TargetPlatform::WINDOWS));

        let ModInstallationOutcome::Installed(mod_id, installed_mod) =
            install_live_mod(&client, TargetPlatform::WINDOWS, dir.path(), None).await
        else {
            panic!("expected a fresh install");
        };

        assert_eq!(mod_id.get(), LIVE_MOD_ID);

        let folder = dir.path().join(&installed_mod.folders[0]);

        assert!(folder.is_dir(), "{} is not a directory", folder.display());
        assert!(
            has_pallet(&folder),
            "no *.pallet.json in {}",
            folder.display()
        );

        // Running again against what we just recorded should download nothing.
        assert!(
            matches!(
                install_live_mod(
                    &client,
                    TargetPlatform::WINDOWS,
                    dir.path(),
                    Some(installed_mod.clone())
                )
                .await,
                ModInstallationOutcome::AlreadyInstalled,
            ),
            "an up to date mod was not left alone",
        );

        // A stale record should make it clear the old copy and lay the mod down
        // again, rather than skip it.
        let stale = InstalledMod {
            date_updated: installed_mod.date_updated - 1,
            folders: installed_mod.folders.clone(),
            game_files: Vec::new(),
        };
        let ModInstallationOutcome::Updated(_, updated) =
            install_live_mod(&client, TargetPlatform::WINDOWS, dir.path(), Some(stale)).await
        else {
            panic!("expected an update");
        };

        assert_eq!(updated.folders, installed_mod.folders);
        assert!(
            has_pallet(&dir.path().join(&updated.folders[0])),
            "update left the mod folder incomplete",
        );

        // The record is only a cache of what is on disk. Someone who deletes a
        // mod folder by hand should get it back, not be told it is installed.
        std::fs::remove_dir_all(&folder).unwrap();

        let ModInstallationOutcome::Installed(_, reinstalled) = install_live_mod(
            &client,
            TargetPlatform::WINDOWS,
            dir.path(),
            Some(installed_mod.clone()),
        )
        .await
        else {
            panic!("a mod whose folder was deleted was not reinstalled");
        };

        assert!(
            has_pallet(&dir.path().join(&reinstalled.folders[0])),
            "reinstall left the mod folder incomplete",
        );
    }

    /// A mod.io refusal has to still be recognisable as one after going through
    /// `anyhow`, or the friendly wording never appears and people see a bare
    /// HTTP status for something that is not their fault.
    #[tokio::test]
    #[ignore = "hits the live mod.io API"]
    async fn explains_a_mod_that_cannot_be_downloaded() {
        let client = live_client(TargetPlatform::WINDOWS);
        // A file id that does not belong to this mod, which mod.io turns down
        // the same way it turns down one whose mod has been hidden.
        let err: anyhow::Error = client
            .download(DownloadAction::File {
                game_id: GameId::new(BONELAB_GAME_ID),
                mod_id: ModId::new(LIVE_MOD_ID),
                file_id: FileId::new(1),
            })
            .chunked()
            .await
            .expect_err("mod.io accepted a file id belonging to another mod")
            .into();

        assert!(
            explain(&err).contains("no longer available to download"),
            "a refusal was reported as {:?} instead of being explained",
            explain(&err),
        );
    }

    /// Quest mods are a separate build, and handing a Quest user the Windows
    /// file would give them something Bonelab cannot load.
    #[tokio::test]
    #[ignore = "hits the live mod.io API"]
    async fn picks_the_mod_file_for_the_target_platform() {
        const WINDOWS_FILE_ID: u64 = 8080373;
        const ANDROID_FILE_ID: u64 = 8080376;

        let r#mod = fetch_live_mod(&live_client(TargetPlatform::WINDOWS)).await;

        assert_eq!(
            file_id_for(&r#mod, TargetPlatform::WINDOWS).unwrap().get(),
            WINDOWS_FILE_ID,
        );
        assert_eq!(
            file_id_for(&r#mod, TargetPlatform::ANDROID).unwrap().get(),
            ANDROID_FILE_ID,
        );
    }

    /// Proves the platform choice reaches the disk, rather than only the file
    /// id we asked for.
    #[tokio::test]
    #[ignore = "hits the live mod.io API"]
    async fn installs_a_different_build_for_quest() {
        let windows_dir = tempfile::tempdir().unwrap();
        let android_dir = tempfile::tempdir().unwrap();

        for (target_platform, dir) in [
            (TargetPlatform::WINDOWS, &windows_dir),
            (TargetPlatform::ANDROID, &android_dir),
        ] {
            let client = Arc::new(live_client(target_platform));
            let ModInstallationOutcome::Installed(_, installed_mod) =
                install_live_mod(&client, target_platform, dir.path(), None).await
            else {
                panic!("expected a fresh install for {target_platform}");
            };

            assert!(
                has_pallet(&dir.path().join(&installed_mod.folders[0])),
                "the {target_platform} install left the mod folder incomplete",
            );
        }

        assert_ne!(
            total_size(windows_dir.path()),
            total_size(android_dir.path()),
            "the Quest install is byte for byte the Windows one",
        );
    }
}
