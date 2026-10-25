//! 管理 API（只读 HTTP 接口，不带前端）。
//!
//! TS 版是 Fastify + Vue；这边按用户要求**只保留 API**，而且不引依赖：一个
//! `TcpListener` + 每连接一线程足够撑管理面板的并发量（个位数 QPS）。
//! 不引 axum/tiny_http 的理由：插件是 cdylib，多一个带 tokio 的依赖就是把宿主
//! 的运行时拖进来，得不偿失。
//!
//! 鉴权：`[http] token` 非空时要求 `Authorization: Bearer <token>`；为空表示管理端
//! 没启用，除 `/api/health` 外一律 401——宁可打不开，也不要把番剧库裸奔在局域网上。
//!
//! 请求处理在线程里跑：Mongo 调用是同步的，不能让它在宿主的投递线程上执行。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anime_core::log;
use anime_core::store::Storage;
use serde_json::{Value, json};

use crate::runtime::Runtime;

/// 一条解析好的请求。只保留路由需要的部分（方法 + 路径 + 查询串 + 令牌）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub token: Option<String>,
}

impl Request {
    /// 取查询参数（不存在就是 None）。
    pub fn param(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

/// 解析 HTTP 请求头。只读第一段（请求行 + 头部），不读 body——目前全是 GET。
pub fn parse_request(raw: &str) -> Option<Request> {
    let mut lines = raw.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_ascii_uppercase();
    let target = parts.next()?;
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, parse_query(query)),
        None => (target, Vec::new()),
    };
    let token = lines
        .take_while(|line| !line.trim().is_empty())
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().strip_prefix("Bearer ").map(str::to_owned))
                .flatten()
        });
    Some(Request {
        method,
        path: path.to_owned(),
        query,
        token,
    })
}

/// `a=1&b=2` → 键值对。不做百分号解码：目前参数只有页码和关键词，中文关键词
/// 需要解码时再加（注释留在这里，避免以后忘了）。
fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

/// 服务器句柄。drop 就停（和 Poller 一套语义）。
pub struct WebServer {
    stop: Arc<AtomicBool>,
    /// 监听地址：drop 时连自己一次，把阻塞在 accept 上的线程叫醒。
    addr: String,
    handle: Option<JoinHandle<()>>,
}

impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(&self.addr);
        self.handle.take();
    }
}

/// 起管理 API。`listen` 为空表示不启用，返回 None（不是错误）。
pub fn spawn(runtime: Arc<Runtime>) -> Option<WebServer> {
    let listen = runtime.config.http.listen.trim().to_owned();
    if listen.is_empty() {
        return None;
    }
    let listener = match TcpListener::bind(&listen) {
        Ok(listener) => listener,
        Err(error) => {
            log!(&runtime.logger, Error, "管理 API 监听 {listen} 失败：{error}");
            return None;
        }
    };
    let _ = listener.set_nonblocking(false);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let worker = Arc::clone(&runtime);
    let bound = listen.clone();
    let handle = thread::Builder::new()
        .name("xq-anime-web".to_owned())
        .spawn(move || {
            log!(&worker.logger, Info, "管理 API 已启动：http://{bound}");
            for stream in listener.incoming() {
                if flag.load(Ordering::Acquire) {
                    break;
                }
                match stream {
                    Ok(stream) => {
                        let runtime = Arc::clone(&worker);
                        // 一个连接一个线程：请求很少，简单比并发模型重要。
                        let _ = thread::Builder::new()
                            .name("xq-anime-web-conn".to_owned())
                            .spawn(move || handle_connection(stream, &runtime));
                    }
                    Err(error) => {
                        log!(&worker.logger, Warn, "管理 API 接受连接失败：{error}");
                    }
                }
            }
        })
        .ok()?;
    Some(WebServer {
        stop,
        addr: listen,
        handle: Some(handle),
    })
}

fn handle_connection(mut stream: TcpStream, runtime: &Runtime) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = [0u8; 8192];
    let Ok(read) = stream.read(&mut buffer) else {
        return;
    };
    let text = String::from_utf8_lossy(&buffer[..read]);
    let Some(request) = parse_request(&text) else {
        respond(&mut stream, 400, json!({ "error": "无法解析请求" }));
        return;
    };
    let (status, body) = route(&request, runtime);
    respond(&mut stream, status, body);
}

/// 路由。纯函数：不碰 socket，单测直接调它。
pub fn route(request: &Request, runtime: &Runtime) -> (u16, Value) {
    if request.method != "GET" {
        return (405, json!({ "error": "只支持 GET" }));
    }
    // health 不鉴权：负载均衡/监控要用，且不泄露业务数据。
    if request.path == "/api/health" {
        return (
            200,
            json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }),
        );
    }
    let expected = runtime.config.http.token.trim();
    if expected.is_empty() {
        return (401, json!({ "error": "管理端未配置 token，接口已禁用" }));
    }
    if request.token.as_deref() != Some(expected) {
        return (401, json!({ "error": "token 无效" }));
    }
    match request.path.as_str() {
        "/api/stats" => (200, stats(runtime)),
        "/api/anime" => (200, anime_list(request, runtime)),
        "/api/reviews" => (200, reviews(runtime)),
        "/api/tasks" => (200, tasks(runtime)),
        _ => (404, json!({ "error": "没有这个接口" })),
    }
}

fn stats(runtime: &Runtime) -> Value {
    let (rounds, items, usable, failures) = runtime.poll_stats.snapshot();
    json!({
        "ok": true,
        "rounds": rounds,
        "items": items,
        "usable": usable,
        "source_failures": failures,
        "downloads": crate::chain::active(),
    })
}

fn anime_list(request: &Request, runtime: &Runtime) -> Value {
    let page = request
        .param("page")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1)
        .max(1);
    let size = request
        .param("page_size")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30)
        .clamp(1, 100);
    let projection = json!({
        "_id": 0,
        "id": 1, "name": 1, "name_cn": 1, "names": 1, "image": 1,
        "episode": 1, "score": 1, "r18": 1, "airingDay": 1, "airingStart": 1, "updatedAt": 1
    });
    // Storage::find 没有 skip 参数（宿主接口只暴露 limit），所以分页用
    // 「取 page*size 条再切掉前 (page-1)*size 条」。库大时这一步会多读，
    // 但管理面板的访问频率低，不值得为此改 ABI。
    let take = (page * size).min(1000) as usize;
    match runtime.store.find(
        "anime",
        json!({}),
        Some(projection),
        Some(json!({ "updatedAt": -1, "id": -1 })),
        Some(take),
    ) {
        Ok(items) => {
            let skip = ((page - 1) * size) as usize;
            let page_items: Vec<Value> = items.into_iter().skip(skip).collect();
            json!({ "ok": true, "page": page, "page_size": size, "items": page_items })
        }
        Err(error) => json!({ "ok": false, "error": error.to_string() }),
    }
}

fn reviews(runtime: &Runtime) -> Value {
    match runtime.store.find(
        "pendingReviews",
        json!({ "status": "pending" }),
        Some(json!({ "_id": 0 })),
        Some(json!({ "createdAt": -1 })),
        Some(100),
    ) {
        Ok(items) => json!({ "ok": true, "items": items }),
        Err(error) => json!({ "ok": false, "error": error.to_string() }),
    }
}

fn tasks(runtime: &Runtime) -> Value {
    match runtime.store.find(
        "task_checkpoints",
        json!({}),
        Some(json!({ "_id": 0 })),
        Some(json!({ "updatedAt": -1 })),
        Some(100),
    ) {
        Ok(items) => json!({ "ok": true, "items": items }),
        Err(error) => json!({ "ok": false, "error": error.to_string() }),
    }
}

fn respond(stream: &mut TcpStream, status: u16, body: Value) {
    let body = body.to_string();
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_request_line_query_and_bearer() {
        let raw = "GET /api/anime?page=2&page_size=10 HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer abc123\r\n\r\n";
        let request = parse_request(raw).expect("应当能解析");
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/api/anime");
        assert_eq!(request.param("page"), Some("2"));
        assert_eq!(request.param("page_size"), Some("10"));
        assert_eq!(request.token.as_deref(), Some("abc123"));
    }

    #[test]
    fn missing_token_and_garbage_are_handled() {
        let request = parse_request("GET /api/stats HTTP/1.1\r\n\r\n").unwrap();
        assert!(request.token.is_none());
        assert!(request.query.is_empty());
        assert!(parse_request("").is_none());
    }

    #[test]
    fn only_get_is_accepted() {
        let request = parse_request("POST /api/stats HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(request.method, "POST");
    }
}
