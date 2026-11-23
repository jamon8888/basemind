<#
.SYNOPSIS
  End-to-end smoke test for a built Basemind Windows setup.exe.

.DESCRIPTION
  Silently installs the given setup.exe into a throwaway per-user directory, then
  verifies: the binary is present, it launches (--version), the CLI/MCP surface is
  available (serve --help), the installer added its directory to the USER Path,
  the profile config template was written under %APPDATA%, and uninstall removes
  the Path entry and the files. All state is restored on exit.

  This is meant for a Windows runner after installer/windows/build-installers.sh,
  but can be run by hand on any Windows machine with a built setup.exe.
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$SetupExe,

  [Parameter(Mandatory = $true)]
  [ValidateSet('full', 'hacienda-legal')]
  [string]$Variant,

  [string]$ExpectedVersion,
  [switch]$Keep
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert([bool]$Condition, [string]$Message) {
  if (-not $Condition) { throw "SMOKE FAILED: $Message" }
  Write-Host "  ok: $Message"
}

$setup = (Resolve-Path -LiteralPath $SetupExe).Path
$triple = 'x86_64-pc-windows-msvc'
$expectedName = if ($Variant -eq 'hacienda-legal') {
  "basemind-hacienda-legal-$triple-setup.exe"
} else {
  "basemind-$triple-setup.exe"
}
Assert ([System.IO.Path]::GetFileName($setup) -eq $expectedName) "artifact is named $expectedName"

$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("basemind-smoke-" + [guid]::NewGuid().ToString('N'))
$installDir = Join-Path $testRoot 'install'
New-Item -ItemType Directory -Path $installDir -Force | Out-Null

$configFile = Join-Path $env:APPDATA ("Basemind\$Variant\basemind.toml")
$configExisted = Test-Path -LiteralPath $configFile
$configBefore = if ($configExisted) { Get-Content -LiteralPath $configFile -Raw } else { $null }
$pathKey = 'HKCU:\Environment'
$pathBefore = (Get-ItemProperty -Path $pathKey -Name Path -ErrorAction SilentlyContinue).Path

try {
  # -------------------------------------------------------------------------
  Write-Host "==> Installing $([System.IO.Path]::GetFileName($setup)) silently"
  # /DIR must be the LAST argument: Inno treats everything after it as the path.
  $installArgs = @('/VERYSILENT', '/NORESTART', '/NOCANCEL', "/DIR=$installDir")
  $installed = Start-Process -FilePath $setup -ArgumentList $installArgs -Wait -PassThru
  Assert ($installed.ExitCode -eq 0) "silent install returned exit code 0"

  # -------------------------------------------------------------------------
  Write-Host '==> Verifying installation'
  $exe = Join-Path $installDir 'basemind.exe'
  Assert (Test-Path -LiteralPath $exe) 'basemind.exe was installed'
  Assert ((Get-Item -LiteralPath $exe).Length -gt 100KB) 'installed binary is non-trivial in size'

  # -------------------------------------------------------------------------
  Write-Host '==> Verifying launch (CLI + MCP surface)'
  $versionOut = (& $exe --version 2>&1 | Out-String).Trim()
  Assert ($LASTEXITCODE -eq 0) "'basemind --version' exits 0 (output: $versionOut)"
  if ($ExpectedVersion) {
    Assert ($versionOut -match [regex]::Escape($ExpectedVersion)) "reported version contains $ExpectedVersion"
  }
  $serveOut = (& $exe serve --help 2>&1 | Out-String)
  Assert ($LASTEXITCODE -eq 0) "'basemind serve --help' exits 0 (stdio MCP command present)"

  # -------------------------------------------------------------------------
  Write-Host '==> Verifying user PATH modification'
  $pathAfter = (Get-ItemProperty -Path $pathKey -Name Path -ErrorAction SilentlyContinue).Path
  $segments = @()
  if ($pathAfter) { $segments = $pathAfter.Split(';') | ForEach-Object { $_.Trim() } }
  $matched = @($segments | Where-Object { $_ -and ([string]::Compare($_, $installDir, $true) -eq 0) })
  Assert ($matched.Count -eq 1) 'install directory appears exactly once in HKCU\Environment\Path'

  # -------------------------------------------------------------------------
  Write-Host '==> Verifying config template'
  Assert (Test-Path -LiteralPath $configFile) "config template written to $configFile"
  $configText = Get-Content -LiteralPath $configFile -Raw
  Assert ($configText -match "Profile: $Variant") 'config template records the installed profile'
  if ($Variant -eq 'hacienda-legal') {
    Assert ($configText -match '\[documents\]') 'hacienda-legal preset carries the documents section'
    Assert ($configText -match 'embedding_preset') 'hacienda-legal preset carries retrieval settings'
  }

  # -------------------------------------------------------------------------
  Write-Host '==> Verifying uninstall'
  $uninstaller = Join-Path $installDir 'unins000.exe'
  Assert (Test-Path -LiteralPath $uninstaller) 'uninstaller was generated'
  $removed = Start-Process -FilePath $uninstaller -ArgumentList @('/VERYSILENT', '/NORESTART') -Wait -PassThru
  Assert ($removed.ExitCode -eq 0) 'silent uninstall returned exit code 0'
  Assert (-not (Test-Path -LiteralPath $exe)) 'basemind.exe removed by uninstall'
  $pathFinal = (Get-ItemProperty -Path $pathKey -Name Path -ErrorAction SilentlyContinue).Path
  $segmentsFinal = @()
  if ($pathFinal) { $segmentsFinal = $pathFinal.Split(';') | ForEach-Object { $_.Trim() } }
  $stillThere = @($segmentsFinal | Where-Object { $_ -and ([string]::Compare($_, $installDir, $true) -eq 0) })
  Assert ($stillThere.Count -eq 0) 'install directory removed from user PATH by uninstall'

  Write-Host 'SMOKE OK'
}
finally {
  # Restore PATH and config to their pre-test state, then drop the temp tree.
  try {
    if ($null -eq $pathBefore) {
      Remove-ItemProperty -Path $pathKey -Name Path -ErrorAction SilentlyContinue
    }
    else {
      Set-ItemProperty -Path $pathKey -Name Path -Value $pathBefore
    }
  }
  catch {
    Write-Warning "Could not restore user PATH: $_"
  }
  try {
    if (Test-Path -LiteralPath $configFile) {
      if ($configExisted) {
        [System.IO.File]::WriteAllText($configFile, $configBefore)
      }
      else {
        Remove-Item -LiteralPath $configFile -Force
        $profileDir = Split-Path -Parent $configFile
        $baseDir = Split-Path -Parent $profileDir
        if ((Test-Path -LiteralPath $profileDir) -and -not (Get-ChildItem -LiteralPath $profileDir -Force)) {
          Remove-Item -LiteralPath $profileDir -Force
        }
        if ((Test-Path -LiteralPath $baseDir) -and -not (Get-ChildItem -LiteralPath $baseDir -Force)) {
          Remove-Item -LiteralPath $baseDir -Force
        }
      }
    }
  }
  catch {
    Write-Warning "Could not restore config template: $_"
  }
  if (-not $Keep) {
    Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
  else {
    Write-Host "Kept test tree at $testRoot"
  }
}
