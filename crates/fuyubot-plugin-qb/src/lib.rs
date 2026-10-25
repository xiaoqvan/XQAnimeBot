//! qBittorrent 插件（qbweb）：注册 `download.qbittorrent` 服务。
//!
//! **手写 WebUI v2 API，不用任何 qBittorrent 封装包**（按要求）。只依赖一个 HTTP
//! 客户端，把它当成「带 cookie 的 JSON 通道」用。
//!
//! ```text
//! 业务插件 ──get_service("download.qbittorrent")──▶ 本插件 ──HTTP──▶ qBittorrent WebUI
//! ```
//!
//! # 适配 qBittorrent 5.0+（WebAPI 2.11）
//!
//! 按 5.0 的接口写，不做 4.x 回退。5.0 改了三处，都能在官方源码
//! （`src/webui/api/torrentscontroller.cpp`、`src/base/torrentfilter.cpp`）里核对：
//!
//! | 项 | 4.x | 5.0+ |
//! | --- | --- | --- |
//! | 暂停/继续 | `torrents/pause`、`torrents/resume`（5.x 上 404） | `torrents/stop`、`torrents/start` |
//! | 添加后不启动 | `add` 的 `paused` | `add` 的 `stopped` |
//! | 列表过滤 | `filter=paused` / `resumed` | `filter=stopped` / `running` |
//!
//! 低于 5.0 的服务器在 `add` / `pause` / `resume` 上会直接报错说清原因，而不是
//! 静默失效——**静默失效最危险**：`add` 传不认识的参数不会报错，只会照常开始下载。
//!
//! 三个关键点：
//!
//! 1. **会话是 cookie**：登录拿到 `SID` 后由 `ureq` 的 cookie jar 自动带上，
//!    jar 挂在 `Agent` 上，所以 Agent 必须复用（每次新建 = 每次重登）。
//! 2. **403 自动重登**：qB 重启 / 会话过期都会 403。这里对 403 做一次「重登 + 重试」，
//!    业务侧不需要自己管登录态。
//! 3. **只做通道，不做业务判断**：列表原样返回 qB 的字段（`progress` 是 0~1 小数、
//!    `state` 是 qB 的字符串枚举），番剧该不该删、算不算完成由业务插件决定。
//!
//! 业务逻辑都在 [`api`] 里，`unsafe extern "C"` 只是薄壳——这样示例/集成测试能直接调，
//! 不必先起宿主。

use std::ffi::{c_char, c_void};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use fuyubot_abi::{HostApi, LogLevel, Plugin};
use fuyubot_service::{
    Handler, Service, Settings, error, ok, opt_bool, opt_str, parse, require_str,
};
use serde_json::{Value, json};
use ureq::http::Response;
use ureq::{Agent, Body};

const SERVICE: &str = "download.qbittorrent";
const PLUGIN: &str = "qb";

/// 本插件按 qBittorrent 5.0 的接口实现，低版本不保证语义（见模块文档）。
const MIN_MAJOR: u64 = 5;

/// `GET /torrents/info` 的 `filter` 白名单，抄自 5.0 的 `TorrentFilter::setTypeByName`。
///
/// 必须自己校验：qB 收到不认识的 filter **不报错**，内部默认成 `All`，
/// 于是「查暂停的任务」会拿到全部任务——这种错不会自己暴露出来。
const FILTERS: [&str; 14] = [
    "all",
    "downloading",
    "seeding",
    "completed",
    "stopped",
    "running",
    "active",
    "inactive",
    "stalled",
    "stalled_uploading",
    "stalled_downloading",
    "checking",
    "moving",
    "errored",
];

/// 启动配置：
///
/// ```json
/// { "url": "http://127.0.0.1:8080", "username": "admin", "password": "***",
///   "timeout_ms": 15000 }
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    pub url: String,
    pub username: String,
    pub password: String,
    pub timeout: Duration,
}

static SETTINGS: Settings<Config> = Settings::new();
static CLIENT: OnceLock<Mutex<Option<QbClient>>> = OnceLock::new();

struct QbPlugin {
    host: HostApi,
}

impl Plugin for QbPlugin {
    fn init(host: HostApi) -> Result<Self, String> {
        Service::new(
            SERVICE,
            vec![
                ("ping", svc_ping as Handler),
                ("login", svc_login as Handler),
                ("version", svc_version as Handler),
                ("transfer", svc_transfer as Handler),
                ("list", svc_list as Handler),
                ("files", svc_files as Handler),
                ("properties", svc_properties as Handler),
                ("add", svc_add as Handler),
                ("pause", svc_pause as Handler),
                ("resume", svc_resume as Handler),
                ("delete", svc_delete as Handler),
                ("set_category", svc_set_category as Handler),
            ],
        )
        .register(&host)?;
        Ok(Self { host })
    }

    fn start(&mut self, config_json: &str) -> Result<(), String> {
        let raw: Value = serde_json::from_str(config_json).unwrap_or(Value::Null);
        // 配置优先，其次环境变量（容器/联调时不必改宿主配置），最后内置默认值。
        let url = opt_str(&raw, "url")
            .map(str::to_owned)
            .or_else(|| std::env::var("QB_URL").ok().filter(|url| !url.trim().is_empty()))
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_owned());
        let username = opt_str(&raw, "username")
            .map(str::to_owned)
            .or_else(|| std::env::var("QB_USERNAME").ok().filter(|name| !name.is_empty()))
            .unwrap_or_else(|| "admin".to_owned());
        let password = opt_str(&raw, "password")
            .map(str::to_owned)
            .or_else(|| std::env::var("QB_PASSWORD").ok())
            .unwrap_or_default();
        let timeout_ms = raw
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(15_000);
        api::configure(&url, &username, &password, timeout_ms);

        match api::call("login", json!({})) {
            Ok(_) => self
                .host
                .log(LogLevel::Info, &format!("[{PLUGIN}] 已登录 {url}")),
            Err(err) => {
                self.host.log(
                    LogLevel::Warn,
                    &format!("[{PLUGIN}] 预热登录失败：{err}（首次调用会重试）"),
                );
                return Ok(());
            }
        }

        // 版本决定了写接口能不能用（5.0 改了端点与参数），启动时就把话说清楚。
        match api::call("ping", json!({})) {
            Ok(info) => {
                let version = info["version"].as_str().unwrap_or("未知");
                let api_version = info["api_version"].as_str().unwrap_or("未知");
                if info["supported"].as_bool().unwrap_or(false) {
                    self.host.log(
                        LogLevel::Info,
                        &format!("[{PLUGIN}] qBittorrent {version}（WebAPI {api_version}），按 5.x 接口工作"),
                    );
                } else {
                    self.host.log(
                        LogLevel::Warn,
                        &format!(
                            "[{PLUGIN}] qBittorrent {version} 低于 5.0：add/pause/resume 会被拒绝，\
                             因为 5.0 把 pause/resume 改成 stop/start、add 的 paused 改成 stopped"
                        ),
                    );
                }
            }
            Err(err) => self
                .host
                .log(LogLevel::Warn, &format!("[{PLUGIN}] 读取版本失败：{err}")),
        }
        Ok(())
    }
}

fuyubot_abi::define_plugin!(QbPlugin, "fuyubot.qb");

// ─── 业务实现（可被示例/测试直接调用） ──────────────────────────────────────

pub mod api {
    use super::*;

    pub fn configure(url: &str, username: &str, password: &str, timeout_ms: u64) {
        SETTINGS.set(Config {
            url: url.trim_end_matches('/').to_owned(),
            username: username.to_owned(),
            password: password.to_owned(),
            timeout: Duration::from_millis(timeout_ms),
        });
        // 配置变了就丢掉旧会话，下次调用重建。
        if let Some(slot) = CLIENT.get() {
            let mut guard = slot.lock().unwrap_or_else(|p| p.into_inner());
            *guard = None;
        }
    }

    pub fn call(method: &str, input: Value) -> Result<Value, String> {
        match method {
            "ping" => ping(),
            "login" => login(),
            "version" => version(),
            "transfer" => transfer(),
            "list" => list(&input),
            "files" => files(&input),
            "properties" => properties(&input),
            "add" => add(&input),
            "pause" => toggle(&input, "pause", "暂停"),
            "resume" => toggle(&input, "resume", "继续"),
            "delete" => delete(&input),
            "set_category" => set_category(&input),
            other => Err(format!("未知方法：{other}")),
        }
    }

    /// 从 `v5.1.2.10` 这种版本串里取主版本号。
    pub fn major_of(version: &str) -> Option<u64> {
        version
            .trim()
            .trim_start_matches(['v', 'V'])
            .split(['.', '-', '+'])
            .next()?
            .parse()
            .ok()
    }

    /// 把调用方给的 filter 归一成 5.0 的取值。
    ///
    /// 4.x 的 `paused` / `resumed` 自动映射（业务代码里留着旧名字也不会悄悄变成“查全部”），
    /// 其余未知值直接拒绝——qB 本身不会拒绝。
    pub(crate) fn normalize_filter(raw: &str) -> Result<String, String> {
        let lower = raw.trim().to_ascii_lowercase();
        let mapped: &str = match lower.as_str() {
            "paused" => "stopped",
            "resumed" => "running",
            other => other,
        };
        if FILTERS.contains(&mapped) {
            Ok(mapped.to_owned())
        } else {
            Err(format!(
                "未知 filter：{raw}（qB 5.0 支持 {}）",
                FILTERS.join(" / ")
            ))
        }
    }

    fn ping() -> Result<Value, String> {
        with_client(|client| {
            let base = client.config.url.clone();
            let version = client.version_text(true)?;
            let api = client.get_text("/api/v2/app/webapiVersion")?;
            let major = major_of(&version).unwrap_or(0);
            Ok(json!({
                "version": version,
                "api_version": api.trim(),
                "url": base,
                // 低于 5.0 时业务侧最好别调 add/pause/resume。
                "supported": major >= MIN_MAJOR,
            }))
        })
    }

    fn login() -> Result<Value, String> {
        with_client(|client| {
            client.logged_in = false;
            client.login()?;
            Ok(json!({ "logged_in": true }))
        })
    }

    fn version() -> Result<Value, String> {
        with_client(|client| {
            let text = client.version_text(true)?;
            Ok(json!({
                "version": text,
                "supported": major_of(&text).unwrap_or(0) >= MIN_MAJOR,
            }))
        })
    }

    fn transfer() -> Result<Value, String> {
        with_client(|client| client.get("取传输状态", "/api/v2/transfer/info", &[]))
    }

    fn list(input: &Value) -> Result<Value, String> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(filter) = opt_str(input, "filter") {
            query.push(("filter", normalize_filter(filter)?));
        }
        if let Some(category) = opt_str(input, "category") {
            query.push(("category", category.to_owned()));
        }
        if let Some(tag) = opt_str(input, "tag") {
            query.push(("tag", tag.to_owned()));
        }
        if let Some(hashes) = input.get("hashes") {
            query.push(("hashes", join_hashes(hashes)));
        }
        if let Some(limit) = input.get("limit").and_then(Value::as_u64) {
            query.push(("limit", limit.min(10_000).to_string()));
        }
        if let Some(sort) = opt_str(input, "sort") {
            query.push(("sort", sort.to_owned()));
        }
        if opt_bool(input, "reverse").unwrap_or(false) {
            query.push(("reverse", "true".to_owned()));
        }
        with_client(|client| client.get("取种子列表", "/api/v2/torrents/info", &query))
    }

    fn files(input: &Value) -> Result<Value, String> {
        let hash = require_str(input, "hash")?.to_owned();
        with_client(|client| client.get("取文件列表", "/api/v2/torrents/files", &[("hash", hash)]))
    }

    fn properties(input: &Value) -> Result<Value, String> {
        let hash = require_str(input, "hash")?.to_owned();
        with_client(|client| {
            client.get(
                "取种子属性",
                "/api/v2/torrents/properties",
                &[("hash", hash)],
            )
        })
    }

    fn add(input: &Value) -> Result<Value, String> {
        let mut form: Vec<(&str, String)> = Vec::new();

        if let Some(urls) = input.get("urls") {
            let text = match urls {
                Value::Array(items) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n"),
                Value::String(text) => text.clone(),
                _ => return Err("urls 只能是字符串或字符串数组".to_owned()),
            };
            if !text.trim().is_empty() {
                form.push(("urls", text));
            }
        }
        if let Some(category) = opt_str(input, "category") {
            form.push(("category", category.to_owned()));
        }
        if let Some(savepath) = opt_str(input, "savepath") {
            form.push(("savepath", savepath.to_owned()));
        }
        if let Some(tags) = opt_str(input, "tags") {
            form.push(("tags", tags.to_owned()));
        }
        if let Some(paused) = opt_bool(input, "paused") {
            // 5.0 起 `add` 只认 `stopped`：4.x 的 `paused` 已被移除，传了也不会报错，
            // 只会照常开始下载——所以这里绝不“两个都发”。
            form.push(("stopped", bool_text(paused)));
        }
        if let Some(skip) = opt_bool(input, "skip_checking") {
            form.push(("skip_checking", bool_text(skip)));
        }
        if let Some(auto) = opt_bool(input, "auto_tmm") {
            form.push(("autoTMM", bool_text(auto)));
        }
        if let Some(rename) = opt_str(input, "rename") {
            form.push(("rename", rename.to_owned()));
        }
        if opt_bool(input, "sequential").unwrap_or(false) {
            form.push(("sequentialDownload", "true".to_owned()));
        }

        let file = opt_str(input, "file");
        if form.is_empty() && file.is_none() {
            return Err("add 至少要给 urls 或 file".to_owned());
        }

        with_client(|client| {
            client.require_v5("添加种子")?;
            let url = client.url("/api/v2/torrents/add");
            let referer = client.config.url.clone();
            match file {
                // .torrent 走 multipart；其余参数照样一起带上。
                Some(path) => {
                    let fields = form.clone();
                    let path = path.to_owned();
                    let text = client.exec("添加种子", move |agent| {
                        let mut upload = ureq::unversioned::multipart::Form::new();
                        for (key, value) in &fields {
                            upload = upload.text(key, value);
                        }
                        let upload = match upload.file("torrents", &path) {
                            Ok(upload) => upload,
                            Err(err) => {
                                return Err(ureq::Error::Io(std::io::Error::new(
                                    std::io::ErrorKind::NotFound,
                                    format!("读不到种子文件 {path}：{err}"),
                                )));
                            }
                        };
                        agent.post(&url).header("Referer", &referer).send(upload)
                    })?;
                    check_add_response(&text)
                }
                None => {
                    let pairs: Vec<(&str, String)> = form.clone();
                    let text = client.exec("添加种子", move |agent| {
                        agent
                            .post(&url)
                            .header("Referer", &referer)
                            // 用借用迭代器：exec 可能因为 403 重试而调用两次闭包。
                            .send_form(pairs.iter().map(|(key, value)| (*key, value.as_str())))
                    })?;
                    check_add_response(&text)
                }
            }
        })
    }

    /// `pause` / `resume` 对应的 5.0 端点（4.x 的 pause/resume 在 5.x 上是 404）。
    pub(crate) fn toggle_path(action: &str) -> Option<&'static str> {
        match action {
            "pause" => Some("/api/v2/torrents/stop"),
            "resume" => Some("/api/v2/torrents/start"),
            _ => None,
        }
    }

    fn toggle(input: &Value, action: &'static str, label: &'static str) -> Result<Value, String> {
        let hashes = input
            .get("hashes")
            .map(join_hashes)
            .ok_or_else(|| "缺少 hashes".to_owned())?;
        if hashes.trim().is_empty() {
            return Err("hashes 为空".to_owned());
        }
        // 5.0 起只有 stop/start；4.x 的 pause/resume 在 5.x 上是 404。
        let path = toggle_path(action).ok_or_else(|| format!("未知动作：{action}"))?;
        with_client(|client| {
            client.require_v5(label)?;
            client.post(label, path, vec![("hashes", hashes.clone())])?;
            Ok(json!({ "action": action, "hashes": hashes, "endpoint": path }))
        })
    }

    fn delete(input: &Value) -> Result<Value, String> {
        let hashes = input
            .get("hashes")
            .map(join_hashes)
            .ok_or_else(|| "缺少 hashes".to_owned())?;
        let delete_files = opt_bool(input, "delete_files").unwrap_or(false);
        with_client(|client| {
            client.post(
                "删除种子",
                "/api/v2/torrents/delete",
                vec![
                    ("hashes", hashes.clone()),
                    ("deleteFiles", bool_text(delete_files)),
                ],
            )?;
            Ok(json!({ "deleted": hashes, "files": delete_files }))
        })
    }

    fn set_category(input: &Value) -> Result<Value, String> {
        let hashes = input
            .get("hashes")
            .map(join_hashes)
            .ok_or_else(|| "缺少 hashes".to_owned())?;
        let category = require_str(input, "category")?.to_owned();
        with_client(|client| {
            client.post(
                "改分类",
                "/api/v2/torrents/setCategory",
                vec![("hashes", hashes.clone()), ("category", category.clone())],
            )?;
            Ok(json!({ "hashes": hashes, "category": category }))
        })
    }

    // ── 客户端 ────────────────────────────────────────────────────────────

    fn with_client<T>(run: impl FnOnce(&mut QbClient) -> Result<T, String>) -> Result<T, String> {
        let config = SETTINGS.get().ok_or_else(|| {
            "download.qbittorrent 未初始化：请先调 api::configure 或在插件配置里给 url/账号"
                .to_owned()
        })?;
        let slot = CLIENT.get_or_init(|| Mutex::new(None));
        let mut guard = slot.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some(QbClient::new(config));
        }
        let client = guard.as_mut().expect("上面刚填过");
        let result = run(client);
        if result.is_err() {
            // 传输层出错就把客户端丢掉，下次重建（cookie 也可能已失效）。
            *guard = None;
        }
        result
    }
}

struct QbClient {
    agent: Agent,
    config: Config,
    logged_in: bool,
    /// 服务器版本缓存（`v5.1.2.10`）。登录后取一次，写操作前用来校验。
    version: Option<String>,
}

impl QbClient {
    fn new(config: Config) -> Self {
        let agent = Agent::config_builder()
            .timeout_global(Some(config.timeout))
            .build()
            .new_agent();
        Self {
            agent,
            config,
            logged_in: false,
            version: None,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.config.url, path)
    }

    /// 取服务器版本，默认走缓存（一次会话只查一次，写操作前会用到）。
    fn version_text(&mut self, refresh: bool) -> Result<String, String> {
        if !refresh && let Some(cached) = &self.version {
            return Ok(cached.clone());
        }
        let text = self.get_text("/api/v2/app/version")?.trim().to_owned();
        self.version = Some(text.clone());
        Ok(text)
    }

    /// 要求服务器 ≥ 5.0。低版本不是“降级运行”而是**语义不同**，必须拦住。
    fn require_v5(&mut self, label: &str) -> Result<(), String> {
        let version = self.version_text(false)?;
        if api::major_of(&version).unwrap_or(0) >= MIN_MAJOR {
            return Ok(());
        }
        Err(format!(
            "{label}：qBittorrent {version} 低于 5.0 —— 本插件按 5.x 接口实现\
             （pause/resume → stop/start、add 的 paused → stopped）。\
             该版本只在读取类方法上可用，写操作请升级 qBittorrent 或改用 4.x 兼容分支。"
        ))
    }

    /// 发一个 GET 并把响应读成纯文本（版本号这类非 JSON 接口用）。
    fn get_text(&mut self, path: &str) -> Result<String, String> {
        let url = self.url(path);
        self.exec("取版本", |agent| agent.get(&url).call())
    }

    /// 登录并刷新 cookie。qB 成功时返回纯文本 `Ok.`，失败返回 `Fails.`。
    fn login(&mut self) -> Result<(), String> {
        let url = self.url("/api/v2/auth/login");
        let referer = self.config.url.clone();
        let response = self
            .agent
            .post(&url)
            .header("Referer", &referer)
            .send_form([
                ("username", self.config.username.as_str()),
                ("password", self.config.password.as_str()),
            ])
            .map_err(|err| describe("登录", err))?;
        let body = read_body(response).map_err(|err| describe("登录", err))?;
        if body.trim() != "Ok." {
            return Err(format!(
                "qBittorrent 登录被拒（返回 {:?}）：检查用户名/密码与 WebUI 的 IP 白名单",
                body.trim()
            ));
        }
        self.logged_in = true;
        Ok(())
    }

    fn ensure_login(&mut self) -> Result<(), String> {
        if self.logged_in {
            return Ok(());
        }
        self.login()
    }

    /// 发一个请求并把响应读成文本。403 时自动重登并重试一次。
    fn exec<F>(&mut self, label: &str, build: F) -> Result<String, String>
    where
        F: Fn(&Agent) -> Result<Response<Body>, ureq::Error>,
    {
        self.ensure_login()?;
        let agent = self.agent.clone();
        match build(&agent) {
            Ok(response) => read_body(response).map_err(|err| describe(label, err)),
            Err(ureq::Error::StatusCode(403)) => {
                // 会话过期：重登一次再试。还失败就是权限/白名单问题，交给调用方报错。
                self.logged_in = false;
                self.login()?;
                let agent = self.agent.clone();
                build(&agent)
                    .map_err(|err| describe(label, err))
                    .and_then(|response| read_body(response).map_err(|err| describe(label, err)))
            }
            Err(err) => Err(describe(label, err)),
        }
    }

    fn exec_json<F>(&mut self, label: &str, build: F) -> Result<Value, String>
    where
        F: Fn(&Agent) -> Result<Response<Body>, ureq::Error>,
    {
        let text = self.exec(label, build)?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|err| format!("{label}：响应不是 JSON（{err}）：{text}"))
    }

    fn get(&mut self, label: &str, path: &str, query: &[(&str, String)]) -> Result<Value, String> {
        let url = self.url(path);
        let query: Vec<(String, String)> = query
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect();
        self.exec_json(label, |agent| {
            let mut request = agent.get(&url);
            for (key, value) in &query {
                request = request.query(key, value);
            }
            request.call()
        })
    }

    fn post(
        &mut self,
        label: &str,
        path: &str,
        form: Vec<(&str, String)>,
    ) -> Result<Value, String> {
        let url = self.url(path);
        let referer = self.config.url.clone();
        self.exec_json(label, |agent| {
            let pairs: Vec<(&str, String)> = form.clone();
            agent
                .post(&url)
                .header("Referer", &referer)
                .send_form(pairs)
        })
    }
}

fn read_body(mut response: Response<Body>) -> Result<String, ureq::Error> {
    response.body_mut().read_to_string()
}

fn describe(what: &str, err: ureq::Error) -> String {
    match err {
        ureq::Error::StatusCode(403) => format!("{what}：HTTP 403（未登录或会话失效）"),
        ureq::Error::StatusCode(code) => format!("{what}：HTTP {code}"),
        ureq::Error::Timeout(_) => format!("{what}：超时"),
        other => format!("{what}：{other}"),
    }
}

// ─── 服务壳（薄层） ────────────────────────────────────────────────────────

fn dispatch(method: &str, input: *const c_char) -> *const c_char {
    match parse(input).and_then(|value| api::call(method, value)) {
        Ok(fields) => ok(fields),
        Err(message) => error(message),
    }
}

unsafe extern "C" fn svc_ping(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("ping", input)
}
unsafe extern "C" fn svc_login(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("login", input)
}
unsafe extern "C" fn svc_version(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("version", input)
}
unsafe extern "C" fn svc_transfer(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("transfer", input)
}
unsafe extern "C" fn svc_list(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("list", input)
}
unsafe extern "C" fn svc_files(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("files", input)
}
unsafe extern "C" fn svc_properties(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("properties", input)
}
unsafe extern "C" fn svc_add(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("add", input)
}
unsafe extern "C" fn svc_pause(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("pause", input)
}
unsafe extern "C" fn svc_resume(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("resume", input)
}
unsafe extern "C" fn svc_delete(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("delete", input)
}
unsafe extern "C" fn svc_set_category(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("set_category", input)
}

// ─── 小工具 ────────────────────────────────────────────────────────────────

/// `hashes` 既接受字符串（`a|b`），也接受字符串数组。
pub fn join_hashes(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("|"),
        _ => String::new(),
    }
}

fn bool_text(value: bool) -> String {
    if value { "true" } else { "false" }.to_owned()
}

/// qB 的写接口成功时返回空体或 `Ok.`；返回 `Fails.` 就是被拒（重复种子、路径无权限等）。
fn check_add_response(text: &str) -> Result<Value, String> {
    let trimmed = text.trim();
    if trimmed == "Fails." {
        return Err("qBittorrent 拒绝了这次添加（常见原因：重复种子、保存路径无权限）".to_owned());
    }
    Ok(json!({ "accepted": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_accept_both_shapes() {
        assert_eq!(join_hashes(&json!("a|b")), "a|b");
        assert_eq!(join_hashes(&json!(["a", "b"])), "a|b");
        assert_eq!(join_hashes(&json!(123)), "");
    }

    #[test]
    fn rejected_add_is_an_error() {
        assert!(check_add_response("Fails.").is_err());
        assert!(check_add_response("").is_ok());
        assert!(check_add_response("Ok.").is_ok());
    }

    #[test]
    fn url_join_has_no_double_slash() {
        let client = QbClient::new(Config {
            url: "http://127.0.0.1:8080".to_owned(),
            username: "a".to_owned(),
            password: "b".to_owned(),
            timeout: Duration::from_secs(1),
        });
        assert_eq!(
            client.url("/api/v2/app/version"),
            "http://127.0.0.1:8080/api/v2/app/version"
        );
    }

    #[test]
    fn bool_flags_use_the_strings_qb_expects() {
        assert_eq!(bool_text(true), "true");
        assert_eq!(bool_text(false), "false");
    }

    #[test]
    fn version_major_is_parsed_from_qb_format() {
        assert_eq!(api::major_of("v5.1.2.10"), Some(5));
        assert_eq!(api::major_of("5.0.0"), Some(5));
        assert_eq!(api::major_of("v4.6.7"), Some(4));
        assert_eq!(api::major_of("  v6.0.0-beta "), Some(6));
        assert_eq!(api::major_of("garbage"), None);
    }

    #[test]
    fn legacy_filters_map_to_5x_names() {
        // 4.x 的 paused / resumed 在 5.0 已被移除，直接透传会被 qB 当成 all。
        assert_eq!(api::normalize_filter("paused").unwrap(), "stopped");
        assert_eq!(api::normalize_filter("RESUMED").unwrap(), "running");
        assert_eq!(api::normalize_filter(" stopped ").unwrap(), "stopped");
        assert_eq!(
            api::normalize_filter("stalled_uploading").unwrap(),
            "stalled_uploading"
        );
    }

    #[test]
    fn unknown_filter_is_rejected_instead_of_matching_everything() {
        // qB 自己不会拒绝未知 filter（内部默认 All），所以必须在这里拦。
        let err = api::normalize_filter("finished").unwrap_err();
        assert!(err.contains("未知 filter"), "{err}");
        assert!(err.contains("stopped"), "错误信息应列出可用取值：{err}");
    }

    #[test]
    fn toggle_targets_5x_endpoints() {
        assert_eq!(api::toggle_path("pause"), Some("/api/v2/torrents/stop"));
        assert_eq!(api::toggle_path("resume"), Some("/api/v2/torrents/start"));
        assert_eq!(api::toggle_path("restart"), None);
    }
}
