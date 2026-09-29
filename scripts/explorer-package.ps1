<#
.SYNOPSIS
  Builds what puts "Open with NexSSH" into Windows 11's context menu for folders.

.DESCRIPTION
  The COM server of the menu entry (explorer/command) and the package with external location
  that declares it (explorer/package/AppxManifest.xml), packed with MakeAppx from the Windows
  SDK.

  The package is not signed: it holds only its manifest and logo, no program code, and Windows
  11 lets a user register such a package without a signature and without administrator rights
  (explorer/src/register.rs). The COM server goes next to it, in its external location.

  Writes NexSSH.msix, nexssh_explorer_command.dll and logo.png into -Out. Building NexSSH with
  NEXSSH_EXPLORER_PACKAGE set to that folder embeds them (desktop/build.rs).

.EXAMPLE
  pwsh scripts/explorer-package.ps1
  $env:NEXSSH_EXPLORER_PACKAGE = "$PWD\target\explorer-package"; npm run tauri build
#>
param(
  # The package's version; NexSSH's (Cargo.toml) by default.
  [string] $Version,
  [string] $Out = 'target\explorer-package'
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

if (-not $Version) {
  $Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
}
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "'$Version' is not a version like 1.2.3" }

# The C runtime goes into the DLL, so it needs no Visual C++ Redistributable; in a target
# folder of its own, so that these flags do not rebuild everything else.
$rustflags = $env:RUSTFLAGS
try {
  $env:RUSTFLAGS = '-C target-feature=+crt-static'
  cargo build --release -p nexssh-explorer-command --target-dir target\explorer-build
  if ($LASTEXITCODE) { throw 'building the COM server failed' }
} finally {
  $env:RUSTFLAGS = $rustflags
}

# MakeAppx of the newest Windows SDK (bin\10.0.x.y\x64).
$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Directory -Filter '10.*' |
  Where-Object { Test-Path (Join-Path $_.FullName 'x64\makeappx.exe') } |
  Sort-Object { [version]$_.Name } -Descending |
  Select-Object -First 1
if (-not $sdk) { throw 'the Windows SDK (MakeAppx) is not installed' }
$makeappx = Join-Path $sdk.FullName 'x64\makeappx.exe'

# Relative to the repository.
if (-not [IO.Path]::IsPathRooted($Out)) { $Out = Join-Path (Get-Location).Path $Out }
Remove-Item $Out -Recurse -Force -ErrorAction Ignore
$content = Join-Path $Out 'content'
New-Item -ItemType Directory $content | Out-Null

$manifest = (Get-Content explorer\package\AppxManifest.xml -Raw).Replace('@VERSION@', "$Version.0")
[IO.File]::WriteAllText((Join-Path $content 'AppxManifest.xml'), $manifest)
Copy-Item explorer\package\logo.png $content
$msix = Join-Path $Out 'NexSSH.msix'
# /nv: the manifest names files that are outside the package (the COM server).
& $makeappx pack /o /nv /d $content /p $msix
if ($LASTEXITCODE) { throw 'MakeAppx failed' }

Copy-Item target\explorer-build\release\nexssh_explorer_command.dll, explorer\package\logo.png $Out
Remove-Item $content -Recurse -Force
Get-ChildItem $Out | Format-Table Name, Length
