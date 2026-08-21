<#
.SYNOPSIS
    Build the Digi-Roll Studio Windows installer.

.DESCRIPTION
    Builds the release exe for x86_64-pc-windows-msvc and compiles
    digi-roll-studio.iss around it, dropping a single setup .exe in dist\.

    Must run on Windows: the MSVC linker produces the binary and the Windows
    SDK's rc.exe stamps the icon into it (see crates/app/build.rs). Cross-
    compiling from macOS is not set up on purpose, which is why the release
    workflow uses a windows runner for this half.

    Needs: the Rust msvc toolchain, and Inno Setup 6.3+ (winget install
    JRSoftware.InnoSetup, or choco install innosetup).

.PARAMETER NoBuild
    Skip cargo and compile the installer around the exe that is already there.
#>
[CmdletBinding()]
param([switch]$NoBuild)

$ErrorActionPreference = 'Stop'

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Push-Location $root
try {
    # One source of truth for the version, the same one the mac script reads.
    $cargo = Get-Content (Join-Path $root 'Cargo.toml') -Raw
    if ($cargo -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
        throw 'Could not read version from [workspace.package] in Cargo.toml'
    }
    $version = $Matches[1]
    Write-Host "==> version $version" -ForegroundColor Cyan

    $target  = 'x86_64-pc-windows-msvc'
    $relDir  = Join-Path $root "target\$target\release"
    $exe     = Join-Path $relDir 'digi_roll_studio.exe'

    if (-not $NoBuild) {
        Write-Host "==> cargo build --release --target $target" -ForegroundColor Cyan
        # .cargo/config.toml adds -C target-feature=+crt-static for this target,
        # so the result does not need a VC redistributable on the target machine.
        & cargo build --release --target $target -p digi_roll_studio
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    }

    if (-not (Test-Path $exe)) { throw "$exe is missing; run without -NoBuild" }

    # If build.rs did not run the resource compiler the exe still works, but it
    # ships with a blank icon and `digi_roll_studio` as its name in every Windows
    # dialog — worth failing the build over rather than shipping.
    $info = (Get-Item $exe).VersionInfo
    if ($info.ProductName -ne 'Digi-Roll Studio') {
        throw ("$exe carries ProductName '$($info.ProductName)' - the version " +
               'resource from crates/app/build.rs is missing. Is rc.exe from the ' +
               'Windows SDK on this machine?')
    }
    Write-Host "==> exe: $($info.ProductName) $($info.FileVersion)" -ForegroundColor Cyan

    # Inno Setup does not put ISCC on PATH by default.
    $iscc = Get-Command ISCC.exe -ErrorAction SilentlyContinue |
            Select-Object -First 1 -ExpandProperty Source
    if (-not $iscc) {
        $iscc = @(
            "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
            "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
            "${env:ProgramFiles(x86)}\Inno Setup 7\ISCC.exe",
            "$env:ProgramFiles\Inno Setup 7\ISCC.exe",
            "${env:LOCALAPPDATA}\Programs\Inno Setup 6\ISCC.exe",
            "${env:LOCALAPPDATA}\Programs\Inno Setup 7\ISCC.exe"
        ) | Where-Object { Test-Path $_ } | Select-Object -First 1
    }
    if (-not $iscc) {
        throw 'ISCC.exe not found. Install Inno Setup 6.3+ (winget install JRSoftware.InnoSetup).'
    }

    New-Item -ItemType Directory -Force -Path (Join-Path $root 'dist') | Out-Null

    # VersionInfoVersion in the .iss is a numeric field: strip any prerelease
    # suffix so tagging v0.2.0-beta does not fail the compile.
    $numeric = ($version -split '[-+]')[0]

    Write-Host "==> $iscc" -ForegroundColor Cyan
    & $iscc `
        "/DAppVersion=$version" `
        "/DVersionInfo=$numeric" `
        "/DSourceDir=$relDir" `
        (Join-Path $PSScriptRoot 'digi-roll-studio.iss')
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }

    $out = Join-Path $root "dist\Digi-Roll-Studio-$version-Windows-x64-Setup.exe"
    if (-not (Test-Path $out)) { throw "expected $out, which is not there" }
    Write-Host "==> done" -ForegroundColor Green
    Write-Host ("  {0}  ({1:N1} MB)" -f $out, ((Get-Item $out).Length / 1MB))
}
finally {
    Pop-Location
}
