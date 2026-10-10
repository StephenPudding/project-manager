//! Short-lived STA helper. GPUI never hosts a browser while browsing cached images.
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;
use std::{
    fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
use windows::{
    Win32::{
        Foundation::{HGLOBAL, HWND, LPARAM, LRESULT, RECT, WPARAM},
        System::{
            Com::StructuredStorage::CreateStreamOnHGlobal, Com::*, LibraryLoader::GetModuleHandleW,
        },
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::{BOOL, HSTRING, Interface, PCWSTR, PWSTR, w},
};

#[derive(Deserialize)]
struct Request {
    url: String,
    file: PathBuf,
}

#[derive(Default, Deserialize)]
struct Frame {
    ready: bool,
    quiet: bool,
    width: f64,
    height: f64,
    clip: Option<Clip>,
}

#[derive(Deserialize)]
struct Clip {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

// Callbacks and the message pump stay on this thread. In particular, never block
// a COM callback waiting for another WebView2 operation to complete.
fn pump() {
    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        MsgWaitForMultipleObjectsEx(None, 15, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
    }
}

fn wait<T>(receiver: &Receiver<T>, timeout: Duration) -> Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        match receiver.try_recv() {
            Ok(value) => return Ok(value),
            Err(TryRecvError::Disconnected) => bail!("WebView2 operation was interrupted"),
            Err(TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            bail!("WebView2 operation timed out");
        }
        pump();
    }
}

fn settle(duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        pump();
    }
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct CaptureWindow(HWND);
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
impl CaptureWindow {
    fn new() -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)?.into();
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: w!("ProjectManagerCapture"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(windows::core::Error::from_thread().into());
            }
            // A rendered, off-screen tool window: no taskbar entry, activation,
            // or flashing window. Hiding the WebView itself would stop painting.
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.lpszClassName,
                w!("Project Manager preview"),
                WS_POPUP,
                GetSystemMetrics(SM_XVIRTUALSCREEN) - 1400,
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                1280,
                800,
                None,
                None,
                Some(instance),
                None,
            )?;
            let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            Ok(Self(window))
        }
    }
}
impl Drop for CaptureWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

struct Controller(ICoreWebView2Controller);
impl Drop for Controller {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Close();
        }
    }
}

fn environment(data: &Path) -> Result<ICoreWebView2Environment> {
    unsafe {
        let mut version = PWSTR::null();
        GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version)
            .context("未安装 Microsoft Edge WebView2 Runtime，请安装后重试：https://go.microsoft.com/fwlink/p/?LinkId=2124703")?;
        drop(CoTaskMemPWSTR::from(version));
        let options: ICoreWebView2EnvironmentOptions =
            CoreWebView2EnvironmentOptions::default().into();
        options.SetAdditionalBrowserArguments(w!("--disable-features=CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows --disable-renderer-backgrounding --disable-background-timer-throttling --mute-audio"))?;
        let (tx, rx) = mpsc::channel();
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            &HSTRING::from(data.join("webview2").as_os_str()),
            &options,
            &CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
                move |result, value| {
                    let _ = tx.send(
                        result
                            .map_err(anyhow::Error::from)
                            .and_then(|_| value.context("WebView2 returned no environment")),
                    );
                    Ok(())
                },
            )),
        )?;
        wait(&rx, Duration::from_secs(30))?
    }
}

fn execute(webview: &ICoreWebView2, script: &str) -> Result<serde_json::Value> {
    let (tx, rx) = mpsc::channel();
    unsafe {
        webview.ExecuteScript(
            &HSTRING::from(script),
            &ExecuteScriptCompletedHandler::create(Box::new(move |result, value| {
                let _ = tx.send(result.map(|_| value));
                Ok(())
            })),
        )?;
    }
    Ok(serde_json::from_str(&wait(&rx, Duration::from_secs(5))??)?)
}

fn capture(
    environment: &ICoreWebView2Environment,
    window: HWND,
    request: Request,
    data: &Path,
) -> Result<()> {
    let url = url::Url::parse(&request.url)?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
    {
        bail!("Preview capture requires a local project URL");
    }
    if request.file.extension().and_then(|s| s.to_str()) != Some("jpg")
        || request
            .file
            .parent()
            .context("Missing preview directory")?
            .canonicalize()?
            != data.join("previews").canonicalize()?
    {
        bail!("Preview output must be inside the configured preview cache");
    }
    let (tx, rx) = mpsc::channel();
    unsafe {
        environment.CreateCoreWebView2Controller(
            window,
            &CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                move |result, value| {
                    let _ = tx.send(
                        result
                            .map_err(anyhow::Error::from)
                            .and_then(|_| value.context("WebView2 returned no controller")),
                    );
                    Ok(())
                },
            )),
        )?;
    }
    let controller = Controller(wait(&rx, Duration::from_secs(20))??);
    unsafe {
        if let Ok(scale) = controller.0.cast::<ICoreWebView2Controller3>() {
            scale.SetShouldDetectMonitorScaleChanges(false)?;
            scale.SetRasterizationScale(1.0)?;
            scale.SetBoundsMode(COREWEBVIEW2_BOUNDS_MODE_USE_RAW_PIXELS)?;
        }
        controller.0.SetBounds(RECT {
            left: 0,
            top: 0,
            right: 1280,
            bottom: 800,
        })?;
        controller.0.SetZoomFactor(1.0)?;
        controller.0.SetIsVisible(true)?;
    }
    let webview = unsafe { controller.0.CoreWebView2()? };
    unsafe {
        let settings = webview.Settings()?;
        settings.SetAreDevToolsEnabled(false)?;
        settings.SetAreDefaultContextMenusEnabled(false)?;
        settings.SetAreDefaultScriptDialogsEnabled(false)?;
        settings.SetIsStatusBarEnabled(false)?;
        settings.SetIsWebMessageEnabled(false)?;
        if let Ok(audio) = webview.cast::<ICoreWebView2_8>() {
            audio.SetIsMuted(true)?;
        }
        let mut token = 0;
        webview.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetHandled(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        if let Ok(downloads) = webview.cast::<ICoreWebView2_4>() {
            downloads.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetCancel(true)?;
                        args.SetHandled(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        let (tx, rx) = mpsc::channel();
        webview.AddScriptToExecuteOnDocumentCreated(
            &HSTRING::from(include_str!("../assets/capture/init.js")),
            &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
                move |result, _| {
                    let _ = tx.send(result);
                    Ok(())
                },
            )),
        )?;
        wait(&rx, Duration::from_secs(5))??;

        let (tx, rx) = mpsc::channel::<Result<()>>();
        let loaded = tx.clone();
        webview.cast::<ICoreWebView2_2>()?.add_DOMContentLoaded(
            &DOMContentLoadedEventHandler::create(Box::new(move |_, _| {
                let _ = loaded.send(Ok(()));
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut success = BOOL::default();
                    args.IsSuccess(&mut success)?;
                    if !success.as_bool() {
                        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                        args.WebErrorStatus(&mut status)?;
                        let _ = tx.send(Err(anyhow::anyhow!(
                            "WebView2 navigation failed ({})",
                            status.0
                        )));
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.Navigate(&HSTRING::from(request.url.as_str()))?;
        wait(&rx, Duration::from_secs(45))??;
    }
    let started = Instant::now();
    let frame = loop {
        let frame: Frame = serde_json::from_value(execute(
            &webview,
            include_str!("../assets/capture/frame.js"),
        )?)?;
        if frame.ready && (frame.quiet || started.elapsed() > Duration::from_secs(4))
            || started.elapsed() > Duration::from_secs(6)
        {
            break frame;
        }
        settle(Duration::from_millis(100));
    };
    // Let the first game frame reach the compositor without waiting for animation to stop.
    settle(Duration::from_millis(150));
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true)? };
    let (tx, rx) = mpsc::channel();
    unsafe {
        webview.CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG,
            &stream,
            &CapturePreviewCompletedHandler::create(Box::new(move |result| {
                let _ = tx.send(result);
                Ok(())
            })),
        )?;
    }
    wait(&rx, Duration::from_secs(10))??;
    let mut stat = STATSTG::default();
    unsafe {
        stream.Stat(&mut stat, STATFLAG_NONAME)?;
    }
    if stat.cbSize == 0 || stat.cbSize > 16 * 1024 * 1024 {
        bail!("Invalid WebView2 preview size");
    }
    let mut bytes = vec![0u8; stat.cbSize as usize];
    let mut read = 0;
    unsafe {
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        stream
            .Read(
                bytes.as_mut_ptr().cast(),
                bytes.len() as u32,
                Some(&mut read),
            )
            .ok()?;
    }
    if read as usize != bytes.len() {
        bail!("Incomplete WebView2 preview");
    }
    let mut output = tempfile::NamedTempFile::new_in(data.join("previews"))?;
    if let Some(clip) = frame
        .clip
        .filter(|_| frame.width > 0.0 && frame.height > 0.0)
    {
        let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)?;
        let sx = image.width() as f64 / frame.width;
        let sy = image.height() as f64 / frame.height;
        let x = (clip.x * sx).floor().max(0.0) as u32;
        let y = (clip.y * sy).floor().max(0.0) as u32;
        let width = ((clip.width * sx).ceil() as u32).min(image.width().saturating_sub(x));
        let height = ((clip.height * sy).ceil() as u32).min(image.height().saturating_sub(y));
        if width == 0 || height == 0 {
            bail!("Invalid project canvas bounds");
        }
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 85)
            .encode_image(&image.crop_imm(x, y, width, height))?;
    } else {
        output.write_all(&bytes)?;
    }
    output
        .persist(&request.file)
        .context("Could not save preview")?;
    Ok(())
}

pub fn run() -> Result<()> {
    let data = PathBuf::from(std::env::var_os("GPM_DATA_DIR").context("Missing data directory")?);
    fs::create_dir_all(data.join("previews"))?;
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let _apartment = Apartment;
    let window = CaptureWindow::new()?;
    let environment = environment(&data)?;
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        // Node closes stdin at the end of the queue, releasing the environment
        // and all controllers. There is no persistent capture browser at idle.
        let line = match rx.try_recv() {
            Ok(line) => line,
            Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {
                pump();
                continue;
            }
        };
        let started = Instant::now();
        let result = serde_json::from_str::<Request>(&line)
            .map_err(anyhow::Error::from)
            .and_then(|request| capture(&environment, window.0, request, &data));
        let response = match result {
            Ok(()) => json!({ "ok": true, "duration": started.elapsed().as_millis() }),
            Err(error) => json!({ "ok": false, "error": format!("{error:#}") }),
        };
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}
