<p align="center"><img src="assets/app-icon.png" width="80" alt="Project Manager"></p>
<h1 align="center">Project Manager</h1>
<p align="center"><a href="README.zh-CN.md">简体中文</a> · <a href="https://github.com/StephenPudding/project-manager/releases/latest">Download</a></p>

A native Windows app for managing local web projects, tools, and games. Browse cached previews, organize multiple project folders, install dependencies, start or stop development servers, and inspect errors from one workspace.

Starting a project also enables LAN preview. Copy its LAN link from the card or project details and open it on a device on the same network. Stopping the project closes the LAN preview too.

Built entirely with **Rust + GPUI**, including scanning, process management, dependency installation, and LAN preview. Previews use the system **Microsoft Edge WebView2 Runtime**, started only for capture and closed afterward. Browsing cached previews runs no background service. Choose your data folder on first launch; it can be changed later in Settings.

## Download

Download the Windows x64 portable ZIP from [Releases](https://github.com/StephenPudding/project-manager/releases/latest), extract it, and open `project-manager.exe`. Current builds do not need or bundle Node.js, Chromium, or Playwright. Preview capture requires the system [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/). Managed npm/pnpm/yarn/bun projects still need their own runtime, package manager, and dependencies; static HTML projects run directly in Rust. The interface follows your Windows display language: Chinese for Chinese systems, English otherwise.

## Build

Requirements: Windows 10/11 x64, stable Rust MSVC, Visual Studio C++ Build Tools with Windows SDK, and WebView2 Runtime for previews.

```powershell
git clone https://github.com/StephenPudding/project-manager.git
cd project-manager
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-native.ps1 -Release
.\target\release\project-manager.exe
```

Omit `-Release` for a Debug build. The script locates the SDK's `fxc.exe` and `rc.exe`; for custom locations, set `GPUI_FXC_PATH` and `GPM_RC_PATH`. The executable works outside the checkout; there is no `runtime/` folder to copy.

To create a portable ZIP after building Release:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/package-release.ps1
```

Only run projects you trust: development commands and dependency installs execute project code. Settings and previews stay in your chosen local data folder; the application does not upload them.

## License

[MIT](LICENSE). Built with [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), [gpui-component](https://github.com/longbridge/gpui-component), and [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/).
