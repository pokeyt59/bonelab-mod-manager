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

## Code Mods

Most Bonelab mods are content: a folder holding a pallet, which the game loads
from its own Mods directory. A few are code mods, assemblies that MelonLoader
loads from **beside the game** instead. Fusion, the multiplayer mod, is one.

Those cannot be installed without being told where the game is, because there is
no dependable way to find it: Bonelab ships on two stores and can be moved
anywhere. Set `BMM_GAME_DIR` to the folder holding `BONELAB_Steam_Windows64.exe`
and they will be installed alongside your other mods:

```sh
BMM_GAME_DIR="C:\Program Files (x86)\Steam\steamapps\common\BONELAB"
```

Without it, a code mod is reported as needing that variable rather than being
put somewhere nothing will ever load it. Whatever a code mod places beside the
game is recorded, so unsubscribing removes it again rather than leaving it
loading forever.

MelonLoader itself is not installed by this program. Get it from
[MelonLoader](https://github.com/LavaGang/MelonLoader) first, or code mods will
sit there unread.

## Configuration

Bonelab Mod Manager is configured through environment variables.

| Variable | Purpose |
| --- | --- |
| `BMM_CONCURRENT_DOWNLOADS` | How many mods to install at a time. Defaults to 4, since hardware differs drastically. |
| `BMM_MODS_DIR` | Installs mods here instead of the built-in path. Use this if you run Bonelab through Proton on Linux or the Steam Deck, where the game's `LocalLow` directory lives inside a Wine prefix. |
| `BMM_PLATFORM` | `windows` or `quest`. Overrides the platform you picked on first run, which is otherwise saved for good. |
| `BMM_GAME_DIR` | The folder holding `BONELAB_Steam_Windows64.exe`. Only code mods need it, and they are reported rather than installed without it. See Code Mods above. |
| `BMM_MODIO_TOKEN` | Signs in with this mod.io token instead of prompting. The token is used as-is and is not saved. |
| `MODIO_API_KEY` | The mod.io API key to run against. Only needed for builds that had no key compiled in. |

## Building From Source

You will need **your own** mod.io API key. Generating one is free: sign in at
[mod.io Account Access](https://mod.io/me/access) and create a key for your
account. Use your own rather than one from someone else's build; a key is read
only and cannot touch anybody's account, but requests are rate limited per key,
so sharing one means sharing the limit.

The key is read while you build and compiled into the executable, so whoever
runs the result never has to supply one:

```sh
MODIO_API_KEY=your_key cargo build --release
```

Building **without** a key does not work, on purpose. It would produce an
executable that cannot reach mod.io at all and would only say so once somebody
ran it, so the build stops and tells you where to get one instead. An empty key
is refused for the same reason, which is the shape an unset CI secret arrives
in.

This applies to `cargo test` too, since the tests build the same crate.

Wherever you put your key, it should not end up in the repository.
`.cargo/config.toml` is in `.gitignore` for exactly that reason, so a key kept
there cannot be committed by accident.

### Setting the API Key

Setting the variable in every new terminal gets old, and forgetting it is worse
than it sounds: the key is read at build time, cargo notices when it changes, so
a build in a terminal that lacks it quietly produces an executable with no key
that fails for whoever runs it, with nothing in the build output to say so.

The tidiest answer works the same on every platform. Put it in
`.cargo/config.toml`, which applies to every build in this checkout and is
ignored by git:

```toml
[env]
MODIO_API_KEY = { value = "your_key", force = true }
```

`force` is not decoration. Without it an environment variable of the same name
takes precedence, so a stale `MODIO_API_KEY` left in a shell would be compiled
in instead of this one, silently.

If you would rather use the environment, every one of these works:

| Shell | Scope | How |
| --- | --- | --- |
| PowerShell | this window | `$env:MODIO_API_KEY = "your_key"` |
| PowerShell | permanent | `[Environment]::SetEnvironmentVariable("MODIO_API_KEY", "your_key", "User")` |
| Command Prompt | this window | `set MODIO_API_KEY=your_key` |
| Command Prompt | permanent | `setx MODIO_API_KEY "your_key"` |
| bash or zsh | this shell | `export MODIO_API_KEY=your_key` |
| bash or zsh | permanent | add that `export` line to `~/.bashrc`, `~/.zshrc` or `~/.profile` |
| fish | permanent | `set -Ux MODIO_API_KEY your_key` |
| any | one command | `MODIO_API_KEY=your_key cargo build --release` |

Two things that catch people out. `setx` does not affect the window you run it
in, only ones opened afterwards. And the permanent options are visible to every
program you run, not just this one, which is a reason to prefer the config file.

For CI, none of the above applies: the workflows read a repository secret named
`MODIO_API_KEY`, set under Settings, Secrets and variables, Actions. Dependabot
pull requests read from the separate Dependabot scope on that same page.

### Where Builds Go

`cargo build` leaves its output under `target`, named after the crate. The build
scripts run the build and then file the executable somewhere easier to find,
under `BUILT`, one folder per operating system:

```sh
./build.sh        # Linux and macOS, or Git Bash on Windows
.\build.ps1       # Windows PowerShell
```

```
BUILT/
└── WINDOWS/
    └── Bonelab-Mod-Manager.exe
```

Pass `--debug` (`-DebugBuild` in PowerShell) for a debug build, which logs
everything it does. Both scripts warn if no API key is available rather than
handing you an executable that cannot talk to mod.io.

Only the operating system you are on can be built this way. The Rust Build
workflow produces all three, and can attach them to a release.

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
