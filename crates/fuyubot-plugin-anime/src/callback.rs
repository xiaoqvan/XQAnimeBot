//! 内联键盘回调（审核按钮）的路由骨架。
//!
//! 对应 TS 的 `CallbackQuery/index.ts` + `hasAnime.ts` + `bangumi.ts`：那里用
//! `callback_data` 的前缀（`Y_anime?` / `F_anime?` / `N_anime?` / `N_ep?` /
//! `colorep?`）分发到不同处理器；Rust 侧先只做审核这一条最短的线：
//!
//! ```text
//! 管理群里的两个按钮
//!   review:approve:<key>  →  pendingReviews.status = approved
//!   review:reject:<key>   →  pendingReviews.status = rejected
//! ```
//!
//! ## 与 TS 的差异（有意为之）
//!
//! - TS 用 `URLSearchParams` 把参数塞在 `?` 后面（`Y_anime?id=1&c=2`），这里用
//!   `:` 分隔的固定前缀。原因是我们目前只有一个动作维度，没必要为两条参数引一套
//!   查询串解析；等参数变多再升级格式，`parse` 是唯一入口。
//! - TDLib 的 `callback_data` 本身必须是 base64（`inlineKeyboardButtonTypeCallback
//!   data:bytes`），所以按钮数据发出去前编码、收到后解码——这一点与 TS 一致。
//! - TS 的审核结果直接改消息文本；这里先只往管理群发一条结果，编辑原消息留给下一步
//!   （需要 `editMessageText` + 保存 message_id，属于接线阶段）。
//!
//! ## 线程模型
//!
//! 回调跑在宿主的投递线程上（见 `docs/HANDOFF.md` §6.10）：`answerCallbackQuery`
//! 只是投递请求，可以立刻做；读/写 `pendingReviews` 是跨插件的同步服务调用，放到
//! 自己的线程里，避免一次 mongo 往返把整条更新流按住。
//!
//! ## 退订
//!
//! [CallbackSubscription] 是守卫：`drop` 时调 `host.off(handle)`，与
//! `fuyubot_simplify::CommandSubscription` 同一套语义。插件 `stop` 里把它置空即可。

use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;
use std::thread;

use anime_core::log;
use anime_core::store::{PENDING_REVIEW_STATUS, Storage};
use fuyubot_abi::{HostApi, SubscriptionHandle};
use fuyubot_simplify::{ReplyButton, SendMessageParams, Simplify, Text, send_params};
use serde_json::Value;

use crate::runtime::Runtime;

/// 用户点了内联按钮时宿主推的更新类型。
pub const CALLBACK_EVENT: &str = "updateNewCallbackQuery";

/// 审核动作。两个按钮一个动作，字段只有这两个值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewAction {
    /// 通过。
    Approve,
    /// 驳回。
    Reject,
}

impl ReviewAction {
    /// 按钮数据里的动作段：`review:<prefix>:<key>`。
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }

    /// 写进 `pendingReviews.status` 的终态。
    ///
    /// 用英文而不是 TS 里的中文状态：这一列只给程序判断，文案在 [report] 里。
    pub const fn status(self) -> &'static str {
        match self {
            Self::Approve => "approved",
            Self::Reject => "rejected",
        }
    }

    /// 回给点击者的一句 toast（`answerCallbackQuery.text`）。
    pub const fn answer_text(self) -> &'static str {
        match self {
            Self::Approve => "已通过",
            Self::Reject => "已驳回",
        }
    }

    /// 按钮上的字。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Approve => "通过",
            Self::Reject => "驳回",
        }
    }
}

/// 解析按钮数据：只认 `review:approve:<id>` / `review:reject:<id>`。
///
/// `id` 是 pendingReviews 的自增主键（数字）。返回 `None` 的情况都在测试里钉住了：
/// - 前缀不对（别的插件/别的按钮发来的回调）——调用方应静默放过，别抢别人的；
/// - 动作名不认识；
/// - id 不是正整数。
pub fn parse(data: &str) -> Option<(ReviewAction, i64)> {
    let rest = data.strip_prefix("review:")?;
    let (action, key) = rest.split_once(':')?;
    let action = match action {
        "approve" => ReviewAction::Approve,
        "reject" => ReviewAction::Reject,
        _ => return None,
    };
    let id = key.trim().parse::<i64>().ok()?;
    if id <= 0 {
        return None;
    }
    Some((action, id))
}

/// 审核键盘（类型化）：直接挂到 [SendMessageParams::reply_markup]，不用再解析 JSON。
pub fn review_buttons(id: i64) -> Vec<Vec<ReplyButton>> {
    vec![vec![
        ReplyButton::callback(
            ReviewAction::Approve.label(),
            callback_data(ReviewAction::Approve, id),
        ),
        ReplyButton::callback(
            ReviewAction::Reject.label(),
            callback_data(ReviewAction::Reject, id),
        ),
    ]]
}

/// 审核按钮的内联键盘：**一行两个**（通过 / 驳回）。
///
/// 布局与 TS `promptAdminConfirmAnime` 一致（`rows: [[正确, 错误]]`）——两个动作
/// 互为对照，分两行反而让管理员多点一次。
///
/// 返回的就是 `sendMessage.reply_markup` 的值，可以直接塞进
/// [SendMessageParams::reply_markup]。
// 审核提示消息的发送方还没接（下一步）；接上以后删掉这个 allow。
#[allow(dead_code)]
pub fn review_keyboard(id: i64) -> Value {
    let params = SendMessageParams {
        reply_markup: vec![vec![
            ReplyButton::callback(
                ReviewAction::Approve.label(),
                callback_data(ReviewAction::Approve, id),
            ),
            ReplyButton::callback(
                ReviewAction::Reject.label(),
                callback_data(ReviewAction::Reject, id),
            ),
        ]],
        ..SendMessageParams::default()
    };

    // `ReplyButton::to_json` / `reply_markup` 在 simplify 里是私有的，而它们才是按钮
    // JSON（base64、style、icon_custom_emoji_id）的唯一一份真相。这里借公开的
    // `send_params` 组装一次、只取 `reply_markup`，比在本模块再抄一遍字段更不容易漂移。
    // `render` 只影响文本，键盘不经过它；传个占位实现即可。
    let payload = send_params(0, &params, &|_: &Text| Value::Null);
    payload.get("reply_markup").cloned().unwrap_or(Value::Null)
}

/// 按钮数据：`review:<action>:<id>`。真正的 base64 编码由 simplify 做。
fn callback_data(action: ReviewAction, id: i64) -> String {
    format!("review:{}:{id}", action.prefix())
}

/// 一条 `updateNewCallbackQuery` 里我们关心的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackQuery {
    /// 回话用的 id（`answerCallbackQuery.callback_query_id`）。
    pub id: i64,
    /// 按钮所在会话。
    pub chat_id: i64,
    /// 按钮所在消息；0 表示更新里没带。
    pub message_id: i64,
    /// 点按钮的人；0 表示更新里没带（用于管理群文案）。
    pub sender_user_id: i64,
    /// **已解码**的按钮数据（base64 → UTF-8）。
    pub data: String,
}

/// 从 `updateNewCallbackQuery` 的负载里取出按钮数据。
///
/// 字段路径照真 `td_api.tl`：`updateNewCallbackQuery.payload: callbackQueryPayload`，
/// 其中 `callbackQueryPayloadData.data: bytes`（JSON 里是 base64）。游戏按钮
/// （`callbackQueryPayloadGame`）与带密码按钮（`...WithPassword`）不是我们的，
/// 返回 `None`。
///
/// 真 Telegram 更新一定做了 base64；为了手写的回放/单测 JSON 也能直接跑，
/// 解不出来时退回原文（见 [decode_data]）。
pub fn callback_query_of(update: &Value) -> Option<CallbackQuery> {
    let payload = update.get("payload");
    if let Some(kind) = payload
        .and_then(|payload| payload.get("@type"))
        .and_then(Value::as_str)
    {
        if kind != "callbackQueryPayloadData" {
            return None;
        }
    }

    let raw = payload
        .and_then(|payload| payload.get("data"))
        .or_else(|| update.get("data"))
        .and_then(Value::as_str)?;

    Some(CallbackQuery {
        id: update.get("id")?.as_i64()?,
        chat_id: update.get("chat_id")?.as_i64()?,
        message_id: update
            .get("message_id")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        sender_user_id: update
            .get("sender_user_id")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        data: decode_data(raw),
    })
}

/// 订阅 `updateNewCallbackQuery`，把认得出的审核按钮交给处理链。
///
/// 返回 `None` 表示宿主拒绝订阅（没接事件总线）。守卫存进插件状态，`drop` 即退订。
pub fn spawn(host: HostApi, runtime: Arc<Runtime>) -> Option<CallbackSubscription> {
    let mut owned: Box<Box<dyn Fn(&CallbackQuery) + Send + Sync>> =
        Box::new(Box::new(move |query: &CallbackQuery| {
            route(&runtime, query);
        }));
    // 递给 ABI 的是**内层 Box 的地址**：它是瘦指针，外层 Box 活着它就稳定。
    let user_data = (&raw mut *owned).cast::<c_void>();
    let handle = host.on(CALLBACK_EVENT, None, callback_trampoline, user_data);
    handle.is_valid().then_some(CallbackSubscription {
        host,
        handle,
        _handler: owned,
    })
}

/// 一个活着的回调订阅：**drop 就是退订**（照抄 `CommandSubscription`）。
///
/// 回调闭包由它自己持有——递给 ABI 的 `user_data` 指的就是这里那个 Box，
/// 退订之后一起释放。
pub struct CallbackSubscription {
    host: HostApi,
    handle: SubscriptionHandle,
    /// 只负责**持有**回调；退订之前它的地址必须稳定。
    _handler: Box<Box<dyn Fn(&CallbackQuery) + Send + Sync>>,
}

impl Drop for CallbackSubscription {
    fn drop(&mut self) {
        if self.handle.is_valid() {
            self.host.off(self.handle);
        }
    }
}

/// C 回调：把负载解成 [CallbackQuery]，再交给闭包。
///
/// 解不出来或不是审核按钮时返回 0 就够——回调是「顺手处理」，不该让一条坏数据
/// 影响宿主继续投递后面的更新。
unsafe extern "C-unwind" fn callback_trampoline(
    user_data: *mut c_void,
    payload: *const c_char,
) -> i32 {
    if user_data.is_null() || payload.is_null() {
        return -1;
    }
    // SAFETY: `user_data` 是 `spawn` 里那个 Box，订阅活多久它就活多久。
    let handler = unsafe { &*(user_data as *const Box<dyn Fn(&CallbackQuery) + Send + Sync>) };
    // SAFETY: 宿主保证 `payload` 是 NUL 结尾的临时字符串。
    let text = unsafe { CStr::from_ptr(payload) }.to_string_lossy();
    let Ok(update) = serde_json::from_str::<Value>(&text) else {
        return 0;
    };
    if let Some(query) = callback_query_of(&update) {
        handler(&query);
    }
    0
}

/// 条目收藏状态：1 想看 / 2 看过 / 3 在看 / 4 搁置 / 5 抛弃（Bangumi v0 的定义）。
pub const COLLECTION_STATUSES: [(u32, &str); 5] = [
    (1, "想看"),
    (2, "看过"),
    (3, "在看"),
    (4, "搁置"),
    (5, "抛弃"),
];

/// 一次收藏回调：`collection:<subject>:<type>`（可带 `:episode:etype`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectionRequest {
    pub subject_id: u64,
    pub kind: u32,
    /// (章节 id, 章节状态)：可选。
    pub episode: Option<(u64, u32)>,
}

/// 解析收藏回调数据；不是收藏按钮返回 None。
pub fn parse_collection(data: &str) -> Option<CollectionRequest> {
    let mut parts = data.split(':');
    if parts.next()? != "collection" {
        return None;
    }
    let subject_id = parts.next()?.parse::<u64>().ok()?;
    let kind = parts.next()?.parse::<u32>().ok()?;
    if !(1..=5).contains(&kind) {
        return None;
    }
    let episode = match (parts.next(), parts.next()) {
        (Some(episode), Some(ep_kind)) => {
            let episode = episode.parse::<u64>().ok()?;
            let ep_kind = ep_kind.parse::<u32>().ok()?;
            if ep_kind > 3 {
                return None;
            }
            Some((episode, ep_kind))
        }
        (None, None) => None,
        _ => return None,
    };
    Some(CollectionRequest {
        subject_id,
        kind,
        episode,
    })
}

/// 收藏键盘：五个状态排成两行（3 + 2）。
pub fn collection_buttons(subject_id: u64, episode: Option<u64>) -> Vec<Vec<ReplyButton>> {
    let make = |kind: u32, label: &str| {
        let data = match episode {
            Some(episode) => format!("collection:{subject_id}:{kind}:{episode}:2"),
            None => format!("collection:{subject_id}:{kind}"),
        };
        ReplyButton::callback(label, data)
    };
    vec![
        COLLECTION_STATUSES[..3]
            .iter()
            .map(|(kind, label)| make(*kind, label))
            .collect(),
        COLLECTION_STATUSES[3..]
            .iter()
            .map(|(kind, label)| make(*kind, label))
            .collect(),
    ]
}

/// 处理收藏按钮：用点击者自己的 token 改收藏状态。
fn handle_collection(runtime: &Arc<Runtime>, query: &CallbackQuery, request: CollectionRequest) {
    let worker = Arc::clone(runtime);
    let query = query.clone();
    let spawned = thread::Builder::new()
        .name("xq-anime-collect".to_owned())
        .spawn(move || {
            let toast = apply_collection(&worker, &query, request);
            if let Err(error) = Simplify::new(worker.host).answer_callback_query(
                query.id,
                Some(&toast),
                false,
            ) {
                log!(&worker.logger, Warn, "answerCallbackQuery 失败：{error}");
            }
            log!(
                &worker.logger,
                Info,
                "收藏回调：条目 {} → {}（用户 {}）",
                request.subject_id,
                request.kind,
                query.sender_user_id
            );
        });
    if let Err(error) = spawned {
        log!(&runtime.logger, Error, "收藏回调线程启动失败：{error}");
    }
}

/// 真正调 Bangumi。返回给用户看的 toast 文案。
fn apply_collection(
    runtime: &Runtime,
    query: &CallbackQuery,
    request: CollectionRequest,
) -> String {
    use anime_core::store::Storage;
    let binding = match runtime.store.bangumi_user_by_tg(query.sender_user_id) {
        Ok(binding) => binding,
        Err(error) => return format!("读取绑定失败：{error}"),
    };
    let Some(binding) = binding.filter(|user| {
        user.access_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
    }) else {
        return "请先 /bindbangumi 绑定 Bangumi 账号".to_owned();
    };
    let access_token = binding.access_token.as_deref().unwrap_or_default();
    let client = anime_core::bangumi::BgmClient::new(runtime.config.bangumi.clone());
    if let Err(error) =
        client.set_subject_collection(access_token, request.subject_id, request.kind)
    {
        return format!("修改收藏失败：{error}");
    }
    if let Some((episode_id, ep_kind)) = request.episode {
        // 章节失败不报错：条目状态已经改成功，章节可能因为未收藏而 400。
        let _ = client.set_episode_collection(access_token, episode_id, ep_kind);
    }
    let label = COLLECTION_STATUSES
        .iter()
        .find(|(kind, _)| *kind == request.kind)
        .map(|(_, label)| *label)
        .unwrap_or("?");
    format!("已标记为「{label}」")
}
/// 认得出就处理，认不出就放过（同一进程里还有别的插件在收同一条更新）。
fn route(runtime: &Arc<Runtime>, query: &CallbackQuery) {
    // 收藏按钮：不弹审核结论，只回一个 toast。
    if let Some(request) = parse_collection(&query.data) {
        handle_collection(runtime, query, request);
        return;
    }
    let Some((action, id)) = parse(&query.data) else {
        log!(
            &runtime.logger,
            Debug,
            "忽略不认识的回调数据：{}",
            query.data
        );
        return;
    };

    // 先回话：客户端在等 answerCallbackQuery，不回就一直转圈。
    // 这里给的是「动作已收到」的乐观提示；真正的结论（记录是否存在、写库成败）
    // 在管理群里，见 [settle]。
    if let Err(error) = Simplify::new(runtime.host).answer_callback_query(
        query.id,
        Some(action.answer_text()),
        false,
    ) {
        log!(
            &runtime.logger,
            Warn,
            "answerCallbackQuery 失败（#{}）：{error}",
            query.id
        );
    }

    // 读/写 pendingReviews 是同步服务调用，不能堵住宿主的投递线程（HANDOFF §6.10）。
    let worker = Arc::clone(runtime);
    let operator = query.sender_user_id;
    let message_id = query.message_id;
    let spawned = thread::Builder::new()
        .name("xq-anime-review".to_owned())
        .spawn(move || settle(&worker, action, id, operator, message_id));
    if let Err(error) = spawned {
        log!(&runtime.logger, Error, "审核回调线程启动失败：{error}");
    }
}

/// 落库 + 通知管理群。
///
/// 幂等性靠 `status == pending`：重复点击（或两个管理员同时点）只认第一次，
/// 后面的只在管理群里回一句「已经处理过」，不会把已通过的记录改成驳回。
fn settle(
    runtime: &Arc<Runtime>,
    action: ReviewAction,
    id: i64,
    operator: i64,
    message_id: i64,
) {
    let existing = match runtime.store.pending_review_get(id) {
        Ok(existing) => existing,
        Err(error) => {
            log!(
                &runtime.logger,
                Error,
                "读取待审核记录失败（#{id}）：{error}"
            );
            notify_admin(runtime, format!("⚠️ 读取待审核记录失败：#{id}\n{error}"));
            return;
        }
    };

    let Some(existing) = existing else {
        log!(&runtime.logger, Warn, "审核的 id 不存在：{id}");
        notify_admin(
            runtime,
            format!("⚠️ 审核记录不存在：#{id}\n可能已被删除，按钮已经过期。"),
        );
        return;
    };

    let status = existing
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if status != PENDING_REVIEW_STATUS {
        log!(
            &runtime.logger,
            Info,
            "审核回调被忽略：#{id} 当前状态是 {status}"
        );
        notify_admin(
            runtime,
            format!("ℹ️ 已经处理过：#{id}（当前状态：{status}），本次点击被忽略。"),
        );
        return;
    }

    if let Err(error) = runtime.store.pending_review_set_status(id, action.status()) {
        log!(
            &runtime.logger,
            Error,
            "更新待审核状态失败（#{id}）：{error}"
        );
        notify_admin(runtime, format!("❌ 写待审核状态失败：#{id}\n{error}"));
        return;
    }

    log!(
        &runtime.logger,
        Info,
        "审核{}：#{id}（操作人 {operator}，消息 {message_id}）",
        action.label()
    );
    notify_admin(runtime, report(action, id, operator, message_id));

    // 通过 = 真去下载；驳回就到此为止。
    if action == ReviewAction::Approve && !crate::review::start_approved_download(runtime, &existing) {
        notify_admin(
            runtime,
            format!("⚠️ 审核通过但没能开始下载：#{id}（记录缺 magnet/title）"),
        );
    }
}

/// 管理群里的结果文案。带 emoji 是为了在一屏消息里一眼扫到结论。
fn report(action: ReviewAction, id: i64, operator: i64, message_id: i64) -> String {
    let (icon, verb) = match action {
        ReviewAction::Approve => ("✅", "审核通过"),
        ReviewAction::Reject => ("❌", "审核驳回"),
    };
    format!("{icon} {verb}：#{id}\n操作人：{operator}｜消息：{message_id}")
}

/// 往管理群发结果文本。
///
/// 没配 `telegram.admin_group_id`（默认 0）就只写日志：审核状态已经落库，
/// 不能因为少一个通知渠道让整条链失败。
fn notify_admin(runtime: &Runtime, text: String) {
    let chat_id = runtime.config.telegram.admin_group_id;
    if chat_id == 0 {
        log!(
            &runtime.logger,
            Warn,
            "未配置 telegram.admin_group_id，审核结果没有发到管理群：{text}"
        );
        return;
    }

    let mut params = SendMessageParams::text(Text::plain(text));
    // TS 的审核提示发在 ADMIN_GROUP_ID 的 NAV_GROUP_THREAD_ID 话题里
    // （见 anime/index.ts::promptAdminConfirmAnime），这里保持一致。
    let thread = runtime.config.telegram.nav_group_thread_id;
    params.topic_id = (thread != 0).then_some(thread);

    if let Err(error) = Simplify::new(runtime.host).send_message(chat_id, &params) {
        log!(
            &runtime.logger,
            Warn,
            "审核结果发管理群失败（chat {chat_id}）：{error}"
        );
    }
}

/// TDLib 的按钮数据是 base64 字节串；解不出来时把原文当数据用。
///
/// 没有直接引 `base64` crate：本模块只做解码，且插件清单不在这次改动的范围里。
/// 等 `fuyubot-simplify` 把回调数据的编解码公开出来，这里应该换成它——编码侧
/// 已经在用 simplify，别再养第二份实现。
fn decode_data(raw: &str) -> String {
    base64_decode(raw).unwrap_or_else(|| raw.to_owned())
}

/// 标准 base64 解码（顺带认 base64url 的 `-_/`），解码结果必须是 UTF-8。
///
/// 只接受合法字符与零填充；`=` 之后还出现数据、或残留位不是零，都算坏数据。
/// 空串解码成空串（`answer` 之类的空数据不该被当成错误）。
fn base64_decode(data: &str) -> Option<String> {
    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    let mut padding = false;

    for byte in data.bytes() {
        if byte == b'=' {
            padding = true;
            continue;
        }
        // padding 之后不能再有数据。
        if padding {
            return None;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32;

        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }

    // 残留位必须是零填充，否则不是合法的 base64（例如长度为 1 的 "Z"）。
    if buffer != 0 {
        return None;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_accepts_both_review_actions() {
        assert_eq!(
            parse("review:approve:123"),
            Some((ReviewAction::Approve, 123))
        );
        assert_eq!(
            parse("review:reject:42"),
            Some((ReviewAction::Reject, 42))
        );
        // id 的首尾空白不算 id 的一部分。
        assert_eq!(
            parse("review:approve:  7  "),
            Some((ReviewAction::Approve, 7))
        );
    }

    #[test]
    fn parse_rejects_missing_prefix() {
        for data in [
            "approve:123",
            "Y_anime?id=1&c=2",
            "reviewx:approve:1",
            "review:",
            "",
        ] {
            assert_eq!(parse(data), None, "{data:?} 不该被认成审核回调");
        }
    }

    #[test]
    fn parse_rejects_non_numeric_or_non_positive_id() {
        for data in [
            "review:approve:",
            "review:reject:",
            "review:approve:   ",
            "review:reject:\u{3000}",
            "review:approve:abc",
            "review:approve:0",
            "review:approve:-1",
        ] {
            assert_eq!(parse(data), None, "{data:?} 的 id 非法");
        }
    }

    #[test]
    fn parse_rejects_extra_colons_and_unknown_actions() {
        for data in [
            "review:approve:1:2",
            "review:reject::x",
            "review:approve:1:",
            "review:delete:1",
        ] {
            assert_eq!(parse(data), None, "{data:?} 不是约定格式");
        }
    }

    #[test]
    fn parses_collection_callbacks() {
        let request = parse_collection("collection:400602:3").expect("应当能解析");
        assert_eq!(request.subject_id, 400602);
        assert_eq!(request.kind, 3);
        assert!(request.episode.is_none());

        let with_episode = parse_collection("collection:400602:3:12345:2").unwrap();
        assert_eq!(with_episode.episode, Some((12345, 2)));

        assert!(parse_collection("collection:400602:9").is_none());
        assert!(parse_collection("collection:400602").is_none());
        assert!(parse_collection("collection:400602:3:1").is_none());
        assert!(parse_collection("review:approve:x").is_none());
    }

    #[test]
    fn collection_keyboard_has_five_statuses() {
        let rows = collection_buttons(400602, None);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 3);
        assert_eq!(rows[1].len(), 2);
        let with_episode = collection_buttons(400602, Some(12345));
        let debug = format!("{:?}", with_episode[0][0]);
        assert!(debug.contains("想看"), "{debug}");
        assert!(debug.contains("collection:400602:1:12345:2"), "{debug}");
    }
    #[test]
    fn review_keyboard_is_one_row_of_two_callback_buttons() {
        let markup = review_keyboard(123);
        assert_eq!(markup["@type"], json!("replyMarkupInlineKeyboard"));

        let rows = markup["rows"].as_array().expect("rows 必须是数组");
        assert_eq!(
            rows.len(),
            1,
            "审核按钮排一行（与 TS promptAdminConfirmAnime 一致）"
        );
        let buttons = rows[0].as_array().expect("row 必须是数组");
        assert_eq!(buttons.len(), 2, "通过 / 驳回 两个按钮");

        let approve = &buttons[0];
        assert_eq!(approve["@type"], json!("inlineKeyboardButton"));
        assert_eq!(approve["text"], json!("通过"));
        assert_eq!(
            approve["type"]["@type"],
            json!("inlineKeyboardButtonTypeCallback")
        );
        assert_eq!(
            approve["type"]["data"],
            json!("cmV2aWV3OmFwcHJvdmU6MTIz"),
            "通过按钮的 callback_data 是 review:approve:123 的 base64"
        );

        let reject = &buttons[1];
        assert_eq!(reject["text"], json!("驳回"));
        assert_eq!(
            reject["type"]["@type"],
            json!("inlineKeyboardButtonTypeCallback")
        );
        assert_eq!(
            reject["type"]["data"],
            json!("cmV2aWV3OnJlamVjdDoxMjM="),
            "驳回按钮的 callback_data 是 review:reject:123 的 base64"
        );
    }

    #[test]
    fn callback_query_of_decodes_the_base64_payload() {
        let update = json!({
            "@type": "updateNewCallbackQuery",
            "id": 555,
            "chat_id": -100123,
            "message_id": 42,
            "sender_user_id": 7,
            "payload": {
                "@type": "callbackQueryPayloadData",
                "data": "cmV2aWV3OmFwcHJvdmU6YWJjMTIz"
            }
        });

        let query = callback_query_of(&update).expect("应认出审核回调");
        assert_eq!(query.id, 555);
        assert_eq!(query.chat_id, -100123);
        assert_eq!(query.message_id, 42);
        assert_eq!(query.sender_user_id, 7);
        assert_eq!(query.data, "review:approve:abc123");
    }

    #[test]
    fn callback_query_of_ignores_other_payload_kinds() {
        let game = json!({
            "@type": "updateNewCallbackQuery",
            "id": 1,
            "chat_id": 2,
            "message_id": 3,
            "payload": { "@type": "callbackQueryPayloadGame", "game_short_name": "x" }
        });
        assert_eq!(callback_query_of(&game), None);

        let no_data = json!({
            "@type": "updateNewCallbackQuery",
            "id": 1,
            "chat_id": 2
        });
        assert_eq!(callback_query_of(&no_data), None);
    }

    #[test]
    fn callback_query_of_falls_back_to_raw_data_for_handwritten_replay() {
        // 回放文件里可能直接写明文（没有 base64）；带冒号的字符串不可能是合法 base64，
        // 所以这里走的是「解码失败 → 当原文」这条路。
        let update = json!({
            "id": 9,
            "chat_id": -1,
            "message_id": 3,
            "data": "review:reject:k-1"
        });
        let query = callback_query_of(&update).expect("明文回放也要能跑");
        assert_eq!(query.data, "review:reject:k-1");
        assert_eq!(query.sender_user_id, 0, "缺字段按 0 处理，不 panic");
    }

    #[test]
    fn base64_decoder_matches_rfc_vectors() {
        // RFC 4648 的样例 + 一段 UTF-8 中文，防止「只对 ASCII 恰好正确」。
        for (encoded, decoded) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9vYmE=", "fooba"),
            ("Zm9vYmFy", "foobar"),
            ("5L2g5aW9", "你好"),
        ] {
            assert_eq!(
                base64_decode(encoded).as_deref(),
                Some(decoded),
                "{encoded:?} 解码结果不对"
            );
        }

        assert_eq!(base64_decode("不是 base64"), None);
        assert_eq!(base64_decode("Z"), None, "残留位非零必须是坏数据");
        assert_eq!(base64_decode("Zg==Zg=="), None, "= 之后不能再有数据");
    }
}
