<#
.SYNOPSIS
    Builds Graysense, packages its installer and updates with Velopack, and publishes a GitHub
    release.

.DESCRIPTION
    The version comes from Cargo.toml and the release notes from its section in CHANGELOG.md.

      1. cargo build --release
      2. Gets vpk, Velopack's packager. It runs on the .NET runtime; no SDK needed.
      3. Downloads the latest published release, so the new one includes a small delta update.
      4. vpk pack: Graysense-win-Setup.exe, a portable zip, the update packages and the feed
         (releases.win.json) that installed copies read.
      5. vpk upload github: creates the v<version> tag and release at the current commit.

    Needs the GitHub CLI signed in with access to the repo, the .NET 8 runtime or later, and the
    commit being released pushed to GitHub.

.PARAMETER Draft
    Leave the release as a draft to look over first. Installed copies ignore drafts.

.PARAMETER NoUpload
    Only build the installer and packages (in target\dist\releases, or -OutDir).

.PARAMETER Version
    Package a different version than Cargo.toml's, with -NoUpload. For testing updates: package an
    older version, install it, and point it at a newer package with GRAYSENSE_UPDATE_FEED.

.PARAMETER OutDir
    Where packages go. It's emptied first.

.PARAMETER SkipBuild
    Package the existing target\release\graysense.exe.

.EXAMPLE
    pwsh scripts\release.ps1
    pwsh scripts\release.ps1 -NoUpload -Version 0.0.9 -OutDir target\dist\old -SkipBuild
#>
param(
    [switch]$Draft,
    [switch]$NoUpload,
    [string]$Version,
    [string]$OutDir,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$repo = 'spatney/graysense-terminal'
# Keep in step with the velopack crate in Cargo.toml.
$vpkVersion = '1.2.161'

function Step([string]$Text) { Write-Host "`n> $Text" -ForegroundColor Cyan }
function Fail([string]$Text) { Write-Host "x $Text" -ForegroundColor Red; exit 1 }

$cargoVersion = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $Version) { $Version = $cargoVersion }
$tag = "v$Version"
$OutDir = if ($OutDir) { [IO.Path]::GetFullPath((Join-Path $root $OutDir)) } else { Join-Path $root 'target\dist\releases' }
$dist = Join-Path $root 'target\dist'

# --- Checks before anything is built or published.
if (-not $NoUpload) {
    if ($Version -ne $cargoVersion) { Fail "-Version only makes local test packages. Releases use Cargo.toml's version ($cargoVersion)." }
    gh auth status --hostname github.com *> $null
    if ($LASTEXITCODE) { Fail 'Sign in to the GitHub CLI first: gh auth login' }
    if (git status --porcelain) { Fail 'Commit your changes first: the release is built from, and tagged at, the current commit.' }
    git fetch --quiet origin
    $head = (git rev-parse HEAD).Trim()
    if (-not (git branch -r --contains $head)) { Fail 'Push this commit to GitHub first (git push).' }
    gh release view $tag --repo $repo *> $null
    if ($LASTEXITCODE -eq 0) { Fail "$tag is already released. Bump the version in Cargo.toml and add a CHANGELOG.md section." }
}

# --- Release notes: the version's section of CHANGELOG.md.
New-Item -ItemType Directory -Force $dist | Out-Null
$notes = Join-Path $dist 'notes.md'
$section = [regex]::Match((Get-Content CHANGELOG.md -Raw), "(?ms)^## \[?$([regex]::Escape($Version))\]?[^\n]*\n(.*?)(?=^## |\z)")
if ($section.Success) {
    Set-Content $notes $section.Groups[1].Value.Trim() -Encoding utf8
} elseif (-not $NoUpload) {
    Fail "CHANGELOG.md has no '## $Version' section."
} else {
    Remove-Item $notes -ErrorAction SilentlyContinue
}

# --- Build.
if (-not $SkipBuild) {
    Step "Building Graysense $cargoVersion"
    cargo build --release --locked
    if ($LASTEXITCODE) { Fail 'cargo build failed.' }
}
$exe = Join-Path $root 'target\release\graysense.exe'
if (-not (Test-Path $exe)) { Fail "There's no release build at $exe." }
$app = Join-Path $dist 'app'
Remove-Item $app -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $app | Out-Null
Copy-Item $exe $app

# --- vpk, run with the installed .NET runtime.
$runtimes = dotnet --list-runtimes 2>$null | Select-String '^Microsoft\.NETCore\.App (\d+)\.' | ForEach-Object { [int]$_.Matches[0].Groups[1].Value }
$tfm = @(10, 9, 8) | Where-Object { $runtimes -contains $_ } | Select-Object -First 1
if (-not $tfm) { Fail 'vpk needs the .NET 8 runtime or later: winget install Microsoft.DotNet.Runtime.8' }
$tools = Join-Path $root "target\tools\vpk-$vpkVersion"
$vpk = Join-Path $tools "tools\net$tfm.0\any\vpk.dll"
if (-not (Test-Path $vpk)) {
    Step "Getting vpk $vpkVersion"
    New-Item -ItemType Directory -Force $tools | Out-Null
    $zip = Join-Path $tools 'vpk.zip'
    Invoke-WebRequest "https://api.nuget.org/v3-flatcontainer/vpk/$vpkVersion/vpk.$vpkVersion.nupkg" -OutFile $zip
    Expand-Archive $zip -DestinationPath $tools -Force
    Remove-Item $zip
}
$env:DOTNET_CLI_TELEMETRY_OPTOUT = '1'
function Invoke-Vpk {
    & dotnet $vpk @args --skip-updates
    if ($LASTEXITCODE) { Fail "vpk $($args[0]) failed." }
}

Remove-Item $OutDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $OutDir | Out-Null
if (-not $NoUpload) {
    $token = (gh auth token --hostname github.com).Trim()
    $latest = gh release list --repo $repo --exclude-drafts --exclude-pre-releases --limit 1 --json tagName --jq '.[0].tagName'
    if ($latest) {
        Step "Downloading $latest to build a delta update from it"
        Invoke-Vpk download github --repoUrl "https://github.com/$repo" --token $token --outputDir $OutDir
    }
}

# --- Package.
Step "Packaging Graysense $Version"
$packArgs = @(
    'pack', '--packId', 'Graysense', '--packVersion', $Version, '--packDir', $app, '--mainExe', 'graysense.exe',
    '--packTitle', 'Graysense', '--packAuthors', 'Graysense', '--outputDir', $OutDir,
    '--icon', (Join-Path $root 'assets\graysense.ico'),
    '--splashImage', (Join-Path $root 'assets\installer-splash.png'), '--splashProgressColor', '#5B8CFF',
    '--shortcuts', 'StartMenuRoot'
)
if (Test-Path $notes) { $packArgs += '--releaseNotes', $notes }
Invoke-Vpk @packArgs

if ($NoUpload) {
    Step "Packages are in $OutDir"
    Get-ChildItem $OutDir -File | Where-Object { $_.Name -match [regex]::Escape($Version) -or $_.Name -notmatch '\d+\.\d+\.\d+' } | ForEach-Object { '  {0,-40} {1,8:N1} MB' -f $_.Name, ($_.Length / 1MB) }
    exit 0
}

# --- Publish.
Step "Publishing $tag"
$publish = if ($Draft) { @() } else { @('--publish') }
Invoke-Vpk upload github --repoUrl "https://github.com/$repo" --token $token --outputDir $OutDir --releaseName "Graysense $Version" --tag $tag --targetCommitish $head @publish

# The GitHub release page also explains how to install.
$body = Join-Path $dist 'release-body.md'
@"
$(Get-Content $notes -Raw)

## Install

Download **Graysense-win-Setup.exe** below and run it. Graysense installs for your account (no admin rights needed), adds itself to the Start menu and opens. The installer isn't code-signed yet, so Windows SmartScreen may warn about it: choose **More info › Run anyway**.

Installed copies keep themselves up to date: new versions download in the background and install when you restart or close Graysense. This repository is private, so updates sign in with your GitHub CLI login (``gh auth login``).

Needs Windows 10 or 11 (x64) and the [GitHub Copilot CLI](https://github.com/github/copilot-cli), signed in.
"@ | Set-Content $body -Encoding utf8
gh release edit $tag --repo $repo --notes-file $body | Out-Null
git fetch --quiet --tags origin

Step "Released Graysense $Version$(if ($Draft) { ' (draft)' })"
gh release view $tag --repo $repo --json 'url,assets' --jq '.url, (.assets[] | "  " + .name)'
