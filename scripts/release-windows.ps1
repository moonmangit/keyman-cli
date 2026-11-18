# Builds the Windows release on this PC and publishes it on GitHub.
#
#   powershell -ExecutionPolicy Bypass -File scripts/release-windows.ps1 -DryRun
#       builds and checks everything, publishes nothing
#   powershell -ExecutionPolicy Bypass -File scripts/release-windows.ps1
#       also creates the GitHub release vX.Y.Z on the current commit of main
#
# Before a release: set the same version in Cargo.toml, gui/Cargo.toml and
# npm/package.json, add a "## X.Y.Z" section to CHANGELOG.md, and merge to main.
# The script makes, in target/release-dist/:
#   keyman-x86_64-pc-windows-msvc.exe   the CLI
#   Keyman-windows-x64.zip              the desktop app (the app's updater reads it)
#   keyman-cli-X.Y.Z.tgz                the npm package, with the Windows binary
#   SHA256SUMS.txt                      the checksums of the three files
# npm is not published by the script: it prints the command to run with your login.
param([switch]$DryRun)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Step($text) { Write-Host "==> $text" -ForegroundColor Cyan }
function Fail($text) { Write-Host "error: $text" -ForegroundColor Red; exit 1 }
function Run($exe, [string[]]$arguments) {
    & $exe @arguments
    if ($LASTEXITCODE -ne 0) { Fail "$exe $($arguments -join ' ') stopped with exit code $LASTEXITCODE" }
}
function CargoVersion($file) {
    $line = Select-String -Path $file -Pattern '^version = "(.*)"' | Select-Object -First 1
    if (-not $line) { Fail "no version in $file" }
    $line.Matches[0].Groups[1].Value
}

Step 'Check the versions and the changelog'
$version = CargoVersion 'Cargo.toml'
$appVersion = CargoVersion 'gui/Cargo.toml'
$npmVersion = (Get-Content npm/package.json -Raw | ConvertFrom-Json).version
if ($version -ne $appVersion -or $version -ne $npmVersion) {
    Fail "Cargo.toml has $version, gui/Cargo.toml has $appVersion and npm/package.json has $npmVersion. Set all three to the same version."
}
$tag = "v$version"
$changelog = Get-Content CHANGELOG.md
$start = [array]::IndexOf($changelog, "## $version")
if ($start -lt 0) { Fail "CHANGELOG.md has no '## $version' section." }
$notes = @()
for ($i = $start + 1; $i -lt $changelog.Count -and -not $changelog[$i].StartsWith('## '); $i++) {
    $notes += $changelog[$i]
}

if (-not $DryRun) {
    Step 'Check git: main, clean, the same as origin/main, no tag yet'
    $branch = (git rev-parse --abbrev-ref HEAD).Trim()
    if ($branch -ne 'main') { Fail "switch to main first (now on $branch)." }
    if (git status --porcelain) { Fail 'the working tree has changes. Commit or stash them first.' }
    Run git @('fetch', '--quiet', '--tags', 'origin')
    if ((git rev-parse HEAD) -ne (git rev-parse origin/main)) { Fail 'main is not the same as origin/main. Pull or push first.' }
    if (git tag --list $tag) { Fail "the tag $tag exists already." }
    Run gh @('auth', 'status')
}

Step "Build the CLI $version (release)"
Run cargo @('build', '--locked', '--release')
Step "Build the app $version (release)"
Push-Location gui
try { Run cargo @('build', '--locked', '--release') } finally { Pop-Location }

$dist = Join-Path $root 'target/release-dist'
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory $dist | Out-Null

Step 'Package the CLI and the app'
$cli = Join-Path $dist 'keyman-x86_64-pc-windows-msvc.exe'
Copy-Item target/release/keyman.exe $cli
$stage = Join-Path $dist 'app'
New-Item -ItemType Directory $stage | Out-Null
Copy-Item gui/target/release/keyman-app.exe (Join-Path $stage 'Keyman.exe')
$zip = Join-Path $dist 'Keyman-windows-x64.zip'
# An antivirus scan can hold the new .exe for a few seconds: try again.
for ($try = 1; ; $try++) {
    try {
        Compress-Archive -Path (Join-Path $stage 'Keyman.exe') -DestinationPath $zip -Force -ErrorAction Stop
        break
    } catch {
        if ($try -ge 10) { Fail "cannot zip the app: $_" }
        Start-Sleep -Seconds 2
    }
}
Remove-Item -Recurse -Force $stage
$cliVersion = & $cli --version
if ($cliVersion -notmatch [regex]::Escape($version)) { Fail "the CLI says '$cliVersion', not $version." }

Step 'Pack the npm package (Windows binary only)'
$native = Join-Path $root 'npm/bin/native'
if (Test-Path $native) { Remove-Item -Recurse -Force $native }
New-Item -ItemType Directory (Join-Path $native 'x86_64-pc-windows-msvc') | Out-Null
Copy-Item target/release/keyman.exe (Join-Path $native 'x86_64-pc-windows-msvc/keyman.exe')
Run npm @('pack', './npm', '--pack-destination', $dist, '--silent')
$tgz = Join-Path $dist "keyman-cli-$version.tgz"
if (-not (Test-Path $tgz)) { Fail "npm pack did not make $tgz." }

Step 'Smoke test the npm package'
$smoke = Join-Path ([System.IO.Path]::GetTempPath()) "keyman-release-smoke-$PID"
if (Test-Path $smoke) { Remove-Item -Recurse -Force $smoke }
New-Item -ItemType Directory $smoke | Out-Null
try {
    Run npm @('install', '--silent', '--prefix', (Join-Path $smoke 'install'), $tgz)
    $command = Join-Path $smoke 'install/node_modules/.bin/keyman.cmd'
    $said = & $command --version
    if ($said -notmatch [regex]::Escape($version)) { Fail "the npm command says '$said', not $version." }
} finally {
    Remove-Item -Recurse -Force $smoke -ErrorAction SilentlyContinue
}

Step 'Write SHA256SUMS.txt'
$files = @($cli, $zip, $tgz)
$sums = $files | ForEach-Object {
    "$((Get-FileHash -Algorithm SHA256 $_).Hash.ToLower())  $(Split-Path -Leaf $_)"
}
$sumsFile = Join-Path $dist 'SHA256SUMS.txt'
[System.IO.File]::WriteAllText($sumsFile, (($sums -join "`n") + "`n"))
Get-Content $sumsFile

if ($DryRun) {
    Step "Dry run: nothing published. The files are in $dist"
    exit 0
}

Step "Create the GitHub release $tag"
$notesFile = Join-Path $dist 'notes.md'
[System.IO.File]::WriteAllText($notesFile, (($notes -join "`n").Trim() + "`n"))
Run gh @('release', 'create', $tag, $cli, $zip, $tgz, $sumsFile,
    '--target', (git rev-parse HEAD).Trim(), '--title', "Keyman $version", '--notes-file', $notesFile)
Remove-Item $notesFile

Step 'Done'
Write-Host "Release: https://github.com/moonmangit/keyman-cli/releases/tag/$tag"
Write-Host 'To publish npm (needs your npm login):'
Write-Host "  npm login"
Write-Host "  npm publish `"$tgz`" --access public"
