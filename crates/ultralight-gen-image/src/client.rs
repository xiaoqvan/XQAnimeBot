//! 渲染子进程的客户端：拉起 `image-shot`、按行发 JSON、读回结果。
//!
//! 为什么还要子进程：Ultralight 要求 Renderer 在**主线程**创建并驱动，而宿主进程的
//! 主线程属于 FuyuBot 的 tokio 运行时。插件里没有「主线程」可用，硬塞只会在某些
//! 平台上直接崩。子进程自己拥有主线程，崩了也只崩截图。
//!
//! 这层是**同步**的：调用方应放进 `spawn_blocking` 或自己的渲染线程；内部用读线程 +
//! 超时，避免子进程挂死拖住调用方。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{Value, json};

use crate::request::RenderRequest;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("找不到 image-shot 可执行文件（设置 IMAGE_SHOT_BIN，或把它放在插件同级）")]
    BinaryMissing,
    #[error("启动 image-shot 失败：{0}")]
    Spawn(String),
    #[error("与 image-shot 通信失败：{0}")]
    Io(String),
    #[error("image-shot 在 {0} ms 内没有响应")]
    Timeout(u64),
    #[error("image-shot 返回错误：{0}")]
    Remote(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// 渲染结果。
#[derive(Debug, Clone)]
pub struct Rendered {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    pub ms: u64,
}

/// 子进程客户端。一个实例对应一个 `image-shot --serve` 进程，可按顺序复用。
pub struct ShotClient {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    timeout: Duration,
    binary: PathBuf,
}

impl ShotClient {
    /// 拉起子进程。`binary` 通常是 `image-shot.exe`。
    pub fn spawn(binary: &Path) -> Result<Self> {
        if !binary.exists() {
            return Err(Error::BinaryMissing);
        }
        let mut child = Command::new(binary)
            .arg("--serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| Error::Spawn(format!("{}: {err}", binary.display())))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Spawn("拿不到 stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Spawn("拿不到 stdout".into()))?;

        // 单独一个读线程：阻塞读 + 主线程 recv_timeout，才能实现超时。
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("image-shot-reader".into())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(line) => {
                            if tx.send(line).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|err| Error::Spawn(err.to_string()))?;

        Ok(Self {
            child,
            stdin,
            lines: rx,
            timeout: Duration::from_secs(30),
            binary: binary.to_path_buf(),
        })
    }

    /// 按约定位置查找同包的 `image-shot` 可执行文件。
    ///
    /// 开发态它就在 `target/<profile>/` 下（和插件 dll 同级），部署时一般放在插件目录里。
    pub fn find_binary() -> Option<PathBuf> {
        let name = if cfg!(windows) {
            "image-shot.exe"
        } else {
            "image-shot"
        };
        let mut candidates = Vec::new();
        if let Ok(path) = std::env::var("IMAGE_SHOT_BIN") {
            candidates.push(PathBuf::from(path));
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            candidates.push(dir.join(name));
            candidates.push(dir.join("ultralight").join(name));
            // `cargo test` / `--example` 时 exe 在 target/debug/{deps,examples}/，二进制在上一层。
            if let Some(parent) = dir.parent() {
                candidates.push(parent.join(name));
            }
        }
        if let Some(manifest) = option_env!("CARGO_MANIFEST_DIR") {
            let manifest = Path::new(manifest);
            for profile in ["debug", "release"] {
                candidates.push(manifest.join("../../target").join(profile).join(name));
            }
        }
        candidates.into_iter().find(|path| path.exists())
    }

    /// 用查找结果自动拉起；找不到就返回 `BinaryMissing`。
    pub fn auto() -> Result<Self> {
        let binary = Self::find_binary().ok_or(Error::BinaryMissing)?;
        Self::spawn(&binary)
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// 发一条原始请求，等一条响应。
    pub fn request(&mut self, request: &Value) -> Result<Value> {
        let mut line = serde_json::to_string(request).map_err(|err| Error::Io(err.to_string()))?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|err| Error::Io(err.to_string()))?;

        let response = self
            .lines
            .recv_timeout(self.timeout)
            .map_err(|_| Error::Timeout(self.timeout.as_millis() as u64))?;
        let value: Value = serde_json::from_str(&response)
            .map_err(|err| Error::Io(format!("响应不是 JSON：{err}")))?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            let message = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("未知错误")
                .to_owned();
            return Err(Error::Remote(message));
        }
        Ok(value)
    }

    /// 出图。
    pub fn render(&mut self, request: &RenderRequest) -> Result<Rendered> {
        let response = self.request(&request.to_json())?;
        let path = response
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Remote("响应缺少 path".into()))?;
        Ok(Rendered {
            path: PathBuf::from(path),
            width: response.get("width").and_then(Value::as_u64).unwrap_or(0) as u32,
            height: response.get("height").and_then(Value::as_u64).unwrap_or(0) as u32,
            bytes: response.get("bytes").and_then(Value::as_u64).unwrap_or(0) as usize,
            ms: response.get("ms").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    /// 健康检查，顺便拿到运行时路径（排障用）。
    pub fn ping(&mut self) -> Result<Value> {
        self.request(&json!({ "op": "ping" }))
    }

    /// 等子进程退出。`Drop` 里也会 kill，这里给需要优雅退出的调用方用。
    pub fn shutdown(&mut self) {
        let _ = self
            .stdin
            .write_all(b"{\"op\":\"shutdown\"}\n")
            .and_then(|_| self.stdin.flush());
        let _ = self.child.wait();
    }
}

impl Drop for ShotClient {
    fn drop(&mut self) {
        let _ = self
            .stdin
            .write_all(b"{\"op\":\"shutdown\"}\n")
            .and_then(|_| self.stdin.flush());
        // 给子进程 500ms 自己退，超时就 kill。
        for _ in 0..50 {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
    }
}
