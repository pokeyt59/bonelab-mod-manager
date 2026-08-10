use std::{
    env,
    ffi::OsString,
    fmt,
    io::{Cursor, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Result};
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
    installed_mod: &InstalledMod,
) -> Result<()> {
    let path = mods_dir.join(&installed_mod.folder);

    debug!("removing \"{}\"", path.display());

    let metadata = match symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            debug!("nothing to remove");

            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };

    // Content mods are directories; code mods are a single `.dll`.
    if metadata.is_dir() {
        remove_dir_all(&path).await?;
    } else {
        remove_file(&path).await?;
    }

    Ok(())
}

/// Picks the mod file built for `target_platform`.
///
/// Mods are published per platform and the builds genuinely differ, so handing
/// a Quest user the Windows file would give them something Bonelab cannot load.
/// `modfile` is only a fallback: the client sends a target platform header, so
/// mod.io has already resolved that field for this platform.
fn file_id_for(r#mod: &Mod, target_platform: TargetPlatform) -> Result<FileId> {
    r#mod
        .platforms
        .iter()
        .find(|platform| platform.target == target_platform)
        .map(|platform| platform.modfile_id)
        .or_else(|| r#mod.modfile.as_ref().map(|modfile| modfile.id))
        .ok_or(anyhow!("Mod does not have a {target_platform} mod file"))
}

pub(crate) async fn install_mod(
    r#mod: Mod,
    progress_bar: ProgressBar,
    client: Arc<Client>,
    target_platform: TargetPlatform,
    mods_dir: PathBuf,
    installed_mod: Option<InstalledMod>,
) -> Result<ModInstallationOutcome> {
    let mut mod_installation = ModInstallation::new(r#mod.name.clone(), progress_bar)?;

    match _install_mod(
        r#mod,
        &mut mod_installation,
        client,
        target_platform,
        mods_dir,
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
            let mut msg = format!("{err:#}");

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
            // the record claims it is.
            let present = try_exists(mods_dir.join(&installed_mod.folder)).await?;

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
        remove_installed_mod(&mods_dir, installed_mod).await?;
    }

    let folder = spawn_blocking(move || extract_mod(bytes, &mods_dir)).await??;

    debug!("extracted mod file");

    let installed_mod = InstalledMod {
        date_updated,
        folder,
    };

    Ok(if updating {
        ModInstallationOutcome::Updated(mod_id, installed_mod)
    } else {
        ModInstallationOutcome::Installed(mod_id, installed_mod)
    })
}

/// Unpacks a mod archive into `mods_dir` and reports the folder it created.
///
/// Bonelab loads a mod from a single `Author.ModName` directory holding a
/// pallet manifest, named either `pallet.json` or after the mod itself, so an
/// archive without exactly one root directory would scatter its contents across
/// the Mods folder. Rejecting it before extracting leaves the Mods folder
/// untouched.
fn extract_mod(bytes: Vec<u8>, mods_dir: &Path) -> Result<OsString> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let root_dir = archive.root_dir(root_dir_common_filter)?.ok_or(anyhow!(
        "Mod file archive does not contain exactly one root folder, \
         so it is not laid out the way Bonelab expects"
    ))?;

    archive.extract(mods_dir)?;

    Ok(root_dir.into_os_string())
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
        _install_mod, extract_mod, file_id_for, Cursor, InstalledMod, Mod, ModInstallation,
        ModInstallationOutcome, Path,
    };

    /// A small mod whose Windows and Android files have different ids, so
    /// picking the wrong platform's file would fetch different bytes.
    const LIVE_MOD_ID: u64 = 6297147;

    fn archive(entries: &[&str]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));

        for entry in entries {
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

        let folder = extract_mod(bytes, dir.path()).unwrap();

        assert_eq!(folder.to_str(), Some("Author.ModName"));
        assert!(dir
            .path()
            .join("Author.ModName")
            .join("Author.ModName.pallet.json")
            .exists());
    }

    #[test]
    fn rejects_an_archive_with_several_root_folders() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&["One.Mod/pallet.json", "Two.Mod/pallet.json"]);

        assert!(extract_mod(bytes, dir.path()).is_err());
        assert!(!dir.path().join("One.Mod").exists());
    }

    #[test]
    fn rejects_an_archive_with_loose_files_at_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(&["pallet.json"]);

        assert!(extract_mod(bytes, dir.path()).is_err());
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

        let folder = dir.path().join(&installed_mod.folder);

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
            folder: installed_mod.folder.clone(),
        };
        let ModInstallationOutcome::Updated(_, updated) =
            install_live_mod(&client, TargetPlatform::WINDOWS, dir.path(), Some(stale)).await
        else {
            panic!("expected an update");
        };

        assert_eq!(updated.folder, installed_mod.folder);
        assert!(
            has_pallet(&dir.path().join(&updated.folder)),
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
            has_pallet(&dir.path().join(&reinstalled.folder)),
            "reinstall left the mod folder incomplete",
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
                has_pallet(&dir.path().join(&installed_mod.folder)),
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
