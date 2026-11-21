<# Destructive integration test for disposable GitHub-hosted Windows runners only. #>
param([Parameter(Mandatory)][string] $Installer)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
  throw 'This test is restricted to disposable GitHub-hosted runners. It replaces the installed NexSSH.'
}
Set-Location (Split-Path $PSScriptRoot)
$dir = Join-Path $env:LOCALAPPDATA 'NexSSH'
$data = Join-Path $env:APPDATA 'NexSSH'
$uninstaller = Join-Path $dir 'uninstall.exe'
if (Test-Path -LiteralPath $uninstaller) { throw 'NexSSH is already installed on the runner' }
$baseline = Join-Path (Get-Location).Path 'target\upgrade-baseline'
New-Item -ItemType Directory -Force $baseline | Out-Null
$releases = gh release list --repo $env:GITHUB_REPOSITORY --limit 100 --json tagName,isPrerelease,isDraft | ConvertFrom-Json
if ($LASTEXITCODE) { throw 'Cannot list baseline releases' }
$tag = ($releases | Where-Object { -not $_.isPrerelease -and -not $_.isDraft -and $_.tagName -match '^v1\.\d+\.\d+$' } | Select-Object -First 1).tagName
if (-not $tag) { throw 'A stable 1.x installer is required for the migration test' }
gh release download $tag --repo $env:GITHUB_REPOSITORY --pattern '*x64-setup.exe' --dir $baseline
if ($LASTEXITCODE) { throw 'Cannot download baseline installer' }
function Run([string] $file, [string] $arguments) {
  $process = Start-Process -FilePath $file -ArgumentList $arguments -WindowStyle Hidden -PassThru
  if (-not $process.WaitForExit(120000)) { throw "Installer timed out: $file" }
  if ($process.ExitCode -ne 0) { throw "Installer returned $($process.ExitCode): $file" }
}
function DataHashes {
  @(Get-ChildItem -LiteralPath $data -File | Sort-Object Name | ForEach-Object { "$($_.Name):$((Get-FileHash -LiteralPath $_.FullName).Hash)" }) -join "`n"
}
New-Item -ItemType Directory -Force $data | Out-Null
# No secrets: these files stand in for data owned by the installed 1.x client.
$utf8 = New-Object System.Text.UTF8Encoding $false
[IO.File]::WriteAllText((Join-Path $data 'settings.json'), '{"language":"ru","theme":"black","explorerMenu":false,"fontSize":15,"autoUpdateCheck":false}', $utf8)
[IO.File]::WriteAllText((Join-Path $data 'servers.json'), '{"version":1,"servers":[],"groups":[]}', $utf8)
[IO.File]::WriteAllText((Join-Path $data 'known_hosts'), '# preserved known hosts', $utf8)
$before = DataHashes
try {
  Run (Get-ChildItem -LiteralPath $baseline -Filter '*-setup.exe').FullName '/S'
  Run $Installer '/S /UPDATE'
  if (-not (Test-Path (Join-Path $dir 'NexSSH.exe'))) { throw 'Slint executable is missing' }
  if (-not (Test-Path (Join-Path $dir 'vcruntime140.dll'))) { throw 'The app-local MSVC runtime is missing' }
  $version = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\NexSSH').DisplayVersion
  if ($version -notmatch '^2\.') { throw "Upgrade did not install 2.x: $version" }
  if ((DataHashes) -ne $before) { throw 'The 1.x user data changed during upgrade' }
  # Repeat the path used by the native updater; /R is omitted to keep the runner noninteractive.
  Run $Installer '/S /UPDATE'
  if ((DataHashes) -ne $before) { throw 'The 2.x update changed user data' }
} finally {
  if (Test-Path -LiteralPath $uninstaller) { Run $uninstaller '/S' }
}
if ((DataHashes) -ne $before) { throw 'Uninstall deleted user data' }
Write-Host "Verified $tag -> Slint -> Slint; settings, servers and known_hosts preserved"
