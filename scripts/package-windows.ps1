<#
.SYNOPSIS
  Builds the Windows (x64) Tunnel Bar installers with a signed updater artifact, merges the Windows entry into the
  release's latest.json and SHA256SUMS.txt, and optionally uploads everything to the GitHub release.

.DESCRIPTION
  Output lands in dist\: Tunnel-Bar_X.Y.Z_x64-setup.exe (+ .sig), Tunnel-Bar_X.Y.Z_x64_en-US.msi, latest.json and
  SHA256SUMS.txt. latest.json and SHA256SUMS.txt start from the copies already on the vX.Y.Z release (the macOS
  script publishes first) and only add or replace the Windows entries.

  The updater key and its DPAPI-protected password are read at run time and only placed in this process's
  environment for the build; nothing secret is printed or written to disk.

.PARAMETER Publish
  Upload the setup .exe, .sig and .msi, then latest.json, then SHA256SUMS.txt to the vX.Y.Z release (--clobber).

.PARAMETER SkipBuild
  Reuse the existing bundle under src-tauri\target\release\bundle (re-signs and re-verifies it).
#>
[CmdletBinding()]
param(
  [switch]$Publish,
  [switch]$SkipBuild,
  [string]$UpdaterKey = 'D:\dev\certs\tunnelbar-updater.key',
  [string]$PasswordFile = 'D:\dev\certs\tunnelbar-updater.key.password.dpapi',
  [string]$Repo = 'svanvliet/tunnel-bar'
)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

function Invoke-Checked([scriptblock]$Block, [string]$What) {
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed (exit $LASTEXITCODE)" }
}

foreach ($v in 'TUNNELBAR_UPDATE_ENDPOINT', 'TUNNELBAR_TEST_PROFILE', 'TAURI_CONFIG') {
  if ([Environment]::GetEnvironmentVariable($v)) { throw "Refusing to build a release with $v set." }
}
if (-not (Test-Path $UpdaterKey)) { throw "Updater key not found at $UpdaterKey" }
if (-not (Test-Path $PasswordFile)) { throw "Updater key password not found at $PasswordFile" }
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path" }

$version = (Get-Content src-tauri\tauri.conf.json -Raw | ConvertFrom-Json).version
$setup = "Tunnel-Bar_${version}_x64-setup.exe"
$msi = "Tunnel-Bar_${version}_x64_en-US.msi"
$bundle = 'src-tauri\target\release\bundle'
$url = "https://github.com/$Repo/releases/download/v$version/$setup"

$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD =
  [Net.NetworkCredential]::new('', (Get-Content $PasswordFile | ConvertTo-SecureString)).Password
try {
  if (-not $SkipBuild) {
    # Kept out of tauri.conf.json: the macOS script builds without the key and signs its tarball itself.
    $overrides = Join-Path ([IO.Path]::GetTempPath()) 'tunnelbar-windows-release.json'
    Set-Content $overrides '{"bundle":{"createUpdaterArtifacts":true}}' -Encoding ascii
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $UpdaterKey -Raw
    try {
      Invoke-Checked { npx tauri build --bundles nsis,msi --config $overrides } 'tauri build'
    } finally {
      Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
      Remove-Item $overrides -ErrorAction SilentlyContinue
    }
  }

  if (Test-Path dist) { Remove-Item dist -Recurse -Force }
  New-Item -ItemType Directory dist | Out-Null
  # Tauri names bundles after productName ("Tunnel Bar"); release assets use hyphens.
  Copy-Item "$bundle\nsis\$($setup -replace '^Tunnel-Bar', 'Tunnel Bar')" "dist\$setup"
  Copy-Item "$bundle\msi\$($msi -replace '^Tunnel-Bar', 'Tunnel Bar')" "dist\$msi"
  # requireSignedVersion needs version:X.Y.Z in the trusted comment, which the build-time signature lacks.
  Invoke-Checked { npx tauri signer sign -f $UpdaterKey --app-version $version "dist\$setup" | Out-Null } 'tauri signer sign'
} finally {
  Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
}
Invoke-Checked { node scripts\verify-updater-sig.mjs "dist\$setup" "dist\$setup.sig" $version } 'signature check'

$env:GH_TOKEN = gh auth token -u svanvliet
Invoke-Checked { gh release download "v$version" -R $Repo -D dist -p latest.json -p SHA256SUMS.txt } 'release download'

$manifest = Get-Content dist\latest.json -Raw | ConvertFrom-Json
if ($manifest.version -ne $version) { throw "Release latest.json is for $($manifest.version), not $version" }
$entry = [ordered]@{ signature = (Get-Content "dist\$setup.sig" -Raw).Trim(); url = $url }
$manifest.platforms | Add-Member -NotePropertyName 'windows-x86_64' -NotePropertyValue $entry -Force
$utf8 = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText("$PWD\dist\latest.json", ($manifest | ConvertTo-Json -Depth 5) + "`n", $utf8)
$check = Get-Content dist\latest.json -Raw | ConvertFrom-Json
if (-not $check.platforms.'windows-x86_64'.signature) { throw 'latest.json merge failed' }

$names = $setup, "$setup.sig", $msi, 'latest.json'
$sums = @(Get-Content dist\SHA256SUMS.txt | Where-Object { $_ -and ($_ -split '\s+', 2)[1] -notin $names })
$sums += $names | ForEach-Object { '{0}  {1}' -f (Get-FileHash "dist\$_" -Algorithm SHA256).Hash.ToLower(), $_ }
[IO.File]::WriteAllText("$PWD\dist\SHA256SUMS.txt", ($sums -join "`n") + "`n", $utf8)
Get-Content dist\SHA256SUMS.txt

if ($Publish) {
  Invoke-Checked { gh release upload "v$version" -R $Repo --clobber "dist\$setup" "dist\$setup.sig" "dist\$msi" } 'upload installers'
  Invoke-Checked { gh release upload "v$version" -R $Repo --clobber dist\latest.json } 'upload latest.json'
  Invoke-Checked { gh release upload "v$version" -R $Repo --clobber dist\SHA256SUMS.txt } 'upload SHA256SUMS.txt'
  Write-Host "Published Windows assets to v$version."
}
