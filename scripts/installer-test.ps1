<#
.SYNOPSIS
  Installs the NexSSH installer the ways people and the in-app updater do, and checks "Open with
  NexSSH" in Explorer's menu for folders after each step.

.DESCRIPTION
  What the installer's hooks (desktop/windows/hooks.nsh) and the app behind them
  (desktop/src/explorer.rs) promise:

  1. A silent install, with nobody to ask: the entries the computer allows, classic ones.
  2. An install the user watches: Windows asks once to trust NexSSH's certificate, then the
     entry is in Windows 11's compact menu.
  3. The in-app update: nothing asked, the entry stays.
  4. Another version installed over it, which uninstalls this one first: the trust stays, and
     the entry is back.
  5. Windows' Uninstall: everything goes, the trust too.

  For a computer where Windows never has to ask (CI runs as an administrator, so the steps that
  need administrator rights simply run). NexSSH must not be installed yet, and the computer must
  not trust its certificate; both are so again at the end. Windows PowerShell (Appx module).

.EXAMPLE
  powershell -File scripts/installer-test.ps1
#>
param(
  # The NSIS installer; the one `tauri build` wrote by default.
  [string] $Installer
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

if (-not $Installer) {
  $Installer = (Get-ChildItem 'target\release\bundle\nsis\*-setup.exe' | Select-Object -First 1).FullName
}
if (-not $Installer -or -not (Test-Path $Installer)) { throw 'no installer: build one with `npm run tauri build`' }
$dir = Join-Path $env:LOCALAPPDATA 'NexSSH'
$exe = Join-Path $dir 'NexSSH.exe'
$uninstaller = Join-Path $dir 'uninstall.exe'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\NexSSH'
# Windows 11's compact menu (Windows Server 2025 has it as well); older ones only the classic.
$build = [int](Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').CurrentBuildNumber
$compact = $build -ge 22000
# NexSSH's messages, also when an installer runs it.
$env:NEXSSH_LOG = 'info'

function Trusted {
  [bool](Get-ChildItem Cert:\LocalMachine\TrustedPeople |
    Where-Object { $_.Subject -eq 'CN=NexSSH' -and $_.Issuer -eq 'CN=NexSSH' })
}

function Registered {
  [bool](Get-AppxPackage -Name 'NexSSH.ExplorerMenu')
}

# The classic entry for folders, which the compact menu replaces.
function Classic {
  Test-Path 'HKCU:\Software\Classes\Directory\shell\NexSSH'
}

function Check([string] $What, $Actual, $Expected) {
  if ($Actual -ne $Expected) { throw "${What}: $Actual, expected $Expected" }
  Write-Host "  ok: $What = $Actual"
}

# Runs a program to its end and returns its exit code. Two minutes at most: longer would mean a
# question nobody answers. What NexSSH writes (its log) is shown.
function Run([string] $File, [string[]] $Arguments) {
  Write-Host "> $File $Arguments"
  $log = [IO.Path]::GetTempFileName()
  $process = Start-Process -FilePath $File -ArgumentList $Arguments -PassThru -RedirectStandardError $log
  # The exit code is only kept with the handle taken while the process runs.
  $null = $process.Handle
  $ended = $process.WaitForExit(120000)
  Get-Content $log | ForEach-Object { Write-Host "  | $_" }
  # A program it started may still have the file open.
  Remove-Item $log -ErrorAction Ignore
  if (-not $ended) { throw "$File did not end within two minutes" }
  $process.ExitCode
}

# Waits until the condition holds, a minute at most.
function WaitFor([string] $What, [scriptblock] $Condition) {
  for ($i = 0; $i -lt 60 -and -not (& $Condition); $i++) { Start-Sleep -Seconds 1 }
  if (-not (& $Condition)) { throw "$What did not happen within a minute" }
}

Write-Host "Installer: $Installer; Windows build $build (compact menu: $compact)"
Check 'NexSSH installed before' (Test-Path $exe) $false
Check 'certificate trusted before' (Trusted) $false

try {
  Write-Host '1. Silent install: the classic entries, nobody asked'
  Check 'exit code' (Run $Installer @('/S')) 0
  Check 'NexSSH installed' (Test-Path $exe) $true
  Check 'certificate trusted' (Trusted) $false
  Check 'package registered' (Registered) $false
  Check 'classic entry' (Classic) $true

  Write-Host '2. What an install the user watches runs: trust once, then the compact menu'
  Check 'exit code' (Run $exe @('--explorer-install', '--trust')) 0
  Check 'certificate trusted' (Trusted) $compact
  Check 'package registered' (Registered) $compact
  Check 'classic entry' (Classic) (-not $compact)

  Write-Host '3. In-app update: the entry stays'
  Check 'exit code' (Run $Installer @('/S', '/UPDATE')) 0
  Check 'certificate trusted' (Trusted) $compact
  Check 'package registered' (Registered) $compact
  Check 'classic entry' (Classic) (-not $compact)

  Write-Host '4. Another version over it, uninstalling this one first: the trust stays'
  # What that installer runs (PageLeaveReinstall of the template): this uninstaller, in place.
  Check 'exit code' (Run $uninstaller @('/S', "_?=$dir")) 0
  Check 'NexSSH installed' (Test-Path $exe) $false
  Check 'certificate trusted' (Trusted) $compact
  Check 'package registered' (Registered) $false
  Check 'classic entry' (Classic) $false
  Check 'exit code' (Run $Installer @('/S')) 0
  Check 'certificate trusted' (Trusted) $compact
  Check 'package registered' (Registered) $compact
  Check 'classic entry' (Classic) (-not $compact)

  Write-Host "5. Windows' Uninstall: everything goes"
  # The uninstaller starts a copy of itself from the temporary folder, which does the work.
  Check 'exit code' (Run $uninstaller @('/S')) 0
  WaitFor 'the uninstall' { -not (Test-Path $uninstallKey) -and -not (Classic) }
  Check 'NexSSH installed' (Test-Path $exe) $false
  Check 'certificate trusted' (Trusted) $false
  Check 'package registered' (Registered) $false
  Check 'classic entry' (Classic) $false
} finally {
  # As it was, also after a failure.
  Get-ChildItem Cert:\LocalMachine\TrustedPeople |
    Where-Object { $_.Subject -eq 'CN=NexSSH' -and $_.Issuer -eq 'CN=NexSSH' } |
    Remove-Item
}
Write-Host 'The installer keeps "Open with NexSSH" where it belongs.'
