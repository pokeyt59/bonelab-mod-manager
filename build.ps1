<#
.SYNOPSIS
    Builds the program and puts the result in BUILT\WINDOWS.

.DESCRIPTION
    Cargo has no post build hook and names its output after the crate, so this
    wrapper runs the build and then files the executable under BUILT, one
    folder per operating system.

    Only the operating system you are on can be built here. The release
    workflow produces all three.

.PARAMETER DebugBuild
    Build the debug profile instead of release. Debug builds log everything
    they do, which is what you want when something is behaving oddly.
#>
param([switch]$DebugBuild)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

# The key is compiled in by `option_env!` and cargo tracks it, so building
# without it silently produces a binary that cannot talk to mod.io at all.
if (-not $env:MODIO_API_KEY -and -not (Test-Path '.cargo\config.toml')) {
    Write-Warning 'No MODIO_API_KEY set and no .cargo\config.toml to supply one.'
    Write-Warning 'The result will have no mod.io API key compiled in and will not work.'
}

if ($DebugBuild) {
    $profileDir = 'debug'
    cargo build
} else {
    $profileDir = 'release'
    cargo build --release
}

if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$destination = Join-Path 'BUILT' 'WINDOWS'
$output = Join-Path $destination 'Bonelab-Mod-Manager.exe'

New-Item -ItemType Directory -Force -Path $destination | Out-Null
Copy-Item "target\$profileDir\bonelab_mod_manager.exe" $output -Force

Write-Host ''
Write-Host "Built $profileDir -> $(Resolve-Path $output)"
