#Requires -Version 5.1
<#
.SYNOPSIS
    Install zgrep from GitHub releases.
.DESCRIPTION
    Downloads the latest (or specified) zgrep release for Windows,
    verifies the SHA256 checksum, and installs the binary.
.PARAMETER Version
    Version tag to install (e.g. v0.1.0). Defaults to latest.
.PARAMETER InstallDir
    Directory to install into. Defaults to ~/.cargo/bin.
#>
[CmdletBinding()]
param(
    [string]$Version,
    [string]$InstallDir
)

$ErrorActionPreference = 'Stop'
$Repo = if ($env:ZGREP_REPO) { $env:ZGREP_REPO } else { 'microsoft/zgrep' }
$Target = 'x86_64-pc-windows-msvc'

function Write-Info { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan }
function Write-Err  { param([string]$Msg) Write-Host "error: $Msg" -ForegroundColor Red; exit 1 }

# Resolve version
if (-not $Version -and $env:ZGREP_VERSION) { $Version = $env:ZGREP_VERSION }
if (-not $Version) {
    Write-Info 'Resolving latest version...'
    $release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
    $Version = $release.tag_name
    if (-not $Version) { Write-Err 'Failed to resolve latest version' }
}
Write-Info "Version: $Version"

# Resolve install directory
if (-not $InstallDir -and $env:ZGREP_INSTALL_DIR) { $InstallDir = $env:ZGREP_INSTALL_DIR }
if (-not $InstallDir) {
    $InstallDir = Join-Path $HOME '.cargo\bin'
    if (-not (Test-Path $InstallDir)) { New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null }
}
Write-Info "Install dir: $InstallDir"

# Download
$baseUrl  = "https://github.com/$Repo/releases/download/$Version"
$archive  = "zgrep-$Version-$Target.zip"
$tmpDir   = Join-Path ([System.IO.Path]::GetTempPath()) "zgrep-install-$([guid]::NewGuid().ToString('N').Substring(0,8))"
New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null

try {
    Write-Info "Downloading $archive..."
    Invoke-WebRequest "$baseUrl/$archive" -OutFile (Join-Path $tmpDir $archive) -UseBasicParsing
    Invoke-WebRequest "$baseUrl/checksums.txt" -OutFile (Join-Path $tmpDir 'checksums.txt') -UseBasicParsing

    # Verify checksum
    Write-Info 'Verifying checksum...'
    $expected = (Get-Content (Join-Path $tmpDir 'checksums.txt') |
        Where-Object { $_ -match $archive } |
        ForEach-Object { ($_ -split '\s+')[0] })
    if (-not $expected) { Write-Err "Archive not found in checksums.txt" }

    $actual = (Get-FileHash (Join-Path $tmpDir $archive) -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $expected) {
        Write-Err "Checksum mismatch: expected $expected, got $actual"
    }
    Write-Info 'Checksum OK'

    # Extract and install
    Write-Info 'Extracting...'
    Expand-Archive -Path (Join-Path $tmpDir $archive) -DestinationPath $tmpDir -Force
    Copy-Item (Join-Path $tmpDir 'zgrep.exe') -Destination (Join-Path $InstallDir 'zgrep.exe') -Force

    # Verify
    $installed = Join-Path $InstallDir 'zgrep.exe'
    if (Test-Path $installed) {
        Write-Info "Installed zgrep to $installed"
    }

    # PATH hint
    if ($env:PATH -notlike "*$InstallDir*") {
        Write-Host ''
        Write-Host "NOTE: $InstallDir is not in your PATH." -ForegroundColor Yellow
        Write-Host "  Add it with:  `$env:PATH += `";$InstallDir`"" -ForegroundColor Yellow
    }
}
finally {
    Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
}
