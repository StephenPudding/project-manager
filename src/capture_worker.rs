use crate::managed_process::{LogBuffer, OwnedProcess};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{ChildStdin, Command},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub struct CaptureWorker {
    process: OwnedProcess,
    input: Option<ChildStdin>,
    output: mpsc::Receiver<Value>,
    diagnostics: LogBuffer,
    pending: Option<Instant>,
    closing: Option<Instant>,
}

impl CaptureWorker {
    pub fn new(data_dir: &Path) -> Result<Self> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--capture-webview2")
            .env("GPM_DATA_DIR", data_dir);
        let mut process = OwnedProcess::spawn_with_pipes(&mut command, true)?;
        let input = process
            .child
            .stdin
            .take()
            .context("无法连接 WebView2 截图进程")?;
        let stdout = process
            .child
            .stdout
            .take()
            .context("无法读取 WebView2 截图结果")?;
        let diagnostics = LogBuffer::default();
        if let Some(stderr) = process.child.stderr.take() {
            diagnostics.pipe(stderr);
        }
        let (tx, output) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                // The helper only emits small JSON messages; bound unexpected output.
                let mut line = Vec::new();
                match (&mut reader).take(64 * 1024).read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let result = serde_json::from_slice(&line).unwrap_or_else(
                            |_| json!({"ok": false, "error": "WebView2 返回了无效的截图结果"}),
                        );
                        if tx.send(result).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self {
            process,
            input: Some(input),
            output,
            diagnostics,
            pending: None,
            closing: None,
        })
    }

    pub fn request(&mut self, url: &str, file: &Path) -> Result<()> {
        if self.pending.is_some() || self.closing.is_some() {
            bail!("截图进程正在处理其他画面");
        }
        let input = self.input.as_mut().context("截图进程已关闭")?;
        serde_json::to_writer(&mut *input, &json!({"url": url, "file": file}))?;
        input.write_all(b"\n")?;
        input.flush()?;
        self.pending = Some(Instant::now());
        Ok(())
    }

    pub fn result(&mut self) -> Option<Result<()>> {
        let started = self.pending?;
        match self.output.try_recv() {
            Ok(value) => {
                self.pending = None;
                Some(if value["ok"].as_bool() == Some(true) {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!(
                        "{}",
                        value["error"].as_str().unwrap_or("获取画面失败")
                    ))
                })
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                if !self.diagnostics.finished() && started.elapsed() < Duration::from_secs(5) {
                    return None;
                }
                self.pending = None;
                let detail = self.diagnostics.read();
                Some(Err(anyhow::anyhow!(
                    "WebView2 截图进程已退出：{}",
                    if detail.trim().is_empty() {
                        "未收到截图结果"
                    } else {
                        detail.trim()
                    }
                )))
            }
            Err(mpsc::TryRecvError::Empty) if started.elapsed() > Duration::from_secs(120) => {
                self.pending = None;
                self.process.stop();
                Some(Err(anyhow::anyhow!(
                    "WebView2 获取画面超时，请查看项目运行日志。"
                )))
            }
            Err(_) => None,
        }
    }

    pub fn close(&mut self) {
        if self.closing.is_none() {
            // EOF lets the STA helper release its controller and environment normally.
            self.input.take();
            self.closing = Some(Instant::now());
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closing.is_some()
    }

    pub fn closed(&mut self) -> bool {
        if let Some(started) = self.closing {
            if self.process.try_wait().is_ok_and(|status| status.is_some()) {
                return true;
            }
            if started.elapsed() > Duration::from_secs(3) {
                self.process.stop();
                return true;
            }
        }
        false
    }
}
