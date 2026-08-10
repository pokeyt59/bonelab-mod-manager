use std::{
    collections::HashMap,
    env::{self, VarError},
    ffi::OsString,
    fmt::{self, Display, Formatter},
    path::PathBuf,
};

use anyhow::{anyhow, Result};
use log::debug;
use modio::types::TargetPlatform;
use serde::{Deserialize, Serialize};
use tokio::fs;

/// Overrides the directory mods are installed into.
///
/// Needed for setups the built-in paths don't cover, such as Bonelab running
/// under Proton on Linux or the Steam Deck, where the game's `LocalLow`
/// directory lives inside a Wine prefix.
const MODS_DIR_VAR: &str = "BMM_MODS_DIR";

/// Overrides the saved platform choice.
///
/// The choice is otherwise made once, on first run, and there is no other way
/// to revisit it.
const PLATFORM_VAR: &str = "BMM_PLATFORM";

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct AppData {
    /// Where Windows builds used to keep the mod.io token, before it moved into
    /// the credential store.
    ///
    /// Kept so an existing install can be migrated, and because dropping it
    /// would change the on disk layout and lose the record of what is
    /// installed, forcing every mod to be downloaded again.
    #[cfg(target_os = "windows")]
    pub(crate) modio_token: Option<String>,
    #[cfg(target_os = "windows")]
    pub(crate) platform: Option<BonelabPlatform>,
    pub(crate) installed_mods: HashMap<u64, InstalledMod>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BonelabPlatform {
    Windows,
    Quest,
}

impl BonelabPlatform {
    /// The mod.io platform whose mod files this Bonelab install can load.
    ///
    /// Quest is an Android device, so its mod files are published under
    /// mod.io's `android` platform rather than `oculus`, which mod.io uses for
    /// Rift-era PC titles.
    pub(crate) fn target_platform(self) -> TargetPlatform {
        match self {
            Self::Windows => TargetPlatform::WINDOWS,
            Self::Quest => TargetPlatform::ANDROID,
        }
    }

    pub(crate) fn from_var() -> Result<Option<Self>> {
        let Some(value) = env::var_os(PLATFORM_VAR) else {
            return Ok(None);
        };

        match value.to_string_lossy().to_lowercase().as_str() {
            "windows" | "pc" => Ok(Some(Self::Windows)),
            "quest" | "android" => Ok(Some(Self::Quest)),
            other => Err(anyhow!(
                "{PLATFORM_VAR} must be \"windows\" or \"quest\", got \"{other}\""
            )),
        }
    }
}

impl TryFrom<usize> for BonelabPlatform {
    type Error = anyhow::Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Windows),
            1 => Ok(Self::Quest),
            other => Err(anyhow!(
                "BonelabPlatform only accepts values equal to 0 or 1, got {other}"
            )),
        }
    }
}

impl Display for BonelabPlatform {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Windows => "Windows",
                Self::Quest => "Quest",
            }
        )
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct InstalledMod {
    pub(crate) date_updated: u64,
    pub(crate) folder: OsString,
}

impl AppData {
    #[cfg(target_os = "macos")]
    const REL_DIR_PATH: &str = "Library/Application Support/com.valentinegb.bonelab_mod_manager";
    #[cfg(target_os = "linux")]
    const REL_DIR_PATH: &str = "var/lib/bonelab_mod_manager";
    #[cfg(target_os = "windows")]
    const REL_DIR_PATH: &str = "bonelab_mod_manager";

    #[cfg(target_family = "unix")]
    fn dir_path() -> Result<PathBuf, VarError> {
        debug!("getting app data dir path");

        Ok(PathBuf::from(env::var("HOME")?).join(Self::REL_DIR_PATH))
    }

    #[cfg(target_os = "windows")]
    fn dir_path() -> Result<PathBuf, VarError> {
        debug!("getting app data dir path");

        Ok(PathBuf::from(env::var("AppData")?).join(Self::REL_DIR_PATH))
    }

    fn path() -> Result<PathBuf, VarError> {
        debug!("getting app data path");

        Ok(Self::dir_path()?.join("app_data"))
    }

    /// Which Bonelab install mods are being managed for.
    ///
    /// Bonelab only ships on Windows and Quest, so a non-Windows host can only
    /// ever be staging mods for a headset.
    pub(crate) fn platform(&self) -> Result<BonelabPlatform> {
        if let Some(platform) = BonelabPlatform::from_var()? {
            debug!("using platform from {PLATFORM_VAR}");

            return Ok(platform);
        }

        #[cfg(target_os = "windows")]
        return self.platform.ok_or(anyhow!("Platform is not set"));
        #[cfg(target_family = "unix")]
        return Ok(BonelabPlatform::Quest);
    }

    pub(crate) fn mods_dir_path(&self) -> Result<PathBuf> {
        debug!("getting mods dir path");

        if let Some(path) = env::var_os(MODS_DIR_VAR) {
            debug!("using mods dir path from {MODS_DIR_VAR}");

            return Ok(PathBuf::from(path));
        }

        match self.platform()? {
            BonelabPlatform::Windows => Ok(Self::local_low_dir_path()?
                .join("Stress Level Zero")
                .join("BONELAB")
                .join("Mods")),
            BonelabPlatform::Quest => Ok(Self::dir_path()?.join("Mods")),
        }
    }

    /// `%UserProfile%\AppData\LocalLow`, where Unity games keep their save data.
    ///
    /// Both the Steam and the Meta PC builds of Bonelab read mods from here.
    #[cfg(target_os = "windows")]
    fn local_low_dir_path() -> Result<PathBuf> {
        Ok(PathBuf::from(env::var("AppData")?)
            .parent()
            .ok_or(anyhow!("AppData env var value does not have parent"))?
            .join("LocalLow"))
    }

    #[cfg(target_family = "unix")]
    fn local_low_dir_path() -> Result<PathBuf> {
        Err(anyhow!(
            "Bonelab for Windows cannot be installed on this operating system; \
             set {MODS_DIR_VAR} to the game's Mods directory to manage a Proton install"
        ))
    }

    async fn write_default() -> Result<Self> {
        let default = Self::default();

        default.write().await?;
        debug!("wrote default app data");

        Ok(default)
    }

    pub(crate) async fn read() -> Result<Self> {
        let path = Self::path()?;

        if !fs::try_exists(&path).await? {
            return Self::write_default().await;
        }

        // Anything that fails to deserialize is either corrupt or was written
        // by a version with a different layout. Neither is worth failing over:
        // the file is a cache of what's installed, and starting over just means
        // mods get reinstalled once.
        match postcard::from_bytes(&fs::read(path).await?) {
            Ok(app_data) => {
                debug!("read app data");

                Ok(app_data)
            }
            Err(err) => {
                debug!("app data is unreadable ({err}), resetting");

                Ok(Self::write_default().await?)
            }
        }
    }

    pub(crate) async fn write(&self) -> Result<()> {
        let path = Self::path()?;

        if !fs::try_exists(&path).await? {
            debug!("app data dir does not exist");
            fs::create_dir_all(Self::dir_path()?).await?;
            debug!("created app data dir");
        }

        fs::write(path, postcard::to_stdvec(self)?).await?;
        debug!("wrote app data");

        Ok(())
    }
}
