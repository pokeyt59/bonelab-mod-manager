#!/usr/bin/env bash
#
# Builds the program and puts the result in BUILT/<operating system>.
#
# Cargo has no post build hook and names its output after the crate, so this
# wrapper runs the build and then files the executable under BUILT, one folder
# per operating system. Only the operating system you are on can be built here;
# the release workflow produces all three.
#
# Pass --debug for a debug build, which logs everything it does.

set -euo pipefail

cd "$(dirname "$0")"

profile_dir=release
cargo_args=(build --release)

if [ "${1-}" = "--debug" ]; then
    profile_dir=debug
    cargo_args=(build)
fi

# The build refuses to proceed without a key anyway. Saying so here saves
# compiling every dependency first only to stop at the last crate.
if [ -z "${MODIO_API_KEY-}" ] && [ ! -f .cargo/config.toml ]; then
    echo "No mod.io API key, and the build needs one." >&2
    echo "Get one free at https://mod.io/me/access, then either:" >&2
    echo "    export MODIO_API_KEY=your_key" >&2
    echo "or put it in .cargo/config.toml. See Building From Source in the README." >&2

    exit 1
fi

case "$(uname -s)" in
    Linux*) operating_system=LINUX; suffix= ;;
    Darwin*) operating_system=MACOS; suffix= ;;
    *) operating_system=WINDOWS; suffix=.exe ;;
esac

cargo "${cargo_args[@]}"

destination="BUILT/$operating_system"
output="$destination/Bonelab-Mod-Manager$suffix"

mkdir -p "$destination"

# Windows will not let a running executable be replaced, and says so in a way
# that makes a successful build look like a failed one.
if ! cp "target/$profile_dir/bonelab_mod_manager$suffix" "$output"; then
    echo >&2
    echo "The build worked, but $output could not be replaced." >&2
    echo "That usually means it is still running. Close it and run this again." >&2
    echo "The new executable is at target/$profile_dir/bonelab_mod_manager$suffix." >&2

    exit 1
fi

echo
echo "Built $profile_dir -> $output"
