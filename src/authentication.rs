use std::{env, sync::Arc};

use anyhow::{anyhow, bail, Result};
use dialoguer::{theme::ColorfulTheme, Input, Password, Select};
#[cfg(target_family = "unix")]
use keyring::Entry;
use log::debug;
use modio::{
    client::Builder,
    types::{id::GameId, TargetPlatform},
    Client,
};

#[cfg(target_os = "windows")]
use crate::app_data::AppData;
use crate::BONELAB_GAME_ID;

/// Supplies a mod.io token without going through the interactive prompt.
const TOKEN_VAR: &str = "BMM_MODIO_TOKEN";

const API_KEY_VAR: &str = "MODIO_API_KEY";

/// The key baked in at build time, falling back to the environment so the
/// program can be built and run without a release key on hand.
fn api_key() -> Result<String> {
    if let Some(key) = option_env!("MODIO_API_KEY").filter(|key| !key.is_empty()) {
        return Ok(key.to_string());
    }

    env::var(API_KEY_VAR).map_err(|_| {
        anyhow!(
            "This build has no mod.io API key compiled in, so {API_KEY_VAR} must be set. \
             Generate a key at https://mod.io/me/access"
        )
    })
}

fn builder(target_platform: TargetPlatform) -> Result<Builder> {
    Ok(Client::builder(api_key()?)
        // Load bearing, not decoration: mod.io answers a request with no user
        // agent with a 404 telling you the domain is deprecated.
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        // mod.io is retiring `api.mod.io`, which is what the client would
        // otherwise default to, in favour of a domain per game.
        .game_host(GameId::new(BONELAB_GAME_ID))
        // Tells mod.io which platform's mod files this install can actually
        // load, so it filters and resolves files for us.
        .target_platform(target_platform))
}

// Windows deliberately does not use the credential store. A mod.io token is
// around 1800 characters, and Windows Credential Manager caps a credential at
// 2560 bytes once the value is encoded as UTF-16, so storing one there fails
// outright. See `keyring_cannot_hold_a_modio_token_on_windows`.

#[cfg(target_family = "unix")]
fn entry() -> Result<Entry> {
    Ok(Entry::new("bonelab_mod_manager", &env::var("USER")?)?)
}

#[cfg(target_family = "unix")]
async fn get_password() -> Result<String> {
    Ok(entry()?.get_password()?)
}

#[cfg(target_os = "windows")]
async fn get_password() -> Result<String> {
    let app_data = AppData::read().await?;

    app_data
        .modio_token
        .ok_or(anyhow!("User does not have mod.io token"))
}

#[cfg(target_family = "unix")]
async fn set_password(password: &str) -> Result<()> {
    Ok(entry()?.set_password(password)?)
}

#[cfg(target_os = "windows")]
async fn set_password(password: &str) -> Result<()> {
    let mut app_data = AppData::read().await?;

    app_data.modio_token = Some(password.to_string());

    app_data.write().await
}

#[cfg(target_family = "unix")]
pub(super) async fn delete_password() -> Result<()> {
    // Not having an entry to remove is the state we were after anyway, so
    // signing out twice is not an error.
    let _ = entry()?.delete_credential();

    Ok(())
}

#[cfg(target_os = "windows")]
pub(super) async fn delete_password() -> Result<()> {
    let mut app_data = AppData::read().await?;

    app_data.modio_token = None;

    app_data.write().await
}

/// What came of asking the user to sign in.
pub(super) enum Authentication {
    SignedIn(Arc<Client>),
    /// The user backed out of the sign in prompt to pick their platform again.
    ChangePlatform,
}

pub(super) async fn authenticate(
    target_platform: TargetPlatform,
    can_change_platform: bool,
) -> Result<Authentication> {
    // Built up front so a missing API key is reported before the user is asked
    // to go and fetch an email code.
    let builder = builder(target_platform)?;

    if let Ok(token) = env::var(TOKEN_VAR) {
        debug!("got token from {TOKEN_VAR}");

        return Ok(Authentication::SignedIn(Arc::new(
            builder.token(token).build()?,
        )));
    }

    if let Ok(token) = get_password().await {
        debug!("got password");

        return Ok(Authentication::SignedIn(Arc::new(
            builder.token(token).build()?,
        )));
    }

    debug!("could not get password");

    let client = builder.build()?;

    println!("You are not signed in");

    let mut items = vec!["Send me an email code", "Let me input my token"];

    // Offered last so the indices of the two sign in methods do not move.
    if can_change_platform {
        items.push("Go back and change platform");
    }

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("How would you like to sign in?")
        .items(&items)
        .default(0)
        .interact()?;

    let token = match selection {
        0 => {
            debug!("user selected email code");

            let email: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Please enter your email address")
                .interact_text()?;

            debug!("created mod.io client");
            client.request_code(&email).await?;
            debug!("requested code");

            let code: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Enter the code emailed to you")
                .interact_text()?;
            let access_token = client.request_token(&code).await?.data().await?;

            debug!("exchanged security code for token");

            access_token.value
        }
        1 => {
            debug!("user selected input token");
            println!("Your token will not be shown as you type or paste it.");
            println!("If your terminal will not accept the paste, set {TOKEN_VAR} instead.");

            Password::with_theme(&ColorfulTheme::default())
                .with_prompt("Enter your token")
                .interact()?
        }
        2 => {
            debug!("user wants to change platform");

            return Ok(Authentication::ChangePlatform);
        }
        other => bail!("Sign in prompt returned an unexpected selection: {other}"),
    };

    set_password(&token).await?;
    debug!("set password");

    Ok(Authentication::SignedIn(Arc::new(client.with_token(token))))
}

#[cfg(test)]
mod tests {
    use keyring::Entry;

    /// Roughly the length of a token mod.io issues.
    const TOKEN_LEN: usize = 1822;

    const PROBE_SERVICE: &str = "bonelab_mod_manager_probe";

    /// Documents why Windows keeps the token in the app data file rather than
    /// the credential store, so nobody "fixes" it back.
    ///
    /// Ignored by default because it writes to the real credential store, which
    /// cannot be pointed somewhere harmless.
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "writes to the real Windows credential store"]
    fn keyring_cannot_hold_a_modio_token_on_windows() {
        let entry = Entry::new(PROBE_SERVICE, "probe").unwrap();

        assert!(
            entry.set_password(&"a".repeat(512)).is_ok(),
            "a short secret should store fine",
        );

        let err = entry
            .set_password(&"a".repeat(TOKEN_LEN))
            .expect_err("Credential Manager took a token sized secret; revisit using it");

        assert!(
            err.to_string().contains("longer than the platform limit"),
            "unexpected failure storing a token sized secret: {err}",
        );

        let _ = entry.delete_credential();
    }

    /// The counterpart to the Windows test: here the credential store really is
    /// used for the token, so a token sized secret has to survive a round trip.
    ///
    /// Needs a running Secret Service on Linux, meaning a desktop session with
    /// something like GNOME Keyring or KWallet, so it cannot run on headless CI.
    #[cfg(target_family = "unix")]
    #[test]
    #[ignore = "needs a desktop session with a credential store"]
    fn keyring_holds_a_modio_token_on_unix() {
        let entry = Entry::new(PROBE_SERVICE, "probe").unwrap();
        let secret = "a".repeat(TOKEN_LEN);

        entry
            .set_password(&secret)
            .expect("could not store a token sized secret");

        assert_eq!(
            entry
                .get_password()
                .expect("could not read the secret back"),
            secret,
            "the credential store altered the secret",
        );

        entry
            .delete_credential()
            .expect("could not delete the secret");

        assert!(
            entry.get_password().is_err(),
            "the secret survived deletion",
        );
    }
}
