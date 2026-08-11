# Bonelab Mod Manager [![Rust CI](https://github.com/valentinegb/bonelab_mod_manager/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/valentinegb/bonelab_mod_manager/actions/workflows/rust-ci.yml)

Keeps your installed Bonelab mods synced with your subscribed mod.io mods.

<img width="858" alt="Screenshot 2023-09-08 at 4 38 43 PM" src="https://github.com/valentinegb/bonelab_mod_manager/assets/35977727/b2c810b3-586b-4103-ad96-f1bc449abf30">

## Features

- Multi-platform
  - macOS (Apple Silicon and Intel)
  - Linux (x64)
  - Windows (x64)
- PC and Quest mod support
- Parallel downloads
- Installs subscribed mod.io mods
- Installs PC mods directly into Bonelab installation
- Updates mods
- Removes unsubscribed mods
- Built with [Rust](http://rust-lang.org)

You can see this project's backlog [here](https://github.com/users/valentinegb/projects/4).

## How to Install

### Windows

> [!NOTE]
> You may be prevented from launching the program by a popup warning you that the program is not safe. It is, and if you don't believe me you can check the code yourself. To launch the program anyway, click "More info" then "Run anyway".

Go to the [latest release on GitHub](https://github.com/valentinegb/bonelab_mod_manager/releases/latest) and download `bonelab_mod_manager.exe`. That's it, you can put that exectuable wherever you like.

### macOS

> [!NOTE]
> Unfortunately, I am not part of Apple's developer program so I am unable to sign the mod manager macOS app package. This means that when you first attempt to launch the app, macOS will stop you saying that the developer cannot be verified and that the application may contain malware. To circumvent this, you can right click the app in your Dock, click Options then Show in Finder, right click the app in finder, click Open, then in the popup click Open again. From there on you should not be prevented from launching the app normally again.

#### Homebrew (Recommended)

[Homebrew](https://brew.sh) is a package manager for macOS. Since Bonelab Mod Manager is not available on the App Store and it does contain a self-updater, this is the recommended installation method if you want easy updates.

Once Homebrew is installed, which can be done from the hyperlink above, to install Bonelab Mod Manager you simply need to enter the following into your terminal:

```zsh
brew install valentinegb/tap/bonelab-mod-manager
```

To update later on, enter this command:

```zsh
brew upgrade bonelab-mod-manager
```

#### Manual

Go to the [latest release on GitHub](https://github.com/valentinegb/bonelab_mod_manager/releases/latest) and download `bonelab_mod_manager.dmg`. When the disk image has downloaded, open it and drag the Bonelab Mod Manager app into the Applications alias. You will have to repeat this process to update.

### Linux

Go to the [latest release on GitHub](https://github.com/valentinegb/bonelab_mod_manager/releases/latest) and download `bonelab_mod_manager.tar.gz`. When the archive has downloaded, extract it. You can put the contained executable wherever you like.

## How to Use

### Initial Setup

There are some steps that you must take once the first time that you launch the program.

1. If you are on macOS or Linux, you will see a popup for keychain or keyring, respectively. Type in your user password.
   > Bonelab Mod Manager uses keychain/keyring to securely store your mod.io token. This ensures that no program can access your token without explicity asking user permission, this includes Bonelab Mod Manager.
2. If you are on Windows, you will be asked whether you play Bonelab on Windows or Quest. This comes first because mod.io is told which platform to serve mod files for when you sign in.
3. You will be prompted to sign in to mod.io. Use the arrow keys to select your method of signing in then press enter.
   - If you select "Send me an email code" you will need to type in the email associated with your mod.io account. You will shortly after receive an email from mod.io containing a code, which you will also need to enter into the program.
   - Selecting "Let me input my token" is a little more complicated. You will need to go to [mod.io Account Access](https://mod.io/me/access) and generate a token with read permissions. (You do **NOT** need to give write permissions and I advise that you don't.) You can name the client and the token whatever you'd like, it doesn't matter. The token is hidden as you type it; if your terminal refuses to paste into a hidden prompt, set the `BMM_MODIO_TOKEN` environment variable instead.
   - "Go back and change platform" returns you to the question in step 2, in case you picked the wrong one. This option is not shown if `BMM_PLATFORM` is set, since that would override whatever you chose.

Picked the wrong platform and only noticed later? Run `--sign-out`, which brings the sign in prompt back, and go back from there.

After these steps, you can use Bonelab Mod Manager like normal.

### Install a Mod

1. Subscribe to the mod on [mod.io](https://mod.io/g/bonelab).
2. Launch Bonelab Mod Manager and it will install the mod for you.

### Update a Mod

1. Ensure you are subscribed to the mod on [mod.io](https://mod.io/g/bonelab).
2. Launch Bonelab Mod Manager and it will update any mods that are out-of-date.

### Remove a Mod

1. Unsubscribe to the mod on [mod.io](https://mod.io/g/bonelab).
2. Launch Bonelab Mod Manager and it will remove the mod.

### Sign Out

Launch the program with `--sign-out` to forget your saved mod.io token:

```sh
bonelab_mod_manager --sign-out
```

The next launch will ask how you would like to sign in again, so this is also
how you switch mod.io accounts. Your installed mods and your platform choice
are kept.

If you launch the program by double clicking it, make a shortcut to the
executable and add ` --sign-out` to the end of the shortcut's Target field.

### Where Your Sign In Is Stored

| Platform | Location |
| --- | --- |
| Windows | `%AppData%\bonelab_mod_manager\app_data`, encrypted for your Windows account |
| macOS | Keychain, under `bonelab_mod_manager` |
| Linux | Secret Service (GNOME Keyring or KWallet), under `bonelab_mod_manager` |

Windows does not use Credential Manager for this. A mod.io token is around 1800
characters, and Credential Manager refuses anything over 2560 bytes once the
value is encoded as UTF-16. The token is encrypted with the Windows data
protection API instead, which ties it to your Windows account, so the file is
useless on another account, on another machine, or sitting in a backup. It is
**not** protection against something already running as you, which can ask
Windows to decrypt it exactly as this program does.

An older install whose token is still in plain text is upgraded the first time
it runs; you will not have to sign in again.

If you play on Quest, the staging folder you copy mods from,
`%AppData%\bonelab_mod_manager\Mods`, sits next to that file. Copying the `Mods`
folder rather than the folder containing it is still the tidy thing to do, but
your sign in is no longer readable if the file does travel.

## Mod Installation Paths

PC mods are installed directly into your Bonelab installation, at
`%UserProfile%/AppData/LocalLow/Stress Level Zero/BONELAB/Mods`. This is the
same path for both the Steam and the Meta PC versions of the game.

Quest mods must be manually put onto your headset, so they are installed to a
staging directory that depends on your OS.

macOS: `~/Library/Application Support/com.valentinegb.bonelab_mod_manager/Mods`

Linux: `~/var/lib/bonelab_mod_manager/Mods`

Windows: `%AppData%/bonelab_mod_manager/Mods`

Copy them from there to `Android/data/com.StressLevelZero.BONELAB/files/Mods` on
your headset.

I know, it isn't ideal having to move the mods from this folder to your headset,
but it's on the roadmap for the mod manager to do that for you, it will in the future!

## Configuration

Bonelab Mod Manager is configured through environment variables.

| Variable | Purpose |
| --- | --- |
| `BMM_CONCURRENT_DOWNLOADS` | How many mods to install at a time. Defaults to 4, since hardware differs drastically. |
| `BMM_MODS_DIR` | Installs mods here instead of the built-in path. Use this if you run Bonelab through Proton on Linux or the Steam Deck, where the game's `LocalLow` directory lives inside a Wine prefix. |
| `BMM_PLATFORM` | `windows` or `quest`. Overrides the platform you picked on first run, which is otherwise saved for good. |
| `BMM_MODIO_TOKEN` | Signs in with this mod.io token instead of prompting. The token is used as-is and is not saved. |
| `MODIO_API_KEY` | The mod.io API key to run against. Only needed for builds that had no key compiled in. |

## Building From Source

Bonelab Mod Manager talks to mod.io with an API key, which you can generate at
[mod.io Account Access](https://mod.io/me/access). Set `MODIO_API_KEY` when you
build to bake the key into the executable:

```sh
MODIO_API_KEY=your_key cargo build --release
```

You can also build without a key and supply `MODIO_API_KEY` at run time instead.

### Testing

`cargo test` runs the offline tests and skips the one that talks to mod.io.

There is also a live test that downloads and installs a real mod, which is the
only way to cover platform file selection, the download itself, and extracting
an archive we did not build. It needs a key and the network:

```sh
MODIO_API_KEY=your_key cargo test -- --ignored --nocapture
```

Build with `MODIO_API_KEY` unset if you do not want the key compiled into the
test binary, then set it only when running the binary. It installs into a
temporary directory, so your Mods folder is not touched.
