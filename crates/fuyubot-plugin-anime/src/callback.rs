//! 内联键盘回调（审核按钮）的路由骨架。
//!
//! 对应 TS 的 `CallbackQuery/index.ts` + `hasAnime.ts` + `bangumi.ts`：用
//! `callback_data` 的前缀（`Y_anime?` / `F_anime?` / `N_anime?` / `N_ep?` /
//! `colorep?`）分发到不同处理器。目前落地的是审核这一条线：
//!
//! ```text
//! Y_anime?r=<id>                确认匹配正确（有待审核记录 → 改状态）
//! Y_anime?id=<ep>&c=<cache>     旧格式确认（没有待审核记录可改）
//! F_anime?r=<id>                匹配错误 → 问管理员要正确条目 → 迁移资源
//! N_anime?c=<cache>             搜不到番剧 → 问管理员要条目 → 重新下载发送
//! N_ep?c=<cache>&id=<anime>     集数不对 → 问管理员要章节 → 迁移资源
//! review:approve:<id>           最早那版按钮，仅兼容还在飞的旧消息
//! collection:<subject>:<kind>   Bangumi 收藏（另一条线，见 [parse_collection]）
//! ```
//!
//! 后三种是**多轮交互**：点了之后改提示语，然后等管理员**回复那条消息**给出
//! 章节 id 或 bgm 链接（[`crate::reply`]）。这是本模块唯一需要「等下一条更新」
//! 的地方，也是 Rust 侧比 TS 多出来的部分——TS 有 `client.iterUpdates()` 这条
//! 更新流，这里得自己订阅 `updateNewMessage` 搭一个。
//!
//! ## 与 TS 的差异（有意为之）
//!
//! - TS 用 `URLSearchParams` 解析参数；这里是一份二十行的 [`Query`]（`k=v&k2=v2`，
//!   只取整数）——回调数据是我们自己生成的，不会有需要转义的字符。
//! - TDLib 的 `callback_data` 本身必须是 base64（`inlineKeyboardButtonTypeCallback
//!   data:bytes`），所以按钮数据发出去前编码、收到后解码——这一点与 TS 一致。
//! - TS 的纠正流程会 `editMessageText` 改写提示消息；宿主侧目前只有
//!   `edit_caption`（改图注），所以这里改成「回复提示消息」说话。语义一致，
//!   管理员看到的仍然是在那一条消息下面来回。
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
use serde_json::{Value, json};

use crate::runtime::Runtime;
use crate::td::Target;

/// 用户点了内联按钮时宿主推的更新类型。
pub const CALLBACK_EVENT: &str = "updateNewCallbackQuery";

/// 审核类回调，与 TS `CallbackQuery/index.ts` 的四个分支逐字对齐。
///
/// 格式统一是 `<命令>?<k>=<v>&...`（TS 用 `URLSearchParams` 解析）：
///
/// ```text
/// Y_anime?r=<reviewId>               确认匹配正确（待审核模式）
/// Y_anime?id=<episodeId>&c=<cacheId> 确认匹配正确（旧格式，无待审核记录时）
/// F_anime?r=<reviewId>               匹配错误，进入纠正流程
/// N_anime?c=<cacheId>                自动搜索失败，请手动提供番剧信息
/// N_ep?c=<cacheId>&id=<animeId>      集数匹配失败，请手动提供集数
/// ```
///
/// 为什么要这套而不是原来的 `review:approve:<id>`：TS 的审核不只是「通过/驳回」两个
/// 动作——`N_anime` / `N_ep` 是**多轮交互**的入口（点了之后管理员回复消息提供正确
/// 的条目链接或集数 id，bot 解析回复继续流程）。一个动作维度装不下这些，所以照搬
/// TS 的「命令 + 查询串」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimeCallback {
    /// `Y_anime`：确认当前匹配正确。
    Confirm(CallbackTarget),
    /// `F_anime`：匹配错误，等待管理员提供正确条目。
    Reject(CallbackTarget),
    /// `N_anime`：搜不到番剧，等待管理员提供条目链接。
    ProvideAnime { cache_id: i64 },
    /// `N_ep`：集数匹配失败，等待管理员提供集数 id。
    ProvideEpisode { cache_id: i64, anime_id: i64 },
}

/// 回调定位到哪条记录。
///
/// 两套并存是 TS 的历史：`r` 是后来的「待审核记录 id」（更短，且能带完整上下文），
/// `id`+`c` 是旧格式。新流程一律带 `r`，旧格式只为兼容还在飞的旧按钮保留。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CallbackTarget {
    /// `r`：pendingReviews 的自增 id。
    pub review_id: Option<i64>,
    /// `id`：Bangumi 章节 id。
    pub episode_id: Option<i64>,
    /// `c`：cacheItem 的 id。
    pub cache_id: Option<i64>,
}

impl CallbackTarget {
    /// 只带待审核 id 的目标（新流程）。
    pub const fn by_review(review_id: i64) -> Self {
        Self {
            review_id: Some(review_id),
            episode_id: None,
            cache_id: None,
        }
    }

    /// 旧格式：章节 id + 缓存 id。
    pub const fn by_episode(episode_id: i64, cache_id: i64) -> Self {
        Self {
            review_id: None,
            episode_id: Some(episode_id),
            cache_id: Some(cache_id),
        }
    }
}

/// 解析 `<命令>?<k>=<v>&...` 形式的回调数据。
///
/// 返回 `None` 的情况：没有 `?`、命令不认识、`N_anime`/`N_ep` 缺必需参数。
/// 「命令认识但参数不全」也算不认识——半截参数进到处理流程只会做错事。
pub fn parse_anime_callback(raw: &str) -> Option<AnimeCallback> {
    let (command, params) = raw.split_once('?')?;
    let params = Query::new(params);
    match command.trim() {
        "Y_anime" => Some(AnimeCallback::Confirm(target_of(&params))),
        "F_anime" => Some(AnimeCallback::Reject(target_of(&params))),
        "N_anime" => Some(AnimeCallback::ProvideAnime {
            cache_id: params.i64("c")?,
        }),
        "N_ep" => Some(AnimeCallback::ProvideEpisode {
            cache_id: params.i64("c")?,
            anime_id: params.i64("id")?,
        }),
        _ => None,
    }
}

/// 从查询串里还原定位目标：`r` 优先，没有再取 `id` + `c`。
fn target_of(params: &Query<'_>) -> CallbackTarget {
    if let Some(review_id) = params.i64("r") {
        return CallbackTarget::by_review(review_id);
    }
    CallbackTarget {
        review_id: None,
        episode_id: params.i64("id"),
        cache_id: params.i64("c"),
    }
}

/// 构造按钮数据（base64 之前的原文）。
pub mod data {
    use super::CallbackTarget;

    /// `Y_anime?r=<id>` —— 有待审核记录时用它，比 `id`+`c` 短。
    pub fn confirm(target: CallbackTarget) -> String {
        let mut out = String::from("Y_anime?");
        push_target(&mut out, target);
        out
    }

    /// `F_anime?r=<id>`
    pub fn reject(target: CallbackTarget) -> String {
        let mut out = String::from("F_anime?");
        push_target(&mut out, target);
        out
    }

    /// `N_anime?c=<cacheId>`
    pub fn provide_anime(cache_id: i64) -> String {
        format!("N_anime?c={cache_id}")
    }

    /// `N_ep?c=<cacheId>&id=<animeId>`
    pub fn provide_episode(cache_id: i64, anime_id: i64) -> String {
        format!("N_ep?c={cache_id}&id={anime_id}")
    }

    fn push_target(out: &mut String, target: CallbackTarget) {
        match target.review_id {
            Some(id) => out.push_str(&format!("r={id}")),
            None => {
                if let Some(id) = target.episode_id {
                    out.push_str(&format!("id={id}"));
                }
                if let Some(id) = target.cache_id {
                    if target.episode_id.is_some() {
                        out.push('&');
                    }
                    out.push_str(&format!("c={id}"));
                }
            }
        }
    }
}

/// 极简查询串解析：`k=v&k2=v2`。只支持整数取值，够回调数据用。
///
/// 不引 `serde_urlencoded`：回调数据只有两三个纯数字参数，引一个依赖换来的只是
/// 百分号解码——而按钮数据我们自己生成，不会有需要转义的字符。
struct Query<'a> {
    raw: &'a str,
}

impl<'a> Query<'a> {
    fn new(raw: &'a str) -> Self {
        Self { raw }
    }

    /// 取某个键的整数值；键不存在或不是整数返回 `None`。
    fn i64(&self, key: &str) -> Option<i64> {
        let prefix = format!("{key}=");
        self.raw
            .split('&')
            .filter_map(|pair| pair.strip_prefix(&prefix))
            .find_map(|value| value.trim().parse::<i64>().ok())
    }
}

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
    // TS 格式（Y_anime?r= / F_anime?r= / N_anime?c= / N_ep?c=&id=）：
    // 它比下面的 `review:approve:<id>` 表达力强（能带「提供/纠正」这类多轮入口），
    // 新提示一律用它，所以先判。
    if let Some(command) = parse_anime_callback(&query.data) {
        handle_anime_callback(runtime, query, command);
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

/// 分发 TS 格式的四种审核回调。
///
/// `Y_anime` 一步到位（改状态 + 回话），另外三种都要**等管理员回一条消息**
/// （[`crate::reply`]），所以统一扔到自己的线程里——等待是分钟级的，
/// 绝不能占着宿主的投递线程。
fn handle_anime_callback(runtime: &Arc<Runtime>, query: &CallbackQuery, command: AnimeCallback) {
    // 先回话：客户端在等 answerCallbackQuery，不回就一直转圈。
    answer(
        runtime,
        query,
        if matches!(command, AnimeCallback::Confirm(_)) {
            "确认成功"
        } else {
            "已收到"
        },
    );

    let worker = Arc::clone(runtime);
    let query = query.clone();
    let spawned = thread::Builder::new()
        .name("xq-anime-review".to_owned())
        .spawn(move || match command {
            AnimeCallback::Confirm(target) => confirm_anime(&worker, &query, target),
            // `F_anime`：匹错了，请管理员给出正确条目。有 `r` 时上下文从待审核记录里捞。
            AnimeCallback::Reject(target) => {
                let (cache_id, old_anime_id) = context_of(&worker, &target);
                correct(&worker, &query, cache_id, old_anime_id, false);
            }
            // `N_anime`：搜不到番剧，提示是在**下载之前**发的——纠正完要重新走一遍下载。
            AnimeCallback::ProvideAnime { cache_id } => {
                correct(&worker, &query, Some(cache_id), None, true)
            }
            // `N_ep`：集数不对，视频已经在缓冲频道了——纠正完只搬资源，不重下。
            AnimeCallback::ProvideEpisode { cache_id, anime_id } => {
                correct(&worker, &query, Some(cache_id), Some(anime_id), false)
            }
        });
    if let Err(error) = spawned {
        log!(&runtime.logger, Error, "审核回调线程启动失败：{error}");
    }
}

/// 回一次 `answerCallbackQuery`。失败只记日志——回话是锦上添花，
/// 真正的结论在管理群里（或提示消息本身）。
fn answer(runtime: &Runtime, query: &CallbackQuery, text: &str) {
    if let Err(error) =
        Simplify::new(runtime.host).answer_callback_query(query.id, Some(text), false)
    {
        log!(
            &runtime.logger,
            Warn,
            "answerCallbackQuery 失败（#{}）：{error}",
            query.id
        );
    }
}

/// `Y_anime`：确认匹配正确。
///
/// 有待审核 id 时走 [settle]（改状态 + 幂等保护）；旧格式（`id`+`c`）没有记录可改，
/// 只在管理群里留一句——它是「已经发出去的片子被人工确认过」，本来也不该再动数据库。
fn confirm_anime(runtime: &Arc<Runtime>, query: &CallbackQuery, target: CallbackTarget) {
    match target.review_id {
        Some(id) => {
            settle(
                runtime,
                ReviewAction::Approve,
                id,
                query.sender_user_id,
                query.message_id,
            );
        }
        None => {
            log!(
                &runtime.logger,
                Info,
                "旧格式确认（无待审核记录）：episode={:?} cache={:?}（用户 {}）",
                target.episode_id,
                target.cache_id,
                query.sender_user_id
            );
            notify_admin(
                runtime,
                format!(
                    "✅ 已确认（旧格式按钮，没有待审核记录可更新）\n章节：{}\n缓存条目：{}",
                    target
                        .episode_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "-".to_owned()),
                    target
                        .cache_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "-".to_owned())
                ),
            );
        }
    }
}

/// 从待审核记录里捞回「缓存条目 id / 当前番剧 id」。
///
/// `F_anime?r=<id>` 只带一个 id，而纠正要知道「原来挂在哪部番下」（资源迁移的
/// 起点）——这个上下文只能回查记录拿。
fn context_of(runtime: &Runtime, target: &CallbackTarget) -> (Option<i64>, Option<i64>) {
    let Some(review_id) = target.review_id else {
        return (target.cache_id, None);
    };
    let doc = match runtime.store.pending_review_get(review_id) {
        Ok(doc) => doc,
        Err(error) => {
            log!(&runtime.logger, Warn, "读待审核记录失败（#{review_id}）：{error}");
            return (target.cache_id, None);
        }
    };
    let anime_id = doc
        .as_ref()
        .and_then(|doc| doc.get("anime"))
        .and_then(|anime| anime.get("id"))
        .and_then(Value::as_i64);
    (target.cache_id, anime_id)
}

/// 一次纠正最多问几轮。
///
/// 无上限地循环等于「管理员发错一次就永远卡住一条线程」；5 轮足够（TS 虽然无限，
/// 但它没有线程成本概念——这里每一次等待都占着一条 OS 线程）。
const MAX_CORRECTION_ROUNDS: usize = 5;

/// 多轮纠正：问 → 等回复 → 解析 → 建档 → 迁移/重发。
///
/// `redownload` 区分两种场景（这是本函数唯一不好看但必须存在的参数）：
/// - `N_anime`（true）：提示发在**下载之前**，视频还没发过，纠正完要重新走下载+发送；
/// - `N_ep` / `F_anime`（false）：视频已经在缓冲频道了，纠正完只需把资源从
///   `cache_resources` 搬到 `resources`（改挂到正确的番剧下）。
fn correct(
    runtime: &Arc<Runtime>,
    query: &CallbackQuery,
    cache_id: Option<i64>,
    old_anime_id: Option<i64>,
    redownload: bool,
) {
    let target = Target::new(query.chat_id, query.message_id);
    runtime.td.reply_text(
        target,
        "请回复这一条消息，提供正确的 bgm.tv 章节链接或章节 id\n\n回复 /cancel 取消",
    );

    // 订阅不上就直说：静默卡住是最糟的结果（管理员不知道 bot 在等他）。
    if !crate::reply::ensure(runtime.host) {
        runtime.td.reply_text(
            target,
            "⚠️ 宿主未接入事件总线，收不到你的回复。请改用 /addanime <条目id> 手动导入。",
        );
        return;
    }

    for _ in 0..MAX_CORRECTION_ROUNDS {
        let Some(text) =
            crate::reply::wait(query.chat_id, query.message_id, crate::reply::DEFAULT_TIMEOUT)
        else {
            runtime.td.reply_text(target, "⏱️ 等待回复超时，本次纠正结束。需要的话请再点一次按钮。");
            return;
        };
        if text.eq_ignore_ascii_case("/cancel") {
            runtime.td.reply_text(target, "已取消");
            return;
        }
        let Some(reference) = crate::reply::parse_reference(&text) else {
            runtime.td.reply_text(
                target,
                "未识别的格式。请提供章节 id（例如 502272）或 bgm 链接，或者回复 /cancel 取消",
            );
            continue;
        };
        let (anime, episode_id) = match resolve_reference(runtime, reference) {
            Ok(resolved) => resolved,
            Err(error) => {
                runtime.td.reply_text(target, format!("未找到：{error}\n请重新提供，或 /cancel 取消"));
                continue;
            }
        };

        if redownload {
            // 视频还没发过：按纠正后的条目重新走一遍下载 + 发正片频道。
            let Some(item) = cache_id.and_then(|id| match runtime.store.cache_item_get(id) {
                Ok(item) => item,
                Err(error) => {
                    log!(&runtime.logger, Warn, "读缓存条目失败（#{id}）：{error}");
                    None
                }
            }) else {
                runtime
                    .td
                    .reply_text(target, "⚠️ 读不到缓存条目，无法重新下载（种子信息丢失）");
                return;
            };
            let mut plan = crate::chain::Plan::channel(anime.id);
            plan.episode_id = episode_id;
            plan.cache_id = cache_id.unwrap_or(0);
            crate::chain::spawn_with(Arc::clone(runtime), item, plan);
            runtime.td.reply_text(
                target,
                format!(
                    "✅ 已按「{}」（{}）重新下载并发送到频道",
                    anime.display_name(),
                    anime.id
                ),
            );
            return;
        }

        // 视频已经发过：把资源从缓存库搬到正片库，改挂到纠正后的条目下。
        let moved = match (cache_id, old_anime_id) {
            (Some(cache_id), Some(old)) if old != anime.id => {
                rebind_resources(runtime, cache_id, old, anime.id, episode_id)
            }
            _ => 0,
        };
        if let Some(old) = old_anime_id
            && old != anime.id
        {
            let _ = runtime.store.delete("cacheAnime", json!({ "id": old }), false);
        }
        let _ = crate::nav::refresh(runtime, anime.id);
        runtime.td.reply_text(
            target,
            format!(
                "✅ 已纠正为「{}」（{}）{}",
                anime.display_name(),
                anime.id,
                episode_id
                    .map(|id| format!("，章节 {id}"))
                    .unwrap_or_default()
                    + &if moved > 0 {
                        format!("；迁移资源 {moved} 条")
                    } else {
                        String::new()
                    }
            ),
        );
        return;
    }
    runtime
        .td
        .reply_text(target, "重试次数太多，本次纠正结束。需要的话请再点一次按钮。");
}

/// 把管理员给的「章节 id / 条目 id」变成库里的一部番剧。
///
/// 章节 id 是首选：纠正时最常说的是「这一集其实是第 N 集」。由章节反查条目，
/// 顺带把这一季的章节也一起入库（后面集数匹配才有得查）。
fn resolve_reference(
    runtime: &Runtime,
    reference: crate::reply::Reference,
) -> anime_core::Result<(anime_core::models::Anime, Option<i64>)> {
    let client = anime_core::bangumi::BgmClient::new(runtime.config.bangumi.clone());
    let (subject_id, episode_id) = match reference {
        crate::reply::Reference::Episode(id) => {
            let episode = client.episode(id)?;
            (episode.subject_id, Some(id as i64))
        }
        crate::reply::Reference::Subject(id) => (id, None),
    };
    if subject_id == 0 {
        return Err(anime_core::Error::Service {
            service: "bangumi".to_owned(),
            message: "章节没有归属条目".to_owned(),
        });
    }
    let subject = client.subject(subject_id)?;
    let episodes = client.episodes(subject_id)?;
    let anime = subject.to_anime();
    runtime.store.save_anime(&anime)?;
    let metas: Vec<_> = episodes
        .iter()
        .map(|episode| episode.to_meta(subject_id))
        .collect();
    runtime.store.upsert_episodes(subject_id as i64, &metas)?;
    Ok((anime, episode_id))
}

/// 把 `cache_resources` 里某个缓存条目的资源搬到 `resources`，改挂到新番剧下。
///
/// 这就是 TS 的 `rebindCacheResourceAnime`：视频消息没变（还在缓冲频道那条），
/// 变的是「它属于哪部番」——不动数据只改归属，比重新下载一遍便宜得多。
fn rebind_resources(
    runtime: &Runtime,
    cache_id: i64,
    old_anime_id: i64,
    new_anime_id: i64,
    episode_id: Option<i64>,
) -> usize {
    let docs = match runtime.store.find(
        "cache_resources",
        json!({ "cache_id": cache_id }),
        None,
        None,
        None,
    ) {
        Ok(docs) => docs,
        Err(error) => {
            log!(&runtime.logger, Warn, "读缓冲资源失败（{cache_id}）：{error}");
            return 0;
        }
    };
    let mut moved = 0usize;
    for doc in docs {
        let Ok(mut resource) =
            serde_json::from_value::<anime_core::models::EpisodeResourceDoc>(doc)
        else {
            continue;
        };
        resource.anime_id = new_anime_id;
        resource.episode_id = episode_id.or(resource.episode_id);
        // 去掉 cache_id 是「搬出去」的动作：resource_upsert 见它为空就写 resources。
        resource.cache_id = None;
        resource.id = None;
        match runtime.store.resource_upsert(&resource) {
            Ok(true) => moved += 1,
            Ok(false) => {}
            Err(error) => log!(&runtime.logger, Warn, "写入正片资源失败：{error}"),
        }
    }
    if moved > 0 {
        // 搬完清掉缓存库里的旧记录：留着会让「已收录 N 集」double count。
        if let Err(error) = runtime
            .store
            .delete("cache_resources", json!({ "cache_id": cache_id }), true)
        {
            log!(&runtime.logger, Warn, "清理缓冲资源失败：{error}");
        }
        log!(
            &runtime.logger,
            Info,
            "资源归属已修正：{old_anime_id} → {new_anime_id}（{moved} 条）"
        );
    }
    moved
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
    fn parse_anime_callback_reads_ts_formats() {
        use AnimeCallback::*;

        // 新流程：只带待审核 id
        assert_eq!(
            parse_anime_callback("Y_anime?r=42"),
            Some(Confirm(CallbackTarget::by_review(42)))
        );
        assert_eq!(
            parse_anime_callback("F_anime?r=7"),
            Some(Reject(CallbackTarget::by_review(7)))
        );
        // 旧格式：章节 id + 缓存 id
        assert_eq!(
            parse_anime_callback("Y_anime?id=1227091&c=9"),
            Some(Confirm(CallbackTarget::by_episode(1227091, 9)))
        );
        // 多轮交互入口
        assert_eq!(
            parse_anime_callback("N_anime?c=9"),
            Some(ProvideAnime { cache_id: 9 })
        );
        assert_eq!(
            parse_anime_callback("N_ep?c=9&id=400602"),
            Some(ProvideEpisode {
                cache_id: 9,
                anime_id: 400602
            })
        );
    }

    #[test]
    fn parse_anime_callback_rejects_unknown_or_incomplete() {
        for raw in [
            "review:approve:1", // 旧格式不再认
            "colorep?id=1&c=2", // 别的功能的回调，交给别处分发
            "chgcol?type=2",
            "Y_anime",  // 没有 ?
            "N_anime",  // 缺 c
            "N_anime?x=1", // 参数名不对
            "N_ep?c=9", // 缺 id
            "N_ep?id=1", // 缺 c
            "N_anime?c=abc", // c 不是整数
            "",
        ] {
            assert_eq!(
                parse_anime_callback(raw),
                None,
                "{raw:?} 不该被认成审核回调"
            );
        }
    }

    #[test]
    fn callback_data_round_trips_through_parse() {
        use AnimeCallback::*;

        // 构造 → 解析 必须回到同一个值，否则按钮点了会路由错。
        for expected in [
            Confirm(CallbackTarget::by_review(42)),
            Reject(CallbackTarget::by_review(7)),
            Confirm(CallbackTarget::by_episode(1227091, 9)),
            Reject(CallbackTarget::by_episode(1, 2)),
        ] {
            let raw = match expected {
                Confirm(target) => data::confirm(target),
                Reject(target) => data::reject(target),
                _ => unreachable!(),
            };
            assert_eq!(parse_anime_callback(&raw), Some(expected), "{raw}");
        }
        assert_eq!(
            parse_anime_callback(&data::provide_anime(9)),
            Some(ProvideAnime { cache_id: 9 })
        );
        assert_eq!(
            parse_anime_callback(&data::provide_episode(9, 400602)),
            Some(ProvideEpisode {
                cache_id: 9,
                anime_id: 400602
            })
        );
    }

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
