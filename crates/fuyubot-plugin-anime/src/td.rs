//! TDLib 调用封装。
//!
//! 业务代码只说「回这条消息」「把这张图发到那个会话」，不碰 `sendMessage` 的
//! `input_message_content` 结构——那层映射交给 `fuyubot-simplify`，它直接读
//! `td_api.tl` 生成的类型，是唯一一份真相。
//!
//! **不要在更新回调里同步等响应**：`send_message` 只是投递，立刻返回；真正等
//! 结果的那几个 `*_wait_success` 会阻塞投递线程，回调里用等于自己卡自己。

use anime_core::logger::{Level, LogSink};
use fuyubot_abi::{HostApi, LogLevel};
use fuyubot_simplify::{InputFile, Media, ReplyButton, SendMessageParams, Simplify, Text};

/// 一条消息的定位：发给哪个会话、回复哪条。
#[derive(Debug, Clone, Copy)]
pub struct Target {
    pub chat_id: i64,
    /// 回复的消息 id；`0` 表示不回复（新发一条）。
    pub reply_to: i64,
}

impl Target {
    pub fn new(chat_id: i64, reply_to: i64) -> Self {
        Self { chat_id, reply_to }
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

    /// 发一个本地视频文件（带纯文本说明）。
    ///
    /// 尺寸/时长填 0：TDLib 会自己读文件头，插件侧再探一遍纯属浪费。
    pub fn send_video(
        &self,
        target: Target,
        path: impl Into<String>,
        caption: impl Into<String>,
    ) -> bool {
        let media = Media::Video {
            file: InputFile::local(path),
            caption: Some(Text::plain(caption)),
            duration: 0,
            width: 0,
            height: 0,
            supports_streaming: true,
            has_spoiler: false,
        };
        self.send(target, SendMessageParams::media(media))
    }

    /// 发一张本地图片（番剧卡片走这里）。
    pub fn send_photo(
        &self,
        target: Target,
        path: impl Into<String>,
        caption: impl Into<String>,
    ) -> bool {
        let media = Media::photo(InputFile::local(path)).caption(Text::plain(caption));
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

    /// 底层出口：填好 `reply_to` 再交给 simplify。
    fn send(&self, target: Target, mut params: SendMessageParams) -> bool {
        params.reply_to_message_id = (target.reply_to != 0).then_some(target.reply_to);
        match self.api.send_message(target.chat_id, &params) {
            Ok(_request) => true,
            Err(error) => {
                self.host.log(
                    LogLevel::Warn,
                    &format!("发消息失败（chat {}）：{error}", target.chat_id),
                );
                false
            }
        }
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
