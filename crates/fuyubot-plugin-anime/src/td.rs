//! TDLib 调用封装。
//!
//! 业务代码只说「回这条消息」「把这张图发到那个会话」，不碰 `sendMessage` 的
//! `input_message_content` 结构——那层映射交给 `fuyubot-simplify`，它直接读
//! `td_api.tl` 生成的类型，是唯一一份真相。
//!
//! **不要在更新回调里同步等响应**：`send_message` 只是投递，立刻返回；真正等
//! 结果的那几个 `*_wait_success` 会阻塞投递线程，回调里用等于自己卡自己。
//!
//! ## 文本格式：导航正文与视频文案走 Markdown
//!
//! TS 在发这些文本之前都过一遍 `parseTextEntities`（`sendAnime.ts` / `anime/text.ts`），
//! 所以正文里的 `[01](https://t.me/c/...)`、`#标签` 到客户端是**可点的链接与标签**。
//! 这里对应地传 `Text::markdown`：simplify 内部也是走 `parseTextEntities`，
//! 解析失败会退回纯文本，不会因为一个没配对的方括号把整条消息发废。
//!
//! 纯提示语（审核提示、错误回话、`/addanime` 的进度）仍然是 `Text::plain`：
//! 那些文本是拿番剧名拼出来的，不该让名字里的 `*` / `_` 变成格式控制符。

use anime_core::logger::{Level, LogSink};
use fuyubot_abi::{HostApi, LogLevel};
use fuyubot_simplify::{
    EditCaptionParams, EditTextParams, InputFile, Media, ReplyButton, SendAlbumParams,
    SendMessageParams, Simplify, Text,
};
use serde_json::Value;

/// 等「发送成功」的上限：30 分钟。
///
/// TS 发视频也是 1800 秒（30 分钟）。一集 1080p 几个 GB，上传加转码远超
/// simplify 的默认值；超时不等于失败，但会把「还在传」误判成「失败了」。
pub const SEND_TIMEOUT_MS: u64 = 1800 * 1000;

/// 等「编辑/应答」这类小动作的上限：10 秒。
///
/// 不是发送——编辑只等一个响应，跟文件大小无关；给 30 分钟会把一次卡住的
/// 刷新拖成半小时不返回。
pub const EDIT_TIMEOUT_MS: u64 = 10 * 1000;

/// 一段视频的真实参数：`duration` / `width` / `height` / 遮罩 / 封面。
///
/// 以前全写 0：Telegram 自己也会算，但**算出来之前**客户端里那一集是
/// 「时长 0:00」且没有缩略图，要点开才知道。填上真实值还有一个好处——
/// `has_spoiler`（`r18`）本来就是靠这个结构带出去的。
#[derive(Debug, Clone, Default)]
pub struct VideoMeta {
    /// 秒。0 表示没探测出来。
    pub duration: i32,
    pub width: i32,
    pub height: i32,
    /// `r18`：封面打码。
    pub spoiler: bool,
    /// 封面图（本地 jpg 路径）。`None` 表示让 Telegram 自己抓一帧。
    pub cover: Option<String>,
}

/// 待发送的一段视频（相册的一项）。
#[derive(Debug, Clone, Default)]
pub struct VideoPart {
    pub path: String,
    pub meta: VideoMeta,
}

/// 一条已发出的消息：`id` + 视频的远程标识。
///
/// `videoid` / `unique_id` 是**发送结果**里才有的（`message.content.video.video.
/// remote`），旧版记资源时不填它们；TS 的 `extractAlbumMsgData` 要填——内联取视频
/// 与集数纠正都靠它定位文件，不必重新上传。
#[derive(Debug, Clone, Default)]
pub struct SentPart {
    pub message_id: i64,
    pub video_id: Option<String>,
    pub unique_id: Option<String>,
}

/// 从最终消息里挑出发送结果要用的三个字段。
fn sent_part(message: &Value) -> SentPart {
    let remote = message.pointer("/content/video/video/remote");
    SentPart {
        message_id: message.get("id").and_then(Value::as_i64).unwrap_or(0),
        video_id: remote
            .and_then(|remote| remote.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        unique_id: remote
            .and_then(|remote| remote.get("unique_id"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

/// 一条消息的定位：发给哪个会话、回复哪条、哪个话题。
#[derive(Debug, Clone, Copy)]
pub struct Target {
    pub chat_id: i64,
    /// 回复的消息 id；`0` 表示不回复（新发一条）。
    pub reply_to: i64,
    /// 话题/楼层（TDLib 的 `topic_id`）；`None` = 主楼。
    pub topic_id: Option<i64>,
}

impl Target {
    pub fn new(chat_id: i64, reply_to: i64) -> Self {
        Self {
            chat_id,
            reply_to,
            topic_id: None,
        }
    }

    /// 指定话题（论坛式群组/TDLib 的 topic）。
    pub fn with_topic(mut self, topic_id: i64) -> Self {
        if topic_id != 0 {
            self.topic_id = Some(topic_id);
        }
        self
    }
}

/// TDLib 门面。`Simplify` 是 `Copy` 的，所以这个结构也能随手复制。
#[derive(Debug, Clone, Copy)]
pub struct Td {
    host: HostApi,
    api: Simplify,
}

impl Td {
    pub fn new(host: HostApi) -> Self {
        Self {
            host,
            api: Simplify::new(host),
        }
    }

    /// 发纯文本。失败只记日志——一条回复发不出去不该带崩整条处理链。
    pub fn reply_text(&self, target: Target, text: impl Into<String>) -> bool {
        self.send(target, SendMessageParams::text(Text::plain(text)))
    }

    /// 发一张本地图片（番剧卡片走这里）。
    pub fn send_photo(
        &self,
        target: Target,
        path: impl Into<String>,
        caption: impl Into<String>,
    ) -> bool {
        let media = Media::photo(InputFile::local(path)).caption(Text::markdown(caption));
        self.send(target, SendMessageParams::media(media))
    }

    /// 发文本 + 内联键盘（审核提示用）。
    pub fn send_text_with_buttons(
        &self,
        target: Target,
        text: impl Into<String>,
        rows: Vec<Vec<ReplyButton>>,
    ) -> bool {
        let mut params = SendMessageParams::text(Text::plain(text));
        params.reply_markup = rows;
        self.send(target, params)
    }

    /// 改一条媒体消息的标题（导航卡的「已收录 N 集」走这里）。
    ///
    /// 只发请求不等结果：改标题失败不影响业务（下一条消息还会再改一次）。
    pub fn edit_caption(&self, chat_id: i64, message_id: i64, text: impl Into<String>) -> bool {
        let params = EditCaptionParams {
            text: Some(Text::markdown(text)),
            reply_markup: Vec::new(),
            timeout_ms: EDIT_TIMEOUT_MS,
        };
        match self.api.edit_message_caption(chat_id, message_id, &params) {
            Ok(_request) => true,
            Err(error) => {
                self.host.log(
                    LogLevel::Warn,
                    &format!("改标题失败（chat {chat_id} / msg {message_id}）：{error}"),
                );
                false
            }
        }
    }

    /// 发视频但**不等**确认（`telegram.wait_send = false` 时用）。
    ///
    /// 只投递、不拿 message_id：导航记账会因此跳过这一集，换来的是不阻塞。
    pub fn send_video_now(&self, target: Target, part: &VideoPart, caption: &str) -> bool {
        let media = video_media(part, Some(caption));
        self.send(target, SendMessageParams::media(media))
    }

    /// 发图但不等确认（同上）。
    pub fn send_photo_now(
        &self,
        target: Target,
        path: impl Into<String>,
        caption: impl Into<String>,
    ) -> bool {
        let media = Media::photo(InputFile::local(path)).caption(Text::markdown(caption));
        self.send(target, SendMessageParams::media(media))
    }

    /// 发图并**等到**最终消息，返回 message_id。仅供后台线程调用（会阻塞）。
    pub fn send_photo_wait(
        &self,
        target: Target,
        path: impl Into<String>,
        caption: impl Into<String>,
    ) -> Option<i64> {
        let media = Media::photo(InputFile::local(path)).caption(Text::markdown(caption));
        let mut params = SendMessageParams::media(media);
        params.reply_to_message_id = (target.reply_to != 0).then_some(target.reply_to);
        params.topic_id = target.topic_id;
        self.wait_sent(target.chat_id, &params).map(|sent| sent.message_id)
    }

    /// 发视频并等到最终消息（导航列表与资源记录都要 id）。
    ///
    /// 上传大文件要几十分钟，所以默认超时是 [SEND_TIMEOUT_MS]（30 分钟），
    /// 不是 simplify 的默认：默认那点时间不够传完一集 1080p。
    pub fn send_video_wait(&self, target: Target, part: &VideoPart, caption: &str) -> Option<SentPart> {
        let mut params = SendMessageParams::media(video_media(part, Some(caption)));
        params.reply_to_message_id = (target.reply_to != 0).then_some(target.reply_to);
        params.topic_id = target.topic_id;
        params.timeout_ms = SEND_TIMEOUT_MS;
        self.wait_sent(target.chat_id, &params)
    }

    /// 一组视频当**相册**发出去（2–10 段，TS 的 `sendMessageAlbum`）。
    ///
    /// 分段上传在 TS 里就是相册：一堆文件同属一集，逐条发会让频道里出现 N 条
    /// 散消息，而且只有第一条有文案。相册只有**首项带文案**，其余留空——
    /// `sendMessageAlbum` 也没有 `reply_markup`（真签名里没有这个字段）。
    ///
    /// 返回值与 `medias` **同序**（simplify 保证），所以 `sent[0]` 就是主消息。
    /// 任一项失败都会让整组失败（simplify 会撤回没发出去的），此时返回 `None`。
    pub fn send_video_album_wait(
        &self,
        target: Target,
        parts: &[VideoPart],
        caption: &str,
    ) -> Option<Vec<SentPart>> {
        let params = SendAlbumParams {
            medias: parts
                .iter()
                .enumerate()
                // 只有首项带文案：相册共用一段标题时，Telegram 会把文案挂在最后一项上。
                .map(|(index, part)| video_media(part, (index == 0).then_some(caption)))
                .collect(),
            caption: None,
            reply_to_message_id: (target.reply_to != 0).then_some(target.reply_to),
            topic_id: target.topic_id,
            timeout_ms: SEND_TIMEOUT_MS,
        };
        match self.api.send_message_album(target.chat_id, &params) {
            Ok(messages) => Some(messages.iter().map(sent_part).collect()),
            Err(error) => {
                self.host.log(
                    LogLevel::Warn,
                    &format!("发相册失败（chat {}，{} 段）：{error}", target.chat_id, parts.len()),
                );
                None
            }
        }
    }

    /// 发一条纯文本并等到确认，返回 message_id。
    ///
    /// 导航的**分页消息**要用它：分页消息要记进 `navVideoMessage`（带 page 与链接），
    /// 拿不到 id 就没法生成正文里的 `[第N页](link)`。
    pub fn send_text_wait(&self, target: Target, text: impl Into<String>) -> Option<i64> {
        let mut params = SendMessageParams::text(Text::markdown(text));
        params.reply_to_message_id = (target.reply_to != 0).then_some(target.reply_to);
        params.topic_id = target.topic_id;
        self.wait_sent(target.chat_id, &params).map(|sent| sent.message_id)
    }

    /// 改一条文本消息（`editMessageText`）。导航正文刷新用它改分页消息。
    ///
    /// 分页消息是**长文本**（4096 上限），不能走 `edit_caption`（那是图注，1024 上限）。
    pub fn edit_text(&self, chat_id: i64, message_id: i64, text: impl Into<String>) -> bool {
        let params = EditTextParams {
            text: Some(Text::markdown(text)),
            timeout_ms: EDIT_TIMEOUT_MS,
            ..EditTextParams::default()
        };
        match self.api.edit_message_text(chat_id, message_id, &params) {
            Ok(_) => true,
            Err(error) => {
                self.host.log(
                    LogLevel::Warn,
                    &format!("改文本失败（chat {chat_id} / msg {message_id}）：{error}"),
                );
                false
            }
        }
    }

    /// 发送并等到最终消息；失败只记日志（调用方据此降级）。
    ///
    /// `send_message` 本身会一路等到 `updateMessageSendSucceeded`，返回的已经是
    /// **最终消息**（不是临时那条），所以直接读 `id` 就行。
    fn wait_sent(&self, chat_id: i64, params: &SendMessageParams) -> Option<SentPart> {
        match self.api.send_message(chat_id, params) {
            Ok(message) => Some(sent_part(&message)),
            Err(error) => {
                self.host
                    .log(LogLevel::Warn, &format!("发送失败（chat {chat_id}）：{error}"));
                None
            }
        }
    }

    /// 底层出口：填好 `reply_to` 再交给 simplify，**不等结果**。
    ///
    /// 用 `*_then` 而不是 `send_message`：后者会阻塞到发送成功，而这里多半是在
    /// 更新回调里被调用的（回调线程就是接收循环那条），在那里等必然等到超时。
    fn send(&self, target: Target, mut params: SendMessageParams) -> bool {
        params.reply_to_message_id = (target.reply_to != 0).then_some(target.reply_to);
        params.topic_id = target.topic_id;
        let chat_id = target.chat_id;
        let host = self.host;
        self.api
            .send_message_then(chat_id, &params, move |outcome| {
                if let Err(error) = outcome {
                    host.log(
                        LogLevel::Warn,
                        &format!("发消息失败（chat {chat_id}）：{error}"),
                    );
                }
            });
        true
    }
}

/// 组装视频媒体：`duration/width/height` 用探测出的真实值，`cover` 用抽出来的那帧。
///
/// `caption` 为 `None` 时这项不带文案（相册里只有首项带）。
fn video_media(part: &VideoPart, caption: Option<&str>) -> Media {
    let meta = part.meta.clone();
    Media::Video {
        file: InputFile::local(part.path.clone()),
        caption: caption.map(Text::markdown),
        // 缩略图交给 Telegram 自己生成：我们已经给了 cover（那一帧的画面文件），
        // 再单独传一张缩略图只是多一次上传。
        thumbnail: None,
        cover: meta.cover.map(InputFile::local),
        // 不指定「从第几秒开始播」：TS 也没给，播放器默认从头开始。
        start_timestamp: None,
        duration: meta.duration.max(0),
        width: meta.width.max(0),
        height: meta.height.max(0),
        supports_streaming: true,
        has_spoiler: meta.spoiler,
    }
}

/// 领域层日志 → 宿主日志。
///
/// `anime-core` 不认识 `HostApi`（它要能脱离宿主单测），所以定了个 `LogSink` trait；
/// 这个结构就是那道桥。前缀 `[anime]` 是必要的：宿主日志里还有别的插件在说话。
pub struct HostLog {
    host: HostApi,
}

impl HostLog {
    pub fn new(host: HostApi) -> Self {
        Self { host }
    }
}

impl LogSink for HostLog {
    fn log(&self, level: Level, message: &str) {
        self.host
            .log(to_log_level(level), &format!("[anime] {message}"));
    }
}

/// 两侧的 level 枚举名字差不多但不是一个东西，映射集中放一处。
fn to_log_level(level: Level) -> LogLevel {
    match level {
        Level::Debug => LogLevel::Debug,
        Level::Info => LogLevel::Info,
        Level::Warn => LogLevel::Warn,
        Level::Error => LogLevel::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_domain_level_maps_to_a_host_level() {
        // 靠穷举匹配保证：哪边加了新 level，这里会编译不过。
        for level in [Level::Debug, Level::Info, Level::Warn, Level::Error] {
            let mapped = to_log_level(level);
            assert!(
                matches!(
                    mapped,
                    LogLevel::Debug | LogLevel::Info | LogLevel::Warn | LogLevel::Error
                ),
                "领域层的 {level:?} 没映射好"
            );
        }
    }

    #[test]
    fn sent_part_reads_id_and_remote_video_ids() {
        let message = serde_json::json!({
            "id": 42,
            "content": {
                "@type": "messageVideo",
                "video": { "video": { "remote": { "id": "AwAC", "unique_id": "AQAD" } } }
            }
        });
        let sent = sent_part(&message);
        assert_eq!(sent.message_id, 42);
        assert_eq!(sent.video_id.as_deref(), Some("AwAC"));
        assert_eq!(sent.unique_id.as_deref(), Some("AQAD"));
    }

    #[test]
    fn sent_part_survives_messages_without_video() {
        // 纯文本消息（导航分页）：没有 videoid 不该 panic。
        let sent = sent_part(&serde_json::json!({ "id": 7, "content": { "@type": "messageText" } }));
        assert_eq!(sent.message_id, 7);
        assert_eq!(sent.video_id, None);
        assert_eq!(sent.unique_id, None);
    }

    #[test]
    fn video_media_carries_probe_values_cover_and_spoiler() {
        let part = VideoPart {
            path: "D:/dl/01.mp4".to_owned(),
            meta: VideoMeta {
                duration: 1440,
                width: 1920,
                height: 1080,
                spoiler: true,
                cover: Some("D:/dl/cover.jpg".to_owned()),
            },
        };
        let media = video_media(&part, Some("某番｜第 01 集"));
        match media {
            Media::Video {
                caption,
                cover,
                duration,
                width,
                has_spoiler,
                ..
            } => {
                assert!(caption.is_some(), "首项要带文案");
                assert!(cover.is_some(), "封面要带出去");
                assert_eq!(duration, 1440);
                assert_eq!(width, 1920);
                assert!(has_spoiler, "r18 要带遮罩");
            }
            other => panic!("应当是视频：{other:?}"),
        }
        // 相册里的非首项不带文案。
        let second = video_media(&part, None);
        assert!(matches!(second, Media::Video { caption: None, .. }));
    }

    #[test]
    fn reply_to_zero_means_no_reply() {
        let target = Target::new(-100, 0);
        assert_eq!(target.reply_to, 0);
        assert_eq!(target.chat_id, -100);
    }

    #[test]
    fn target_keeps_both_ids() {
        let target = Target::new(-1001234567890, 42);
        assert_eq!(target.chat_id, -1001234567890);
        assert_eq!(target.reply_to, 42);
    }
}
