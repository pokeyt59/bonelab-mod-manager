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

# The build refuses to proceed without a key anyway. Saying so here saves
# compiling every dependency first only to stop at the last crate.
if (-not $env:MODIO_API_KEY -and -not (Test-Path '.cargo\config.toml')) {
    Write-Host 'No mod.io API key, and the build needs one.' -ForegroundColor Red
    Write-Host 'Get one free at https://mod.io/me/access, then either:'
    Write-Host '    $env:MODIO_API_KEY = "your_key"'
    Write-Host 'or put it in .cargo\config.toml. See Building From Source in the README.'

    exit 1
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

# Windows will not let a running executable be replaced, and the error it gives
# says nothing about why, so a build that succeeded would appear to have failed.
try {
    Copy-Item "target\$profileDir\bonelab_mod_manager.exe" $output -Force -ErrorAction Stop
} catch {
    Write-Host ''
    Write-Host "The build worked, but $output could not be replaced." -ForegroundColor Red
    Write-Host 'That usually means it is still running. Close it and run this again.'
    Write-Host "The new executable is at target\$profileDir\bonelab_mod_manager.exe."

    exit 1
}

Write-Host ''
Write-Host "Built $profileDir -> $(Resolve-Path $output)"
