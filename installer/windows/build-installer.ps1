<#
.SYNOPSIS
  Build one Basemind per-user Windows setup.exe with Inno Setup 6.

.DESCRIPTION
  Stages the release payload (the same basemind.exe + DLLs that scripts/package-release.sh
  puts in the release .zip), generates the profile config template, invokes ISCC with the
  defines basemind.iss needs, and verifies the produced PE executable. The existing .zip
  is never modified or removed.

.PARAMETER Variant
  full | hacienda-legal. Selects product name, install directory, config template and AppId.

.PARAMETER Triple
  Release target triple. Only x86_64-pc-windows-msvc is supported by this installer.

.PARAMETER Version
  Release version. Defaults to the root Cargo.toml version.

.PARAMETER ZipPath
  Optional release archive to take the payload from (preferred: it is the exact shipped .zip).

.PARAMETER SourceDir
  Optional directory containing an already-built basemind.exe (+ DLLs). Used when ZipPath
  is not supplied. Defaults to target/<triple>/release.

.PARAMETER OutDir
  Directory for the produced setup.exe. Defaults to <repo>/dist/installer.

.PARAMETER RepoRoot
  Repository root. Defaults to the parent of this script's directory's parent.

.EXAMPLE
  powershell -File installer/windows/build-installer.ps1 -Variant full
  powershell -File installer/windows/build-installer.ps1 -Variant hacienda-legal `
    -ZipPath basemind-hacienda-legal-x86_64-pc-windows-msvc.zip
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidateSet('full', 'hacienda-legal')]
  [string]$Variant,

  [string]$Triple = 'x86_64-pc-windows-msvc',
  [string]$Version,
  [string]$ZipPath,
  [string]$SourceDir,
  [string]$OutDir,
  [string]$RepoRoot
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not $RepoRoot) {
  $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
if (-not $OutDir) {
  $OutDir = Join-Path $RepoRoot 'dist\installer'
}
if ($Triple -ne 'x86_64-pc-windows-msvc') {
  throw "This installer targets x86_64-pc-windows-msvc only (got '$Triple')."
}

if (-not $Version) {
  $cargoText = Get-Content -LiteralPath (Join-Path $RepoRoot 'Cargo.toml') -Raw
  if ($cargoText -match '(?m)^version\s*=\s*"([^"]+)"') {
    $Version = $Matches[1]
  }
  else {
    throw 'Could not read the release version from Cargo.toml; pass -Version explicitly.'
  }
}

# Product identity is stable per variant so upgrades replace the same installed app.
if ($Variant -eq 'hacienda-legal') {
  $setupName = "basemind-hacienda-legal-$Triple-setup.exe"
  $appId = '{{6F0D4A8E-2B71-4C35-9A08-1D73E5B4C902}}'
}
else {
  $setupName = "basemind-$Triple-setup.exe"
  $appId = '{{2C9B81F6-5D40-4E2A-8F17-6B0A9C3D5E41}}'
}

# ---------------------------------------------------------------------------
# 1. Stage the payload.
# ---------------------------------------------------------------------------
$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("basemind-installer-$Variant-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage -Force | Out-Null

try {
  if ($ZipPath) {
    $zip = (Resolve-Path -LiteralPath $ZipPath).Path
    Write-Host "==> Staging payload from release archive: $zip"
    Expand-Archive -LiteralPath $zip -DestinationPath $stage -Force
  }
  else {
    if (-not $SourceDir) {
      $SourceDir = Join-Path $RepoRoot "target\$Triple\release"
    }
    $SourceDir = (Resolve-Path -LiteralPath $SourceDir).Path
    Write-Host "==> Staging payload from build output: $SourceDir"
    Copy-Item -LiteralPath (Join-Path $SourceDir 'basemind.exe') -Destination $stage -Force
    foreach ($dll in Get-ChildItem -LiteralPath $SourceDir -Filter '*.dll' -File -ErrorAction SilentlyContinue) {
      Copy-Item -LiteralPath $dll.FullName -Destination $stage -Force
    }
    # Release packagers may place additional DLLs in deps/ next to the binary.
    $deps = Join-Path $SourceDir 'deps'
    if (Test-Path -LiteralPath $deps) {
      foreach ($dll in Get-ChildItem -LiteralPath $deps -Filter '*.dll' -File -ErrorAction SilentlyContinue) {
        Copy-Item -LiteralPath $dll.FullName -Destination $stage -Force
      }
    }
  }

  $exeInStage = Join-Path $stage 'basemind.exe'
  if (-not (Test-Path -LiteralPath $exeInStage)) {
    throw "No basemind.exe in the staged payload ($stage). Build the $Triple release binary first."
  }

  # -------------------------------------------------------------------------
  # 2. Generate the config template shipped with this profile.
  #    basemind reads basemind.toml from each project root; the installer puts a
  #    ready-to-copy template at a predictable per-profile location under %APPDATA%.
  # -------------------------------------------------------------------------
  $header = @(
    '# Basemind Windows installer config template',
    "# Profile: $Variant",
    "# Version: $Version",
    '#',
    '# basemind reads basemind.toml from each project root. Copy this file there',
    '# (or run `basemind init` in that project) before scanning.',
    ''
  )
  if ($Variant -eq 'hacienda-legal') {
    $preset = Join-Path $RepoRoot 'examples\basemind-hacienda-legal.toml'
    if (-not (Test-Path -LiteralPath $preset)) {
      throw "Missing legal preset: $preset"
    }
    $lines = $header + @('# Canonical preset: examples/basemind-hacienda-legal.toml', '') +
             @(Get-Content -LiteralPath $preset)
  }
  else {
    $lines = $header + @('# Full profile. All settings default when omitted; run `basemind init`',
                         '# in a project root for the fully commented scaffold.', '')
  }
  [System.IO.File]::WriteAllLines((Join-Path $stage 'basemind.toml'), [string[]]$lines,
    [System.Text.UTF8Encoding]::new($false))

  # -------------------------------------------------------------------------
  # 3. Locate Inno Setup's compiler.
  # -------------------------------------------------------------------------
  $iscc = $null
  $candidates = @()
  if ($env:ISCC) { $candidates += $env:ISCC }
  $found = Get-Command iscc -ErrorAction SilentlyContinue
  if ($found) { $candidates += $found.Source }
  $candidates += "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
  $candidates += "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
  foreach ($candidate in $candidates) {
    if ($candidate -and (Test-Path -LiteralPath $candidate)) {
      $iscc = (Resolve-Path -LiteralPath $candidate).Path
      break
    }
  }
  if (-not $iscc) {
    throw 'Inno Setup 6 compiler (ISCC.exe) not found. Install Inno Setup 6 or set $env:ISCC to its path.'
  }
  Write-Host "==> Using Inno Setup compiler: $iscc"

  # -------------------------------------------------------------------------
  # 4. Compile.
  # -------------------------------------------------------------------------
  New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
  $issFile = Join-Path $PSScriptRoot 'basemind.iss'
  $isccArgs = @(
    "/O$OutDir",
    "/F$setupName",
    $(if ($Variant -eq 'hacienda-legal') { '/DLegalVariant=1' } else { '/DLegalVariant=0' }),
    "/D`"Variant=$Variant`"",
    "/D`"Version=$Version`"",
    "/D`"Triple=$Triple`"",
    "/D`"SourceDir=$stage`"",
    "/D`"SetupName=$setupName`"",
    "/D`"AppId=$appId`"",
    $issFile
  )
  Write-Host "==> Compiling $setupName (profile: $Variant, version: $Version)"
  & $iscc @isccArgs
  if ($LASTEXITCODE -ne 0) {
    throw "ISCC failed with exit code $LASTEXITCODE"
  }

  # -------------------------------------------------------------------------
  # 5. Verify the artifact is a real PE executable, not an error placeholder.
  # -------------------------------------------------------------------------
  $outExe = Join-Path $OutDir $setupName
  if (-not (Test-Path -LiteralPath $outExe)) {
    throw "Expected installer was not produced: $outExe"
  }
  $stream = [System.IO.File]::OpenRead($outExe)
  try {
    $magic = New-Object byte[] 2
    $null = $stream.Read($magic, 0, 2)
  }
  finally {
    $stream.Dispose()
  }
  if (($magic[0] -ne 0x4D) -or ($magic[1] -ne 0x5A)) {
    throw "Installer is not a PE executable (missing MZ header): $outExe"
  }
  $size = (Get-Item -LiteralPath $outExe).Length
  if ($size -lt 100KB) {
    throw "Installer is suspiciously small ($size bytes): $outExe"
  }
  Write-Host "✓ Created $outExe ($size bytes, PE/MZ verified)"
}
finally {
  Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
