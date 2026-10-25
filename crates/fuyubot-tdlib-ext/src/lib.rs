//! 宿主 SDK（`fuyubot-simplify`）没覆盖的 TDLib **读操作**封装，供动漫插件直接调用。
//!
//! `fuyubot-simplify` 只做「发消息 / 编辑 / 删除」这类写操作；`getMessage`、`getChat`、
//! `getChatMember` 这些读操作在插件里只能自己拼 JSON、自己等响应。本 crate 把动漫插件
//! 需要的那一小撮补上：一个方法一个函数，参数构造器单独拆成 `*_params`（**不碰宿主**，
//! 可以脱离 TDLib 单测），每个方法都写清对应的 TDLib 请求与返回字段。
//!
//! ## 用起来长这样
//!
//! ```ignore
//! use fuyubot_tdlib_ext::TdExt;
//!
//! let td = TdExt::new(host);
//! let message = td.get_message(-1001234567890, 42)?;
//! let link = td.get_message_link(-1001234567890, 42, Some(90))?;
//! let me = td.get_me()?;
//! ```
//!
//! ## 一次调用到底发生了什么
//!
//! 1. `*_params` 把参数拼成 TDLib 的请求 JSON（`@type` + 字段）；
//! 2. `HostApi::td_invoke` 投递请求，拿到 `RequestId`；
//! 3. 在 response 通道上按 `request_id` 等响应（`HostApi::wait_for_update`），
//!    默认上限 `DEFAULT_TIMEOUT_MS`；
//! 4. 响应 `@type == "error"` 折成错误，其余原样返回。
//!
//! 第 3 步不是可选项：**`td_invoke` 只返回请求号**，TDLib 的响应走另一条通道
//! （`RESPONSE_EVENT`，只发给发起请求的那个插件），不显式去等就拿不到值。
//! 代价是这些方法**会阻塞**当前线程，直到响应到达或超时——**不要在更新/命令回调里
//! 同步调用**：回调通常跑在宿主的投递线程上，阻塞它最坏会让每个请求都等满超时。
//! 回调里只想「发出去、不等结果」时，用 `*_params` + `HostApi::td_invoke` 自己投递，
//! 再订阅响应通道处理。
//!
//! ## 字段版本
//!
//! 参数按宿主当前使用的 TDLib 1.8.66 / 1.8.68 的 `td_api.tl` 构造：`getMessageLink` 的
//! `checklist_task_id` / `poll_option_id` / `for_album` / `in_message_thread`、
//! `pinChatMessage` 的 `only_for_self`、`answerInlineQuery` 的 `button` 都是必填
//! （或可空）字段，少一个 TDLib 会直接报解析错误，所以这里一律显式写全。
//!
//! ## 错误
//!
//! - `TdExtError::Host`：ABI 调用失败，或等响应超时（链路问题，不是业务问题）；
//! - `TdExtError::Payload`：响应不是合法 JSON、TDLib 回了 `error`、或响应里缺需要的字段。

use std::ffi::CStr;

use fuyubot_abi::{FILTER_EVENT_TYPE_KEY, HostApi, RESPONSE_EVENT, WaitHandle};
use serde_json::{Map, Value, json};
use thiserror::Error;

/// 等 TDLib 响应的默认上限（毫秒）。
///
/// 取 30 秒：正常请求几十毫秒就回来，这个值只用来兜住「TDLib 没起来 / 响应丢了」，
/// 不至于让调用方永久卡死。要更短/更长用 `TdExt::invoke_with_timeout`。
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// 封装层的错误。
#[derive(Debug, Error)]
pub enum TdExtError {
    /// 宿主调用本身失败：ABI 报错，或等响应超时。
    #[error("宿主调用失败：{0}")]
    Host(String),
    /// 响应拿到了但不可用：不是合法 JSON、TDLib 返回 `error`、或缺少需要的字段。
    #[error("响应不可用：{0}")]
    Payload(String),
}

/// TDLib 扩展方法门面。
///
/// 只持有一个宿主句柄，所有方法都是 `&self`。`HostApi` 本身是 `Copy` 的，所以这个
/// 结构也是 `Copy`——和 `fuyubot_simplify::Simplify` 一起塞进插件运行时状态几乎没有成本。
#[derive(Debug, Clone, Copy)]
pub struct TdExt {
    host: HostApi,
}

impl TdExt {
    /// 包一个宿主句柄。
    pub fn new(host: HostApi) -> Self {
        Self { host }
    }

    /// 投递一个 TDLib 请求并等它的响应（超时用 `DEFAULT_TIMEOUT_MS`）。
    ///
    /// 参数就是完整的请求 JSON（含 `@type`），通常来自本 crate 的 `*_params`。
    /// 成功时返回 TDLib 的响应 JSON；TDLib 返回 `error` 时是 `TdExtError::Payload`。
    pub fn invoke(&self, params: &Value) -> Result<Value, TdExtError> {
        self.invoke_with_timeout(params, DEFAULT_TIMEOUT_MS)
    }

    /// 同 `invoke`，但自己定等响应的上限（毫秒；`0` = 只看当前队列里有没有）。
    ///
    /// 超时返回 `TdExtError::Host`——这是「宿主/链路没给响应」，不是 TDLib 的业务错误。
    pub fn invoke_with_timeout(
        &self,
        params: &Value,
        timeout_ms: u64,
    ) -> Result<Value, TdExtError> {
        let method = params
            .get("@type")
            .and_then(Value::as_str)
            .unwrap_or("<未知方法>");
        let request = self
            .host
            .td_invoke(&params.to_string())
            .map_err(|error| TdExtError::Host(format!("发起 {method} 失败：{error}")))?;

        // 响应通道按 request_id 过滤：宿主只会把这条响应发给发起它的插件。
        let mut filter = Map::new();
        filter.insert(
            FILTER_EVENT_TYPE_KEY.to_owned(),
            Value::String(RESPONSE_EVENT.to_owned()),
        );
        filter.insert("request_id".to_owned(), json!(request.id));
        let handle = self
            .host
            .wait_for_update(&Value::Object(filter).to_string(), timeout_ms);

        let Some(payload) = wait_payload(handle) else {
            return Err(TdExtError::Host(format!(
                "等 {method} 的响应超时（{timeout_ms} ms，请求 {request}）"
            )));
        };
        let response: Value = serde_json::from_str(&payload)
            .map_err(|error| TdExtError::Payload(format!("{method} 的响应不是合法 JSON：{error}")))?;
        if response.get("@type").and_then(Value::as_str) == Some("error") {
            return Err(TdExtError::Payload(tdlib_error(method, &response)));
        }
        Ok(response)
    }

    /// `getMessage`：按会话 + 消息 id 取一条消息。
    ///
    /// 对应 TDLib 请求 `getMessage chat_id:int53 message_id:int53 = Message`。
    /// 返回 `Message`：`id` / `chat_id` / `date` / `content`（`messageText` /
    /// `messagePhoto` …）/ `sender_id` / `reply_to` / `edit_date` 等。
    /// 消息不存在时 TDLib 返回 `error`（400 Message not found），这里折成
    /// `TdExtError::Payload`。
    pub fn get_message(&self, chat_id: i64, message_id: i64) -> Result<Value, TdExtError> {
        self.invoke(&get_message_params(chat_id, message_id))
    }

    /// `getChat`：取会话的基本信息。
    ///
    /// 对应 `getChat chat_id:int53 = Chat`。返回 `Chat`：`id` / `title` / `type`
    /// （`chatTypePrivate` / `chatTypeSupergroup` / `chatTypeBasicGroup` /
    /// `chatTypeSecret`）/ `permissions` / `last_message` / `positions` 等。
    pub fn get_chat(&self, chat_id: i64) -> Result<Value, TdExtError> {
        self.invoke(&get_chat_params(chat_id))
    }

    /// `getSupergroup`：取超级群组的基本信息。
    ///
    /// 对应 `getSupergroup supergroup_id:int53 = Supergroup`。返回 `Supergroup`：
    /// `id` / `username` / `status` / `member_count` / `is_channel` / `is_forum` 等。
    pub fn get_supergroup(&self, supergroup_id: i64) -> Result<Value, TdExtError> {
        self.invoke(&get_supergroup_params(supergroup_id))
    }

    /// `getSupergroupFullInfo`：取超级群组的完整信息（含成员数、简介、置顶消息）。
    ///
    /// 对应 `getSupergroupFullInfo supergroup_id:int53 = SupergroupFullInfo`。
    /// 返回 `description` / `member_count` / `administrator_count` / `restricted_count` /
    /// `can_get_members` / `is_all_history_available` / `pinned_message_id` 等。
    pub fn get_supergroup_full_info(&self, supergroup_id: i64) -> Result<Value, TdExtError> {
        self.invoke(&get_supergroup_full_info_params(supergroup_id))
    }

    /// `getChatMember`：查某个**用户**在会话里的成员信息。
    ///
    /// 对应 `getChatMember chat_id:int53 member_id:MessageSender = ChatMember`。
    /// 本方法固定用 `messageSenderUser`；返回 `ChatMember`：`member_id` / `status`
    /// （`chatMemberStatusCreator` / `chatMemberStatusAdministrator` /
    /// `chatMemberStatusMember` / `chatMemberStatusRestricted` /
    /// `chatMemberStatusLeft` / `chatMemberStatusBanned`）等。
    ///
    /// 以频道身份出现的成员（`messageSenderChat`）不在本方法的签名里：那是另一种请求体，
    /// 需要时用 `get_chat_member_params` 的写法自己拼 `td_invoke`。
    pub fn get_chat_member(&self, chat_id: i64, user_id: i64) -> Result<Value, TdExtError> {
        self.invoke(&get_chat_member_params(chat_id, user_id))
    }

    /// `getUser`：取用户的基本信息。
    ///
    /// 对应 `getUser user_id:int53 = User`。返回 `User`：`id` / `first_name` /
    /// `last_name` / `usernames` / `phone_number` / `status` / `is_bot` / `is_premium` 等。
    pub fn get_user(&self, user_id: i64) -> Result<Value, TdExtError> {
        self.invoke(&get_user_params(user_id))
    }

    /// `getMessageLink`：要一条消息的 `https://t.me/...` 链接，**只返回 link 字符串**。
    ///
    /// 对应 TDLib 的 `getMessageLink chat_id:int53 message_id:int53
    /// media_timestamp:int32 checklist_task_id:int32 poll_option_id:string
    /// for_album:Bool in_message_thread:Bool = MessageLink`（离线方法，不联网）。
    /// 响应是 `MessageLink { link:string, is_public:Bool }`，这里取其中的 `link`；
    /// `media_timestamp` 给 `Some(n)` 时生成「从第 n 秒开始播放」的媒体链接。
    ///
    /// 响应里没有 `link` 时返回 `TdExtError::Payload`。
    pub fn get_message_link(
        &self,
        chat_id: i64,
        message_id: i64,
        media_timestamp: Option<i32>,
    ) -> Result<Value, TdExtError> {
        let response = self.invoke(&get_message_link_params(
            chat_id,
            message_id,
            media_timestamp,
        ))?;
        message_link(&response)
    }

    /// `answerInlineQuery`：机器人回一条内联查询的结果。
    ///
    /// 对应 `answerInlineQuery inline_query_id:int64 is_personal:Bool
    /// button:inlineQueryResultsButton results:vector<InputInlineQueryResult>
    /// cache_time:int32 next_offset:string = Ok`。
    /// `results` 必须是 `inputInlineQueryResult*` 数组（**input** 形态，不是查询里收到的
    /// `inlineQueryResult*`）；`next_offset` 为 `None` 时写空串（没有下一页）。
    /// 返回 `{"@type":"ok"}`。
    pub fn answer_inline_query(
        &self,
        inline_query_id: i64,
        results: Value,
        cache_time: i32,
        is_personal: bool,
        next_offset: Option<String>,
    ) -> Result<Value, TdExtError> {
        self.invoke(&answer_inline_query_params(
            inline_query_id,
            results,
            cache_time,
            is_personal,
            next_offset,
        ))
    }

    /// `pinChatMessage`：把一条消息置顶。
    ///
    /// 对应 `pinChatMessage chat_id:int53 message_id:int53 disable_notification:Bool
    /// only_for_self:Bool = Ok`。返回 `{"@type":"ok"}`。
    /// 本方法固定「通知大家 + 对所有人可见」（两个标志都是 `false`）。
    pub fn pin_chat_message(&self, chat_id: i64, message_id: i64) -> Result<Value, TdExtError> {
        self.invoke(&pin_chat_message_params(chat_id, message_id))
    }

    /// `getMe`：当前登录账号自己的 `User`。
    ///
    /// 对应 `getMe = User`，无参数。返回字段同 `get_user`。
    pub fn get_me(&self) -> Result<Value, TdExtError> {
        self.invoke(&get_me_params())
    }
}

// ── 参数构造器（纯函数，不碰宿主，可单测） ──────────────────────────

/// `getMessage` 的请求体。
///
/// 对应 `getMessage chat_id:int53 message_id:int53 = Message`。
pub fn get_message_params(chat_id: i64, message_id: i64) -> Value {
    json!({
        "@type": "getMessage",
        "chat_id": chat_id,
        "message_id": message_id,
    })
}

/// `getChat` 的请求体。
///
/// 对应 `getChat chat_id:int53 = Chat`。
pub fn get_chat_params(chat_id: i64) -> Value {
    json!({
        "@type": "getChat",
        "chat_id": chat_id,
    })
}

/// `getSupergroup` 的请求体。
///
/// 对应 `getSupergroup supergroup_id:int53 = Supergroup`。
pub fn get_supergroup_params(supergroup_id: i64) -> Value {
    json!({
        "@type": "getSupergroup",
        "supergroup_id": supergroup_id,
    })
}

/// `getSupergroupFullInfo` 的请求体。
///
/// 对应 `getSupergroupFullInfo supergroup_id:int53 = SupergroupFullInfo`。
pub fn get_supergroup_full_info_params(supergroup_id: i64) -> Value {
    json!({
        "@type": "getSupergroupFullInfo",
        "supergroup_id": supergroup_id,
    })
}

/// `getChatMember` 的请求体（成员固定为 `messageSenderUser`）。
///
/// 对应 `getChatMember chat_id:int53 member_id:MessageSender = ChatMember`；
/// TDLib 的 `member_id` 是个带 `@type` 的 `MessageSender`，不能直接传数字。
pub fn get_chat_member_params(chat_id: i64, user_id: i64) -> Value {
    json!({
        "@type": "getChatMember",
        "chat_id": chat_id,
        "member_id": {
            "@type": "messageSenderUser",
            "user_id": user_id,
        },
    })
}

/// `getUser` 的请求体。
///
/// 对应 `getUser user_id:int53 = User`。
pub fn get_user_params(user_id: i64) -> Value {
    json!({
        "@type": "getUser",
        "user_id": user_id,
    })
}

/// `getMessageLink` 的请求体。
///
/// 对应 TDLib 1.8.66/1.8.68 的 `getMessageLink chat_id:int53 message_id:int53
/// media_timestamp:int32 checklist_task_id:int32 poll_option_id:string
/// for_album:Bool in_message_thread:Bool = MessageLink`。
///
/// 本 crate 只暴露常用的 `media_timestamp`（`None` → `0`）；其余字段固定成「不针对
/// 清单任务 / 投票选项，不取整个相册，不取话题评论」的默认值。要改这些标志，直接构造
/// 自己的请求体交给 `TdExt::invoke`。
pub fn get_message_link_params(
    chat_id: i64,
    message_id: i64,
    media_timestamp: Option<i32>,
) -> Value {
    json!({
        "@type": "getMessageLink",
        "chat_id": chat_id,
        "message_id": message_id,
        // None 表示从 0 秒开始（TDLib 用 0 表示「没有时间戳」）。
        "media_timestamp": media_timestamp.unwrap_or(0),
        "checklist_task_id": 0,
        "poll_option_id": "",
        "for_album": false,
        "in_message_thread": false,
    })
}

/// `answerInlineQuery` 的请求体。
///
/// 对应 `answerInlineQuery inline_query_id:int64 is_personal:Bool
/// button:inlineQueryResultsButton results:vector<InputInlineQueryResult>
/// cache_time:int32 next_offset:string = Ok`。
///
/// `button` 传 `null`：`td_api.tl` 里没有「无按钮」的构造器，文档写的是
/// 「pass null if none」。`next_offset` 为 `None` 时写空串。
pub fn answer_inline_query_params(
    inline_query_id: i64,
    results: Value,
    cache_time: i32,
    is_personal: bool,
    next_offset: Option<String>,
) -> Value {
    json!({
        "@type": "answerInlineQuery",
        "inline_query_id": inline_query_id,
        "is_personal": is_personal,
        "button": Value::Null,
        "results": results,
        "cache_time": cache_time,
        "next_offset": next_offset.unwrap_or_default(),
    })
}

/// `pinChatMessage` 的请求体。
///
/// 对应 `pinChatMessage chat_id:int53 message_id:int53 disable_notification:Bool
/// only_for_self:Bool = Ok`。两个布尔字段在 1.8.66/1.8.68 里都是必填，这里取
/// 「发通知、对所有人可见」。
pub fn pin_chat_message_params(chat_id: i64, message_id: i64) -> Value {
    json!({
        "@type": "pinChatMessage",
        "chat_id": chat_id,
        "message_id": message_id,
        "disable_notification": false,
        "only_for_self": false,
    })
}

/// `getMe` 的请求体（无参数）。
///
/// 对应 `getMe = User`。
pub fn get_me_params() -> Value {
    json!({ "@type": "getMe" })
}

// ── 内部工具 ────────────────────────────────────────────────────────

/// 把 `WaitHandle` 里的响应 JSON 拷成 `String`（宿主缓冲只在下一次宿主调用前有效）。
///
/// 没匹配到（超时/取消）或指针为空时返回 `None`。
fn wait_payload(handle: WaitHandle) -> Option<String> {
    if !handle.did_match() || handle.payload_json.is_null() {
        return None;
    }
    // SAFETY: 宿主保证 payload_json 是 NUL 结尾的合法 UTF-8 指针，且在下一次宿主调用
    // 之前有效；这里当场拷成 String，不把指针带出去。
    let payload = unsafe { CStr::from_ptr(handle.payload_json) };
    Some(payload.to_string_lossy().into_owned())
}

/// 从 `MessageLink` 响应里抽出 `link`（这是 `get_message_link` 唯一要的字段）。
fn message_link(response: &Value) -> Result<Value, TdExtError> {
    response
        .get("link")
        .and_then(Value::as_str)
        .map(|link| Value::String(link.to_owned()))
        .ok_or_else(|| {
            TdExtError::Payload(format!(
                "getMessageLink 的响应里没有字符串 link：{response}"
            ))
        })
}

/// 把 TDLib 的 `{"@type":"error","code":…,"message":…}` 折成一句人能读的原因。
fn tdlib_error(method: &str, response: &Value) -> String {
    let code = response.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("TDLib 没给出原因");
    format!("{method} 返回 TDLib 错误 {code}：{message}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn get_message_params_carry_both_ids() {
        let params = get_message_params(-1001234567890, 42);
        assert_eq!(params["@type"], json!("getMessage"));
        assert_eq!(params["chat_id"], json!(-1001234567890_i64));
        assert_eq!(params["message_id"], json!(42));
    }

    #[test]
    fn get_chat_params_target_the_chat() {
        let params = get_chat_params(-1009876543210);
        assert_eq!(params["@type"], json!("getChat"));
        assert_eq!(params["chat_id"], json!(-1009876543210_i64));
    }

    #[test]
    fn get_supergroup_params_use_supergroup_id() {
        let params = get_supergroup_params(77);
        assert_eq!(params["@type"], json!("getSupergroup"));
        assert_eq!(params["supergroup_id"], json!(77));
    }

    #[test]
    fn get_supergroup_full_info_params_use_supergroup_id() {
        let params = get_supergroup_full_info_params(88);
        assert_eq!(params["@type"], json!("getSupergroupFullInfo"));
        assert_eq!(params["supergroup_id"], json!(88));
    }

    #[test]
    fn get_chat_member_params_wrap_the_user_as_message_sender() {
        let params = get_chat_member_params(-100, 123456);
        assert_eq!(params["@type"], json!("getChatMember"));
        assert_eq!(params["chat_id"], json!(-100));
        assert_eq!(params["member_id"]["@type"], json!("messageSenderUser"));
        assert_eq!(params["member_id"]["user_id"], json!(123456));
    }

    #[test]
    fn get_user_params_use_user_id() {
        let params = get_user_params(42);
        assert_eq!(params["@type"], json!("getUser"));
        assert_eq!(params["user_id"], json!(42));
    }

    #[test]
    fn get_message_link_params_fill_every_required_field() {
        let params = get_message_link_params(-100, 7, None);
        assert_eq!(params["@type"], json!("getMessageLink"));
        assert_eq!(params["chat_id"], json!(-100));
        assert_eq!(params["message_id"], json!(7));
        // None 就是「没有时间戳」，TDLib 用 0 表示。
        assert_eq!(params["media_timestamp"], json!(0));
        assert_eq!(params["checklist_task_id"], json!(0));
        assert_eq!(params["poll_option_id"], json!(""));
        assert_eq!(params["for_album"], json!(false));
        assert_eq!(params["in_message_thread"], json!(false));
    }

    #[test]
    fn get_message_link_params_keep_media_timestamp() {
        let params = get_message_link_params(1, 2, Some(90));
        assert_eq!(params["media_timestamp"], json!(90));
    }

    #[test]
    fn answer_inline_query_params_default_to_empty_next_offset() {
        let results = json!([{ "@type": "inputInlineQueryResultArticle", "id": "a" }]);
        let params = answer_inline_query_params(999, results.clone(), 300, true, None);
        assert_eq!(params["@type"], json!("answerInlineQuery"));
        assert_eq!(params["inline_query_id"], json!(999));
        assert_eq!(params["is_personal"], json!(true));
        assert_eq!(params["cache_time"], json!(300));
        assert_eq!(params["results"], results);
        assert_eq!(params["next_offset"], json!(""));
        // 没有按钮：td_api.tl 没给「无按钮」构造器，只能传 null。
        assert!(params["button"].is_null());
    }

    #[test]
    fn answer_inline_query_params_keep_next_offset() {
        let params = answer_inline_query_params(1, json!([]), 60, false, Some("page2".to_owned()));
        assert_eq!(params["next_offset"], json!("page2"));
        assert_eq!(params["is_personal"], json!(false));
        assert_eq!(params["cache_time"], json!(60));
    }

    #[test]
    fn pin_chat_message_params_pin_for_everyone_with_notification() {
        let params = pin_chat_message_params(-100, 5);
        assert_eq!(params["@type"], json!("pinChatMessage"));
        assert_eq!(params["chat_id"], json!(-100));
        assert_eq!(params["message_id"], json!(5));
        assert_eq!(params["disable_notification"], json!(false));
        assert_eq!(params["only_for_self"], json!(false));
    }

    #[test]
    fn get_me_params_have_only_the_type() {
        let params = get_me_params();
        assert_eq!(params, json!({ "@type": "getMe" }));
        assert_eq!(
            params.as_object().map(|object| object.len()),
            Some(1),
            "getMe 不接受参数，多写字段 TDLib 会报解析错误"
        );
    }

    #[test]
    fn errors_carry_the_reason() {
        let host = TdExtError::Host("等响应超时".to_owned());
        assert!(matches!(&host, TdExtError::Host(_)));
        assert!(host.to_string().contains("超时"));

        let payload = TdExtError::Payload("缺 link".to_owned());
        assert!(matches!(&payload, TdExtError::Payload(_)));
        assert!(payload.to_string().contains("缺 link"));
    }

    #[test]
    fn message_link_extracts_only_the_link() {
        let response = json!({
            "@type": "messageLink",
            "link": "https://t.me/c/1234/42",
            "is_public": false,
        });
        assert_eq!(message_link(&response).unwrap(), json!("https://t.me/c/1234/42"));

        // 少了 link（或类型不对）要报 Payload，而不是悄悄给 null。
        let error = message_link(&json!({ "@type": "messageLink" })).unwrap_err();
        assert!(matches!(error, TdExtError::Payload(_)));
        let error = message_link(&json!({ "link": 42 })).unwrap_err();
        assert!(matches!(error, TdExtError::Payload(_)));
    }

    #[test]
    fn default_timeout_is_not_zero() {
        // 0 在 ABI 里的含义是「只看当前队列」，拿来当默认值会让所有请求立刻超时。
        assert!(DEFAULT_TIMEOUT_MS > 0);
    }
}
