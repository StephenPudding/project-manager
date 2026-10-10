use crate::{
    backend::Snapshot,
    capture_worker::CaptureWorker,
    managed_process::{LogBuffer, OwnedProcess, project_command},
    project_network::{Network, Server},
    project_scan,
};
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{OnceLock, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Default, Deserialize, Serialize)]
struct Preview {
    time: Option<u64>,
    error: Option<String>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
struct Installation {
    state: String,
    error: Option<String>,
    #[serde(default)]
    logs: String,
}
#[derive(Clone, Copy, PartialEq)]
enum Owner {
    User,
    Capture,
}

struct Session {
    owner: Owner,
    process: Option<OwnedProcess>,
    logs: LogBuffer,
    files: Option<Server>,
    lan: Option<Server>,
    lan_attempted: bool,
    started: Instant,
    next_probe: Instant,
    probe: Option<mpsc::Receiver<bool>>,
}
struct InstallJob {
    process: Option<OwnedProcess>,
    logs: LogBuffer,
    manager: &'static str,
    exit: Option<Result<ExitStatus, String>>,
    exited: Option<Instant>,
}
struct Capture {
    id: String,
    sent: bool,
}

pub struct Service {
    pub snapshot: Snapshot,
    pub directory: PathBuf,
    previews: BTreeMap<String, Preview>,
    dependencies: BTreeMap<String, Installation>,
    sessions: HashMap<String, Session>,
    installs: HashMap<String, InstallJob>,
    queue: VecDeque<String>,
    capture: Option<Capture>,
    worker: Option<CaptureWorker>,
    network: Option<Network>,
    next_addresses: Instant,
    addresses: Vec<std::net::Ipv4Addr>,
}

fn load<T: serde::de::DeserializeOwned + Default>(file: &Path) -> Result<T> {
    if !file.try_exists()? {
        return Ok(T::default());
    }
    serde_json::from_reader(fs::File::open(file)?)
        .with_context(|| format!("无法读取缓存文件：{}", crate::storage::display_path(file)))
}

pub fn save(file: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    if fs::read(file).ok().as_deref() == Some(bytes.as_slice()) {
        return Ok(());
    }
    let parent = file.parent().context("数据目录不可用")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.persist(file).context("无法保存本机数据")?;
    Ok(())
}

impl Service {
    pub fn new(directory: PathBuf, snapshot: Snapshot) -> Result<Self> {
        fs::create_dir_all(directory.join("previews"))?;
        let previews: BTreeMap<String, Preview> = load(&directory.join("previews.json"))?;
        let mut dependencies: BTreeMap<String, Installation> =
            load(&directory.join("dependencies.json"))?;
        for state in dependencies.values_mut() {
            if state.state == "installing" {
                state.state = "error".into();
                state.error = Some("上次依赖安装已中断，请重试。".into());
            }
        }
        let mut service = Self {
            snapshot: snapshot.cached(),
            directory,
            previews,
            dependencies,
            sessions: HashMap::new(),
            installs: HashMap::new(),
            queue: VecDeque::new(),
            capture: None,
            worker: None,
            network: None,
            next_addresses: Instant::now(),
            addresses: Vec::new(),
        };
        // Older snapshots and metadata retain the same project IDs and preview filenames.
        for project in &service.snapshot.projects {
            if let Some(time) = project.captured_at {
                service
                    .previews
                    .entry(project.id.clone())
                    .or_insert(Preview {
                        time: Some(time),
                        error: project.capture_error.clone(),
                    });
            }
        }
        service.decorate();
        Ok(service)
    }

    pub fn busy(&self) -> bool {
        !self.installs.is_empty()
            || !self.queue.is_empty()
            || self.capture.is_some()
            || self.worker.is_some()
            || self
                .sessions
                .values()
                .any(|session| session.process.is_some() || session.files.is_some())
    }

    pub fn persist(&self) -> Result<()> {
        save(
            &self.directory.join("native-snapshot.json"),
            &self.snapshot.cached(),
        )
    }

    pub fn refresh(&mut self) {
        if self
            .sessions
            .values()
            .all(|session| session.process.is_none() && session.files.is_none())
        {
            self.network.take();
        }
        self.decorate();
    }

    fn index(&self, id: &str) -> Result<usize> {
        self.snapshot
            .projects
            .iter()
            .position(|project| project.id == id)
            .context("项目不存在，请重新扫描目录")
    }

    fn network(&mut self) -> Result<&Network> {
        if self.network.is_none() {
            self.network = Some(Network::new()?);
        }
        Ok(self.network.as_ref().unwrap())
    }

    fn decorate(&mut self) {
        self.snapshot.cache_path = crate::storage::display_path(&self.directory.join("previews"));
        self.snapshot.queue_length = self.queue.len() + usize::from(self.capture.is_some());
        self.snapshot.service_active = self.busy();
        for project in &mut self.snapshot.projects {
            project.favorite = self.snapshot.settings.favorites.contains(&project.id);
            project.engine =
                project.categories(self.snapshot.settings.game_engines_only)[0].to_owned();
            if let Some(preview) = self.previews.get(&project.id) {
                project.captured_at = preview.time;
                project.capture_error = preview.error.clone();
                project.preview = preview
                    .time
                    .filter(|_| {
                        self.directory
                            .join("previews")
                            .join(format!("{}.jpg", project.id))
                            .is_file()
                    })
                    .map(|time| format!("/previews/{}.jpg?v={time}", project.id));
            }
            if let Some(dependency) = self.dependencies.get(&project.id) {
                project.install_state = dependency.state.clone();
                project.install_error = dependency.error.clone();
                if matches!(dependency.state.as_str(), "installing" | "error") {
                    project.dependencies_installed = false;
                    project.logs = dependency.logs.clone();
                }
            }
        }
    }

    pub fn execute(&mut self, endpoint: &str, value: &Value) -> Result<()> {
        match endpoint {
            "settings" => {
                let mut settings = self.snapshot.settings.clone();
                if let Some(id) = value["favorite"].as_str() {
                    self.index(id)?;
                    if settings.favorites.iter().any(|favorite| favorite == id) {
                        settings.favorites.retain(|favorite| favorite != id);
                    } else {
                        settings.favorites.push(id.into());
                    }
                }
                if let Some(auto) = value["autoCapture"].as_bool() {
                    settings.auto_capture = auto;
                }
                if let Some(games) = value["gameEnginesOnly"].as_bool() {
                    settings.game_engines_only = games;
                }
                if let Some(theme) = value["theme"].as_str() {
                    settings.theme = crate::theme::find(theme).id.into();
                }
                if let Some(order) = value.get("sortOrder") {
                    settings.sort_order = serde_json::from_value(order.clone())?;
                }
                save(&self.directory.join("settings.json"), &settings)?;
                self.snapshot.settings = settings;
            }
            "scan" => self.scan(value)?,
            "capture-all" => {
                let root = value["root"].as_str().map(project_scan::key);
                if root.as_ref().is_some_and(|root| {
                    !self
                        .snapshot
                        .settings
                        .roots
                        .iter()
                        .any(|value| project_scan::key(value) == *root)
                }) {
                    bail!("目录未加入工作台");
                }
                let ids: Vec<_> = self
                    .snapshot
                    .projects
                    .iter()
                    .filter(|project| {
                        root.as_ref()
                            .is_none_or(|root| project_scan::key(&project.root) == *root)
                    })
                    .map(|project| project.id.clone())
                    .collect();
                for id in ids {
                    self.enqueue(&id, value["force"].as_bool().unwrap_or(false))?;
                }
                save(&self.directory.join("previews.json"), &self.previews)?;
            }
            _ => {
                let mut parts = endpoint.split('/');
                let (Some("projects"), Some(id), Some(action), None) =
                    (parts.next(), parts.next(), parts.next(), parts.next())
                else {
                    bail!("不支持此管理操作");
                };
                self.index(id)?;
                match action {
                    "start" => self.start(id, Owner::User)?,
                    "stop" => {
                        self.queue.retain(|queued| queued != id);
                        if self
                            .capture
                            .as_ref()
                            .is_some_and(|capture| capture.id == id)
                        {
                            self.worker.take();
                            self.finish_capture(Err(anyhow::anyhow!("画面获取已取消")))?;
                        }
                        self.stop(id);
                    }
                    "capture" => {
                        self.enqueue(id, true)?;
                        save(&self.directory.join("previews.json"), &self.previews)?;
                    }
                    "install" => self.install(id)?,
                    "folder" => {
                        crate::backend::open(&self.snapshot.projects[self.index(id)?].path)?
                    }
                    _ => bail!("不支持此管理操作"),
                }
            }
        }
        self.decorate();
        Ok(())
    }

    fn scan(&mut self, value: &Value) -> Result<()> {
        if !self.installs.is_empty() {
            bail!("正在安装依赖，请稍候。");
        }
        let requested = if let Some(roots) = value.get("roots") {
            serde_json::from_value(roots.clone())?
        } else if let Some(root) = value["root"].as_str() {
            vec![root.to_owned()]
        } else {
            self.snapshot.settings.roots.clone()
        };
        let roots = project_scan::roots(&requested)?;
        let mut projects = project_scan::scan(&roots)?;
        // Complete the scan and settings write before changing the active library.
        let mut settings = self.snapshot.settings.clone();
        settings.root = roots.first().cloned().unwrap_or_default();
        settings.roots = roots;
        save(&self.directory.join("settings.json"), &settings)?;
        for project in &mut projects {
            if let Some(previous) = self
                .snapshot
                .projects
                .iter()
                .find(|previous| previous.id == project.id)
            {
                project.status = previous.status.clone();
                project.url = previous.url.clone();
                project.lan_urls = previous.lan_urls.clone();
                project.lan_error = previous.lan_error.clone();
                project.error = previous.error.clone();
                project.logs = previous.logs.clone();
                project.capture = previous.capture.clone();
            }
            // A manual install outside the manager clears a previous failed-install state.
            if project.dependencies_installed {
                if let Some(state) = self
                    .dependencies
                    .get_mut(&project.id)
                    .filter(|state| state.state == "error")
                {
                    state.state = "done".into();
                    state.error = None;
                }
            }
        }
        let removed: Vec<_> = self
            .sessions
            .keys()
            .filter(|id| !projects.iter().any(|project| project.id == **id))
            .cloned()
            .collect();
        for id in removed {
            self.stop(&id);
        }
        self.queue
            .retain(|id| projects.iter().any(|project| project.id == *id));
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| !projects.iter().any(|project| project.id == capture.id))
        {
            self.worker.take();
            self.capture.take();
        }
        self.snapshot.settings = settings;
        self.snapshot.projects = projects;
        save(
            &self.directory.join("dependencies.json"),
            &self.dependencies,
        )?;
        Ok(())
    }

    fn start(&mut self, id: &str, owner: Owner) -> Result<()> {
        let index = self.index(id)?;
        if matches!(
            self.snapshot.projects[index].status.as_str(),
            "starting" | "running"
        ) {
            if owner == Owner::User {
                if let Some(session) = self.sessions.get_mut(id) {
                    session.owner = owner;
                }
            }
            return Ok(());
        }
        if self.installs.contains_key(id) {
            bail!("正在安装依赖，请稍候。");
        }
        let project = &self.snapshot.projects[index];
        let directory = PathBuf::from(&project.path);
        let pkg = project_scan::read_json(&directory.join("package.json")).unwrap_or_default();
        if !project_scan::dependencies_present(&directory, &pkg)
            || self
                .dependencies
                .get(id)
                .is_some_and(|state| state.state == "error")
        {
            bail!("项目尚未安装依赖，请先点击“安装依赖”。");
        }
        if self
            .sessions
            .values()
            .filter(|session| session.process.is_some() || session.files.is_some())
            .count()
            >= 4
        {
            bail!("最多同时运行 4 个项目，请先停止一个项目。");
        }
        let mut session = Session {
            owner,
            process: None,
            logs: LogBuffer::default(),
            files: None,
            lan: None,
            lan_attempted: false,
            started: Instant::now(),
            next_probe: Instant::now(),
            probe: None,
        };
        let url = if let Some(script) = project_scan::development_script(&pkg) {
            let manager = project_scan::package_manager(&directory, &pkg)?;
            let mut command = project_command(&directory, manager, &["run", script])?;
            let mut process = OwnedProcess::spawn(&mut command)?;
            session.logs = process.collect_logs();
            session.process = Some(process);
            self.network()?;
            None
        } else {
            if !directory.join("index.html").is_file() {
                bail!("项目缺少开发命令或 index.html，请重新扫描。");
            }
            let files = self.network()?.files(&directory)?;
            let url = format!("http://127.0.0.1:{}/", files.port);
            session.files = Some(files);
            Some(url)
        };
        let project = &mut self.snapshot.projects[index];
        project.status = "starting".into();
        project.url = url;
        project.error = None;
        project.logs.clear();
        project.lan_urls.clear();
        project.lan_error = None;
        self.sessions.insert(id.to_owned(), session);
        Ok(())
    }

    fn stop(&mut self, id: &str) {
        self.sessions.remove(id); // Job close also stops child processes and LAN sockets.
        if let Ok(index) = self.index(id) {
            let project = &mut self.snapshot.projects[index];
            project.status = "idle".into();
            project.url = None;
            project.error = None;
            project.lan_urls.clear();
            project.lan_error = None;
            if matches!(project.capture.as_str(), "queued" | "capturing") {
                project.capture = "idle".into();
            }
        }
    }

    fn install(&mut self, id: &str) -> Result<()> {
        let index = self.index(id)?;
        if self.installs.contains_key(id) {
            bail!("正在安装依赖，请稍候。");
        }
        let project = &self.snapshot.projects[index];
        if matches!(project.status.as_str(), "starting" | "running")
            || self.queue.iter().any(|queued| queued == id)
            || self
                .capture
                .as_ref()
                .is_some_and(|capture| capture.id == id)
        {
            bail!("请先停止运行或等待预览获取完成，再安装依赖。");
        }
        let directory = PathBuf::from(&project.path);
        let pkg = project_scan::read_json(&directory.join("package.json"))?;
        let manager = project_scan::package_manager(&directory, &pkg)?;
        let args: &[&str] = match manager {
            "npm" => &["install", "--include=dev", "--no-audit", "--no-fund"],
            "pnpm" => &["install", "--prod=false"],
            _ => &["install"],
        };
        let mut command = project_command(&directory, manager, args)?;
        command.env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0");
        let mut process = OwnedProcess::spawn(&mut command)?;
        let logs = process.collect_logs();
        self.dependencies.insert(
            id.into(),
            Installation {
                state: "installing".into(),
                error: None,
                logs: format!("> {manager} install\n"),
            },
        );
        if let Err(error) = save(
            &self.directory.join("dependencies.json"),
            &self.dependencies,
        ) {
            let state = self.dependencies.get_mut(id).unwrap();
            state.state = "error".into();
            state.error = Some(format!("保存安装结果失败：{error:#}"));
            return Err(error);
        }
        self.installs.insert(
            id.into(),
            InstallJob {
                process: Some(process),
                logs,
                manager,
                exit: None,
                exited: None,
            },
        );
        self.stop(id);
        self.snapshot.projects[index].dependencies_installed = false;
        Ok(())
    }

    fn enqueue(&mut self, id: &str, force: bool) -> Result<()> {
        let index = self.index(id)?;
        if (!force
            && self
                .previews
                .get(id)
                .is_some_and(|preview| preview.time.is_some())
            && self
                .directory
                .join("previews")
                .join(format!("{id}.jpg"))
                .is_file())
            || self.queue.iter().any(|queued| queued == id)
            || self
                .capture
                .as_ref()
                .is_some_and(|capture| capture.id == id)
        {
            return Ok(());
        }
        if self.installs.contains_key(id)
            || !self.snapshot.projects[index].dependencies_installed
            || self
                .dependencies
                .get(id)
                .is_some_and(|state| state.state == "error")
        {
            self.previews.entry(id.into()).or_default().error = Some("项目依赖未安装".into());
            return Ok(());
        }
        self.snapshot.projects[index].capture = "queued".into();
        self.queue.push_back(id.into());
        Ok(())
    }

    pub fn poll(&mut self) -> Result<()> {
        self.poll_sessions();
        self.poll_installations()?;
        self.poll_capture()?;
        self.refresh();
        Ok(())
    }

    fn poll_sessions(&mut self) {
        if Instant::now() >= self.next_addresses
            && self
                .sessions
                .values()
                .any(|session| session.owner == Owner::User)
        {
            self.addresses = crate::project_network::lan_addresses();
            self.next_addresses = Instant::now() + Duration::from_secs(5);
        }
        static URL: OnceLock<Regex> = OnceLock::new();
        let pattern = URL.get_or_init(|| {
            Regex::new(r"https?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\]):\d+[^\s\x1b]*")
                .unwrap()
        });
        for (id, session) in &mut self.sessions {
            let Some(project) = self
                .snapshot
                .projects
                .iter_mut()
                .find(|project| project.id == *id)
            else {
                continue;
            };
            project.logs = session.logs.read();
            let failure = match session.process.as_mut().map(OwnedProcess::try_wait) {
                Some(Ok(Some(code))) => Some(format!(
                    "启动进程已退出（{}）",
                    code.code()
                        .map_or_else(|| code.to_string(), |code| code.to_string())
                )),
                Some(Err(error)) => Some(format!("无法读取项目进程状态：{error}")),
                _ if project.status == "starting"
                    && session.started.elapsed() > Duration::from_secs(60) =>
                {
                    Some("60 秒内未能启动，请查看运行日志。".into())
                }
                _ => None,
            };
            if let Some(error) = failure {
                session.process.take();
                session.files.take();
                session.lan.take();
                session.probe.take();
                project.status = "error".into();
                project.error = Some(error);
                project.url = None;
                project.lan_urls.clear();
                project.lan_error = None;
            }
            if project.status == "starting" {
                if project.url.is_none() {
                    if let Some(matched) = pattern.find(&project.logs) {
                        if let Ok(mut url) =
                            url::Url::parse(matched.as_str().trim_end_matches([')', ',', ';']))
                        {
                            match url.host_str() {
                                Some("0.0.0.0") => {
                                    let _ = url.set_host(Some("127.0.0.1"));
                                }
                                Some("[::]") => {
                                    let _ = url.set_host(Some("[::1]"));
                                }
                                _ => {}
                            }
                            project.url = Some(url.into());
                        }
                    }
                }
                if let Some(probe) = &session.probe {
                    match probe.try_recv() {
                        Ok(ready) => {
                            session.probe = None;
                            session.next_probe = Instant::now() + Duration::from_millis(350);
                            if ready {
                                project.status = "running".into();
                            }
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            session.probe = None;
                        }
                        Err(_) => {}
                    }
                }
                if session.probe.is_none()
                    && project.status == "starting"
                    && Instant::now() >= session.next_probe
                {
                    if let (Some(network), Some(url)) = (&self.network, &project.url) {
                        match network.probe(url) {
                            Ok(probe) => session.probe = Some(probe),
                            Err(error) => {
                                project.error = Some(format!("{error:#}"));
                                project.status = "error".into();
                                session.process.take();
                                session.files.take();
                            }
                        }
                    }
                }
            }
            if project.status == "running" && session.owner == Owner::User {
                if !session.lan_attempted {
                    session.lan_attempted = true;
                    if let (Some(network), Some(url)) = (&self.network, &project.url) {
                        match network.relay(url) {
                            Ok(server) => session.lan = Some(server),
                            Err(error) => {
                                project.lan_error = Some(format!("局域网预览启动失败：{error:#}"))
                            }
                        }
                    }
                }
                if let (Some(server), Some(url)) = (&session.lan, &project.url) {
                    project.lan_urls = self
                        .addresses
                        .iter()
                        .filter_map(|ip| {
                            let mut url = url::Url::parse(url).ok()?;
                            url.set_host(Some(&ip.to_string())).ok()?;
                            url.set_port(Some(server.port)).ok()?;
                            url.set_scheme("http").ok()?;
                            Some(url.into())
                        })
                        .collect();
                    project.lan_error = project
                        .lan_urls
                        .is_empty()
                        .then(|| "未找到可用的局域网 IPv4 地址，请连接 Wi-Fi 或有线网络。".into());
                }
            }
        }
    }

    fn poll_installations(&mut self) -> Result<()> {
        let mut completed = Vec::new();
        for (id, job) in &mut self.installs {
            if job.exit.is_none() {
                match job.process.as_mut().unwrap().try_wait() {
                    Ok(Some(status)) => job.exit = Some(Ok(status)),
                    Err(error) => job.exit = Some(Err(error.to_string())),
                    _ => {}
                }
                if job.exit.is_some() {
                    job.process.take();
                    job.exited = Some(Instant::now());
                }
            }
            if let Some(state) = self.dependencies.get_mut(id) {
                state.logs = format!("> {} install\n{}", job.manager, job.logs.read());
            }
            if job.exit.is_some()
                && (job.logs.finished()
                    || job
                        .exited
                        .is_some_and(|time| time.elapsed() > Duration::from_secs(2)))
            {
                completed.push(id.clone());
            }
        }
        for id in completed {
            let job = self.installs.remove(&id).unwrap();
            let index = self.index(&id)?;
            let directory = PathBuf::from(&self.snapshot.projects[index].path);
            let installed = project_scan::read_json(&directory.join("package.json"))
                .is_ok_and(|pkg| project_scan::dependencies_present(&directory, &pkg));
            let result = match job.exit.unwrap() {
                Ok(status) if status.success() && installed => Ok(()),
                Ok(status) if status.success() => {
                    Err("安装命令已结束，但未找到依赖文件，请查看安装日志。".into())
                }
                Ok(status) => Err(format!(
                    "{} 安装失败（退出码 {}），请查看日志后重试。",
                    job.manager,
                    status
                        .code()
                        .map_or_else(|| status.to_string(), |code| code.to_string())
                )),
                Err(error) => Err(error),
            };
            let state = self.dependencies.get_mut(&id).unwrap();
            state.state = if result.is_ok() { "done" } else { "error" }.into();
            state.error = result.err();
            self.snapshot.projects[index].dependencies_installed = state.error.is_none();
            self.snapshot.projects[index].logs = state.logs.clone();
            if state.error.is_none() {
                if let Some(preview) = self
                    .previews
                    .get_mut(&id)
                    .filter(|preview| preview.error.as_deref() == Some("项目依赖未安装"))
                {
                    preview.error = None;
                }
            }
            save(
                &self.directory.join("dependencies.json"),
                &self.dependencies,
            )?;
            save(&self.directory.join("previews.json"), &self.previews)?;
        }
        Ok(())
    }

    fn poll_capture(&mut self) -> Result<()> {
        if self.worker.as_mut().is_some_and(CaptureWorker::closed) {
            self.worker.take();
        }
        if let Some(capture) = &self.capture {
            if capture.sent {
                if let Some(result) = self.worker.as_mut().and_then(CaptureWorker::result) {
                    if result.is_err() {
                        self.worker.take();
                    }
                    self.finish_capture(result)?;
                }
            } else {
                let index = self.index(&capture.id)?;
                let project = &self.snapshot.projects[index];
                if project.status == "error" {
                    self.finish_capture(Err(anyhow::anyhow!(
                        "{}",
                        project.error.as_deref().unwrap_or("项目启动失败")
                    )))?;
                } else if project.status == "running" {
                    let url = project
                        .url
                        .clone()
                        .context("项目没有提供有效的本机预览地址")?;
                    let file = self
                        .directory
                        .join("previews")
                        .join(format!("{}.jpg", project.id));
                    let result = (|| -> Result<()> {
                        if self.worker.is_none() {
                            self.worker = Some(CaptureWorker::new(&self.directory)?);
                        }
                        self.worker.as_mut().unwrap().request(&url, &file)
                    })();
                    match result {
                        Ok(()) => self.capture.as_mut().unwrap().sent = true,
                        Err(error) => {
                            self.worker.take();
                            self.finish_capture(Err(error))?;
                        }
                    }
                }
            }
        }
        if self.capture.is_none() {
            if let Some(id) = self.queue.pop_front() {
                // A just-closed helper finishes before another batch starts.
                if self
                    .worker
                    .as_ref()
                    .is_some_and(|worker| worker.is_closing())
                {
                    self.queue.push_front(id);
                    return Ok(());
                }
                self.capture = Some(Capture {
                    id: id.clone(),
                    sent: false,
                });
                let index = self.index(&id)?;
                self.snapshot.projects[index].capture = "capturing".into();
                if let Err(error) = self.start(&id, Owner::Capture) {
                    self.finish_capture(Err(error))?;
                }
            } else if let Some(worker) = self.worker.as_mut() {
                worker.close();
            }
        }
        Ok(())
    }

    fn finish_capture(&mut self, result: Result<()>) -> Result<()> {
        let Some(capture) = self.capture.take() else {
            return Ok(());
        };
        if self
            .sessions
            .get(&capture.id)
            .is_some_and(|session| session.owner == Owner::Capture)
        {
            self.stop(&capture.id);
        }
        let preview = self.previews.entry(capture.id.clone()).or_default();
        match result {
            Ok(()) => {
                preview.time =
                    Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64);
                preview.error = None;
            }
            Err(error) => preview.error = Some(format!("{error:#}")),
        }
        if let Err(error) = save(&self.directory.join("previews.json"), &self.previews) {
            self.previews.get_mut(&capture.id).unwrap().error =
                Some(format!("保存预览缓存失败：{error:#}"));
        }
        if let Ok(index) = self.index(&capture.id) {
            self.snapshot.projects[index].capture = if self.previews[&capture.id].error.is_some() {
                "error"
            } else {
                "done"
            }
            .into();
        }
        Ok(())
    }

    pub fn shutdown(&mut self) {
        self.worker.take();
        self.capture.take();
        self.queue.clear();
        self.sessions.clear();
        self.network.take();
        for (id, job) in self.installs.drain() {
            if let Some(state) = self.dependencies.get_mut(&id) {
                state.state = "error".into();
                state.error = Some("上次依赖安装已中断，请重试。".into());
                state.logs = job.logs.read();
            }
        }
        self.snapshot = self.snapshot.cached();
        self.decorate();
        let _ = save(
            &self.directory.join("dependencies.json"),
            &self.dependencies,
        );
        let _ = self.persist();
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown();
    }
}
