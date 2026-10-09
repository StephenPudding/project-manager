use crate::assets::ActionIcon;
use crate::backend::{self, Backend, Event, Project, Request, Snapshot};
use crate::dev_servers::{DevServer, Discovery, Monitor};
use crate::smooth_scroll::SmoothScroll;
use crate::theme::{self, Colors};
use gpui::{prelude::*, *};
use gpui_component::Disableable;
use gpui_component::{
    Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarShow},
    switch::Switch,
    tooltip::Tooltip,
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

const TITLE_BAR_HEIGHT: f32 = 40.;
actions!(workbench, [CloseOverlay, FocusSearch]);
#[derive(Clone, PartialEq)]
enum View {
    All,
    Favorites,
    Running,
    Directory(String),
}
impl View {
    fn includes(&self, project: &Project) -> bool {
        match self {
            Self::All => true,
            Self::Favorites => project.favorite,
            Self::Running => project.status == "running",
            Self::Directory(root) => &project.root == root,
        }
    }

    fn label(&self) -> String {
        match self {
            Self::All => "项目库".into(),
            Self::Favorites => "我的收藏".into(),
            Self::Running => "正在运行".into(),
            Self::Directory(root) => root_name(root),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SortOrder {
    Modified,
    Created,
    Name,
}

#[derive(Clone, Copy, PartialEq)]
enum RunningTab {
    All,
    Managed,
    External,
}

#[derive(Clone, Copy, PartialEq)]
enum SettingsSection {
    Appearance,
    Directories,
    Preview,
    Storage,
}

impl SettingsSection {
    fn label(self) -> &'static str {
        match self {
            Self::Appearance => "外观主题",
            Self::Directories => "项目目录",
            Self::Preview => "扫描与预览",
            Self::Storage => "数据存储",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Appearance => IconName::Palette,
            Self::Directories => IconName::Folder,
            Self::Preview => IconName::Settings2,
            Self::Storage => IconName::Folder,
        }
    }
}

impl RunningTab {
    fn includes(self, entry: &RunningEntry) -> bool {
        match self {
            Self::All => true,
            Self::Managed => entry.project.is_some(),
            Self::External => entry.project.is_none(),
        }
    }
}

#[derive(Clone)]
struct RunningEntry {
    server: DevServer,
    project: Option<Project>,
    owned: bool,
}

impl RunningEntry {
    fn name(&self) -> &str {
        self.project
            .as_ref()
            .map_or(&self.server.name, |project| &project.name)
    }

    fn key(&self) -> String {
        format!(
            "service-{}-{}-{}",
            self.server.pid,
            self.server.port,
            self.project
                .as_ref()
                .map_or("", |project| project.id.as_str())
        )
    }
}
impl SortOrder {
    fn label(self) -> &'static str {
        match self {
            Self::Modified => "最近更新",
            Self::Created => "创建日期",
            Self::Name => "名称 A–Z",
        }
    }
}

pub struct Workbench {
    backend: Backend,
    monitor: Monitor,
    discovery: Discovery,
    discovering: bool,
    running_tab: RunningTab,
    snapshot: Snapshot,
    images: HashMap<String, (u64, Arc<RenderImage>, f32)>,
    loading_images: HashMap<String, u64>,
    toast_timer: Option<Task<()>>,
    list_state: ListState,
    smooth_scroll: SmoothScroll,
    list_key: (Vec<String>, usize, u32),
    search: Entity<InputState>,
    directory: Entity<InputState>,
    storage_input: Entity<InputState>,
    storage_loading: bool,
    storage_required: bool,
    storage_directory: Option<String>,
    storage_previous: Option<String>,
    storage_error: String,
    storage_pending: bool,
    draft_roots: Vec<String>,
    view: View,
    engine: String,
    sort_order: SortOrder,
    list: bool,
    selected: Option<String>,
    settings_open: bool,
    settings_section: SettingsSection,
    settings_scroll: ScrollHandle,
    auto_capture: bool,
    game_engines_only: bool,
    draft_theme: String,
    busy: bool,
    connected: bool,
    message: String,
    message_at: Instant,
    error: bool,
    show_logs: bool,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}
fn text(value: impl Into<SharedString>, size: f32, color: u32) -> Div {
    let size = if (10.0..12.0).contains(&size) {
        12.0
    } else {
        size
    };
    div()
        .text_size(px(size))
        .text_color(rgb(color))
        .child(value.into())
}
fn icon(name: impl Into<Icon>, size: f32) -> Icon {
    Icon::new(name).size(px(size))
}
fn root_name(root: &str) -> String {
    root.rsplit(['\\', '/'])
        .find(|part| !part.is_empty())
        .unwrap_or(root)
        .to_owned()
}
fn root_key(root: &str) -> String {
    root.replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}
fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    glyph: impl Into<Icon>,
) -> Button {
    Button::new(id)
        .label(label)
        .icon(glyph)
        .with_size(gpui_component::Size::Medium)
}
fn preview_rounding<T: Styled>(mut element: T, corners: Corners<Pixels>) -> T {
    let radii = &mut element.style().corner_radii;
    radii.top_left = Some(corners.top_left.into());
    radii.top_right = Some(corners.top_right.into());
    radii.bottom_left = Some(corners.bottom_left.into());
    radii.bottom_right = Some(corners.bottom_right.into());
    element
}
impl Workbench {
    fn colors(&self) -> Colors {
        theme::find(if self.settings_open {
            &self.draft_theme
        } else {
            &self.snapshot.settings.theme
        })
        .colors()
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_open {
            self.settings_open = false;
            theme::apply(&self.snapshot.settings.theme, cx);
        }
    }

    fn search_field(&self, width: f32) -> Input {
        // Input::h only sizes multiline editors; Styled::h sizes the single-line border too.
        Styled::h(
            Input::new(&self.search)
                .prefix(icon(IconName::Search, 18.))
                .cleanable(true)
                .with_size(gpui_component::Size::Medium),
            px(36.),
        )
        .min_h(px(36.))
        .flex_shrink_0()
        .w(px(width))
        .px(px(12.))
        .py_0()
        .text_size(px(14.))
        .line_height(px(20.))
        .rounded(px(8.))
    }

    fn title_bar(&self, window: &Window) -> impl IntoElement {
        let colors = self.colors();
        let maximized = window.is_maximized();
        div()
            .id("custom-title-bar")
            .w_full()
            .h(px(TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(rgb(colors.bg))
            .border_b_1()
            .border_color(rgb(colors.line))
            .child(
                div()
                    .id("window-drag-area")
                    .window_control_area(WindowControlArea::Drag)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px_5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(rgb(colors.accent))
                    .child(img(crate::assets::APP_ICON).size(px(22.)))
                    .child(
                        text("Project Manager", 13., colors.ink).font_weight(FontWeight::MEDIUM),
                    ),
            )
            .children(
                [
                    (
                        "window-minimize",
                        IconName::WindowMinimize,
                        WindowControlArea::Min,
                        "最小化",
                    ),
                    (
                        "window-maximize",
                        if maximized {
                            IconName::WindowRestore
                        } else {
                            IconName::WindowMaximize
                        },
                        WindowControlArea::Max,
                        if maximized { "还原" } else { "最大化" },
                    ),
                    (
                        "window-close",
                        IconName::WindowClose,
                        WindowControlArea::Close,
                        "关闭",
                    ),
                ]
                .into_iter()
                .map(|(id, glyph, area, label)| {
                    let close = area == WindowControlArea::Close;
                    div()
                        .id(id)
                        // Windows handles dragging, double-click, snapping, and caption button actions.
                        .window_control_area(area)
                        .w(px(48.))
                        .h_full()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(colors.ink))
                        .hover(move |s| {
                            s.bg(rgb(if close { 0xc94b4b } else { colors.hover }))
                                .text_color(rgb(if close { 0xffffff } else { colors.ink }))
                        })
                        .active(move |s| {
                            s.bg(rgb(if close { 0xaf3939 } else { colors.pressed }))
                                .text_color(rgb(if close { 0xffffff } else { colors.ink }))
                        })
                        .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
                        .child(icon(glyph, 16.))
                }),
            )
    }

    pub fn new(
        backend: Backend,
        events: async_channel::Receiver<Event>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索项目、类型或端口…"));
        let directory = cx.new(|cx| InputState::new(window, cx).placeholder("E:\\projects"));
        let storage_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("例如 D:\\ProjectManagerData"));
        let storage_subscription =
            cx.subscribe(&storage_input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.storage_error.clear();
                    cx.notify();
                }
            });
        let subscription = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update(cx, |this, cx| this.receive(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let (monitor, discoveries) = Monitor::start(backend::workspace());
        cx.spawn(async move |this, cx| {
            while let Ok(discovery) = discoveries.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.discovery = discovery;
                        this.discovering = false;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            backend,
            monitor,
            discovery: Discovery::default(),
            discovering: false,
            running_tab: RunningTab::All,
            snapshot: Snapshot::default(),
            images: HashMap::new(),
            loading_images: HashMap::new(),
            toast_timer: None,
            list_state: ListState::new(0, ListAlignment::Top, px(200.)).measure_all(),
            smooth_scroll: SmoothScroll::default(),
            list_key: (Vec::new(), 0, 0),
            search,
            directory,
            storage_input,
            storage_loading: true,
            storage_required: true,
            storage_directory: None,
            storage_previous: None,
            storage_error: String::new(),
            storage_pending: false,
            draft_roots: Vec::new(),
            view: View::All,
            engine: "全部引擎".into(),
            sort_order: SortOrder::Modified,
            list: false,
            selected: None,
            settings_open: false,
            settings_section: SettingsSection::Appearance,
            settings_scroll: ScrollHandle::new(),
            auto_capture: true,
            game_engines_only: false,
            draft_theme: "default".into(),
            busy: false,
            connected: false,
            message: "正在读取本地项目缓存…".into(),
            message_at: Instant::now(),
            error: false,
            show_logs: false,
            focus,
            _subscriptions: vec![subscription, storage_subscription],
        }
    }
    fn receive(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Storage {
                directory,
                previous,
                error,
            } => {
                self.storage_loading = false;
                self.storage_required = directory.is_none();
                self.storage_directory = directory;
                self.storage_previous = previous;
                self.storage_error = error.unwrap_or_default();
                self.message.clear();
            }
            Event::Snapshot(snapshot) => {
                if self.connected && self.snapshot == snapshot {
                    return;
                }
                if !self.connected && !self.busy && !self.error {
                    self.message.clear();
                }
                self.apply(snapshot, cx);
                self.connected = true;
            }
            Event::Completed(snapshot, message) => {
                self.storage_pending = false;
                self.apply(snapshot, cx);
                if self.view == View::Running {
                    self.discovering = true;
                    self.monitor.refresh();
                }
                self.busy = false;
                self.connected = true;
                self.error = false;
                self.message = message;
                self.message_at = Instant::now();
                if self.message == "设置已保存" {
                    self.close_settings(cx);
                }
                let stamp = self.message_at;
                self.toast_timer = Some(cx.spawn(async move |this, cx| {
                    Timer::after(Duration::from_secs(5)).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.message_at == stamp && !this.busy && !this.error {
                            this.message.clear();
                            cx.notify();
                        }
                    });
                }));
            }
            Event::Error(error, disconnected) => {
                if self.storage_pending {
                    self.storage_error = error.clone();
                    self.storage_pending = false;
                }
                self.message = error;
                self.message_at = Instant::now();
                self.error = true;
                self.busy = false;
                if disconnected {
                    self.connected = false;
                }
            }
        }
        cx.notify();
    }
    fn apply(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        if !self.settings_open && self.snapshot.settings.theme != snapshot.settings.theme {
            theme::apply(&snapshot.settings.theme, cx);
        }
        if self.snapshot.settings.game_engines_only != snapshot.settings.game_engines_only {
            self.engine = "全部引擎".into();
            self.smooth_scroll.cancel();
            self.list_state.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
        }
        if matches!(&self.view, View::Directory(root) if !snapshot.settings.roots.contains(root)) {
            self.view = View::All;
            self.engine = "全部引擎".into();
        }
        self.images
            .retain(|id, _| snapshot.projects.iter().any(|p| &p.id == id));
        for project in &snapshot.projects {
            let Some(stamp) = project.captured_at else {
                continue;
            };
            if self.images.get(&project.id).map(|(time, _, _)| *time) == Some(stamp)
                || self.loading_images.get(&project.id) == Some(&stamp)
            {
                continue;
            }
            self.loading_images.insert(project.id.clone(), stamp);
            let project = project.clone();
            let cache = snapshot.cache_path.clone();
            let id = project.id.clone();
            // File access, JPEG decoding and resizing never run on the UI thread.
            let load = cx.background_executor().spawn(async move {
                let path = backend::preview_path(&cache, &project)?;
                let decoded = image::ImageReader::open(path).ok()?.decode().ok()?;
                let aspect = decoded.width() as f32 / decoded.height().max(1) as f32;
                let mut pixels = if decoded.width().max(decoded.height()) > 1600 {
                    decoded
                        .resize(1600, 1600, image::imageops::FilterType::Triangle)
                        .into_rgba8()
                } else {
                    decoded.into_rgba8()
                };
                // GPUI's renderer consumes BGRA pixels.
                for pixel in pixels.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                Some((
                    Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])),
                    aspect,
                ))
            });
            cx.spawn(async move |this, cx| {
                let loaded = load.await;
                let _ = this.update(cx, |this, cx| {
                    if this.loading_images.get(&id) == Some(&stamp) {
                        this.loading_images.remove(&id);
                    }
                    if this
                        .snapshot
                        .projects
                        .iter()
                        .any(|p| p.id == id && p.captured_at == Some(stamp))
                    {
                        if let Some((image, aspect)) = loaded {
                            this.images.insert(id, (stamp, image, aspect));
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
        }
        self.snapshot = snapshot;
    }
    fn send(
        &mut self,
        operations: Vec<(String, serde_json::Value)>,
        launch: Option<String>,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = false;
        self.message = "正在处理，请稍候…".into();
        self.message_at = Instant::now();
        if self
            .backend
            .tx
            .send(Request {
                operations,
                launch,
                message: message.into(),
            })
            .is_err()
        {
            self.busy = false;
            self.error = true;
            self.message = "后台服务未连接，请重新打开工作台。".into();
        }
        cx.notify();
    }
    fn action(&mut self, p: &Project, action: &str, cx: &mut Context<Self>) {
        let (endpoint, value, launch, message) = match action {
            "favorite" => (
                "settings".into(),
                json!({"favorite": p.id}),
                None,
                "收藏已更新",
            ),
            "start" => (
                format!("projects/{}/start", p.id),
                json!({}),
                Some(p.id.clone()),
                "项目已在独立窗口打开",
            ),
            "stop" => (
                format!("projects/{}/stop", p.id),
                json!({}),
                None,
                "项目已停止",
            ),
            "capture" => (
                format!("projects/{}/capture", p.id),
                json!({}),
                None,
                "已加入画面获取队列",
            ),
            "install" => (
                format!("projects/{}/install", p.id),
                json!({}),
                None,
                "已开始安装依赖，可在项目详情中查看日志",
            ),
            _ => return,
        };
        self.send(vec![(endpoint, value)], launch, message, cx);
    }
    fn settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.smooth_scroll.cancel();
        self.draft_roots = self.snapshot.settings.roots.clone();
        self.directory
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.storage_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.storage_error.clear();
        self.auto_capture = self.snapshot.settings.auto_capture;
        self.game_engines_only = self.snapshot.settings.game_engines_only;
        self.draft_theme = theme::find(&self.snapshot.settings.theme).id.into();
        self.selected = None;
        self.settings_open = true;
        self.settings_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }
    fn add_root(&mut self, root: String) {
        let root = root.trim().trim_matches('"');
        if !root.is_empty()
            && !self
                .draft_roots
                .iter()
                .any(|item| root_key(item) == root_key(root))
        {
            self.draft_roots.push(root.to_owned());
        }
    }
    fn filtered(&self, cx: &App) -> Vec<Project> {
        let search = self.search.read(cx).value().to_lowercase();
        let mut projects = self
            .snapshot
            .projects
            .iter()
            .filter(|p| {
                self.view.includes(p)
                    && (self.engine == "全部引擎"
                        || p.categories(self.snapshot.settings.game_engines_only)
                            .contains(&self.engine.as_str()))
                    && format!(
                        "{} {}",
                        p.name,
                        p.categories(self.snapshot.settings.game_engines_only)
                            .join(" ")
                    )
                    .to_lowercase()
                    .contains(&search)
            })
            .cloned()
            .collect::<Vec<_>>();
        projects.sort_by(|a, b| {
            let order = match self.sort_order {
                SortOrder::Modified => b.modified.total_cmp(&a.modified),
                SortOrder::Created => match (a.created_at, b.created_at) {
                    (Some(a), Some(b)) => b.total_cmp(&a),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                },
                SortOrder::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            order
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.id.cmp(&b.id))
        });
        projects
    }
    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.colors();
        let count = self.snapshot.projects.len();
        let favorite = self.snapshot.projects.iter().filter(|p| p.favorite).count();
        let running = self.running_entries().len();
        div()
            .w(px(228.))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(colors.panel))
            .border_r_1()
            .border_color(rgb(colors.line))
            .p_5()
            .flex()
            .flex_col()
            .child(text("工作空间", 11., colors.muted).mt_1().mb_3().px_3())
            .children(
                [
                    (View::All, "全部项目", IconName::LayoutDashboard, count),
                    (View::Favorites, "我的收藏", IconName::Star, favorite),
                    (View::Running, "正在运行", IconName::ExternalLink, running),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (view, label, glyph, count))| {
                    div()
                        .id(("navigation", index))
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .h(px(44.))
                        .mb_1()
                        .rounded(px(9.))
                        .cursor_pointer()
                        .bg(rgb(if self.view == view { colors.selected } else { colors.panel }))
                        .text_color(rgb(if self.view == view { colors.selected_text } else { colors.muted }))
                        .hover(move |style| style.bg(rgb(colors.hover)))
                        .child(icon(glyph, 17.))
                        .child(text(
                            label,
                            13.,
                            if self.view == view { colors.selected_text } else { colors.muted },
                        ))
                        .child(div().flex_1())
                        .child(text(count.to_string(), 11., colors.muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.view = view.clone();
                            this.discovering = this.view == View::Running;
                            this.monitor.watch(this.discovering);
                            this.engine = "全部引擎".into();
                            this.smooth_scroll.cancel();
                            this.list_state.scroll_to(ListOffset {
                                item_ix: 0,
                                offset_in_item: px(0.),
                            });
                            cx.notify();
                        }))
                }),
            )
            .child(div().h(px(1.)).bg(rgb(colors.line)).my_4())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .mb_2()
                    .child(text("项目目录", 11., colors.muted))
                    .child(
                        button("manage-roots", "管理", IconName::Settings2)
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(|this, _, window, cx| this.settings(window, cx))),
                    ),
            )
            .child(
                div()
                    .id("root-list")
                    .flex_1()
                    .min_h(px(60.))
                    .overflow_y_scroll()
                    .mb_4()
                    .children(
                        self.snapshot.settings.roots.iter().cloned()
                            .enumerate()
                            .map(|(index, root)| {
                                let active = matches!(&self.view, View::Directory(selected_root) if selected_root == &root);
                                let tooltip_path = root.clone();
                                let count = self
                                    .snapshot
                                    .projects
                                    .iter()
                                    .filter(|p| p.root == root)
                                    .count();
                                div()
                                    .id(("root", index))
                                    .rounded_lg()
                                    .px_3()
                                    .py_2()
                                    .mb_1()
                                    .cursor_pointer()
                                    .bg(rgb(if active { colors.selected } else { colors.panel }))
                                    .hover(move |s| s.bg(rgb(colors.hover)))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_color(rgb(colors.accent))
                                            .child(icon(IconName::Folder, 15.))
                                            .child(
                                                text(
                                                    root_name(&root),
                                                    12.,
                                                    colors.ink,
                                                )
                                                .flex_1()
                                                .truncate(),
                                            )
                                            .child(text(count.to_string(), 10., colors.muted)),
                                    )
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(tooltip_path.clone()).build(window, cx)
                                    })
                                    .child(text(root.clone(), 9., colors.muted).mt_1().truncate())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.view = View::Directory(root.clone());
                                        this.monitor.watch(false);
                                        this.discovering = false;
                                        this.engine = "全部引擎".into();
                                        this.smooth_scroll.cancel();
                                        this.list_state.scroll_to(ListOffset {
                                            item_ix: 0,
                                            offset_in_item: px(0.),
                                        });
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
            .child(
                button("settings", "工作台设置", IconName::Settings2)
                    .ghost()
                    .mt_5()
                    .on_click(cx.listener(|this, _, window, cx| this.settings(window, cx))),
            )
    }
    fn art(
        &self,
        project: &Project,
        height: f32,
        width: f32,
        corners: Corners<Pixels>,
    ) -> AnyElement {
        let colors = self.colors();
        // GPUI's overflow mask is rectangular; round each painted layer explicitly.
        let art = preview_rounding(
            div()
                .relative()
                .w_full()
                .h(px(height))
                .overflow_hidden()
                .bg(rgb(colors.preview_bg)),
            corners,
        );
        if let Some((_, image, aspect)) = self.images.get(&project.id) {
            let fit_width = width.min(height * aspect);
            let fit_height = fit_width / aspect;
            art.flex()
                .items_center()
                .justify_center()
                .child(preview_rounding(
                    img(image.clone())
                        .absolute()
                        .inset_0()
                        .w(px(width))
                        .h(px(height))
                        .object_fit(ObjectFit::Fill)
                        .opacity(0.13),
                    corners,
                ))
                .child(
                    img(image.clone())
                        .w(px(fit_width))
                        .h(px(fit_height))
                        .flex_shrink_0()
                        .object_fit(ObjectFit::Contain)
                        .when(width - fit_width < 24. && height - fit_height < 24., |s| {
                            preview_rounding(s, corners)
                        }),
                )
                .into_any_element()
        } else {
            let message = match project.capture.as_str() {
                "capturing" => "正在生成项目预览",
                "queued" => "画面即将就绪",
                "error" => "预览暂未获取",
                _ => "等待第一次相遇",
            };
            art.flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(rgb(colors.preview_muted))
                .child(icon(IconName::Frame, 32.))
                .child(text(message, 13., colors.preview_text))
                .child(text("自动获取 · 独立缓存", 10., colors.preview_muted))
                .into_any_element()
        }
    }
    fn card(&self, p: Project, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors();
        let running = p.status == "running";
        let installing = p.install_state == "installing";
        let needs_install = !running && (!p.dependencies_installed || installing);
        let action_color = if installing {
            0x326da8
        } else if needs_install {
            0x9b6a1d
        } else {
            colors.accent
        };
        let p_detail = p.clone();
        let p_fav = p.clone();
        let p_play = p.clone();
        let p_stop = p.clone();
        let p_refresh = p.clone();
        let error_project_id = p.id.clone();
        let path = p.path.clone();
        let (status, status_color) = if installing {
            ("正在安装依赖", 0x326da8)
        } else if p.status == "stopping" {
            ("正在停止", 0x9b6a1d)
        } else if p.capture == "capturing" {
            ("正在获取画面", 0x326da8)
        } else if p.status == "starting" {
            ("正在启动", 0x326da8)
        } else if running {
            ("正在运行", 0x327d52)
        } else if p.install_state == "error" {
            ("依赖安装失败", 0xbf4b45)
        } else if p.status == "error" {
            ("启动失败", 0xbf4b45)
        } else if p.capture == "queued" {
            ("等待获取画面", 0x9b6a1d)
        } else if !p.dependencies_installed {
            ("缺少依赖", 0x9b6a1d)
        } else if p.capture == "error" || p.capture_error.is_some() {
            ("预览获取失败", 0xbf4b45)
        } else {
            ("准备就绪", 0x737c86)
        };
        let failed = matches!(status, "依赖安装失败" | "启动失败" | "预览获取失败");
        let art_height = if self.list {
            130.
        } else {
            (width * 0.62).clamp(170., 245.)
        };
        let body = div()
            .flex_1()
            .min_w_0()
            .p_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        text(p.name.clone(), 16., colors.ink)
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis(),
                    )
                    .child(
                        Button::new(SharedString::from(format!("fav-{}", p.id)))
                            .icon(icon(IconName::Star, 18.))
                            .ghost()
                            .size(px(36.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(rgb(if p.favorite {
                                colors.accent_border
                            } else {
                                colors.line
                            }))
                            .bg(rgb(if p.favorite {
                                colors.soft
                            } else {
                                colors.surface
                            }))
                            .text_color(rgb(if p.favorite {
                                colors.accent
                            } else {
                                colors.muted
                            }))
                            .tooltip(if p.favorite {
                                "取消收藏"
                            } else {
                                "收藏项目"
                            })
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.action(&p_fav, "favorite", cx)
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .mt_2()
                    .child(
                        text(p.engine.clone(), 10., colors.tag_text)
                            .px_2()
                            .py(px(3.))
                            .rounded(px(4.))
                            .bg(rgb(colors.tag_bg)),
                    )
                    .child(text("·", 11., colors.muted))
                    .child(
                        text(
                            if self.snapshot.settings.roots.len() > 1 {
                                root_name(&p.root)
                            } else if p.preview.is_some() {
                                "实机画面已就绪".into()
                            } else {
                                "等待自动预览".into()
                            },
                            11.,
                            colors.muted,
                        )
                        .truncate(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mt_4()
                    .pt_3()
                    .flex_wrap()
                    .gap_2()
                    .border_t_1()
                    .border_color(rgb(colors.line))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .flex_shrink_0()
                            .child(div().size(px(6.)).rounded_full().bg(rgb(status_color)))
                            .child(text(status, 12., status_color).font_weight(FontWeight::MEDIUM))
                            .when(failed, |s| {
                                s.child(
                                    Button::new(SharedString::from(format!("error-{}", p.id)))
                                        .label("查看报错")
                                        .ghost()
                                        .small()
                                        .h(px(28.))
                                        .px_2()
                                        .text_color(rgb(status_color))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.smooth_scroll.cancel();
                                            this.selected = Some(error_project_id.clone());
                                            this.show_logs = true;
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .max_w_full()
                            .flex_wrap()
                            .justify_end()
                            .items_center()
                            .gap_2()
                            .ml_auto()
                            .child(
                                Button::new(SharedString::from(format!("folder-{}", p.id)))
                                    .icon(icon(IconName::FolderOpen, 18.))
                                    .ghost()
                                    .size(px(36.))
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_color(rgb(colors.line))
                                    .tooltip("打开项目文件夹")
                                    .on_click(move |_, _, _| {
                                        let _ = backend::open(&path);
                                    }),
                            )
                            .child(
                                Button::new(SharedString::from(format!("capture-{}", p.id)))
                                    .icon(icon(ActionIcon::Refresh, 18.))
                                    .ghost()
                                    .size(px(36.))
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_color(rgb(colors.line))
                                    .tooltip("更新项目预览")
                                    .disabled(self.busy || needs_install)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.action(&p_refresh, "capture", cx)
                                    })),
                            )
                            .when(running, |s| {
                                s.child(
                                    Button::new(SharedString::from(format!("stop-{}", p.id)))
                                        .child(
                                            div().size(px(11.)).rounded(px(2.)).bg(rgb(0xa34b3e)),
                                        )
                                        .child(
                                            text("停止", 14., 0xa34b3e)
                                                .font_weight(FontWeight::MEDIUM),
                                        )
                                        .h(px(36.))
                                        .min_w(px(80.))
                                        .px(px(12.))
                                        .rounded(px(8.))
                                        .border_1()
                                        .border_color(rgb(0xe8c9c2))
                                        .bg(rgb(0xf9ece8))
                                        .tooltip("停止运行此项目")
                                        .disabled(self.busy)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.action(&p_stop, "stop", cx)
                                        })),
                                )
                            })
                            .child(
                                Button::new(SharedString::from(format!("play-{}", p.id)))
                                    .icon(if installing {
                                        icon(IconName::LoaderCircle, 18.)
                                    } else if needs_install {
                                        icon(IconName::ArrowDown, 18.)
                                    } else {
                                        icon(ActionIcon::Play, 18.)
                                    })
                                    .child(
                                        text(
                                            if installing {
                                                "安装中…"
                                            } else if needs_install {
                                                "安装依赖"
                                            } else if running {
                                                "进入"
                                            } else {
                                                "运行"
                                            },
                                            14.,
                                            action_color,
                                        )
                                        .font_weight(FontWeight::MEDIUM),
                                    )
                                    .h(px(36.))
                                    .min_w(px(80.))
                                    .px(px(12.))
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_color(rgb(if needs_install {
                                        0xe8d5ad
                                    } else {
                                        colors.accent_border
                                    }))
                                    .bg(rgb(if needs_install { 0xfcf3df } else { colors.soft }))
                                    .text_color(rgb(action_color))
                                    .tooltip(if installing {
                                        "正在安装依赖，点击项目画面可查看日志"
                                    } else if needs_install {
                                        "使用项目的包管理器安装依赖"
                                    } else if running {
                                        "打开正在运行的项目"
                                    } else {
                                        "启动项目"
                                    })
                                    .disabled(self.busy || installing)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.action(
                                            &p_play,
                                            if needs_install { "install" } else { "start" },
                                            cx,
                                        )
                                    })),
                            ),
                    ),
            );
        let art = div()
            .id(SharedString::from(format!("art-{}", p.id)))
            .relative()
            .cursor_pointer()
            .when(self.list, |s| s.w(px(185.)).flex_shrink_0())
            .when(!self.list, |s| s.w_full())
            .child(self.art(
                &p,
                art_height,
                if self.list { 185. } else { width - 2. },
                Corners {
                    top_left: px(12.),
                    top_right: px(if self.list { 0. } else { 12. }),
                    bottom_left: px(if self.list { 12. } else { 0. }),
                    bottom_right: px(0.),
                },
            ))
            .child(
                text(
                    if running { "●  LIVE" } else { "实机画面" },
                    9.,
                    colors.preview_text,
                )
                .absolute()
                .top_3()
                .left_3()
                .px_2()
                .py_1()
                .rounded(px(4.))
                .bg(rgba((colors.preview_bg << 8) | 0xcc)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.smooth_scroll.cancel();
                this.selected = Some(p_detail.id.clone());
                this.show_logs = false;
                cx.notify();
            }));
        div()
            .id(SharedString::from(format!("card-{}", p.id)))
            .w(px(width))
            .rounded(px(13.))
            .overflow_hidden()
            .bg(rgb(colors.surface))
            .border_1()
            .border_color(rgb(colors.line))
            .hover(move |s| s.border_color(rgb(colors.accent_border)).shadow_md())
            .when(self.list, |s| s.flex().items_center())
            .child(art)
            .child(body)
            .into_any_element()
    }
    fn running_entries(&self) -> Vec<RunningEntry> {
        let managed: Vec<_> = self
            .snapshot
            .projects
            .iter()
            .filter(|project| project.status == "running")
            .collect();
        let mut entries = self
            .discovery
            .servers
            .iter()
            .map(|server| {
                let owned = managed
                    .iter()
                    .find(|project| {
                        project
                            .url
                            .as_deref()
                            .is_some_and(|url| server.matches_url(url))
                    })
                    .copied();
                let project = owned
                    .or_else(|| {
                        server.directory.as_deref().and_then(|directory| {
                            let directory = root_key(directory);
                            self.snapshot
                                .projects
                                .iter()
                                .filter(|project| {
                                    let path = root_key(&project.path);
                                    directory == path
                                        || directory
                                            .strip_prefix(&path)
                                            .is_some_and(|suffix| suffix.starts_with('\\'))
                                })
                                .max_by_key(|project| project.path.len())
                        })
                    })
                    .cloned();
                let mut server = server.clone();
                if let Some(url) = owned.and_then(|project| project.url.clone()) {
                    server.url = url;
                }
                RunningEntry {
                    server,
                    project,
                    owned: owned.is_some(),
                }
            })
            .collect::<Vec<_>>();
        // Keep managed projects visible while native discovery is pending or unavailable.
        for project in managed {
            if entries.iter().any(|entry| {
                entry.owned && entry.project.as_ref().is_some_and(|p| p.id == project.id)
            }) {
                continue;
            }
            let url = project.url.clone().unwrap_or_default();
            let parsed = url::Url::parse(&url).ok();
            entries.push(RunningEntry {
                server: DevServer {
                    pid: 0,
                    port: parsed
                        .as_ref()
                        .and_then(|url| url.port_or_known_default())
                        .unwrap_or(0),
                    addresses: vec![],
                    url,
                    name: project.name.clone(),
                    directory: Some(project.path.clone()),
                    kind: project.engine.clone(),
                    started: 0,
                },
                project: Some(project.clone()),
                owned: true,
            });
        }
        entries.sort_by(|a, b| {
            a.server
                .port
                .cmp(&b.server.port)
                .then_with(|| a.name().cmp(b.name()))
        });
        entries
    }

    fn running_content(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors();
        let search = self.search.read(cx).value().to_lowercase();
        let matching = self
            .running_entries()
            .into_iter()
            .filter(|entry| {
                format!(
                    "{} {} {} {} {} {}",
                    entry.name(),
                    entry.server.kind,
                    entry.server.directory.as_deref().unwrap_or(""),
                    entry.server.url,
                    entry.server.pid,
                    entry.project.as_ref().map_or("", |p| p.engine.as_str())
                )
                .to_lowercase()
                .contains(&search)
            })
            .collect::<Vec<_>>();
        let managed_count = matching
            .iter()
            .filter(|entry| entry.project.is_some())
            .count();
        let counts = [
            matching.len(),
            managed_count,
            matching.len() - managed_count,
        ];
        let entries = matching
            .into_iter()
            .filter(|entry| self.running_tab.includes(entry))
            .collect::<Vec<_>>();
        let width = f32::from(window.viewport_size().width);
        let key = (
            entries.iter().map(RunningEntry::key).collect(),
            1,
            width.to_bits(),
        );
        if self.list_key != key {
            self.smooth_scroll.cancel();
            self.list_state.reset(entries.len().max(1) + 1);
            self.list_key = key;
        }
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .child(
                list(
                    self.list_state.clone(),
                    cx.processor(move |this, index: usize, _, cx| {
                        let item = if index == 0 {
                            this.running_header(entries.len(), counts, cx)
                                .into_any_element()
                        } else if entries.is_empty() {
                            div()
                                .h(px(240.))
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_3()
                                .border_1()
                                .border_color(rgb(colors.line))
                                .rounded_xl()
                                .child(text(
                                    if this.discovering {
                                        "正在发现本机服务…"
                                    } else if this.discovery.warning.is_some() {
                                        "未能完整读取本机服务"
                                    } else if search.is_empty() {
                                        match this.running_tab {
                                            RunningTab::All => "没有发现正在监听的开发服务",
                                            RunningTab::Managed => "没有正在运行的管理器项目",
                                            RunningTab::External => "没有正在运行的外部项目",
                                        }
                                    } else {
                                        "没有匹配的服务"
                                    },
                                    17.,
                                    colors.muted,
                                ))
                                .child(text(
                                    if search.is_empty() {
                                        match this.running_tab {
                                            RunningTab::All => {
                                                "在终端启动 npm run dev 后，这里会自动更新。"
                                            }
                                            RunningTab::Managed => {
                                                "已加入管理器的项目启动后会显示在这里。"
                                            }
                                            RunningTab::External => {
                                                "未加入管理器的本机服务会显示在这里。"
                                            }
                                        }
                                    } else {
                                        "可以搜索项目名称、目录、PID 或端口。"
                                    },
                                    12.,
                                    colors.muted,
                                ))
                                .into_any_element()
                        } else {
                            this.running_row(entries[index - 1].clone(), cx)
                        };
                        div().px(px(36.)).pb_3().child(item).into_any_element()
                    }),
                )
                .flex_1()
                .min_h_0(),
            )
            .child(self.smooth_scroll.layer(self.list_state.clone()))
            .child(
                Scrollbar::vertical(&self.smooth_scroll.handle(&self.list_state))
                    .scrollbar_show(ScrollbarShow::Always),
            )
            .into_any_element()
    }

    fn running_header(
        &self,
        count: usize,
        counts: [usize; 3],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = self.colors();
        div()
            .pt_6()
            .pb_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                text("正在运行", 18., colors.ink).font_weight(FontWeight::SEMIBOLD),
                            )
                            .child(text(format!("{count} 个端口"), 12., colors.muted)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(self.search_field(280.))
                            .child(
                                button(
                                    "refresh-ports",
                                    if self.discovering {
                                        "正在发现…"
                                    } else {
                                        "刷新端口"
                                    },
                                    icon(ActionIcon::Refresh, 18.),
                                )
                                .h(px(36.))
                                .px_4()
                                .rounded(px(8.))
                                .primary()
                                .disabled(self.discovering)
                                .tooltip("重新发现本机开发服务；此页面每 5 秒自动更新")
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.discovering = true;
                                        this.monitor.refresh();
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            )
            .child(
                div().flex().items_center().gap_2().mt_4().children(
                    [
                        (RunningTab::All, "全部"),
                        (RunningTab::Managed, "管理器项目"),
                        (RunningTab::External, "外部项目"),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (tab, label))| {
                        let active = self.running_tab == tab;
                        Button::new(("running-tab", index))
                            .ghost()
                            .h(px(36.))
                            .min_w(px(82.))
                            .px(px(16.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(rgb(if active {
                                colors.accent_border
                            } else {
                                colors.line
                            }))
                            .bg(rgb(if active { colors.selected } else { colors.bg }))
                            .child(
                                text(
                                    label,
                                    14.,
                                    if active {
                                        colors.selected_text
                                    } else {
                                        colors.muted
                                    },
                                )
                                .font_weight(if active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                }),
                            )
                            .child(text(counts[index].to_string(), 12., colors.muted))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.running_tab = tab;
                                this.smooth_scroll.cancel();
                                this.list_state.scroll_to(ListOffset {
                                    item_ix: 0,
                                    offset_in_item: px(0.),
                                });
                                cx.notify();
                            }))
                    }),
                ),
            )
            .when_some(self.discovery.warning.clone(), |s, warning| {
                s.child(text(warning, 12., 0xa34b3e).mt_3())
            })
    }

    fn running_row(&self, entry: RunningEntry, cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors();
        let name = entry.name().to_owned();
        let id = entry.key();
        let url = entry.server.url.clone();
        let directory = entry.server.directory.clone();
        let endpoint = entry
            .server
            .addresses
            .iter()
            .map(|address| {
                if address.contains(':') {
                    format!("[{address}]:{}", entry.server.port)
                } else {
                    format!("{address}:{}", entry.server.port)
                }
            })
            .collect::<Vec<_>>()
            .join(" · ");
        let preview = if let Some(project) = entry
            .project
            .as_ref()
            .filter(|p| self.images.contains_key(&p.id))
        {
            let project_id = project.id.clone();
            div()
                .id(SharedString::from(format!("{id}-preview")))
                .flex_shrink_0()
                .cursor_pointer()
                .child(self.art(project, 68., 100., Corners::all(px(7.))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(project_id.clone());
                    this.show_logs = false;
                    cx.notify();
                }))
                .into_any_element()
        } else {
            div()
                .w(px(100.))
                .h(px(68.))
                .flex_shrink_0()
                .rounded(px(7.))
                .bg(rgb(colors.panel))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(colors.accent))
                .child(icon(IconName::ExternalLink, 26.))
                .into_any_element()
        };
        div()
            .id(SharedString::from(id.clone()))
            .flex()
            .items_center()
            .gap_4()
            .p_4()
            .w_full()
            .bg(rgb(colors.surface))
            .border_1()
            .border_color(rgb(colors.line))
            .rounded(px(12.))
            .child(preview)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        text(name, 16., colors.ink)
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate(),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .mt_1()
                            .child(text(entry.server.kind.clone(), 12., colors.accent))
                            .child(text(
                                if entry.owned {
                                    "工作台启动"
                                } else {
                                    "外部启动"
                                },
                                12.,
                                colors.muted,
                            ))
                            .when(entry.server.pid != 0, |s| {
                                s.child(text(
                                    format!("PID {}", entry.server.pid),
                                    12.,
                                    colors.muted,
                                ))
                            }),
                    )
                    .child(
                        text(
                            directory.clone().unwrap_or_else(|| "目录信息不可用".into()),
                            12.,
                            colors.muted,
                        )
                        .mt_1()
                        .truncate(),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{id}-port")))
                    .flex_shrink_0()
                    .tooltip(move |window, cx| Tooltip::new(endpoint.clone()).build(window, cx))
                    .child(
                        text(
                            if entry.server.port == 0 {
                                "端口待就绪".into()
                            } else {
                                format!(":{}", entry.server.port)
                            },
                            17.,
                            colors.accent,
                        )
                        .font_weight(FontWeight::SEMIBOLD)
                        .px_3()
                        .py_2()
                        .rounded(px(7.))
                        .bg(rgb(colors.soft)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .when_some(directory, |s, path| {
                        s.child(
                            Button::new(SharedString::from(format!("{id}-folder")))
                                .icon(icon(IconName::FolderOpen, 18.))
                                .ghost()
                                .size(px(36.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(rgb(colors.line))
                                .tooltip("打开项目文件夹")
                                .on_click(move |_, _, _| {
                                    let _ = backend::open(&path);
                                }),
                        )
                    })
                    .when_some(entry.project.filter(|_| entry.owned), |s, project| {
                        s.child(
                            button(
                                SharedString::from(format!("{id}-stop")),
                                "停止",
                                IconName::Close,
                            )
                            .h(px(36.))
                            .px_3()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(rgb(0xe8c9c2))
                            .bg(rgb(0xf9ece8))
                            .text_color(rgb(0xa34b3e))
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.action(&project, "stop", cx)
                                }),
                            ),
                        )
                    })
                    .child(
                        button(
                            SharedString::from(format!("{id}-open")),
                            "打开",
                            IconName::ExternalLink,
                        )
                        .h(px(36.))
                        .px_3()
                        .rounded(px(8.))
                        .primary()
                        .disabled(url.is_empty())
                        .tooltip(url.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Err(error) = backend::open(&url) {
                                this.error = true;
                                this.message = format!("无法打开页面：{error}");
                                this.message_at = Instant::now();
                                cx.notify();
                            }
                        })),
                    ),
            )
            .into_any_element()
    }

    fn content(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if self.view == View::Running {
            return self.running_content(window, cx);
        }
        let width = (f32::from(window.viewport_size().width) - 228. - 72.).max(400.);
        let columns = if self.list {
            1
        } else if width > 1300. {
            4
        } else if width > 850. {
            3
        } else {
            2
        };
        let card_width = (width - 22. * (columns - 1) as f32) / columns as f32;
        let projects = self.filtered(cx);
        let row_count = projects.len().div_ceil(columns).max(1);
        let key = (
            projects.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
            columns,
            width.to_bits(),
        );
        if self.list_key != key {
            self.smooth_scroll.cancel();
            let previous = self.list_state.logical_scroll_top();
            let same_projects = self.list_key.0 == key.0 && self.list_key.1 == key.1;
            self.list_state.reset(row_count + 1);
            if same_projects {
                self.list_state.scroll_to(previous);
            }
            self.list_key = key;
        }
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .child(
                list(
                    self.list_state.clone(),
                    cx.processor(move |this, index, _, cx| {
                        let item = if index == 0 {
                            this.library_header(width, projects.len(), cx)
                                .into_any_element()
                        } else if projects.is_empty() {
                            this.library_empty().into_any_element()
                        } else {
                            let first = (index - 1) * columns;
                            div()
                                .flex()
                                .gap(px(22.))
                                .pb(px(22.))
                                .children(
                                    projects
                                        .iter()
                                        .skip(first)
                                        .take(columns)
                                        .cloned()
                                        .map(|p| this.card(p, card_width, cx)),
                                )
                                .into_any_element()
                        };
                        div().px(px(36.)).child(item).into_any_element()
                    }),
                )
                .flex_1()
                .min_h_0(),
            )
            .child(self.smooth_scroll.layer(self.list_state.clone()))
            .child(
                Scrollbar::vertical(&self.smooth_scroll.handle(&self.list_state))
                    .scrollbar_show(ScrollbarShow::Always),
            )
            .into_any_element()
    }
    fn library_header(
        &self,
        width: f32,
        project_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = self.colors();
        let mut engines = self
            .snapshot
            .projects
            .iter()
            .filter(|p| self.view.includes(p))
            .flat_map(|p| {
                p.categories(self.snapshot.settings.game_engines_only)
                    .into_iter()
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        engines.sort();
        engines.dedup();
        engines.insert(0, "全部引擎".into());
        let sort_view = cx.entity().downgrade();
        let selected_sort = self.sort_order;
        div()
            .pt_6()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .mb_4()
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_3()
                            .child(
                                text(self.view.label(), 18., colors.ink)
                                    .max_w(px(230.))
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD),
                            )
                            .child(
                                text(format!("{} 个项目", project_count), 11., colors.muted)
                                    .flex_shrink_0(),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap_3()
                            .child(self.search_field(if width < 850. { 200. } else { 320. }))
                            .child(
                                Button::new("sort")
                                    .icon(icon(IconName::SortDescending, 18.))
                                    .ghost()
                                    .h(px(36.))
                                    .min_w(px(140.))
                                    .px(px(14.))
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_color(rgb(colors.line))
                                    .child(text(self.sort_order.label(), 14., colors.ink))
                                    .child(icon(IconName::ChevronDown, 14.))
                                    .dropdown_menu_with_anchor(
                                        Corner::TopRight,
                                        move |mut menu, _, _| {
                                            for (order, label) in [
                                                (SortOrder::Modified, "最近更新"),
                                                (SortOrder::Created, "创建日期（最新优先）"),
                                                (SortOrder::Name, "名称 A–Z"),
                                            ] {
                                                let view = sort_view.clone();
                                                menu = menu.item(
                                                    PopupMenuItem::new(label)
                                                        .checked(selected_sort == order)
                                                        .on_click(move |_, _, cx| {
                                                            let _ = view.update(cx, |this, cx| {
                                                                this.sort_order = order;
                                                                this.smooth_scroll.cancel();
                                                                this.list_state.scroll_to(
                                                                    ListOffset {
                                                                        item_ix: 0,
                                                                        offset_in_item: px(0.),
                                                                    },
                                                                );
                                                                cx.notify();
                                                            });
                                                        }),
                                                );
                                            }
                                            menu
                                        },
                                    ),
                            )
                            .child(
                                Button::new("layout")
                                    .icon(icon(
                                        if self.list {
                                            IconName::LayoutDashboard
                                        } else {
                                            IconName::Menu
                                        },
                                        18.,
                                    ))
                                    .ghost()
                                    .size(px(36.))
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_color(rgb(colors.line))
                                    .tooltip(if self.list {
                                        "切换到卡片视图"
                                    } else {
                                        "切换到列表视图"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.list = !this.list;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                button("scan", "扫描", icon(ActionIcon::Refresh, 18.))
                                    .h(px(36.))
                                    .px(px(16.))
                                    .rounded(px(8.))
                                    .primary()
                                    .tooltip("扫描全部目录")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let mut ops = vec![("scan".into(), json!({}))];
                                        if this.snapshot.settings.auto_capture {
                                            ops.push(("capture-all".into(), json!({})));
                                        }
                                        this.send(ops, None, "项目目录已重新扫描", cx);
                                    })),
                            ),
                    ),
            )
            .child(
                div().flex().items_center().justify_between().mb_5().child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(engines.into_iter().map(|engine| {
                            let active = self.engine == engine;
                            Button::new(SharedString::from(format!("engine-{engine}")))
                                .ghost()
                                .h(px(36.))
                                .min_w(px(82.))
                                .px(px(16.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(rgb(if active {
                                    colors.accent_border
                                } else {
                                    colors.line
                                }))
                                .bg(rgb(if active { colors.selected } else { colors.bg }))
                                .text_color(rgb(if active { colors.accent } else { colors.muted }))
                                .child(
                                    text(
                                        if engine == "全部引擎"
                                            && !self.snapshot.settings.game_engines_only
                                        {
                                            "全部类型".into()
                                        } else {
                                            engine.clone()
                                        },
                                        14.,
                                        if active {
                                            colors.selected_text
                                        } else {
                                            colors.muted
                                        },
                                    )
                                    .font_weight(if active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    }),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.engine = engine.clone();
                                    cx.notify();
                                }))
                        })),
                ),
            )
    }
    fn library_empty(&self) -> impl IntoElement {
        let colors = self.colors();
        div().when(true, |s| {
            s.child(
                div()
                    .h(px(250.))
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors.line))
                    .child(
                        div()
                            .text_color(rgb(colors.accent))
                            .child(icon(IconName::Inbox, 34.)),
                    )
                    .child(text(
                        if self.connected {
                            "还没有找到这个世界"
                        } else {
                            "正在连接你的项目工作空间"
                        },
                        17.,
                        colors.muted,
                    ))
                    .child(text(
                        "尝试其他关键词，或在设置中选择项目目录。",
                        12.,
                        colors.muted,
                    )),
            )
        })
    }
    fn theme_picker(&self, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.colors();
        let columns = if width >= 600. { 3 } else { 2 };
        let card_width = (width - 12. * (columns - 1) as f32) / columns as f32;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(theme::THEMES.chunks(columns).map(|row| {
                div()
                    .flex()
                    .gap_3()
                    .children(row.iter().copied().map(|choice| {
                        let active = self.draft_theme == choice.id;
                        let preview = choice.colors();
                        Button::new(SharedString::from(format!("theme-{}", choice.id)))
                            .ghost()
                            .w(px(card_width))
                            .h(px(120.))
                            .p_0()
                            .flex_shrink_0()
                            .rounded(px(10.))
                            .border_2()
                            .border_color(rgb(if active { colors.accent } else { colors.line }))
                            .bg(rgb(preview.surface))
                            .disabled(self.busy)
                            .child(
                                div()
                                    .w(px(card_width - 4.))
                                    .h(px(116.))
                                    .flex()
                                    .flex_col()
                                    .p_2()
                                    .gap_2()
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(58.))
                                            .flex_shrink_0()
                                            .flex()
                                            .rounded(px(5.))
                                            .border_1()
                                            .border_color(rgb(preview.line))
                                            .bg(rgb(preview.bg))
                                            .child(
                                                div()
                                                    .w(px(30.))
                                                    .h_full()
                                                    .flex_shrink_0()
                                                    .p_1()
                                                    .rounded(px(4.))
                                                    .bg(rgb(preview.panel))
                                                    .child(
                                                        div()
                                                            .h(px(5.))
                                                            .w(px(13.))
                                                            .rounded(px(2.))
                                                            .bg(rgb(preview.ink))
                                                            .mb_2(),
                                                    )
                                                    .child(
                                                        div()
                                                            .h(px(7.))
                                                            .w_full()
                                                            .rounded(px(2.))
                                                            .bg(rgb(preview.selected))
                                                            .mb_1(),
                                                    )
                                                    .child(
                                                        div()
                                                            .h(px(3.))
                                                            .w(px(14.))
                                                            .rounded(px(2.))
                                                            .bg(rgb(preview.accent_border)),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .p_2()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .justify_between()
                                                            .items_center()
                                                            .child(
                                                                div()
                                                                    .w(px(30.))
                                                                    .h(px(4.))
                                                                    .rounded(px(2.))
                                                                    .bg(rgb(preview.ink)),
                                                            )
                                                            .child(
                                                                div()
                                                                    .w(px(17.))
                                                                    .h(px(7.))
                                                                    .rounded(px(2.))
                                                                    .bg(rgb(preview.accent)),
                                                            ),
                                                    )
                                                    .child(div().flex().flex_1().gap_1().children(
                                                        (0..3).map(|_| {
                                                            div()
                                                                .flex_1()
                                                                .min_w_0()
                                                                .rounded(px(3.))
                                                                .bg(rgb(preview.surface))
                                                                .border_1()
                                                                .border_color(rgb(preview.line))
                                                        }),
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_1()
                                            .child(
                                                text(choice.name, 13., preview.ink)
                                                    .truncate()
                                                    .font_weight(FontWeight::MEDIUM),
                                            )
                                            .child(
                                                div()
                                                    .size(px(18.))
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded_full()
                                                    .when(active, |s| {
                                                        s.bg(rgb(preview.ink)).child(
                                                            icon(IconName::Check, 12.).text_color(
                                                                rgb(preview.primary_text),
                                                            ),
                                                        )
                                                    }),
                                            ),
                                    )
                                    .child(div().flex().gap(px(3.)).children(choice.palette.map(
                                        |color| {
                                            div()
                                                .w(px((card_width - 32.) / 5.))
                                                .h(px(5.))
                                                .flex_shrink_0()
                                                .rounded(px(2.))
                                                .bg(rgb(color))
                                        },
                                    ))),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.draft_theme = choice.id.into();
                                theme::apply(choice.id, cx);
                                cx.notify();
                            }))
                    }))
            }))
    }

    fn choose_settings_directories(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("选择项目目录".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picker.await {
                let _ = this.update(cx, |this, cx| {
                    for path in paths {
                        this.add_root(path.to_string_lossy().into_owned());
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn settings_directories(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.colors();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Styled::h(Input::new(&self.directory), px(40.))
                            .min_h(px(40.))
                            .flex_1()
                            .min_w_0()
                            .px_3()
                            .py_0()
                            .text_size(px(14.))
                            .line_height(px(20.))
                            .rounded(px(8.)),
                    )
                    .child(
                        button("add-root", "添加", icon(IconName::Plus, 18.))
                            .h(px(40.))
                            .px_4()
                            .rounded(px(8.))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_root(this.directory.read(cx).value().to_string());
                                this.directory
                                    .update(cx, |input, cx| input.set_value("", window, cx));
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("browse-directory")
                            .icon(icon(IconName::FolderOpen, 18.))
                            .size(px(40.))
                            .rounded(px(8.))
                            .tooltip("选择目录，支持多选")
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.choose_settings_directories(cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(text("已添加的目录", 12., colors.muted))
                    .child(text(
                        format!("{} 个", self.draft_roots.len()),
                        12.,
                        colors.muted,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(self.draft_roots.is_empty(), |s| {
                        s.child(
                            div()
                                .w_full()
                                .h(px(164.))
                                .rounded(px(10.))
                                .border_1()
                                .border_color(rgb(colors.line))
                                .bg(rgb(colors.surface))
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_3()
                                .text_color(rgb(colors.accent))
                                .child(icon(IconName::FolderOpen, 28.))
                                .child(text("还没有项目目录", 14., colors.ink))
                                .child(text("输入路径或选择文件夹添加。", 12., colors.muted)),
                        )
                    })
                    .children(self.draft_roots.iter().enumerate().map(|(index, root)| {
                        let tooltip_path = root.clone();
                        div()
                            .id(("settings-root", index))
                            .w_full()
                            .min_h(px(76.))
                            .flex()
                            .items_center()
                            .gap_3()
                            .p_4()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(rgb(colors.line))
                            .bg(rgb(colors.surface))
                            .tooltip(move |window, cx| {
                                Tooltip::new(tooltip_path.clone()).build(window, cx)
                            })
                            .child(
                                div()
                                    .size(px(36.))
                                    .flex_shrink_0()
                                    .rounded(px(8.))
                                    .bg(rgb(colors.panel))
                                    .text_color(rgb(colors.accent))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon(IconName::Folder, 20.)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        text(root_name(root), 14., colors.ink)
                                            .truncate()
                                            .font_weight(FontWeight::MEDIUM),
                                    )
                                    .child(text(root.clone(), 12., colors.muted).mt_1().truncate()),
                            )
                            .child(
                                button(("remove-root", index), "移除", icon(IconName::Close, 14.))
                                    .ghost()
                                    .h(px(32.))
                                    .px_2()
                                    .text_color(rgb(colors.muted))
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if index < this.draft_roots.len() {
                                            this.draft_roots.remove(index);
                                        }
                                        cx.notify();
                                    })),
                            )
                    })),
            )
    }

    fn settings_option(
        &self,
        id: &'static str,
        title: &'static str,
        description: &'static str,
        checked: bool,
        change: fn(&mut Self, bool),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = self.colors();
        div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_5()
            .min_h(px(92.))
            .p_5()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(colors.line))
            .bg(rgb(colors.surface))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(text(title, 14., colors.ink).font_weight(FontWeight::MEDIUM))
                    .child(
                        text(description, 12., colors.muted)
                            .line_height(px(20.))
                            .mt_2(),
                    ),
            )
            .child(
                Switch::new(id)
                    .checked(checked)
                    .tooltip(title)
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, value: &bool, _, cx| {
                        change(this, *value);
                        cx.notify();
                    })),
            )
    }

    fn settings_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.colors();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.settings_option(
                "auto-capture",
                "自动获取预览",
                "扫描时，自动获取缺失的项目画面。",
                self.auto_capture,
                |this, value| this.auto_capture = value,
                cx,
            ))
            .child(self.settings_option(
                "game-engines-only",
                "仅识别游戏引擎",
                "隐藏前端框架分类，其他项目保留在 Web 中。",
                self.game_engines_only,
                |this, value| this.game_engines_only = value,
                cx,
            ))
            .child(
                div()
                    .mt_3()
                    .p_5()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(colors.line))
                    .bg(rgb(colors.surface))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                text("更新项目画面", 14., colors.ink)
                                    .font_weight(FontWeight::MEDIUM),
                            )
                            .child(text("重新获取所有项目的预览。", 12., colors.muted).mt_2()),
                    )
                    .child(
                        button(
                            "refresh-all",
                            "刷新全部画面",
                            icon(ActionIcon::Refresh, 18.),
                        )
                        .h(px(40.))
                        .px_4()
                        .rounded(px(8.))
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.send(
                                vec![("capture-all".into(), json!({"force": true}))],
                                None,
                                "所有项目已加入预览队列",
                                cx,
                            );
                        })),
                    ),
            )
    }

    fn choose_storage(&mut self, window: &Window, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择数据存储目录".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picker.await {
                if let Some(path) = paths.first() {
                    let path = crate::storage::display_path(path);
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.storage_input
                            .update(cx, |input, cx| input.set_value(path, window, cx));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn storage_disabled(&self, cx: &App) -> bool {
        let value = self.storage_input.read(cx).value();
        self.busy
            || self.storage_loading
            || self.snapshot.service_active
            || value.trim().is_empty()
            || self
                .storage_directory
                .as_ref()
                .is_some_and(|path| root_key(path) == root_key(value.trim()))
    }

    fn save_storage(&mut self, cx: &mut Context<Self>) {
        if self.storage_disabled(cx) {
            return;
        }
        self.storage_error.clear();
        self.storage_pending = true;
        self.send(
            vec![(
                "storage".into(),
                json!({"path": self.storage_input.read(cx).value().to_string()}),
            )],
            None,
            "数据存储目录已设置",
            cx,
        );
    }

    fn storage_form(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.colors();
        div().flex().flex_col().gap_5()
            .when_some(self.storage_directory.clone(), |s, path| s.child(
                div().p_5().rounded(px(10.)).bg(rgb(colors.panel))
                    .child(text("当前存储目录", 12., colors.muted))
                    .child(text(path.clone(), 14., colors.ink).mt_2())
                    .child(button("open-storage", "打开目录", icon(IconName::FolderOpen, 16.))
                        .ghost().h(px(36.)).mt_3().on_click(move |_, _, _| { let _ = backend::open(&path); }))
            ))
            .child(div().flex().flex_col().gap_3()
                .child(text(if self.storage_required {"数据存储目录"} else {"新的存储目录"}, 14., colors.ink)
                    .font_weight(FontWeight::MEDIUM))
                .child(Styled::h(Input::new(&self.storage_input).disabled(self.busy || self.snapshot.service_active), px(42.))
                    .w_full().text_size(px(14.)))
                .child(div().flex().justify_end().child(button("choose-storage", "选择文件夹…", icon(IconName::FolderOpen, 18.))
                    .h(px(40.)).px_4().disabled(self.busy || self.snapshot.service_active)
                    .on_click(cx.listener(|this, _, window, cx| this.choose_storage(window, cx))))))
            .child(text(if self.storage_required {
                "请选择一个空文件夹保存项目列表、收藏、设置和预览。已有的数据目录也可以直接选择使用。"
            } else {
                "选择空文件夹，迁移完成后使用新位置。原目录保留备份。"
            }, 13., colors.muted).line_height(px(22.)))
            .when_some(self.storage_previous.clone(), |s, path| s.child(
                div().p_4().rounded(px(10.)).bg(rgb(colors.panel))
                    .child(text("发现已有数据，将自动迁移到新目录。", 13., colors.ink))
                    .child(text(path, 12., colors.muted).mt_2())
            ))
            .when(self.snapshot.service_active, |s| s.child(text("请先停止运行中的项目，并等待扫描、安装或获取画面完成。", 13., colors.muted)))
            .when(self.storage_pending, |s| s.child(text("正在准备数据，请稍候…", 13., colors.accent)))
            .when(!self.storage_error.is_empty(), |s| s.child(text(self.storage_error.clone(), 13., 0xc54646).line_height(px(22.))))
    }

    fn storage_setup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.colors();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(660.))
                    .p_8()
                    .rounded(px(16.))
                    .border_1()
                    .border_color(rgb(colors.line))
                    .bg(rgb(colors.surface))
                    .flex()
                    .flex_col()
                    .gap_6()
                    .child(
                        text("欢迎使用 Project Manager", 26., colors.ink)
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(text("首次使用，请先设置数据存储位置。", 14., colors.muted))
                    .when(self.storage_loading, |s| {
                        s.child(text("正在读取存储设置…", 14., colors.muted))
                    })
                    .when(!self.storage_loading, |s| s.child(self.storage_form(cx)))
                    .child(
                        div().flex().justify_end().child(
                            button(
                                "finish-storage-setup",
                                "保存并进入",
                                icon(IconName::Check, 18.),
                            )
                            .primary()
                            .h(px(44.))
                            .px_6()
                            .disabled(self.storage_disabled(cx))
                            .on_click(cx.listener(|this, _, _, cx| this.save_storage(cx))),
                        ),
                    ),
            )
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        self.add_root(self.directory.read(cx).value().to_string());
        let roots_changed = self
            .draft_roots
            .iter()
            .map(|root| root_key(root))
            .collect::<Vec<_>>()
            != self
                .snapshot
                .settings
                .roots
                .iter()
                .map(|root| root_key(root))
                .collect::<Vec<_>>();
        let mut ops = Vec::new();
        if roots_changed {
            ops.push(("scan".into(), json!({"roots": self.draft_roots})));
        }
        ops.push((
            "settings".into(),
            json!({
                "autoCapture": self.auto_capture,
                "gameEnginesOnly": self.game_engines_only,
                "theme": self.draft_theme,
            }),
        ));
        if roots_changed && self.auto_capture {
            ops.push(("capture-all".into(), json!({})));
        }
        self.engine = "全部引擎".into();
        self.send(ops, None, "设置已保存", cx);
    }

    fn settings_modal(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors();
        let width = (f32::from(window.viewport_size().width) - 64.).min(920.);
        let height = (f32::from(window.viewport_size().height) - TITLE_BAR_HEIGHT - 48.).min(660.);
        let navigation_width = 184.;
        let content_width = width - navigation_width - 2. - 64.;
        let description = match self.settings_section {
            SettingsSection::Appearance => "点击主题即时预览，保存后保留选择。",
            SettingsSection::Directories => "管理项目来源文件夹。",
            SettingsSection::Preview => "设置项目识别和画面获取方式。",
            SettingsSection::Storage => "设置配置、项目列表与预览的保存位置。",
        };
        let content = match self.settings_section {
            SettingsSection::Appearance => self.theme_picker(content_width, cx).into_any_element(),
            SettingsSection::Directories => self.settings_directories(cx).into_any_element(),
            SettingsSection::Preview => self.settings_preview(cx).into_any_element(),
            SettingsSection::Storage => self.storage_form(cx).into_any_element(),
        };
        let panel = div()
            .w(px(width))
            .h(px(height))
            .flex()
            .flex_col()
            .rounded(px(16.))
            .overflow_hidden()
            .bg(rgb(colors.bg))
            .border_1()
            .border_color(rgb(colors.line))
            .shadow_xl()
            .child(
                div()
                    .h(px(64.))
                    .flex_shrink_0()
                    .px_6()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(colors.line))
                    .child(text("工作台设置", 20., colors.ink).font_weight(FontWeight::SEMIBOLD))
                    .child(
                        Button::new("close-settings")
                            .icon(icon(IconName::Close, 18.))
                            .ghost()
                            .size(px(36.))
                            .rounded(px(8.))
                            .tooltip("关闭设置")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_settings(cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(navigation_width))
                            .flex_shrink_0()
                            .h_full()
                            .px_4()
                            .py_5()
                            .bg(rgb(colors.panel))
                            .border_r_1()
                            .border_color(rgb(colors.line))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .children(
                                [
                                    SettingsSection::Appearance,
                                    SettingsSection::Directories,
                                    SettingsSection::Preview,
                                    SettingsSection::Storage,
                                ]
                                .into_iter()
                                .enumerate()
                                .map(|(index, section)| {
                                    let active = self.settings_section == section;
                                    Button::new(("settings-section", index))
                                        .ghost()
                                        .w_full()
                                        .h(px(44.))
                                        .px_3()
                                        .rounded(px(8.))
                                        .bg(rgb(if active {
                                            colors.selected
                                        } else {
                                            colors.panel
                                        }))
                                        .disabled(self.busy)
                                        .child(
                                            div()
                                                .w(px(navigation_width - 56.))
                                                .flex()
                                                .items_center()
                                                .gap_3()
                                                .text_color(rgb(if active {
                                                    colors.selected_text
                                                } else {
                                                    colors.muted
                                                }))
                                                .child(icon(section.icon(), 18.))
                                                .child(
                                                    text(
                                                        section.label(),
                                                        14.,
                                                        if active {
                                                            colors.selected_text
                                                        } else {
                                                            colors.muted
                                                        },
                                                    )
                                                    .font_weight(if active {
                                                        FontWeight::SEMIBOLD
                                                    } else {
                                                        FontWeight::MEDIUM
                                                    }),
                                                ),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.settings_section = section;
                                            this.settings_scroll.set_offset(point(px(0.), px(0.)));
                                            cx.notify();
                                        }))
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .px_6()
                                    .pt_6()
                                    .pb_5()
                                    .flex_shrink_0()
                                    .child(
                                        text(self.settings_section.label(), 22., colors.ink)
                                            .font_weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(text(description, 12., colors.muted).mt_2()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .relative()
                                    .child(
                                        div()
                                            .id("settings-content-scroll")
                                            .size_full()
                                            .overflow_y_scroll()
                                            .track_scroll(&self.settings_scroll)
                                            .pl_6()
                                            .pr(px(40.))
                                            .pb_6()
                                            .child(content),
                                    )
                                    .child(
                                        Scrollbar::vertical(&self.settings_scroll)
                                            .scrollbar_show(ScrollbarShow::Always),
                                    ),
                            )
                            .child(
                                div()
                                    .h(px(72.))
                                    .flex_shrink_0()
                                    .px_6()
                                    .border_t_1()
                                    .border_color(rgb(colors.line))
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .gap_3()
                                    .child(
                                        Button::new("cancel-settings")
                                            .label("取消")
                                            .h(px(40.))
                                            .min_w(px(80.))
                                            .rounded(px(8.))
                                            .disabled(self.busy)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.close_settings(cx);
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        button(
                                            "save-settings",
                                            if self.settings_section == SettingsSection::Storage {
                                                "迁移并使用"
                                            } else {
                                                "保存设置"
                                            },
                                            icon(IconName::Check, 18.),
                                        )
                                        .primary()
                                        .h(px(40.))
                                        .px_5()
                                        .rounded(px(8.))
                                        .disabled(
                                            if self.settings_section == SettingsSection::Storage {
                                                self.storage_disabled(cx)
                                            } else {
                                                self.busy
                                            },
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                if this.settings_section == SettingsSection::Storage
                                                {
                                                    this.save_storage(cx);
                                                } else {
                                                    this.save_settings(cx);
                                                }
                                            }),
                                        ),
                                    ),
                            ),
                    ),
            );
        self.overlay(panel).into_any_element()
    }
    fn detail_modal(&self, p: Project, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors();
        let installing = p.install_state == "installing";
        let needs_install = p.status != "running" && (!p.dependencies_installed || installing);
        let p_play = p.clone();
        let p_capture = p.clone();
        let p_stop = p.clone();
        let path = p.path.clone();
        let art_height = (f32::from(window.viewport_size().height)
            - TITLE_BAR_HEIGHT
            - if self.show_logs { 420. } else { 295. })
        .clamp(200., 540.);
        let width = (f32::from(window.viewport_size().width) - 120.).min(960.);
        let detail = div()
            .w(px(width))
            .rounded(px(18.))
            .bg(rgb(colors.bg))
            .border_1()
            .border_color(rgb(colors.line))
            .shadow_xl()
            .p_6()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .mb_5()
                    .child(text(p.name.clone(), 25., colors.ink).font_weight(FontWeight::SEMIBOLD))
                    .child(
                        Button::new("close-detail")
                            .icon(IconName::Close)
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(div().rounded_lg().overflow_hidden().child(self.art(
                &p,
                art_height,
                width - 48.,
                Corners::all(px(8.)),
            )))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .mt_4()
                    .mb_3()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                button(
                                    "detail-capture",
                                    "更新画面",
                                    icon(ActionIcon::Refresh, 18.),
                                )
                                .disabled(self.busy || needs_install)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.action(&p_capture, "capture", cx),
                                )),
                            )
                            .when(p.status == "running", |s| {
                                s.child(
                                    button("stop", "停止", IconName::Close)
                                        .disabled(self.busy)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.action(&p_stop, "stop", cx)
                                        })),
                                )
                            })
                            .child(
                                button(
                                    "detail-play",
                                    if installing {
                                        "安装中…"
                                    } else if needs_install {
                                        "安装依赖"
                                    } else if p.status == "running" {
                                        "打开项目"
                                    } else {
                                        "启动项目"
                                    },
                                    if installing {
                                        icon(IconName::LoaderCircle, 18.)
                                    } else if needs_install {
                                        icon(IconName::ArrowDown, 18.)
                                    } else {
                                        icon(ActionIcon::Play, 18.)
                                    },
                                )
                                .primary()
                                .disabled(self.busy || installing)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.action(
                                            &p_play,
                                            if needs_install { "install" } else { "start" },
                                            cx,
                                        )
                                    },
                                )),
                            ),
                    ),
            )
            .when(
                p.error.is_some() || p.capture_error.is_some() || p.install_error.is_some(),
                |s| {
                    s.child(
                        text(
                            p.install_error
                                .clone()
                                .or(p.error.clone())
                                .or(p.capture_error.clone())
                                .unwrap_or_default(),
                            11.,
                            0xac6549,
                        )
                        .mb_3(),
                    )
                },
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(rgb(colors.line))
                    .pt_3()
                    .child(text(p.path.clone(), 11., colors.muted))
                    .child(
                        button("detail-folder", "打开文件夹", IconName::FolderOpen)
                            .ghost()
                            .on_click(move |_, _, _| {
                                let _ = backend::open(&path);
                            }),
                    ),
            )
            .child(
                button(
                    "toggle-logs",
                    if self.show_logs {
                        "收起日志"
                    } else {
                        "查看日志"
                    },
                    IconName::SquareTerminal,
                )
                .ghost()
                .xsmall()
                .mt_2()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_logs = !this.show_logs;
                    cx.notify();
                })),
            )
            .when(self.show_logs, |s| {
                s.child(
                    div()
                        .id("logs-scroll")
                        .h(px(110.))
                        .overflow_y_scroll()
                        .rounded_lg()
                        .bg(rgb(colors.panel))
                        .p_3()
                        .mt_2()
                        .child(text(
                            if p.logs.is_empty() {
                                "尚未启动".to_owned()
                            } else {
                                p.logs.clone()
                            },
                            10.,
                            colors.muted,
                        )),
                )
            });
        self.overlay(detail).into_any_element()
    }
    fn overlay(&self, panel: impl IntoElement) -> Stateful<Div> {
        let colors = self.colors();
        div()
            .id("modal-overlay")
            .occlude()
            .absolute()
            .inset_0()
            .bg(rgba((colors.overlay << 8) | 0xb5))
            .flex()
            .items_center()
            .justify_center()
            .child(panel)
    }
}
impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.colors();
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| self.snapshot.projects.iter().find(|p| &p.id == id))
            .cloned();
        div()
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(rgb(colors.bg))
            .text_color(rgb(colors.ink))
            .font_family("Microsoft YaHei UI")
            .text_size(px(13.))
            .key_context("Workbench")
            .on_action(cx.listener(|this, _: &CloseOverlay, _, cx| {
                this.close_settings(cx);
                this.selected = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                if this.storage_required || this.storage_loading {
                    return;
                }
                this.smooth_scroll.cancel();
                this.list_state.scroll_to(ListOffset {
                    item_ix: 0,
                    offset_in_item: px(0.),
                });
                this.search.read(cx).focus_handle(cx).focus(window);
                cx.notify();
            }))
            .child(self.title_bar(window))
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    // Focusing consumes mouse-down's default action. Keep it off the
                    // title bar so Windows can drag, double-click, and use caption buttons.
                    .track_focus(&self.focus)
                    .when(self.storage_required || self.storage_loading, |s| {
                        s.child(self.storage_setup(cx))
                    })
                    .when(!self.storage_required && !self.storage_loading, |s| {
                        s.child(
                            div()
                                .size_full()
                                .flex()
                                .child(self.sidebar(cx))
                                .child(self.content(window, cx)),
                        )
                    })
                    .when_some(selected, |s, p| s.child(self.detail_modal(p, window, cx)))
                    .when(self.settings_open, |s| {
                        s.child(self.settings_modal(window, cx))
                    })
                    .when(
                        !self.storage_required
                            && !self.storage_loading
                            && !self.message.is_empty()
                            && (self.busy
                                || self.error
                                || self.message_at.elapsed() < Duration::from_secs(5)),
                        |s| {
                            s.child(
                                div()
                                    .absolute()
                                    .bottom_5()
                                    .left(px(260.))
                                    .right_8()
                                    .flex()
                                    .justify_center()
                                    .child(
                                        text(
                                            self.message.clone(),
                                            12.,
                                            if self.error {
                                                0xffe2cd
                                            } else {
                                                colors.primary_text
                                            },
                                        )
                                        .px_5()
                                        .py_3()
                                        .rounded_lg()
                                        .bg(rgb(if self.error { 0x704c3c } else { colors.ink }))
                                        .shadow_lg(),
                                    ),
                            )
                        },
                    ),
            )
    }
}
