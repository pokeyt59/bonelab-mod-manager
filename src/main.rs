mod app_data;
mod authentication;
mod installation;

use std::{collections::HashSet, env, sync::Arc};

use anyhow::Result;
use app_data::AppData;
use authentication::{authenticate, delete_password};
use console::{style, Key, Term};
#[cfg(target_os = "windows")]
use dialoguer::{theme::ColorfulTheme, Select};
#[cfg(debug_assertions)]
use env_logger::Env;
use indicatif::{MultiProgress, ProgressBar};
use installation::{install_mod, remove_installed_mod, ModInstallationOutcome};
use log::debug;
use modio::{
    request::{filter::prelude::Eq as _, mods::filters::GameId},
    types::mods::Mod,
    util::{Paginate, PaginateError},
};
use tokio::task::JoinSet;

#[cfg(target_os = "windows")]
use crate::app_data::BonelabPlatform;

const BONELAB_GAME_ID: u64 = 3809;

const CONCURRENT_DOWNLOADS_VAR: &str = "BMM_CONCURRENT_DOWNLOADS";

async fn try_main() -> Result<()> {
    debug!("entering `try_main()`");

    let mut app_data = AppData::read().await?;

    // Choosing the platform has to come before signing in: the mod.io client
    // is told which platform to serve mod files for when it is built.
    #[cfg(target_os = "windows")]
    if app_data.platform.is_none() && BonelabPlatform::from_var()?.is_none() {
        let select = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Which platform do you play Bonelab on?")
            .item("Windows")
            .item("Quest")
            .default(0)
            .interact()?;

        app_data.platform = Some(BonelabPlatform::try_from(select)?);
        app_data.write().await?;
    }

    debug!("platform chosen");

    let target_platform = app_data.platform()?.target_platform();
    let mods_dir = app_data.mods_dir_path()?;

    debug!("mods dir is \"{}\"", mods_dir.display());

    // authenticate with mod.io
    let client = Arc::new(authenticate(target_platform).await?);

    // get subscribed mods
    let request = client
        .get_user_subscriptions()
        .filter(GameId::eq(BONELAB_GAME_ID));
    let mut pages = request.paged();
    let mut subscriptions = Vec::new();

    while let Some(page) = pages.next().await? {
        subscriptions.extend(page);
    }

    debug!("got {} subscribed mods", subscriptions.len());

    // remove installed mod if not subscribed
    let subscribed_ids: HashSet<u64> = subscriptions
        .iter()
        .map(|subscription| subscription.id.get())
        .collect();
    let unsubscribed: Vec<_> = app_data
        .installed_mods
        .iter()
        .filter(|(installed_mod_id, _)| !subscribed_ids.contains(installed_mod_id))
        .map(|(installed_mod_id, installed_mod)| (*installed_mod_id, installed_mod.clone()))
        .collect();
    let removed_mods = unsubscribed.len();

    for (installed_mod_id, installed_mod) in unsubscribed {
        debug!(
            "removing installed mod with id `{installed_mod_id}` and folder \"{}\"",
            installed_mod.folder.to_string_lossy(),
        );

        remove_installed_mod(&mods_dir, &installed_mod).await?;
        app_data.installed_mods.remove(&installed_mod_id);
    }

    app_data.write().await?;
    debug!("removed unsubscribed mods");

    // spawn a task for each mod
    let mut set = JoinSet::new();
    let multi_progress = MultiProgress::new();
    let concurrent_downloads: u8 = match env::var(CONCURRENT_DOWNLOADS_VAR) {
        Ok(concurrent_downloads) => concurrent_downloads.parse()?,
        Err(_) => 4,
    };

    // Snapshotted so the loop below is free to record results into `app_data`
    // as they arrive.
    let previously_installed = app_data.installed_mods.clone();
    let spawn_next = |set: &mut JoinSet<Result<ModInstallationOutcome>>,
                      subscriptions: &mut Vec<Mod>| {
        let Some(subscription) = subscriptions.pop() else {
            debug!("no more subscriptions");

            return false;
        };
        let installed_mod = previously_installed.get(&subscription.id.get()).cloned();

        debug!("spawning task for \"{}\"", subscription.name);
        set.spawn(install_mod(
            subscription,
            multi_progress.add(ProgressBar::new_spinner()),
            Arc::clone(&client),
            target_platform,
            mods_dir.clone(),
            installed_mod,
        ));
        debug!("spawned task");

        true
    };

    for _ in 0..concurrent_downloads {
        if !spawn_next(&mut set, &mut subscriptions) {
            break;
        }
    }

    let mut installed = 0;
    let mut updated = 0;
    let mut already_installed = 0;
    let mut failed = 0;

    while let Some(res) = set.join_next().await {
        // Only this loop touches the app data, so concurrent installs can't
        // overwrite each other's bookkeeping the way they could when every
        // task read and wrote the file itself.
        match res?? {
            ModInstallationOutcome::Installed(mod_id, installed_mod) => {
                app_data.installed_mods.insert(mod_id.get(), installed_mod);
                app_data.write().await?;
                installed += 1;
            }
            ModInstallationOutcome::Updated(mod_id, installed_mod) => {
                app_data.installed_mods.insert(mod_id.get(), installed_mod);
                app_data.write().await?;
                updated += 1;
            }
            ModInstallationOutcome::AlreadyInstalled => already_installed += 1,
            ModInstallationOutcome::Failed => failed += 1,
        }

        debug!("task finished, another concurrent download can be made");
        spawn_next(&mut set, &mut subscriptions);
    }

    println!(
        "\n\n{} installed, {} updated, {} already installed, {} removed, and {} failed\n",
        style(installed).bold().green(),
        style(updated).bold().cyan(),
        style(already_installed).bold(),
        style(removed_mods).bold().yellow(),
        style(failed).bold().red(),
    );
    debug!("all done!");

    Ok(())
}

/// Holds the window open so someone who launched the program by double
/// clicking it can read the summary before it disappears.
fn wait_to_quit() {
    debug!("waiting to quit");

    let term = Term::stdout();

    // A redirected run has nobody to press the key, and reading one would hang.
    if !term.is_term() {
        debug!("not attached to a terminal, quitting immediately");

        return;
    }

    term.write_line(&style("Press q to quit").bold().to_string())
        .unwrap();

    while !matches!(term.read_key(), Ok(Key::Char('q')) | Err(_)) {}
}

/// Whether mod.io rejected our credentials.
///
/// The error arrives wrapped differently depending on which call produced it,
/// so unwrapping one layer isn't enough.
fn is_auth_error(err: &anyhow::Error) -> bool {
    if let Some(err) = err.downcast_ref::<modio::Error>() {
        return err.is_auth();
    }

    match err.downcast_ref::<PaginateError>() {
        Some(PaginateError::Request(err)) => err.is_auth(),
        _ => false,
    }
}

#[tokio::main]
async fn main() {
    #[cfg(debug_assertions)]
    env_logger::Builder::from_env(Env::default().default_filter_or("debug")).init();
    #[cfg(not(debug_assertions))]
    env_logger::init();

    debug!("`env_logger` initialized");

    match try_main().await {
        Ok(_) => println!(
            "{}",
            style("Completed without any unrecoverable errors!")
                .bold()
                .green()
        ),
        Err(err) => {
            if is_auth_error(&err) && delete_password().await.is_ok() {
                eprintln!(
                    "{}: Authentication failed, you have been signed out",
                    style("Error").red()
                );

                return wait_to_quit();
            }

            if let Ok(backtrace) = env::var("RUST_BACKTRACE") {
                if backtrace == "1" {
                    eprintln!(
                        "{}: {err:#}\n{}",
                        style("Error").bold().red(),
                        err.backtrace()
                    );
                    return wait_to_quit();
                }
            }

            eprintln!("{}: {err:#}", style("Error").red());
        }
    }

    wait_to_quit();

    debug!("quitting");
}
