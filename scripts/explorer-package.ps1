<#
.SYNOPSIS
  Builds what puts "Open with NexSSH" into Windows 11's context menu for folders.

.DESCRIPTION
  The COM server of the menu entry (explorer/command) and the package with external location
  that declares it (explorer/package/AppxManifest.xml), packed with MakeAppx and signed with
  SignTool from the Windows SDK.

  Releases sign it with NexSSH's certificate: NEXSSH_EXPLORER_CERT holds it with its private
  key (a PFX file, base64) and NEXSSH_EXPLORER_CERT_PASSWORD the file's password (the
  EXPLORER_SIGNING_CERT and EXPLORER_SIGNING_PASSWORD secrets). A computer trusts that
  certificate once, which the user allows with administrator rights (explorer/src/trust.rs);
  it stays the same from release to release. Without it (development builds, CI), a
  certificate is made for this build only, and its private key is deleted right after signing.

  Writes NexSSH.msix, NexSSH.cer (the certificate, without its key),
  nexssh_explorer_command.dll and logo.png into -Out. Building NexSSH with
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

# The tools of the newest Windows SDK (bin\10.0.x.y\x64).
$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Directory -Filter '10.*' |
  Where-Object { Test-Path (Join-Path $_.FullName 'x64\makeappx.exe') } |
  Sort-Object { [version]$_.Name } -Descending |
  Select-Object -First 1
if (-not $sdk) { throw 'the Windows SDK (MakeAppx, SignTool) is not installed' }
$makeappx = Join-Path $sdk.FullName 'x64\makeappx.exe'
$signtool = Join-Path $sdk.FullName 'x64\signtool.exe'

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

$pfx = Join-Path $Out 'signing.pfx'
$cer = Join-Path $Out 'NexSSH.cer'
if ($env:NEXSSH_EXPLORER_CERT) {
  try {
    [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:NEXSSH_EXPLORER_CERT))
    & $signtool sign /fd SHA256 /f $pfx /p $env:NEXSSH_EXPLORER_CERT_PASSWORD $msix
    if ($LASTEXITCODE) { throw 'SignTool failed' }
    $cert = [Security.Cryptography.X509Certificates.X509Certificate2]::new($pfx, $env:NEXSSH_EXPLORER_CERT_PASSWORD, 'EphemeralKeySet')
    [IO.File]::WriteAllBytes($cer, $cert.Export('Cert'))
  } finally {
    Remove-Item $pfx -Force -ErrorAction Ignore
  }
} else {
  Write-Warning 'NEXSSH_EXPLORER_CERT is not set: signing with a certificate made for this build only'
  # Subject = the package's publisher (CN=NexSSH); code signing only; valid long enough for any
  # installer of this build.
  $cert = New-SelfSignedCertificate -Type Custom -Subject 'CN=NexSSH' -FriendlyName 'NexSSH Explorer menu' `
    -KeyUsage DigitalSignature -KeyAlgorithm RSA -KeyLength 3072 -NotAfter (Get-Date).AddYears(30) `
    -CertStoreLocation 'Cert:\CurrentUser\My' `
    -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
  try {
    $password = [Guid]::NewGuid().ToString('N')
    Export-PfxCertificate -Cert $cert -FilePath $pfx -Password (ConvertTo-SecureString $password -AsPlainText -Force) | Out-Null
    & $signtool sign /fd SHA256 /f $pfx /p $password $msix
    if ($LASTEXITCODE) { throw 'SignTool failed' }
    Export-Certificate -Cert $cert -FilePath $cer -Type CERT | Out-Null
  } finally {
    # The private key goes: the package is signed, and nothing else ever will be.
    Remove-Item $pfx -Force -ErrorAction Ignore
    Remove-Item "Cert:\CurrentUser\My\$($cert.Thumbprint)" -DeleteKey
  }
}

Copy-Item target\explorer-build\release\nexssh_explorer_command.dll, explorer\package\logo.png $Out
Remove-Item $content -Recurse -Force
Get-ChildItem $Out | Format-Table Name, Length
