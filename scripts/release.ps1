<#
.SYNOPSIS
    Builds Graysense (Tauri), packages its installer and updates with Velopack, signs them with
    Azure Artifact Signing, and publishes a GitHub release.

.DESCRIPTION
    The version comes from src-tauri\Cargo.toml (package.json and tauri.conf.json must match) and
    the release notes from its section in CHANGELOG.md.

      1. npx tauri build --no-bundle: the web UI (vite build) and the release exe.
      2. Gets vpk, Velopack's packager. It runs on the .NET runtime; no SDK needed.
      3. Downloads the latest published release, so the new one includes a small delta update.
      4. vpk pack: Graysense-win-Setup.exe, a portable zip, the update packages and the feed
         (releases.win.json) that installed copies read. Every executable is signed with Azure
         Artifact Signing (signtool and Microsoft's Azure.CodeSigning.Dlib come with vpk), using
         your Azure CLI sign-in (az login), which needs the "Artifact Signing Certificate Profile
         Signer" role on the signing account.
      5. vpk upload github: creates the v<version> tag and release at the current commit.

    Needs Node.js, Rust, the GitHub CLI signed in with access to the repo, the Azure CLI signed in
    (unless -NoSign), the .NET 8 runtime or later, and the commit being released pushed to GitHub.

.PARAMETER Draft
    Leave the release as a draft to look over first. Installed copies ignore drafts.

.PARAMETER NoUpload
    Only build the installer and packages (in target\dist\releases, or -OutDir).

.PARAMETER NoSign
    Don't sign (local test packages).

.PARAMETER Version
    Package a different version than Cargo.toml's, with -NoUpload. For testing updates: package an
    older version, install it, and point it at a newer package with GRAYSENSE_UPDATE_FEED.

.PARAMETER OutDir
    Where packages go. It's emptied first.

.PARAMETER SkipBuild
    Package the existing src-tauri\target\release\graysense.exe.

.EXAMPLE
    pwsh scripts\release.ps1
    pwsh scripts\release.ps1 -NoUpload -NoSign -Version 0.0.9 -OutDir target\dist\old -SkipBuild
#>
param(
    [switch]$Draft,
    [switch]$NoUpload,
    [switch]$NoSign,
    [string]$Version,
    [string]$OutDir,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$repo = 'spatney/graysense-terminal'
# Keep in step with the velopack crate in src-tauri\Cargo.toml.
$vpkVersion = '1.2.161'
# Azure Artifact Signing (formerly Trusted Signing).
$signing = [ordered]@{
    Endpoint               = 'https://eus.codesigning.azure.net'
    CodeSigningAccountName = 'rayfinfabricator'
    CertificateProfileName = 'rayfin-fabricator'
}

function Step([string]$Text) { Write-Host "`n> $Text" -ForegroundColor Cyan }
function Fail([string]$Text) { Write-Host "x $Text" -ForegroundColor Red; exit 1 }

$cargoVersion = (Select-String -Path src-tauri\Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$npmVersion = (Get-Content package.json -Raw | ConvertFrom-Json).version
$tauriVersion = (Get-Content src-tauri\tauri.conf.json -Raw | ConvertFrom-Json).version
if ($npmVersion -ne $cargoVersion -or $tauriVersion -ne $cargoVersion) {
    Fail "Versions differ: src-tauri\Cargo.toml $cargoVersion, package.json $npmVersion, tauri.conf.json $tauriVersion."
}
if (-not $Version) { $Version = $cargoVersion }
$tag = "v$Version"
$OutDir = if ($OutDir) { [IO.Path]::GetFullPath((Join-Path $root $OutDir)) } else { Join-Path $root 'target\dist\releases' }
$dist = Join-Path $root 'target\dist'

# --- Checks before anything is built or published.
if (-not $NoUpload) {
    if ($Version -ne $cargoVersion) { Fail "-Version only makes local test packages. Releases use Cargo.toml's version ($cargoVersion)." }
    if ($NoSign) { Fail 'Releases are signed. -NoSign is for local test packages.' }
    gh auth status --hostname github.com *> $null
    if ($LASTEXITCODE) { Fail 'Sign in to the GitHub CLI first: gh auth login' }
    if (git status --porcelain) { Fail 'Commit your changes first: the release is built from, and tagged at, the current commit.' }
    git fetch --quiet origin
    $head = (git rev-parse HEAD).Trim()
    if (-not (git branch -r --contains $head)) { Fail 'Push this commit to GitHub first (git push).' }
    gh release view $tag --repo $repo *> $null
    if ($LASTEXITCODE -eq 0) { Fail "$tag is already released. Bump the version (Cargo.toml, package.json, tauri.conf.json) and add a CHANGELOG.md section." }
}
if (-not $NoSign) {
    az account show --only-show-errors *> $null
    if ($LASTEXITCODE) { Fail 'Sign in to the Azure CLI first (az login): signing uses your Azure sign-in.' }
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
    if (-not (Test-Path node_modules)) { npm ci --no-audit --no-fund; if ($LASTEXITCODE) { Fail 'npm ci failed.' } }
    npx tauri build --no-bundle
    if ($LASTEXITCODE) { Fail 'tauri build failed.' }
}
$exe = Join-Path $root 'src-tauri\target\release\graysense.exe'
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

# --- Package (and sign).
Step "Packaging Graysense $Version$(if (-not $NoSign) { ', signed' })"
$packArgs = @(
    'pack', '--packId', 'Graysense', '--packVersion', $Version, '--packDir', $app, '--mainExe', 'graysense.exe',
    '--packTitle', 'Graysense', '--packAuthors', 'Sachin Patney', '--outputDir', $OutDir, '--runtime', 'win-x64',
    '--icon', (Join-Path $root 'assets\graysense.ico'),
    '--splashImage', (Join-Path $root 'assets\installer-splash.png'), '--splashProgressColor', '#7C8CFF',
    '--shortcuts', 'StartMenuRoot',
    # Installs Microsoft Edge WebView2 first on the rare Windows that lacks it.
    '--framework', 'webview2'
)
if (Test-Path $notes) { $packArgs += '--releaseNotes', $notes }
if (-not $NoSign) {
    $metadata = Join-Path $dist 'signing.json'
    $signing | ConvertTo-Json | Set-Content $metadata -Encoding utf8
    $packArgs += '--azureTrustedSignFile', $metadata
}
Invoke-Vpk @packArgs

if (-not $NoSign) {
    Step 'Checking signatures'
    $setup = Get-ChildItem $OutDir -Filter '*Setup.exe' | Select-Object -First 1
    $check = @($setup.FullName)
    # vpk signs the copies it packages (not the ones in $app): check those, they're what installs.
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $full = Get-ChildItem $OutDir -Filter "*-$Version-full.nupkg" | Select-Object -First 1
    if (-not $full) { Fail "There's no full package for $Version in $OutDir." }
    $signed = Join-Path $dist 'signed'
    Remove-Item $signed -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory $signed | Out-Null
    $zip = [IO.Compression.ZipFile]::OpenRead($full.FullName)
    try {
        foreach ($entry in $zip.Entries | Where-Object { $_.FullName -like 'lib/app/*.exe' }) {
            $out = Join-Path $signed $entry.Name
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $out, $true)
            $check += $out
        }
    } finally {
        $zip.Dispose()
    }
    foreach ($file in $check) {
        $sig = Get-AuthenticodeSignature $file
        if ($sig.Status -ne 'Valid') { Fail "$([IO.Path]::GetFileName($file)) isn't signed ($($sig.Status))." }
        Write-Host "  $([IO.Path]::GetFileName($file)): $($sig.SignerCertificate.Subject)"
    }
}

if ($NoUpload) {
    Step "Packages are in $OutDir"
    Get-ChildItem $OutDir -File | Where-Object { $_.Name -match [regex]::Escape($Version) -or $_.Name -notmatch '\d+\.\d+\.\d+' } | ForEach-Object { '  {0,-40} {1,8:N1} MB' -f $_.Name, ($_.Length / 1MB) }
    exit 0
}

# --- Publish.
Step "Publishing $tag"
$publish = @(if (-not $Draft) { '--publish' })
Invoke-Vpk upload github --repoUrl "https://github.com/$repo" --token $token --outputDir $OutDir --releaseName "Graysense $Version" --tag $tag --targetCommitish $head @publish

# The GitHub release page also explains how to install.
$body = Join-Path $dist 'release-body.md'
@"
$(Get-Content $notes -Raw)

## Install

Download **Graysense-win-Setup.exe** below and run it. Graysense installs for your account (no admin rights needed), adds itself to the Start menu and opens. The installer and app are code-signed; Windows SmartScreen can still ask about a new app while its reputation builds: choose **More info › Run anyway**.

Installed copies keep themselves up to date: new versions download in the background and install when you restart or close Graysense. This repository is private, so updates sign in with your GitHub CLI login (``gh auth login``).

Needs Windows 10 or 11 (x64) and the [GitHub Copilot CLI](https://github.com/github/copilot-cli), signed in.
"@ | Set-Content $body -Encoding utf8
gh release edit $tag --repo $repo --notes-file $body | Out-Null
git fetch --quiet --tags origin

Step "Released Graysense $Version$(if ($Draft) { ' (draft)' })"
gh release view $tag --repo $repo --json 'url,assets' --jq '.url, (.assets[] | "  " + .name)'
