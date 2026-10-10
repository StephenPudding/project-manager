use anyhow::{Context, Result, bail};
use regex::Regex;
use std::{
    io::Read,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

#[derive(Clone, Default)]
pub struct LogBuffer(Arc<LogState>);

#[derive(Default)]
struct LogState {
    text: Mutex<String>,
    readers: AtomicUsize,
}

impl LogBuffer {
    pub fn read(&self) -> String {
        self.0.text.lock().unwrap().clone()
    }

    pub fn finished(&self) -> bool {
        self.0.readers.load(Ordering::Acquire) == 0
    }

    pub fn pipe(&self, mut output: impl Read + Send + 'static) {
        let buffer = self.clone();
        buffer.0.readers.fetch_add(1, Ordering::Relaxed);
        thread::spawn(move || {
            let mut chunk = [0; 4096];
            // Retain incomplete UTF-8 sequences across pipe reads.
            let mut pending = Vec::new();
            while let Ok(length) = output.read(&mut chunk) {
                if length == 0 {
                    break;
                }
                pending.extend_from_slice(&chunk[..length]);
                let valid = match std::str::from_utf8(&pending) {
                    Ok(_) => pending.len(),
                    Err(error) => error.valid_up_to(),
                };
                if valid > 0 {
                    buffer.append(&String::from_utf8_lossy(&pending[..valid]));
                    pending.drain(..valid);
                }
                if pending.len() > 4 {
                    buffer.append(&String::from_utf8_lossy(&pending));
                    pending.clear();
                }
            }
            if !pending.is_empty() {
                buffer.append(&String::from_utf8_lossy(&pending));
            }
            buffer.0.readers.fetch_sub(1, Ordering::Release);
        });
    }

    fn append(&self, chunk: &str) {
        static ANSI: OnceLock<Regex> = OnceLock::new();
        let clean = ANSI
            .get_or_init(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap())
            .replace_all(chunk, "");
        let mut text = self.0.text.lock().unwrap();
        text.push_str(&clean);
        const LIMIT: usize = 16 * 1024;
        if text.len() > LIMIT {
            let mut start = text.len() - LIMIT;
            while !text.is_char_boundary(start) {
                start += 1;
            }
            text.drain(..start);
        }
    }
}

/// A job owns only processes created by this application, including their descendants.
pub struct OwnedProcess {
    pub child: Child,
    #[cfg(windows)]
    job: std::os::windows::io::OwnedHandle,
}

impl OwnedProcess {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        command.stdin(Stdio::null());
        Self::spawn_with_pipes(command, false)
    }

    pub fn spawn_with_pipes(command: &mut Command, stdin: bool) -> Result<Self> {
        command
            .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::{
                io::{AsRawHandle, FromRawHandle, OwnedHandle},
                process::CommandExt,
            };
            use windows_sys::Win32::{
                Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
                System::{
                    Diagnostics::ToolHelp::{
                        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                        Thread32Next,
                    },
                    JobObjects::{
                        AssignProcessToJobObject, CreateJobObjectW,
                        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                        JobObjectExtendedLimitInformation, SetInformationJobObject,
                    },
                    Threading::{
                        CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread,
                        THREAD_SUSPEND_RESUME,
                    },
                },
            };
            let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if raw.is_null() {
                return Err(std::io::Error::last_os_error()).context("无法创建项目进程组");
            }
            let job = unsafe { OwnedHandle::from_raw_handle(raw) };
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if unsafe {
                SetInformationJobObject(
                    raw,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as _,
                    std::mem::size_of_val(&limits) as u32,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error()).context("无法设置项目进程组");
            }
            // Suspend until ownership is assigned, so no child can escape the job before assignment.
            command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
            let mut child = command
                .spawn()
                .context("无法启动命令，请确认项目所需的运行环境已安装并加入 PATH")?;
            let result = (|| -> Result<()> {
                if unsafe { AssignProcessToJobObject(raw, child.as_raw_handle()) } == 0 {
                    return Err(std::io::Error::last_os_error()).context("无法管理项目进程组");
                }
                let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
                if snapshot == INVALID_HANDLE_VALUE {
                    return Err(std::io::Error::last_os_error().into());
                }
                let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
                let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
                entry.dwSize = std::mem::size_of_val(&entry) as u32;
                let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
                while found {
                    if entry.th32OwnerProcessID == child.id() {
                        let thread =
                            unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                        if thread.is_null() {
                            return Err(std::io::Error::last_os_error().into());
                        }
                        let resumed = unsafe { ResumeThread(thread) };
                        unsafe {
                            CloseHandle(thread);
                        }
                        if resumed == u32::MAX {
                            return Err(std::io::Error::last_os_error().into());
                        }
                        return Ok(());
                    }
                    found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
                }
                bail!("未找到项目启动线程");
            })();
            if let Err(error) = result {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            Ok(Self { child, job })
        }
        #[cfg(not(windows))]
        Ok(Self {
            child: command.spawn()?,
        })
    }

    pub fn collect_logs(&mut self) -> LogBuffer {
        let logs = LogBuffer::default();
        if let Some(pipe) = self.child.stdout.take() {
            logs.pipe(pipe);
        }
        if let Some(pipe) = self.child.stderr.take() {
            logs.pipe(pipe);
        }
        logs
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn stop(&mut self) {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(
                    self.job.as_raw_handle(),
                    1,
                );
            }
        }
        #[cfg(not(windows))]
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Only fixed command tokens are passed to cmd.exe. The project path is passed separately as cwd.
pub fn project_command(
    directory: &std::path::Path,
    manager: &str,
    arguments: &[&str],
) -> Result<Command> {
    if !matches!(manager, "npm" | "pnpm" | "yarn" | "bun")
        || arguments.iter().any(|arg| {
            !matches!(
                *arg,
                "run"
                    | "dev"
                    | "dev:web"
                    | "install"
                    | "--include=dev"
                    | "--no-audit"
                    | "--no-fund"
                    | "--prod=false"
            )
        })
    {
        bail!("不支持此项目命令");
    }
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("cmd.exe");
        command
            .args(["/d", "/s", "/c"])
            .arg(format!("{manager} {}", arguments.join(" ")));
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new(manager);
        command.args(arguments);
        command
    };
    command
        .current_dir(directory)
        .env("FORCE_COLOR", "0")
        .env("NO_COLOR", "1")
        .env("NODE_ENV", "development")
        .env("BROWSER", "none");
    Ok(command)
}
