param([switch]$Release)
$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent $PSScriptRoot
if (-not $env:GPUI_FXC_PATH) {
    $kits = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue
    if ($kits.KitsRoot10) {
        $compiler = Get-ChildItem -Path (Join-Path $kits.KitsRoot10 'bin\*\x64\fxc.exe') -File | Sort-Object FullName -Descending | Select-Object -First 1
        if ($compiler) { $env:GPUI_FXC_PATH = $compiler.FullName }
    }
}
if ($env:GPUI_FXC_PATH -and -not $env:GPM_RC_PATH) {
    $env:GPM_RC_PATH = Join-Path (Split-Path -Parent $env:GPUI_FXC_PATH) 'rc.exe'
}
if ($Release) { cargo build --release --manifest-path (Join-Path $workspace 'Cargo.toml') }
else { cargo build --manifest-path (Join-Path $workspace 'Cargo.toml') }
exit $LASTEXITCODE
