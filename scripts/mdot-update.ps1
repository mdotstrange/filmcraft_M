<#
.SYNOPSIS
  Pull official FilmCraft updates into the `mdot` customization branch, build, and install.

.DESCRIPTION
  Branch layout:
    main  - clean mirror of upstream (github.com/storytold/filmcraft), never edited by hand
    mdot  - main + local customizations (this script lives only here)

  Steps:
    1. git fetch upstream, fast-forward main to upstream/main, push main to origin
    2. merge main into mdot (stops on conflicts, leaving the merge in progress to resolve)
    3. release build of filmcraft + filmcraft-cli, same flags as packaging/windows/package.ps1
    4. copy both exes to %LOCALAPPDATA%\FilmCraft-M (official install in Program Files is untouched)
       and create Start-menu / desktop shortcuts on first install
    5. smoke-test filmcraft-cli --version

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\mdot-update.ps1
  powershell -ExecutionPolicy Bypass -File scripts\mdot-update.ps1 -SkipPull   # rebuild current mdot only
  powershell -ExecutionPolicy Bypass -File scripts\mdot-update.ps1 -NoPush     # don't push to origin
#>
param(
  [switch] $SkipPull,
  [switch] $SkipBuild,
  [switch] $NoPush,
  [string] $InstallDir = (Join-Path $env:LOCALAPPDATA 'FilmCraft-M')
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Target = 'x86_64-pc-windows-msvc'

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

function RepoGit { & git -C $Root @args }

# rustup installs to ~/.cargo/bin; a shell opened before the install doesn't have it on PATH yet.
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  $env:Path = (Join-Path $env:USERPROFILE '.cargo\bin') + ';' + $env:Path
}

# ---- 1-2. sync with upstream ----------------------------------------------------------------------
if (-not $SkipPull) {
  if (RepoGit status --porcelain --untracked-files=no) {
    throw 'working tree has uncommitted changes; commit or stash them first'
  }
  if (-not (RepoGit remote | Where-Object { $_ -eq 'upstream' })) {
    Invoke-Native 'add upstream remote' { RepoGit remote add upstream https://github.com/storytold/filmcraft.git }
  }
  Invoke-Native 'git fetch upstream' { RepoGit fetch upstream }

  Invoke-Native 'checkout main' { RepoGit checkout --quiet main }
  Invoke-Native 'fast-forward main to upstream/main' { RepoGit merge --ff-only upstream/main }
  if (-not $NoPush) {
    Write-Output '==> push main to origin'
    & git -C $Root push origin main
    if ($LASTEXITCODE -ne 0) { Write-Warning 'push of main to origin failed; continuing' }
  }

  Invoke-Native 'checkout mdot' { RepoGit checkout --quiet mdot }
  Write-Output '==> merge main into mdot'
  & git -C $Root merge --no-edit main
  if ($LASTEXITCODE -ne 0) {
    Write-Output ''
    Write-Output 'MERGE CONFLICT: upstream changed code that mdot customizes.'
    Write-Output 'Resolve the conflicts (git status), commit, then rerun with -SkipPull.'
    Write-Output 'To abandon this update instead: git merge --abort'
    exit 1
  }
  if (-not $NoPush) {
    Write-Output '==> push mdot to origin'
    & git -C $Root push origin mdot
    if ($LASTEXITCODE -ne 0) { Write-Warning 'push of mdot to origin failed; continuing' }
  }
}

# ---- 3. build ------------------------------------------------------------------------------------
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
$Bin = Join-Path $TargetDir "$Target\release"
if (-not $SkipBuild) {
  # Same as packaging/windows/package.ps1: static CRT, build stamp, embedded icon/VERSIONINFO.
  $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = '-C target-feature=+crt-static'
  $env:FILMCRAFT_REQUIRE_WINRES = '1'
  $env:FILMCRAFT_BUILD_SHA = (RepoGit rev-parse HEAD)
  $env:FILMCRAFT_BUILD_DATE = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')
  # Optional Japanese glyphs: clone https://github.com/storytold/craft-fonts next to the repo.
  $fonts = Join-Path (Split-Path $Root -Parent) 'craft-fonts'
  if (-not $env:CRAFT_FONTS_DIR -and (Test-Path $fonts)) { $env:CRAFT_FONTS_DIR = $fonts }
  Invoke-Native "cargo build --release ($Target)" {
    cargo build --manifest-path (Join-Path $Root 'Cargo.toml') --release --locked -p filmcraft -p filmcraft-cli --target $Target
  }
}

# ---- 4. install ----------------------------------------------------------------------------------
$running = Get-Process filmcraft -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -and $_.Path.StartsWith($InstallDir, [StringComparison]::OrdinalIgnoreCase) }
if ($running) { throw "FilmCraft-M is running from $InstallDir; close it and rerun with -SkipPull -SkipBuild" }

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
foreach ($exe in 'filmcraft.exe', 'filmcraft-cli.exe') {
  $src = Join-Path $Bin $exe
  if (-not (Test-Path $src)) { throw "missing build output $src" }
  Copy-Item -Force $src $InstallDir
}
Write-Output "==> installed to $InstallDir"

$shell = New-Object -ComObject WScript.Shell
$links = @(
  (Join-Path ([Environment]::GetFolderPath('Programs')) 'FilmCraft-M.lnk'),
  (Join-Path ([Environment]::GetFolderPath('Desktop')) 'FilmCraft-M.lnk')
)
foreach ($link in $links) {
  if (Test-Path $link) { continue }
  $s = $shell.CreateShortcut($link)
  $s.TargetPath = Join-Path $InstallDir 'filmcraft.exe'
  $s.WorkingDirectory = $InstallDir
  $s.Description = 'FilmCraft (mdot custom build)'
  $s.Save()
  Write-Output "==> created shortcut $link"
}

# ---- 5. smoke test -------------------------------------------------------------------------------
Invoke-Native 'filmcraft-cli --version' { & (Join-Path $InstallDir 'filmcraft-cli.exe') --version }
Write-Output "mdot at $(RepoGit rev-parse --short HEAD)"
