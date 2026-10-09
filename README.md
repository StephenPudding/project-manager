<p align="center"><img src="assets/app-icon.png" width="80" alt="Project Manager"></p>
<h1 align="center">Project Manager</h1>
<p align="center"><a href="README.zh-CN.md">简体中文</a> · <a href="https://github.com/StephenPudding/project-manager/releases/latest">Download</a></p>

A native Windows app for managing local web projects, tools, and games. Browse cached previews, organize multiple project folders, install dependencies, start or stop development servers, and inspect errors from one workspace.

Built with **Rust + GPUI**, with an on-demand **Node.js / Playwright** worker for scanning and previews. Choose your data folder on first launch; it can be changed later in Settings. No user project screenshots or caches are included in this repository.

## Download

Download the Windows x64 portable ZIP from [Releases](https://github.com/StephenPudding/project-manager/releases/latest), extract the **entire archive**, and open `project-manager.exe`. The portable package includes the capture runtime. Individual managed projects may still need their own package managers and dependencies. The current UI is in Simplified Chinese.

## Build

Requirements: Windows 10/11 x64, stable Rust MSVC, Visual Studio C++ Build Tools with Windows SDK, and Node.js 20+.

```powershell
git clone https://github.com/StephenPudding/project-manager.git
cd project-manager
npm ci --prefix runtime
Push-Location runtime
npx playwright install chromium
Pop-Location
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-native.ps1 -Release
.\target\release\project-manager.exe
```

Omit `-Release` for a Debug build. The script locates the SDK's `fxc.exe` and `rc.exe`; for custom locations, set `GPUI_FXC_PATH` and `GPM_RC_PATH`. Keep source-built executables inside the checkout so they can find `runtime/`.

To create a portable ZIP after installing dependencies and building Release:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/package-release.ps1
```

Only run projects you trust: development commands and dependency installs execute project code. Settings and previews stay in your chosen local data folder; the application does not upload them.

## License

[MIT](LICENSE). Built with [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), [gpui-component](https://github.com/longbridge/gpui-component), and [Playwright](https://github.com/microsoft/playwright).
