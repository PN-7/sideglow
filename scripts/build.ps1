# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param(
    [ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')]
    [string]$Target = 'x86_64-pc-windows-msvc'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($env:OS -ne 'Windows_NT') {
    throw 'This script builds the Windows native client on Windows.'
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'Install Rust and the Visual Studio C++ build tools, then open a new terminal.'
}

$projectDirectory = Split-Path -Parent $PSScriptRoot
$originalRustFlags = $env:RUSTFLAGS
Push-Location $projectDirectory
try {
    if (-not (Test-Path -LiteralPath 'Cargo.lock')) {
        throw 'Cargo.lock is missing. Use the complete source package.'
    }
    $localeSourceDirectory = Join-Path $projectDirectory 'locales'
    foreach ($localeFile in @('ko.json', 'en.json', 'ja.json')) {
        $localeSourcePath = Join-Path $localeSourceDirectory $localeFile
        if (-not (Test-Path -LiteralPath $localeSourcePath -PathType Leaf)) {
            throw "Language file is missing: $localeSourcePath. Use the complete source package."
        }
    }

    # Include the Microsoft C runtime in the application. Windows API DLLs
    # remain system dependencies supplied by Windows itself.
    $env:RUSTFLAGS = (($originalRustFlags, '-C target-feature=+crt-static') | Where-Object { $_ }) -join ' '
    & cargo build --release --locked --target $Target
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo build failed with exit code $LASTEXITCODE."
    }

    $builtExe = Join-Path $projectDirectory "target\$Target\release\sideglow.exe"
    if (-not (Test-Path -LiteralPath $builtExe)) {
        throw "Expected executable was not produced: $builtExe"
    }
    $outputDirectory = Join-Path $projectDirectory 'dist'
    New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
    $outputExe = Join-Path $outputDirectory 'sideglow.exe'
    Copy-Item -LiteralPath $builtExe -Destination $outputExe -Force
    $localeOutputDirectory = Join-Path $outputDirectory 'locales'
    New-Item -ItemType Directory -Force -Path $localeOutputDirectory | Out-Null
    # Copy the supplied catalogs and any extra source catalogs. Do not remove
    # user-added packs already present in the output directory.
    foreach ($localeFile in Get-ChildItem -LiteralPath $localeSourceDirectory -Filter '*.json' -File) {
        Copy-Item -LiteralPath $localeFile.FullName -Destination (Join-Path $localeOutputDirectory $localeFile.Name) -Force
    }
    $hash = (Get-FileHash -LiteralPath $outputExe -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  sideglow.exe" | Set-Content -LiteralPath (Join-Path $outputDirectory 'SHA256SUMS.txt') -Encoding ASCII
    Write-Host "Built: $outputExe"
    Write-Host "Languages: $localeOutputDirectory"
    Write-Host "SHA256: $hash"
}
finally {
    if ($null -eq $originalRustFlags) {
        Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
    }
    else {
        $env:RUSTFLAGS = $originalRustFlags
    }
    Pop-Location
}
