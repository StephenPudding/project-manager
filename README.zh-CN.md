<p align="center"><img src="assets/app-icon.png" width="80" alt="Project Manager"></p>
<h1 align="center">Project Manager</h1>
<p align="center"><a href="README.md">English</a> · <a href="https://github.com/StephenPudding/project-manager/releases/latest">下载软件</a></p>

Windows 原生本地项目管理工具。通过缓存预览浏览网页应用、工具和游戏，管理多个项目文件夹，安装依赖、运行或停止开发服务、查看报错。

启动项目时自动开启局域网预览。在卡片或项目详情中复制局域网链接，用同一网络下的设备打开即可；停止项目时一并关闭局域网预览。

整个管理器使用 **Rust + GPUI**，包括目录扫描、进程管理、依赖安装和局域网预览。预览截图调用系统 **Microsoft Edge WebView2 Runtime**，仅在获取画面时启动，完成后关闭。浏览缓存时没有后台服务。首次打开必须选择数据目录，之后可在设置中更换。

## 下载

从 [Releases](https://github.com/StephenPudding/project-manager/releases/latest) 下载 Windows x64 便携 ZIP，解压后运行 `project-manager.exe`。当前构建的管理器不需要也不附带 Node.js、Chromium 或 Playwright，截图使用系统 [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。被管理的 npm/pnpm/yarn/bun 项目仍需要各自的运行环境、包管理器和依赖；静态 HTML 项目由 Rust 直接提供服务。界面自动跟随 Windows 显示语言：中文系统使用中文，其他语言使用英文。

## 构建

环境要求：Windows 10/11 x64、稳定版 Rust MSVC、带 Windows SDK 的 Visual Studio C++ Build Tools；截图需要 WebView2 Runtime。

```powershell
git clone https://github.com/StephenPudding/project-manager.git
cd project-manager
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-native.ps1 -Release
.\target\release\project-manager.exe
```

去掉 `-Release` 可构建 Debug。脚本自动寻找 Windows SDK 中的 `fxc.exe` 和 `rc.exe`；自定义安装位置可设置 `GPUI_FXC_PATH`、`GPM_RC_PATH`。构建出的程序可放到仓库外运行，无需复制 `runtime/` 文件夹。

完成 Release 构建后，可生成便携包：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/package-release.ps1
```

请只运行可信项目：开发命令和依赖安装会执行项目代码。设置和预览保存在自选的本机数据目录，应用不会上传这些数据。

## 协议

[MIT](LICENSE)。使用 [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)、[gpui-component](https://github.com/longbridge/gpui-component) 和 [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/)。
