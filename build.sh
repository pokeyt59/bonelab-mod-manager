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

# The key is compiled in by `option_env!` and cargo tracks it, so building
# without it silently produces a binary that cannot talk to mod.io at all.
if [ -z "${MODIO_API_KEY-}" ] && [ ! -f .cargo/config.toml ]; then
    echo "warning: no MODIO_API_KEY set and no .cargo/config.toml to supply one." >&2
    echo "warning: the result will have no mod.io API key compiled in." >&2
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
cp "target/$profile_dir/bonelab_mod_manager$suffix" "$output"

echo
echo "Built $profile_dir -> $output"
