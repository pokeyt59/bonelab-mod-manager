use std::{env, sync::Arc};

use anyhow::{bail, Result};
// Only the Windows token store has a missing token to report; the Unix one
// leaves that to the keyring.
#[cfg(target_os = "windows")]
use anyhow::anyhow;
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

/// The mod.io API key, read from the environment while the program is built.
///
/// Required rather than optional. Without it the program cannot reach mod.io at
/// all, and nothing would say so until somebody ran it, so the build stops here
/// instead of shipping something broken.
const API_KEY: &str = env!(
    "MODIO_API_KEY",
    "no mod.io API key to build with.\n\
     Get one free at https://mod.io/me/access, then build with \
     MODIO_API_KEY=your_key cargo build --release\n\
     See Building From Source in the README for other ways to set it."
);

// An unset secret in CI arrives as an empty string rather than as nothing at
// all, which would otherwise sail past `env!` and only fail once someone ran it.
const _: () = assert!(
    !API_KEY.is_empty(),
    "MODIO_API_KEY is set, but to an empty value.\n\
     Get a key free at https://mod.io/me/access.\n\
     See Building From Source in the README."
);

fn builder(target_platform: TargetPlatform) -> Builder {
    Client::builder(API_KEY.to_string())
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
        .target_platform(target_platform)
}

// Windows deliberately does not use the credential store. A mod.io token is
// around 1800 characters, and Windows Credential Manager caps a credential at
// 2560 bytes once the value is encoded as UTF-16, so storing one there fails
// outright. See `keyring_cannot_hold_a_modio_token_on_windows`. It goes in the
// app data file encrypted with the data protection API instead, which has no
// such limit because the file is ours.

/// Marks a stored token as encrypted.
///
/// Builds from before the token was encrypted wrote it as plain text, and the
/// leading NUL, which no token can begin with, is what tells the two apart.
#[cfg(target_os = "windows")]
const ENCRYPTED_MARKER: &[u8] = b"\0dpapi1\0";

/// What was found in the app data.
#[cfg(target_os = "windows")]
enum StoredToken {
    Encrypted(String),
    /// Left in plain text by a build from before the token was encrypted, so it
    /// wants storing again properly.
    Legacy(String),
}

#[cfg(target_os = "windows")]
fn decode_stored_token(stored: &[u8]) -> Result<StoredToken> {
    let Some(ciphertext) = stored.strip_prefix(ENCRYPTED_MARKER) else {
        return Ok(StoredToken::Legacy(String::from_utf8(stored.to_vec())?));
    };

    Ok(StoredToken::Encrypted(String::from_utf8(
        crate::dpapi::unprotect(ciphertext)?,
    )?))
}

#[cfg(target_os = "windows")]
fn encode_token(token: &str) -> Result<Vec<u8>> {
    let mut encoded = ENCRYPTED_MARKER.to_vec();

    encoded.extend(crate::dpapi::protect(token.as_bytes())?);

    Ok(encoded)
}

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
    let stored = app_data
        .modio_token
        .ok_or(anyhow!("User does not have mod.io token"))?;

    match decode_stored_token(&stored)? {
        StoredToken::Encrypted(token) => Ok(token),
        StoredToken::Legacy(token) => {
            debug!("encrypting a token an older build left in plain text");
            set_password(&token).await?;

            Ok(token)
        }
    }
}

#[cfg(target_family = "unix")]
async fn set_password(password: &str) -> Result<()> {
    Ok(entry()?.set_password(password)?)
}

#[cfg(target_os = "windows")]
async fn set_password(password: &str) -> Result<()> {
    let mut app_data = AppData::read().await?;

    app_data.modio_token = Some(encode_token(password)?);

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
    let builder = builder(target_platform);

    if let Ok(token) = env::var(TOKEN_VAR) {
        debug!("got token from {TOKEN_VAR}");

        return Ok(Authentication::SignedIn(Arc::new(
            builder.token(token).build()?,
        )));
    }

    // A stored token that will not decrypt, because the app data came from
    // another account or machine, is not an error worth stopping for: the user
    // simply signs in again. Worth saying why in the log, though.
    match get_password().await {
        Ok(token) => {
            debug!("got password");

            return Ok(Authentication::SignedIn(Arc::new(
                builder.token(token).build()?,
            )));
        }
        Err(err) => debug!("could not get password: {err:#}"),
    }

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

    #[cfg(target_os = "windows")]
    use super::{decode_stored_token, encode_token, StoredToken, ENCRYPTED_MARKER};

    /// Roughly the length of a token mod.io issues.
    const TOKEN_LEN: usize = 1822;

    const PROBE_SERVICE: &str = "bonelab_mod_manager_probe";

    /// Nothing readable should reach the app data file, at the length that
    /// actually matters.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_stored_token_is_encrypted_and_comes_back_intact() {
        let token = "ey".to_string() + &"J".repeat(TOKEN_LEN - 2);
        let encoded = encode_token(&token).unwrap();

        assert!(
            encoded.starts_with(ENCRYPTED_MARKER),
            "stored token is not marked as encrypted",
        );
        assert!(
            !encoded
                .windows(token.len())
                .any(|window| window == token.as_bytes()),
            "the token is sitting in the app data in the clear",
        );

        let StoredToken::Encrypted(decoded) = decode_stored_token(&encoded).unwrap() else {
            panic!("an encrypted token was not recognised as one");
        };

        assert_eq!(decoded, token);
    }

    /// The upgrade path: whatever an older build wrote has to keep working, or
    /// everyone gets signed out by the update.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_plain_text_token_is_read_and_flagged_for_encrypting() {
        let StoredToken::Legacy(token) =
            decode_stored_token(b"eyJhbGciOiJIUzI1.plain.text").unwrap()
        else {
            panic!("a plain text token was mistaken for an encrypted one");
        };

        assert_eq!(token, "eyJhbGciOiJIUzI1.plain.text");
    }

    /// App data carried to another machine, or corrupted, must report an error
    /// so the caller can ask for a fresh sign in, rather than panicking.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_blob_that_will_not_decrypt_is_an_error() {
        let mut encoded = encode_token("something worth protecting").unwrap();
        let last = encoded.len() - 1;

        encoded[last] ^= 0xff;

        assert!(
            decode_stored_token(&encoded).is_err(),
            "a tampered blob was accepted",
        );
    }

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
