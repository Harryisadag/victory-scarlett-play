# Builds a release of victory-scarlett-play: dist\victory-scarlett-play-<version>.zip and its .sha256 (the launcher's
# updater checks the zip against it). Optionally publishes it as a GitHub release (-Publish, needs `gh` logged in).
#
#   .\scripts\make-release.ps1                 build + pack
#   .\scripts\make-release.ps1 -Publish        ... and create the GitHub release v<version> with both files
#   -Relay https://...workers.dev              the report relay's address built into the launcher (else reports off)
#
# The version is apps\launcher\Cargo.toml's. The zip holds Launcher.exe, victory\victory.exe (diagnostics build, the
# one Scarlett's figures are measured with), LICENSE and README.txt. Never anything from the game.
param([switch] $Publish, [string] $Relay = $env:VSP_RELAY, [string] $Notes = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path apps\launcher\Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
if (-not $version) { throw 'no version in apps\launcher\Cargo.toml' }
Write-Host "victory-scarlett-play $version"

if ($Relay) { $env:VSP_RELAY = $Relay } else { Remove-Item Env:VSP_RELAY -ErrorAction SilentlyContinue }
cargo xtask policy
if ($LASTEXITCODE -ne 0) { throw 'asset policy failed' }
cargo build --profile diag -p victory --features diagnostics --locked
if ($LASTEXITCODE -ne 0) { throw 'victory build failed' }
cargo build --release -p launcher --locked
if ($LASTEXITCODE -ne 0) { throw 'launcher build failed' }

$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$stage = Join-Path $root 'dist\victory-scarlett-play'
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force (Join-Path $stage 'victory') | Out-Null
Copy-Item (Join-Path $target 'diag\victory.exe') (Join-Path $stage 'victory\victory.exe')
Copy-Item (Join-Path $target 'release\launcher.exe') (Join-Path $stage 'Launcher.exe')
Copy-Item LICENSE $stage
@"
victory-scarlett-play $version

1. Unzip this folder anywhere (not inside your game folder).
2. Run Launcher.exe and pick the folder of your own game dump (the one with game_scarlett_master.exe).
3. Run the first-run setup once (about 15 minutes), then press Play.

No game files are included. Source code (GPL-3.0-or-later): https://github.com/Harryisadag/victory-scarlett-play
"@ | Set-Content -Encoding utf8 (Join-Path $stage 'README.txt')

$zip = Join-Path $root "dist\victory-scarlett-play-$version.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
# Windows' tar (bsdtar) writes a standard zip with '/' separators (PowerShell 5.1's Compress-Archive writes '\').
Push-Location (Join-Path $root 'dist')
tar -a -c -f $zip victory-scarlett-play
$ok = $LASTEXITCODE -eq 0
Pop-Location
if (-not $ok) { throw 'zip failed' }
$hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
"$hash  victory-scarlett-play-$version.zip" | Set-Content -Encoding ascii "$zip.sha256"
Write-Host "$zip"
Write-Host "sha256 $hash"

if ($Publish) {
    if (-not $Notes) { $Notes = "victory-scarlett-play $version" }
    gh release create "v$version" $zip "$zip.sha256" --repo Harryisadag/victory-scarlett-play --title "v$version" --notes $Notes
    if ($LASTEXITCODE -ne 0) { throw 'gh release create failed' }
}
