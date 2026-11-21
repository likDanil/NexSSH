<# Builds the Slint executable and compatible NSIS installer. Does not install or publish it. #>
param(
  [string] $Out = 'target\native-dist',
  [switch] $SkipBuild,
  [switch] $SkipExplorer,
  [switch] $RequireSignature
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path $PSScriptRoot
Set-Location $root
$version = (Select-String -Path native/Cargo.toml -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
if ($version -notmatch '^2\.\d+\.\d+(-(alpha|beta|rc)\.[1-9]\d*)?$') { throw "Unsupported native release version: $version" }
if ($RequireSignature -and -not $env:TAURI_SIGNING_PRIVATE_KEY) { throw 'TAURI_SIGNING_PRIVATE_KEY is required' }
if ($RequireSignature -and $env:NEXSSH_UPDATER_PUBLIC_KEY) { throw 'Publishing with a public-key override is forbidden' }
if ($RequireSignature -and $SkipBuild) { throw 'Publishing requires a fresh build' }
if ($RequireSignature -and (-not $env:NEXSSH_EXPLORER_CERT -or $SkipExplorer)) { throw 'A release needs the existing Explorer certificate' }
$outPath = [IO.Path]::GetFullPath((Join-Path $root $Out))
if ($outPath -eq $root -or -not $outPath.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Output must be inside the repository' }
New-Item -ItemType Directory -Force $outPath | Out-Null
if (Get-ChildItem -LiteralPath $outPath -File) { throw "Output is not empty: $outPath" }
$oldPackage = $env:NEXSSH_EXPLORER_PACKAGE
try {
  if (-not $SkipExplorer) {
    & "$PSScriptRoot\explorer-package.ps1" -Version ($version -replace '-.*$', '')
    $env:NEXSSH_EXPLORER_PACKAGE = Join-Path $root 'target\explorer-package'
  } else { $env:NEXSSH_EXPLORER_PACKAGE = $null }
  if (-not $SkipBuild) {
    cargo build --locked -p nexssh-slint --release
    if ($LASTEXITCODE) { throw 'Slint build failed' }
  }
  $targetDir = if ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) } else { Join-Path $root 'target' }
  $releaseDir = Join-Path $targetDir 'release'
  $sourceBinary = Join-Path $releaseDir 'nexssh-slint.exe'
  if ((Get-Item -LiteralPath $sourceBinary).VersionInfo.ProductVersion -ne $version) { throw 'The built executable has a different version' }
  # Rust's MSVC build imports VCRUNTIME140.dll. Ship the redistributable beside the app,
  # rather than requiring a separate machine-wide installation on a clean Windows PC.
  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  $runtime = & $vswhere -latest -products '*' -find 'VC\Redist\MSVC\**\x64\Microsoft.VC*.CRT\vcruntime140.dll' |
    Where-Object { $_ -notmatch '\\onecore\\' } |
    Sort-Object { [version](Get-Item -LiteralPath $_).VersionInfo.FileVersion } -Descending | Select-Object -First 1
  if (-not $runtime -or -not (Test-Path -LiteralPath $runtime)) { throw 'The MSVC x64 redistributable vcruntime140.dll is missing from Visual Studio' }
  $runtimeDir = Join-Path $root 'target\native-runtime'
  New-Item -ItemType Directory -Force $runtimeDir | Out-Null
  Copy-Item -LiteralPath $runtime -Destination (Join-Path $runtimeDir 'vcruntime140.dll')
  $binary = Join-Path $releaseDir 'NexSSH.exe'
  Copy-Item -LiteralPath $sourceBinary -Destination $binary
  # Use the existing bundler and installer identity. The application itself has no Tauri runtime.
  $config = @{
    version = $version
    build = @{ beforeBuildCommand = $null; beforeBundleCommand = $null }
    bundle = @{
      targets = @('nsis'); createUpdaterArtifacts = $false
      resources = @{ '../target/native-runtime/vcruntime140.dll' = 'vcruntime140.dll' }
      windows = @{
        webviewInstallMode = @{ type = 'skip' }; allowDowngrades = $false
        nsis = @{ installerHooks = (Join-Path $root 'native\windows\hooks.nsh') }
      }
    }
  }
  $configPath = Join-Path $outPath 'bundle-config.json'
  $config | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $configPath -Encoding utf8
  & "$root\node_modules\.bin\tauri.cmd" bundle --ci --bundles nsis --no-binary-patching --config $configPath
  if ($LASTEXITCODE) { throw 'NSIS bundling failed' }
  $installer = Join-Path $releaseDir "bundle\nsis\NexSSH_${version}_x64-setup.exe"
  if (-not (Test-Path -LiteralPath $installer)) { throw "Installer missing: $installer" }
  $destination = Join-Path $outPath ([IO.Path]::GetFileName($installer))
  Copy-Item -LiteralPath $installer -Destination $destination
  if ($env:TAURI_SIGNING_PRIVATE_KEY) {
    & "$root\node_modules\.bin\tauri.cmd" signer sign --app-version $version $destination | Out-Null
    if ($LASTEXITCODE) { throw 'Signing the installer failed' }
  }
  $hash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
  "$hash  $([IO.Path]::GetFileName($destination))" | Set-Content (Join-Path $outPath 'SHA256SUMS.txt') -Encoding ascii
  Remove-Item -LiteralPath $configPath
  Write-Host "Prepared NexSSH $version in $outPath"
} finally {
  $env:NEXSSH_EXPLORER_PACKAGE = $oldPackage
}
