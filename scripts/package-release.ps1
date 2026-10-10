param([string]$Version = '0.1.0')
$ErrorActionPreference = 'Stop'
$workspace = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$executable = Join-Path $workspace 'target\release\project-manager.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Build Release first: scripts/build-native.ps1 -Release' }
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?$') { throw 'Invalid version' }
$distribution = Join-Path $workspace 'dist'
New-Item -ItemType Directory -Path $distribution -Force | Out-Null
$packageName = "project-manager-$Version-windows-x64"
$staging = Join-Path $distribution ($packageName + '-' + [guid]::NewGuid().ToString('N').Substring(0,8))
$package = Join-Path $staging $packageName
New-Item -ItemType Directory -Path $package -Force | Out-Null
Copy-Item -LiteralPath $executable -Destination $package
$finder = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $finder)) { throw 'Visual Studio Build Tools are required to package the redistributable C++ runtime.' }
$visualStudio = & $finder -latest -products '*' -property installationPath
$crt = Get-ChildItem -Path (Join-Path $visualStudio 'VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT\vcruntime140.dll') |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $crt) { throw 'Visual C++ redistributable runtime was not found.' }
foreach ($name in @('vcruntime140.dll', 'vcruntime140_1.dll')) {
    Copy-Item -LiteralPath (Join-Path $crt.DirectoryName $name) -Destination $package
}
foreach ($name in @('README.md','README.zh-CN.md','LICENSE','SECURITY.md','THIRD_PARTY_NOTICES.md')) {
    Copy-Item -LiteralPath (Join-Path $workspace $name) -Destination $package
}
# Explicit allowlist: never copy the workspace, user's data directory, browser profiles or logs.
# Documentation's only image is our original app icon.
New-Item -ItemType Directory -Path (Join-Path $package 'assets') | Out-Null
Copy-Item -LiteralPath (Join-Path $workspace 'assets\app-icon.png') -Destination (Join-Path $package 'assets')
# Carry the Rust dependencies' original license/notice texts with the portable binary.
$cargoDirectory = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$registry = Join-Path $cargoDirectory 'registry\src'
$licenseDirectory = Join-Path $package 'licenses'
New-Item -ItemType Directory -Path $licenseDirectory | Out-Null
foreach ($notice in @('WebView2-SDK-LICENSE.txt', 'webview2-rs-LICENSE.txt')) {
    Copy-Item -LiteralPath (Join-Path $workspace "third_party\$notice") -Destination $licenseDirectory
}
$lockText = Get-Content -LiteralPath (Join-Path $workspace 'Cargo.lock') -Raw
foreach ($dependency in [regex]::Matches($lockText, '(?m)^name = "([^"]+)"\r?\nversion = "([^"]+)"')) {
    $crateName = $dependency.Groups[1].Value + '-' + $dependency.Groups[2].Value
    $crate = Get-ChildItem -Path (Join-Path $registry "*\$crateName") -Directory -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $crate) { continue }
    $texts = Get-ChildItem -LiteralPath $crate.FullName -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)' }
    if ($texts) {
        $licenseTarget = Join-Path $licenseDirectory $crateName
        New-Item -ItemType Directory -Path $licenseTarget | Out-Null
        foreach ($license in $texts) { Copy-Item -LiteralPath $license.FullName -Destination $licenseTarget }
    }
}
# Never distribute diagnostic logs from runtime dependencies.
foreach ($diagnostic in @(Get-ChildItem -LiteralPath $package -File -Recurse -Filter '*.log')) {
    $diagnosticPath = [System.IO.Path]::GetFullPath($diagnostic.FullName)
    if (-not $diagnosticPath.StartsWith($package + '\',[System.StringComparison]::OrdinalIgnoreCase)) { throw 'Unexpected diagnostic path' }
    Remove-Item -LiteralPath $diagnosticPath
}
$archive = Join-Path $distribution "$packageName.zip"
if (Test-Path -LiteralPath $archive) { throw "Archive already exists; choose a new version: $archive" }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory($staging,$archive,[System.IO.Compression.CompressionLevel]::Optimal,$false)
$hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
[System.IO.File]::WriteAllText((Join-Path $distribution "$packageName.sha256"), "$hash  $packageName.zip`n")
Write-Output $archive
