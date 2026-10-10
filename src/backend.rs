use crate::project_service::Service;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    path::PathBuf,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default)]
    pub auto_capture: bool,
    #[serde(default)]
    pub game_engines_only: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub sort_order: SortOrder,
    #[serde(default)]
    pub favorites: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortOrder {
    Created,
    Name,
    #[default]
    #[serde(other)]
    Modified,
}

fn default_theme() -> String {
    "default".into()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    #[serde(default)]
    pub game_engines: Vec<String>,
    #[serde(default)]
    pub frameworks: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub root: String,
    pub engine: String,
    #[serde(default)]
    pub classification: Option<Classification>,
    pub modified: f64,
    #[serde(default)]
    pub created_at: Option<f64>,
    pub favorite: bool,
    pub status: String,
    pub url: Option<String>,
    #[serde(default)]
    pub lan_urls: Vec<String>,
    #[serde(default)]
    pub lan_error: Option<String>,
    pub preview: Option<String>,
    pub captured_at: Option<u64>,
    pub capture: String,
    pub capture_error: Option<String>,
    pub error: Option<String>,
    pub logs: String,
    pub dependencies_installed: bool,
    #[serde(default)]
    pub install_state: String,
    #[serde(default)]
    pub install_error: Option<String>,
}

impl Project {
    pub fn categories(&self, games_only: bool) -> Vec<&str> {
        if let Some(classification) = &self.classification {
            let mut labels: Vec<&str> = classification
                .game_engines
                .iter()
                .map(String::as_str)
                .collect();
            if !games_only {
                labels.extend(classification.frameworks.iter().map(String::as_str));
            }
            if labels.is_empty() {
                labels.push("Web");
            }
            labels
        } else if !games_only
            || matches!(
                self.engine.as_str(),
                "Phaser" | "Babylon.js" | "Cocos" | "LayaAir" | "PixiJS" | "Three.js" | "Galacean"
            )
        {
            vec![self.engine.as_str()]
        } else {
            vec!["Web"]
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub settings: Settings,
    pub cache_path: String,
    pub queue_length: usize,
    pub projects: Vec<Project>,
    #[serde(skip)]
    pub service_active: bool,
}

impl Snapshot {
    fn normalize_roots(&mut self) {
        // Old caches had one root and no per-project directory membership.
        if self.settings.roots.is_empty() && !self.settings.root.is_empty() {
            self.settings.roots.push(self.settings.root.clone());
        }
        for project in &mut self.projects {
            project.engine = project.categories(self.settings.game_engines_only)[0].to_owned();
            if project.root.is_empty() {
                project.root = project
                    .path
                    .rsplit_once(['\\', '/'])
                    .map(|(parent, _)| parent.to_owned())
                    .unwrap_or_else(|| self.settings.root.clone());
            }
        }
    }

    pub(crate) fn cached(&self) -> Self {
        let mut snapshot = self.clone();
        snapshot.normalize_roots();
        snapshot.service_active = false;
        snapshot.queue_length = 0;
        for project in &mut snapshot.projects {
            project.status = "idle".into();
            project.url = None;
            project.lan_urls.clear();
            project.lan_error = None;
            if project.install_state == "installing" {
                project.install_state = "error".into();
                project.install_error = Some("上次依赖安装已中断，请重试。".into());
                project.dependencies_installed = false;
            }
            if matches!(project.capture.as_str(), "queued" | "capturing") {
                project.capture = "idle".into();
            }
        }
        snapshot
    }
}

pub struct Request {
    pub operations: Vec<(String, Value)>,
    pub launch: Option<String>,
    pub message: String,
}
pub enum Event {
    Storage {
        directory: Option<String>,
        previous: Option<String>,
        error: Option<String>,
    },
    Snapshot(Snapshot),
    Completed(Snapshot, String),
    Error(String, bool),
}

#[derive(Clone)]
pub struct Backend {
    pub tx: mpsc::Sender<Request>,
    closing: Arc<AtomicBool>,
    thread: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

pub fn workspace() -> PathBuf {
    if let Some(root) = std::env::var_os("GPM_WORKSPACE") {
        return PathBuf::from(root);
    }
    if let Ok(exe) = std::env::current_exe() {
        for root in exe.ancestors().skip(1).take(5) {
            if root.join("Cargo.toml").is_file() && root.join("src/main.rs").is_file() {
                return root.to_owned();
            }
        }
        if let Some(root) = exe.parent() {
            return root.to_owned();
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn emit(events: &async_channel::Sender<Event>, event: Event) {
    let _ = events.send_blocking(event);
}

impl Backend {
    pub fn start() -> (Self, async_channel::Receiver<Event>) {
        let (tx, rx) = mpsc::channel::<Request>();
        let (events, receiver) = async_channel::unbounded();
        let closing = Arc::new(AtomicBool::new(false));
        let worker_closing = closing.clone();
        let worker = thread::Builder::new()
            .name("project-manager".into())
            .spawn(move || worker_loop(rx, events, worker_closing))
            .expect("Unable to create project worker");
        (
            Self {
                tx,
                closing,
                thread: Arc::new(Mutex::new(Some(worker))),
            },
            receiver,
        )
    }

    pub fn shutdown(&self) {
        self.closing.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Request {
            operations: Vec::new(),
            launch: None,
            message: String::new(),
        });
        if let Some(worker) = self.thread.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(
    rx: mpsc::Receiver<Request>,
    events: async_channel::Sender<Event>,
    worker_closing: Arc<AtomicBool>,
) {
    let legacy = std::env::var_os("GPM_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace().join(".data"));
    let migration_source = legacy.join("settings.json").is_file().then_some(legacy);
    let mut directory = None;
    let mut service = None;
    let initial = crate::storage::load_location().and_then(|location| {
        location
            .map(|path| crate::storage::load_snapshot(&path).map(|snapshot| (path, snapshot)))
            .transpose()
    });
    match initial {
        Ok(Some((path, snapshot))) => {
            directory = Some(path.clone());
            emit(
                &events,
                Event::Storage {
                    directory: Some(crate::storage::display_path(&path)),
                    previous: None,
                    error: None,
                },
            );
            emit(&events, Event::Snapshot(snapshot.clone()));
            match Service::new(path, snapshot) {
                Ok(loaded) => {
                    emit(&events, Event::Snapshot(loaded.snapshot.clone()));
                    service = Some(loaded);
                }
                Err(error) => emit(&events, Event::Error(format!("{error:#}"), true)),
            }
        }
        Ok(None) => emit(
            &events,
            Event::Storage {
                directory: None,
                previous: migration_source
                    .as_ref()
                    .map(|path| crate::storage::display_path(path)),
                error: None,
            },
        ),
        Err(error) => emit(
            &events,
            Event::Storage {
                directory: None,
                previous: migration_source
                    .as_ref()
                    .map(|path| crate::storage::display_path(path)),
                error: Some(format!("{error:#}")),
            },
        ),
    }
    let mut pending: Vec<(String, String)> = Vec::new();
    let mut last_snapshot: Option<Snapshot> = None;
    let mut last_emit = Instant::now();
    let mut last_error = None;
    while !worker_closing.load(Ordering::Relaxed) {
        // Cache browsing has no timer, listener, child process, or network runtime.
        let command = if service.as_ref().is_some_and(Service::busy) || !pending.is_empty() {
            rx.recv_timeout(Duration::from_millis(200))
        } else {
            rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        if worker_closing.load(Ordering::Relaxed) {
            break;
        }
        match command {
            Ok(command) => {
                if command.operations.len() == 1 && command.operations[0].0 == "storage" {
                    let configured = directory.is_some();
                    let result = (|| -> Result<Service> {
                        if service.as_ref().is_some_and(Service::busy) {
                            bail!(
                                "请先停止管理器启动的项目，并等待扫描、安装和画面获取完成，再更换数据目录。"
                            );
                        }
                        let requested = command.operations[0].1["path"]
                            .as_str()
                            .context("请选择数据目录")?;
                        let source = directory.as_deref().or(migration_source.as_deref());
                        let (path, snapshot) =
                            crate::storage::configure(requested, source, !configured)?;
                        // Remember the new locator even if a damaged metadata file needs attention.
                        directory = Some(path.clone());
                        service.take();
                        emit(
                            &events,
                            Event::Storage {
                                directory: Some(crate::storage::display_path(&path)),
                                previous: None,
                                error: None,
                            },
                        );
                        Service::new(path, snapshot)
                    })();
                    match result {
                        Ok(loaded) => {
                            emit(
                                &events,
                                Event::Completed(loaded.snapshot.clone(), command.message),
                            );
                            last_snapshot = Some(loaded.snapshot.clone());
                            service = Some(loaded);
                        }
                        Err(error) => emit(&events, Event::Error(format!("{error:#}"), false)),
                    }
                    continue;
                }
                let result = (|| -> Result<()> {
                    let path = directory.as_ref().context("请先设置数据存储目录")?;
                    if service.is_none() {
                        service = Some(Service::new(
                            path.clone(),
                            crate::storage::load_snapshot(path)?,
                        )?);
                    }
                    let active = service.as_mut().unwrap();
                    for (endpoint, value) in &command.operations {
                        if worker_closing.load(Ordering::Relaxed) {
                            bail!("启动已取消");
                        }
                        active.execute(endpoint, value)?;
                    }
                    active.poll()?;
                    active.persist()
                })();
                match result {
                    Ok(()) => {
                        let active = service.as_ref().unwrap();
                        if let Some(id) = command.launch {
                            pending.push((id, command.message));
                        } else {
                            emit(
                                &events,
                                Event::Completed(active.snapshot.clone(), command.message),
                            );
                        }
                        last_error = None;
                    }
                    Err(error) => {
                        if let Some(active) = service.as_mut() {
                            active.refresh();
                            let _ = active.persist();
                        }
                        emit(&events, Event::Error(format!("{error:#}"), false));
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(active) = service.as_mut() {
                    if let Err(error) = active.poll() {
                        active.refresh();
                        let error = format!("{error:#}");
                        if last_error.as_ref() != Some(&error) {
                            emit(&events, Event::Error(error.clone(), false));
                            last_error = Some(error);
                        }
                    }
                }
            }
            Err(_) => break,
        }
        if worker_closing.load(Ordering::Relaxed) {
            break;
        }
        if let Some(active) = service.as_ref() {
            pending.retain(|(id, message)| {
                let project = active
                    .snapshot
                    .projects
                    .iter()
                    .find(|project| project.id == *id);
                match project {
                    Some(project) if project.status == "starting" => true,
                    Some(project) if project.status == "running" => {
                        let opened = project
                            .url
                            .as_deref()
                            .context("项目没有提供有效的本机预览地址")
                            .and_then(open);
                        match opened {
                            Ok(()) => emit(
                                &events,
                                Event::Completed(active.snapshot.clone(), message.clone()),
                            ),
                            Err(error) => emit(&events, Event::Error(format!("{error:#}"), false)),
                        }
                        false
                    }
                    Some(project) => {
                        emit(
                            &events,
                            Event::Error(
                                project.error.clone().unwrap_or_else(|| "启动已取消".into()),
                                false,
                            ),
                        );
                        false
                    }
                    None => {
                        emit(
                            &events,
                            Event::Error("项目不存在，请重新扫描目录".into(), false),
                        );
                        false
                    }
                }
            });
            if last_snapshot.as_ref() != Some(&active.snapshot)
                && (last_emit.elapsed() >= Duration::from_millis(400) || !active.busy())
            {
                if let Err(error) = active.persist() {
                    let error = format!("{error:#}");
                    if last_error.as_ref() != Some(&error) {
                        emit(&events, Event::Error(error.clone(), false));
                        last_error = Some(error);
                    }
                }
                emit(&events, Event::Snapshot(active.snapshot.clone()));
                last_snapshot = Some(active.snapshot.clone());
                last_emit = Instant::now();
            }
        }
    }
    drop(service); // Drop closes owned jobs and browser helpers, then saves the idle cache.
}

pub fn open(target: &str) -> Result<()> {
    #[cfg(windows)]
    {
        Command::new("explorer.exe")
            .arg(target)
            .creation_flags(0x08000000)
            .spawn()?;
    }
    #[cfg(not(windows))]
    Command::new("xdg-open").arg(target).spawn()?;
    Ok(())
}
