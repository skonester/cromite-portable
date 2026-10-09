# Assembles the Cromite Portable folder and zips it.
#
# Builds "Cromite Portable.exe" from LauncherSource.cs (the updater refuses to
# run without it), places the updater beside it, adds the helper scripts and
# licences, then writes <OutDir>\Cromite-Portable.zip and SHA256SUMS.txt.
#
# The browser itself is not bundled: Cromite-Updater.exe downloads and verifies
# it on first run.
#
#   pwsh .github/scripts/package.ps1 -UpdaterExe updater/target/release/Cromite-Updater.exe

param(
    [Parameter(Mandatory)] [string] $UpdaterExe,
    [string] $OutDir = "dist"
)

$ErrorActionPreference = "Stop"

$Root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$OutDir = [IO.Path]::GetFullPath([IO.Path]::Combine($Root, $OutDir))
$Stage = Join-Path $OutDir "Cromite Portable"

if (Test-Path $OutDir) { Remove-Item $OutDir -Recurse -Force }
New-Item -ItemType Directory -Path $Stage | Out-Null
New-Item -ItemType Directory -Path (Join-Path $Stage "licenses") | Out-Null

# 1. Launcher, built with the .NET Framework compiler that ships with Windows
#    (the same one Update-Cromite.ps1 uses for option [4]).
$Csc = Join-Path $env:WINDIR "Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $Csc)) { throw "csc.exe not found at $Csc" }
& $Csc /nologo /optimize+ /target:winexe `
    "/out:$(Join-Path $Stage 'Cromite Portable.exe')" `
    "/win32icon:$(Join-Path $Root 'app.ico')" `
    /reference:System.dll,System.Windows.Forms.dll `
    (Join-Path $Root "LauncherSource.cs")
if ($LASTEXITCODE -ne 0) { throw "Launcher build failed (csc exit $LASTEXITCODE)" }

# 2. Updater and the files the portable folder uses at runtime.
Copy-Item (Resolve-Path (Join-Path $Root $UpdaterExe)) (Join-Path $Stage "Cromite-Updater.exe")
foreach ($File in "Cromite.bat", "SetDefaultBrowser.bat", "Update-Cromite.ps1",
                  "LauncherSource.cs", "app.ico", "LICENSE", "README.md") {
    Copy-Item (Join-Path $Root $File) $Stage
}

# 3. Licences for code and fonts compiled into the updater.
Copy-Item (Join-Path $Root "updater/THIRD_PARTY_NOTICES.md") (Join-Path $Stage "licenses")
Copy-Item (Join-Path $Root "updater/assets/OFL.txt") (Join-Path $Stage "licenses/OFL-AtkinsonHyperlegible.txt")

@"
Cromite Portable
================

1. Extract this whole folder somewhere you can write to (not Program Files).
2. Run Cromite-Updater.exe. On first run it downloads Cromite into .\app,
   then starts the browser.
3. After that, start Cromite with "Cromite Portable.exe", or run
   Cromite-Updater.exe again whenever you want to check for a newer build.

Your profile is kept in .\data next to these files.
"@ | Set-Content -Encoding utf8 (Join-Path $Stage "START HERE.txt")

# 4. Zip with the folder at the top level, so extracting gives one folder.
$Zip = Join-Path $OutDir "Cromite-Portable.zip"
Compress-Archive -Path $Stage -DestinationPath $Zip -CompressionLevel Optimal
Copy-Item (Join-Path $Stage "Cromite-Updater.exe") $OutDir

$Sums = foreach ($Asset in "Cromite-Portable.zip", "Cromite-Updater.exe") {
    $Hash = (Get-FileHash (Join-Path $OutDir $Asset) -Algorithm SHA256).Hash.ToLower()
    "$Hash  $Asset"
}
$Sums | Set-Content -Encoding ascii (Join-Path $OutDir "SHA256SUMS.txt")

Get-ChildItem $Stage -Recurse -File | ForEach-Object { $_.FullName.Substring($Stage.Length + 1) }
$Sums
