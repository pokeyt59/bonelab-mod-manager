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
    /// The mod.io token, encrypted for the current Windows user.
    ///
    /// Bytes rather than text because the encrypted form is not valid UTF-8.
    /// Widening the type did not change the on disk layout, since postcard
    /// writes a `String` and a `Vec<u8>` the same way, as a length followed by
    /// that many bytes, so files written when the token was plain text still
    /// load. That matters: [`AppData::read`] resets anything it cannot
    /// deserialize, which would throw away every installed mod record and
    /// download the lot again. `authentication` owns what the bytes mean.
    #[cfg(target_os = "windows")]
    pub(crate) modio_token: Option<Vec<u8>>,
    #[cfg(target_os = "windows")]
    pub(crate) platform: Option<BonelabPlatform>,
    pub(crate) installed_mods: HashMap<u64, InstalledMod>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
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
    /// Every folder this mod put in the Mods directory.
    ///
    /// Usually one. Some downloads are packs: several complete mods in a single
    /// archive, each with its own pallet, which Bonelab loads independently. All
    /// of them have to be recorded, or unsubscribing would leave the rest behind.
    pub(crate) folders: Vec<OsString>,
}

/// The shape written before packs were supported, when a mod was one folder.
///
/// Kept so an existing install carries over rather than being reset, which would
/// cost the record of every installed mod and download all of them again.
#[derive(Deserialize)]
struct LegacyAppData {
    #[cfg(target_os = "windows")]
    modio_token: Option<Vec<u8>>,
    #[cfg(target_os = "windows")]
    platform: Option<BonelabPlatform>,
    installed_mods: HashMap<u64, LegacyInstalledMod>,
}

#[derive(Deserialize)]
struct LegacyInstalledMod {
    date_updated: u64,
    folder: OsString,
}

impl From<LegacyAppData> for AppData {
    fn from(legacy: LegacyAppData) -> Self {
        Self {
            #[cfg(target_os = "windows")]
            modio_token: legacy.modio_token,
            #[cfg(target_os = "windows")]
            platform: legacy.platform,
            installed_mods: legacy
                .installed_mods
                .into_iter()
                .map(|(id, installed_mod)| {
                    (
                        id,
                        InstalledMod {
                            date_updated: installed_mod.date_updated,
                            folders: vec![installed_mod.folder],
                        },
                    )
                })
                .collect(),
        }
    }
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

        let bytes = fs::read(path).await?;

        if let Ok(app_data) = postcard::from_bytes(&bytes) {
            debug!("read app data");

            return Ok(app_data);
        }

        // A file from before packs were supported reads as the older shape
        // instead. Convert it rather than starting over, which would mean
        // downloading every installed mod again.
        if let Ok(legacy) = postcard::from_bytes::<LegacyAppData>(&bytes) {
            debug!("converting app data from the layout used before mod packs");

            let app_data = Self::from(legacy);

            app_data.write().await?;

            return Ok(app_data);
        }

        // Anything else is corrupt or from a layout nothing here knows. Not
        // worth failing over: the file is a cache of what is installed, and
        // starting over just means the mods get installed again once.
        debug!("app data is unreadable, resetting");

        Self::write_default().await
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

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    const A_MOD: u64 = 6297147;

    /// Exactly what a build from before either change wrote: the token in plain
    /// text, and one folder per mod.
    #[derive(Serialize)]
    struct OldestAppData {
        modio_token: Option<String>,
        platform: Option<BonelabPlatform>,
        installed_mods: HashMap<u64, OldestInstalledMod>,
    }

    #[derive(Serialize)]
    struct OldestInstalledMod {
        date_updated: u64,
        folder: OsString,
    }

    fn app_data_from_an_older_build() -> Vec<u8> {
        let mut installed_mods = HashMap::new();

        installed_mods.insert(
            A_MOD,
            OldestInstalledMod {
                date_updated: 1699999999,
                folder: OsString::from("Aubies12.Bean"),
            },
        );

        postcard::to_stdvec(&OldestAppData {
            modio_token: Some("a.plain.text.token".to_string()),
            platform: Some(BonelabPlatform::Quest),
            installed_mods,
        })
        .unwrap()
    }

    /// [`AppData::read`] resets anything it cannot make sense of, which would
    /// cost every existing user their record of what is installed and download
    /// all of it again, gigabytes for a full subscription list. Supporting packs
    /// did change the layout, so the old one has to be read and converted rather
    /// than thrown away.
    #[test]
    fn app_data_from_before_packs_converts_instead_of_being_lost() {
        let bytes = app_data_from_an_older_build();

        assert!(
            postcard::from_bytes::<AppData>(&bytes).is_err(),
            "the old layout parsed as the current one, so `read` would never \
             reach the conversion and would silently use whatever it decoded",
        );

        let converted = AppData::from(
            postcard::from_bytes::<LegacyAppData>(&bytes)
                .expect("app data from an older build can no longer be read at all"),
        );

        assert_eq!(
            converted.installed_mods[&A_MOD].folders,
            vec![OsString::from("Aubies12.Bean")],
            "the record of what is installed was lost",
        );
        assert_eq!(converted.platform, Some(BonelabPlatform::Quest));
        // Widening the token to bytes deliberately did not change the layout, so
        // a plain text one still arrives intact and can be encrypted in place.
        assert_eq!(
            converted.modio_token.as_deref(),
            Some(&b"a.plain.text.token"[..]),
            "the old token did not survive as bytes",
        );
    }
}
