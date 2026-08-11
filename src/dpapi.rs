//! Encrypting a secret at rest with the Windows data protection API.
//!
//! Windows Credential Manager, which is where the Unix builds keep the mod.io
//! token, cannot hold one: a token runs to around 1800 characters and the
//! platform refuses anything over 2560 bytes once encoded as UTF-16. The data
//! protection API has no such limit, because the ciphertext goes in a file of
//! ours rather than a system store.
//!
//! Be clear about what this buys. The key is derived from the signed in Windows
//! account, so the ciphertext is useless to another user, on another machine, or
//! sitting in a backup, and a Quest owner who copies the whole app data folder
//! to their headset carries a dead blob rather than a live token. It is **not**
//! protection against malware already running as that user, which can simply
//! ask the same API to decrypt it, exactly as this program does.

use std::{io, ptr, slice};

use anyhow::{anyhow, Context, Result};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB},
};

/// Describes a slice to the API in the form it expects.
///
/// The blob holds a `*mut` even for input the API only reads.
fn blob(bytes: &[u8]) -> Result<CRYPT_INTEGER_BLOB> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).context("secret is too large to encrypt")?,
        pbData: bytes.as_ptr().cast_mut(),
    })
}

/// Copies out what the API wrote and releases the buffer it allocated.
///
/// # Safety
///
/// `out` must be a blob the data protection API has just filled in, and must
/// not have been released already.
unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // A zero length output would leave the pointer dangling, and building a
    // slice from it would be undefined even for a length of zero.
    if out.pbData.is_null() || out.cbData == 0 {
        return Vec::new();
    }

    let copied = unsafe { slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();

    unsafe { LocalFree(out.pbData.cast()) };

    copied
}

/// Encrypts `plaintext` so that only the current Windows user can read it back.
///
/// No extra entropy is mixed in. It would have to be a constant in a public
/// source file, so it would stop nobody, while permanently orphaning every
/// stored token if it were ever changed.
pub(crate) fn protect(plaintext: &[u8]) -> Result<Vec<u8>> {
    let input = blob(plaintext)?;
    let mut output = CRYPT_INTEGER_BLOB::default();

    let succeeded = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            0,
            &mut output,
        )
    };

    if succeeded == 0 {
        return Err(io::Error::last_os_error()).context("could not encrypt the mod.io token");
    }

    Ok(unsafe { take(output) })
}

/// Reverses [`protect`].
///
/// Fails if the blob was written by a different Windows account or on a
/// different machine, or if it has been tampered with, all of which callers
/// should treat as simply not having a token.
pub(crate) fn unprotect(ciphertext: &[u8]) -> Result<Vec<u8>> {
    let input = blob(ciphertext)?;
    let mut output = CRYPT_INTEGER_BLOB::default();

    // The description the API can hand back would have to be freed as well, and
    // nothing here wants it.
    let succeeded = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            0,
            &mut output,
        )
    };

    if succeeded == 0 {
        return Err(io::Error::last_os_error())
            .context("could not decrypt the stored mod.io token");
    }

    let plaintext = unsafe { take(output) };

    if plaintext.is_empty() {
        return Err(anyhow!("the stored mod.io token decrypted to nothing"));
    }

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Roughly the length of a token mod.io issues, and the length Credential
    /// Manager refused, which is the whole reason this module exists.
    const TOKEN_LEN: usize = 1822;

    #[test]
    fn round_trips_a_token_sized_secret() {
        let secret = "a".repeat(TOKEN_LEN);
        let protected = protect(secret.as_bytes()).unwrap();

        assert_eq!(unprotect(&protected).unwrap(), secret.as_bytes());
    }

    #[test]
    fn the_secret_is_not_left_in_the_ciphertext() {
        let secret = b"a-very-recognisable-token-value";
        let protected = protect(secret).unwrap();

        assert!(
            !protected
                .windows(secret.len())
                .any(|window| window == secret),
            "the secret appears in the ciphertext in the clear",
        );
    }

    #[test]
    fn refuses_a_tampered_blob() {
        let mut protected = protect(b"something worth protecting").unwrap();
        let last = protected.len() - 1;

        protected[last] ^= 0xff;

        assert!(
            unprotect(&protected).is_err(),
            "a tampered blob was accepted",
        );
    }

    #[test]
    fn refuses_something_that_was_never_encrypted() {
        assert!(
            unprotect(b"not a blob at all").is_err(),
            "arbitrary bytes were accepted as a blob",
        );
    }

    /// Not something the program ever does, but the length handling in `take`
    /// is easy to get wrong and undefined behaviour is a poor way to find out.
    /// Whether the API accepts an empty input at all is left open.
    #[test]
    fn an_empty_secret_never_comes_back_as_one() {
        if let Ok(protected) = protect(b"") {
            assert!(unprotect(&protected).is_err());
        }
    }
}
