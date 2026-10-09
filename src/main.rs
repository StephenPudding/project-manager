#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod assets;
mod backend;
mod dev_servers;
mod smooth_scroll;
mod storage;
mod theme;
mod ui;
use gpui::*;
use gpui_component::{Root, Theme, ThemeMode};
use ui::{CloseOverlay, FocusSearch, Workbench};
fn main() {
    let (backend, events) = backend::Backend::start();
    Application::new()
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_component::init(cx);
            Theme::change(ThemeMode::Light, None, cx);
            theme::apply("default", cx);
            cx.bind_keys([
                KeyBinding::new("escape", CloseOverlay, Some("Workbench")),
                KeyBinding::new("ctrl-f", FocusSearch, Some("Workbench")),
            ]);
            let cleanup = backend.clone();
            cx.on_app_quit(move |_| {
                cleanup.shutdown();
                async {}
            })
            .detach();
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1440.), px(940.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(1040.), px(740.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Project Manager".into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| Workbench::new(backend, events, window, cx));
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("Unable to create native window");
            cx.activate(true);
        });
}
