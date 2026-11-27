# Builds a release of victory-scarlett-play: dist\victory-scarlett-play-<version>.zip and its .sha256 (the launcher's
# updater checks the zip against it). Optionally publishes it as a GitHub release (-Publish, needs `gh` logged in).
#
#   .\scripts\make-release.ps1                 build + pack
#   .\scripts\make-release.ps1 -Publish        ... and create the GitHub release v<version> with both files
#   -Relay https://...workers.dev              the report relay's address (default: this project's relay; '' = reports off)
#
# The version is apps\launcher\Cargo.toml's. The zip holds Launcher.exe, victory\victory.exe (Scarlett, diagnostics
# build, the one Scarlett's figures are measured with), victory-rt\victory.exe (NVIDIA-RT: Kemerd/VIctory-NVIDIA-RT,
# this repository's branch `engine-rt`, release build), LICENSE and README.txt. Never anything from the game, and
# nothing of NVIDIA's (the launcher fetches DLSS from NVIDIA on the player's PC).
#   -RtTree <folder>   a checkout of branch engine-rt (default: a worktree ..\vsp-engine-rt, made if missing)
param([switch] $Publish, [string] $Relay = $(if ($env:VSP_RELAY) { $env:VSP_RELAY } else { 'https://victory-scarlett-relay.htspam1.workers.dev' }), [string] $Notes = '',
    [string] $RtTree = '')
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

# NVIDIA-RT, from branch engine-rt, into a target folder of its own (its crates share names with Scarlett's).
if (-not $RtTree) {
    $RtTree = Join-Path (Split-Path $root) 'vsp-engine-rt'
    if (-not (Test-Path $RtTree)) { git worktree add $RtTree engine-rt; if ($LASTEXITCODE -ne 0) { throw 'no engine-rt worktree' } }
}
$rtTarget = "$target-rt"
$savedTarget = $env:CARGO_TARGET_DIR
Push-Location $RtTree
$env:CARGO_TARGET_DIR = $rtTarget
cargo xtask policy
$ok = $LASTEXITCODE -eq 0
if ($ok) { cargo build --release -p victory --locked; $ok = $LASTEXITCODE -eq 0 }
$env:CARGO_TARGET_DIR = $savedTarget
Pop-Location
if (-not $ok) { throw 'NVIDIA-RT build failed' }
# Staged apart from dist\victory-scarlett-play (a copy someone may be running), and made from nothing every time:
# only the files below go into the zip.
$stageRoot = Join-Path $root 'dist\stage'
$stage = Join-Path $stageRoot 'victory-scarlett-play'
Remove-Item -Recurse -Force $stageRoot -ErrorAction SilentlyContinue
if (Test-Path $stageRoot) { throw "could not clear $stageRoot (a file in it is in use)" }
New-Item -ItemType Directory -Force (Join-Path $stage 'victory') | Out-Null
Copy-Item (Join-Path $target 'diag\victory.exe') (Join-Path $stage 'victory\victory.exe')
New-Item -ItemType Directory -Force (Join-Path $stage 'victory-rt') | Out-Null
Copy-Item (Join-Path $rtTarget 'release\victory.exe') (Join-Path $stage 'victory-rt\victory.exe')
Copy-Item (Join-Path $target 'release\launcher.exe') (Join-Path $stage 'Launcher.exe')
Copy-Item LICENSE $stage
@"
victory-scarlett-play $version

1. Unzip this folder anywhere (not inside your game folder).
2. Run Launcher.exe and pick the folder of your own game dump (the one with game_scarlett_master.exe).
3. The game layer is NVIDIA-RT (DLSS, frame generation, Reflex, HDR10, ultrawide; no setup). Let it fetch
   NVIDIA's DLSS files from NVIDIA once. Optional: tick "Build all shaders first" for the first Play - one
   long wait, then no black or missing graphics while the shaders are built (else the first 10-15 minutes
   in game look unfinished; the SHADERS number at the top counts down to 0).
   On an AMD card the launcher picks Scarlett instead (run its first-run setup once, about 15 minutes).
4. Press Play, then A / Enter on the start screen.

No game files are included. Source code (GPL-3.0-or-later): https://github.com/Harryisadag/victory-scarlett-play
"@ | Set-Content -Encoding utf8 (Join-Path $stage 'README.txt')

$zip = Join-Path $root "dist\victory-scarlett-play-$version.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
# Windows' tar (bsdtar) writes a standard zip with '/' separators (PowerShell 5.1's Compress-Archive writes '\').
Push-Location $stageRoot
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
