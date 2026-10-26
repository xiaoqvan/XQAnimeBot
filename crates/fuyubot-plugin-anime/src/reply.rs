//! 等管理员的下一条消息：多轮纠正（`N_anime` / `N_ep` / `F_anime`）的底座。
//!
//! TS 里这件事是一行 `for await (const update of client.iterUpdates())`——
//! 客户端本身是一条更新流，想等多久等多久。Rust 这边没有那条流，只能自己搭：
//!
//! ```text
//! 订阅 updateNewMessage ──► 全局注册表（chat_id, 被回复的消息 id）──► mpsc::Sender
//!      ▲                                                                    │
//!      └──────────────────────── 一次订阅，多处等待 ─────────────────────────┘
//! ```
//!
//! ## 三个有意的设计取舍
//!
//! - **一次订阅、一直活着**：订阅是进程级的（[`ensure`]），不是每次等待都订阅/退订。
//!   反复订阅会让宿主的事件表抖动，而且第一次订阅和第一次等待之间有丢更新的窗口。
//!   代价是插件活着期间一直有一条订阅——它只是查个 HashMap，可以忽略。
//! - **有超时**：TS 的 `iterUpdates` 没有超时，管理员忘了回就永远挂着一条线程。
//!   这里默认等 30 分钟，超时后提示「已超时」，下次点按钮重新来一次即可。
//! - **只看「回复了提示消息的文本」**：与 TS 一致（它匹配 `reply_to.message_id`）。
//!   不看「同一会话里的任意消息」——管理群里同时可能有别的对话，认错消息就等于认错答案。
//!
//! ## 宿主侧前提
//!
//! `host.on("updateNewMessage", ...)` 必须真实投递消息更新；宿主没接事件总线时
//! [ensure] 返回 false，等待会直接失败（调用方应当提示管理员改用命令，而不是静默卡住）。

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use fuyubot_abi::{HostApi, SubscriptionHandle};
use serde_json::Value;

/// 等待一条回复的默认上限。
///
/// 管理员不是机器人，点完按钮去查 bgm 可能要几分钟；30 分钟足够，又不会让线程
/// 挂到天荒地老（TS 那个「永不超时」在无人值守时就是线程泄漏）。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// 消息更新事件名。
pub const MESSAGE_EVENT: &str = "updateNewMessage";

/// 一次等待的钥匙：**哪个会话的哪条消息**。
type Key = (i64, i64);

/// 正在等待的收件箱。钥匙 → 投递通道。
static WAITERS: LazyLock<Mutex<HashMap<Key, Sender<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 进程级的订阅守卫。只在第一次 [`ensure`] 时写入，之后一直活到插件卸载。
static SUBSCRIPTION: OnceLock<MessageSubscription> = OnceLock::new();

/// 订阅消息更新（只做一次）。返回 false = 宿主没接事件总线，等待不可能成功。
pub fn ensure(host: HostApi) -> bool {
    if SUBSCRIPTION.get().is_some() {
        return true;
    }
    let Some(subscription) = MessageSubscription::spawn(host) else {
        return false;
    };
    // 并发下可能有人先写进去：那也没关系，后写的这个会被 drop（= 退订），
    // 用 `get_or_init` 的返回值判断是否真的装上了。
    let installed = SUBSCRIPTION.get_or_init(|| subscription);
    installed.handle.is_valid()
}

/// 等一条回复提示消息的**文本**。超时或没订阅上返回 `None`。
///
/// 只收文本：管理员发图/发表情时这条等待继续，不该被无关消息打断
/// （TS 也是 `message.content._ === "messageText"` 才处理）。
pub fn wait(chat_id: i64, message_id: i64, timeout: Duration) -> Option<String> {
    let receiver = register(chat_id, message_id)?;
    let text = match receiver.recv_timeout(timeout) {
        Ok(text) => Some(text),
        Err(RecvTimeoutError::Timeout) => None,
        // 发送端被 drop（理论上不会：订阅活着就没人动它），按超时处理。
        Err(RecvTimeoutError::Disconnected) => None,
    };
    unregister(chat_id, message_id);
    text
}

fn register(chat_id: i64, message_id: i64) -> Option<Receiver<String>> {
    let (sender, receiver) = channel();
    let mut waiters = lock();
    waiters.insert((chat_id, message_id), sender);
    Some(receiver)
}

/// 取回等待并注销。注销必须在超时之后也执行，否则这个 key 会永远留在表里
/// （管理员下次点同一个按钮时，旧通道会把新回复吞掉）。
fn unregister(chat_id: i64, message_id: i64) {
    lock().remove(&(chat_id, message_id));
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<Key, Sender<String>>> {
    WAITERS.lock().unwrap_or_else(|error| error.into_inner())
}

/// 一条消息更新里我们关心的三个字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    /// 所在会话。
    pub chat_id: i64,
    /// 消息自己的 id（用于判断是否被回复）。
    pub message_id: i64,
    /// 回复的是哪条消息（0 = 不是回复）。
    pub reply_to: i64,
    /// 文本内容；非文本消息为 `None`。
    pub text: Option<String>,
}

/// 从 `updateNewMessage` 里取出 [IncomingMessage]。
///
/// 字段路径照真 `td_api.tl`，但**写得宽容**：宿主/回放给的 JSON 可能没有 `@type`，
/// `reply_to` 也可能是扁平的 `reply_to_message_id`（早期回放数据的形状）。
/// 取不到 chat_id / message_id 就返回 `None`——那不是消息更新。
pub fn message_of(update: &Value) -> Option<IncomingMessage> {
    let message = update.get("message").unwrap_or(update);
    let chat_id = message.get("chat_id").and_then(Value::as_i64)?;
    let message_id = message.get("id").and_then(Value::as_i64)?;

    let reply_to = message
        .get("reply_to")
        .and_then(|reply| reply.get("message_id").and_then(Value::as_i64))
        .or_else(|| message.get("reply_to_message_id").and_then(Value::as_i64))
        .unwrap_or(0);

    let text = message
        .get("content")
        .and_then(|content| {
            content
                .get("text")
                .and_then(|text| text.get("text").or(Some(text)))
        })
        .or_else(|| message.get("text"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);

    Some(IncomingMessage {
        chat_id,
        message_id,
        reply_to,
        text,
    })
}

/// 把一条更新交给正在等待的人。没人等就返回 false（正常情况：绝大多数消息
/// 都不是回复我们的提示）。
fn deliver(message: &IncomingMessage) -> bool {
    let Some(text) = message.text.as_deref() else {
        return false;
    };
    let key = (message.chat_id, message.reply_to);
    let sender = lock().get(&key).cloned();
    match sender {
        Some(sender) => sender.send(text.to_owned()).is_ok(),
        None => false,
    }
}

/// `updateNewMessage` 的订阅：`drop` 即退订（照抄 `CallbackSubscription`）。
struct MessageSubscription {
    host: HostApi,
    handle: SubscriptionHandle,
    /// 只负责**持有**回调；递给 ABI 的 `user_data` 指的就是它，退订之前地址必须稳定。
    _handler: Box<Box<dyn Fn(&IncomingMessage) + Send + Sync>>,
}

impl MessageSubscription {
    fn spawn(host: HostApi) -> Option<Self> {
        let mut owned: Box<Box<dyn Fn(&IncomingMessage) + Send + Sync>> =
            Box::new(Box::new(move |message: &IncomingMessage| {
                deliver(message);
            }));
        let user_data = (&raw mut *owned).cast::<c_void>();
        let handle = host.on(MESSAGE_EVENT, None, trampoline, user_data);
        handle.is_valid().then_some(Self {
            host,
            handle,
            _handler: owned,
        })
    }
}

impl Drop for MessageSubscription {
    fn drop(&mut self) {
        if self.handle.is_valid() {
            self.host.off(self.handle);
        }
    }
}

unsafe extern "C-unwind" fn trampoline(user_data: *mut c_void, payload: *const c_char) -> i32 {
    if user_data.is_null() || payload.is_null() {
        return -1;
    }
    // SAFETY: `user_data` 是 `spawn` 里那个 Box，订阅活多久它就活多久。
    let handler = unsafe { &*(user_data as *const Box<dyn Fn(&IncomingMessage) + Send + Sync>) };
    // SAFETY: 宿主保证 `payload` 是 NUL 结尾的临时字符串。
    let text = unsafe { CStr::from_ptr(payload) }.to_string_lossy();
    let Ok(update) = serde_json::from_str::<Value>(&text) else {
        return 0;
    };
    if let Some(message) = message_of(&update) {
        handler(&message);
    }
    0
}

/// 管理员回复里「指的是哪个条目 / 哪一集」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reference {
    /// Bangumi 章节 id（`bgm.tv/ep/<id>`）。
    Episode(u64),
    /// Bangumi 条目 id（`bgm.tv/subject/<id>`）。
    Subject(u64),
}

/// 解析管理员的回复：纯数字 → 章节 id；`bgm.tv/ep/…` → 章节；`bgm.tv/subject/…` → 条目。
///
/// 纯数字按**章节**处理是与 TS 一致的（`getEpisodeById(parsedId)`）——管理员纠正时
/// 最常给的就是「这一集是第几集」。想给条目请用链接形式或 `/addanime`。
///
/// 返回 `None` 只表示「这句话不像 id」，**不包括** `/cancel`：那是命令不是编号，
/// 由调用方先判。
pub fn parse_reference(text: &str) -> Option<Reference> {
    let text = text.trim();
    if let Some(id) = extract_after(text, "/ep/") {
        return Some(Reference::Episode(id));
    }
    if let Some(id) = extract_after(text, "/subject/") {
        return Some(Reference::Subject(id));
    }
    // 纯数字：与 TS 一致，按章节 id 处理。
    text.parse::<u64>().ok().map(Reference::Episode)
}

/// 在一段文本里找 `marker` 后面紧跟的数字（允许 `/`、`?`、空格、行尾收尾）。
///
/// 认 `bangumi.tv` 和 `bgm.tv` 两个域名：老链接域名还是前者。
fn extract_after(text: &str, marker: &str) -> Option<u64> {
    let rest = text.split_once(marker)?.1;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(chat_id: i64, id: i64, reply_to: i64, text: Option<&str>) -> Value {
        let text = match text {
            Some(text) => serde_json::json!({ "@type": "messageText", "text": { "text": text } }),
            None => serde_json::json!({ "@type": "messagePhoto" }),
        };
        serde_json::json!({
            "@type": "updateNewMessage",
            "message": {
                "chat_id": chat_id,
                "id": id,
                "reply_to": { "@type": "messageReplyToMessage", "message_id": reply_to },
                "content": text,
            }
        })
    }

    #[test]
    fn parses_a_reply_to_our_prompt() {
        let message = message_of(&update(-100, 555, 42, Some("502272"))).expect("应当解析出来");
        assert_eq!(message.chat_id, -100);
        assert_eq!(message.message_id, 555);
        assert_eq!(message.reply_to, 42);
        assert_eq!(message.text.as_deref(), Some("502272"));
    }

    #[test]
    fn non_text_messages_have_no_text() {
        // 发张图不算回答：等待继续，不能被无关消息打断。
        let message = message_of(&update(-100, 555, 42, None)).expect("消息本身要能解析");
        assert_eq!(message.text, None);
    }

    #[test]
    fn missing_chat_id_is_not_a_message_update() {
        assert!(message_of(&serde_json::json!({ "message": { "id": 1 } })).is_none());
    }

    #[test]
    fn bare_digits_mean_an_episode_like_ts() {
        assert_eq!(parse_reference("502272"), Some(Reference::Episode(502272)));
        assert_eq!(parse_reference("  502272 \n"), Some(Reference::Episode(502272)));
    }

    #[test]
    fn links_are_read_from_both_domains() {
        assert_eq!(
            parse_reference("https://bgm.tv/ep/1227091"),
            Some(Reference::Episode(1227091))
        );
        assert_eq!(
            parse_reference("https://bangumi.tv/subject/400602/"),
            Some(Reference::Subject(400602))
        );
    }

    #[test]
    fn unparsable_text_is_none() {
        // 「不是编号」和「/cancel」都不是编号；/cancel 由调用方先判。
        assert_eq!(parse_reference("这不是 id"), None);
        assert_eq!(parse_reference("/cancel"), None);
        assert_eq!(parse_reference(""), None);
    }

    #[test]
    fn a_registered_waiter_receives_the_reply() {
        // 直接测 deliver 这段：订阅需要真宿主，这里只验证「钥匙匹配 + 投递」。
        let receiver = register(-100, 42).expect("应当注册上");
        let message = message_of(&update(-100, 555, 42, Some("12345"))).expect("解析");
        assert!(deliver(&message));
        assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).ok().as_deref(), Some("12345"));
        // 回复错了消息（不是回复 42）就投递不到。
        unregister(-100, 42);
        let other = message_of(&update(-100, 556, 99, Some("x"))).expect("解析");
        assert!(!deliver(&other));
    }

    #[test]
    fn waiting_times_out_instead_of_hanging_forever() {
        let receiver = register(-100, 7).expect("应当注册上");
        let outcome = receiver.recv_timeout(Duration::from_millis(10));
        assert!(matches!(outcome, Err(RecvTimeoutError::Timeout)));
        unregister(-100, 7);
    }
}
