//! TDLib 调用封装。
//!
//! 业务代码只说「回这条消息」「把这张图发到那个会话」，不碰 `sendMessage` 的
//! `input_message_content` 结构——那层映射交给 `fuyubot-simplify`，它直接读
//! `td_api.tl` 生成的类型，是唯一一份真相。
//!
//! **不要在更新回调里同步等响应**：`send_message` 只是投递，立刻返回；真正等
//! 结果的那几个 `*_wait_success` 会阻塞投递线程，回调里用等于自己卡自己。

use fuyubot_abi::{HostApi, LogLevel};
use fuyubot_simplify::{SendMessageParams, Simplify, Text};

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

#[cfg(test)]
mod tests {
    use super::*;

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
