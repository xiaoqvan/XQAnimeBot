//! 命令订阅：在 simplify 的 `on_command` 之上多带一个「谁发的」。
//!
//! `fuyubot-simplify` 的 [Command] 只有 chat/message/name/args，没有发送者——而
//! 管理命令必须知道是谁在敲（TS 版靠它做 admin 校验）。ABI 暂时没把 sender 放进
//! 公共结构，所以这里自己订阅 `updateNewMessage`，用 [command_of] 解析命令、
//! 顺手把 `sender_id.user_id` 取出来。
//!
//! 语义与 `on_command` 保持一致：非命令 / 自己发的更新直接忽略，返回 0。

use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;

use fuyubot_abi::{HostApi, SubscriptionHandle};
use fuyubot_simplify::{Command, command_of};
use serde_json::Value;

use crate::bot;
use crate::runtime::Runtime;

pub const COMMAND_EVENT: &str = "updateNewMessage";

/// 订阅命令。返回 `None` 表示宿主拒绝订阅（没接事件总线）。
pub fn spawn(host: HostApi, runtime: Arc<Runtime>) -> Option<CommandSubscription> {
    let mut owned: Box<Box<dyn Fn(&Command, Option<i64>) + Send + Sync>> =
        Box::new(Box::new(move |command: &Command, sender: Option<i64>| {
            bot::dispatch(&runtime, command, sender);
        }));
    let user_data = (&raw mut *owned).cast::<c_void>();
    let handle = host.on(COMMAND_EVENT, None, command_trampoline, user_data);
    handle.is_valid().then_some(CommandSubscription {
        host,
        handle,
        _handler: owned,
    })
}

/// 守卫：drop 就退订（和 simplify 的 `CommandSubscription` 语义相同）。
pub struct CommandSubscription {
    host: HostApi,
    handle: SubscriptionHandle,
    _handler: Box<Box<dyn Fn(&Command, Option<i64>) + Send + Sync>>,
}

impl Drop for CommandSubscription {
    fn drop(&mut self) {
        if self.handle.is_valid() {
            self.host.off(self.handle);
        }
    }
}

/// 从 `updateNewMessage` 里取发送者 user id。
///
/// 频道发言的 sender 是 `messageSenderChat`（没有 user_id），这种一律返回 None——
/// 管理命令在频道里敲本来也不该生效。
fn sender_of(update: &Value) -> Option<i64> {
    update
        .get("message")?
        .get("sender_id")?
        .get("user_id")?
        .as_i64()
}

extern "C-unwind" fn command_trampoline(user_data: *mut c_void, payload: *const c_char) -> i32 {
    if user_data.is_null() || payload.is_null() {
        return -1;
    }
    // SAFETY: `user_data` 是 [spawn] 里那个 Box，订阅活多久它就活多久。
    let handler = unsafe { &*(user_data as *const Box<dyn Fn(&Command, Option<i64>) + Send + Sync>) };
    // SAFETY: 宿主保证 `payload` 是 NUL 结尾的临时字符串。
    let text = unsafe { CStr::from_ptr(payload) }.to_string_lossy();
    let Ok(update) = serde_json::from_str::<Value>(&text) else {
        return 0;
    };
    if let Some(command) = command_of(&update) {
        handler(&command, sender_of(&update));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sender_is_read_from_message_sender() {
        let update = json!({
            "message": {
                "id": 1,
                "sender_id": { "@type": "messageSenderUser", "user_id": 42 }
            }
        });
        assert_eq!(sender_of(&update), Some(42));
    }

    #[test]
    fn channel_sender_has_no_user_id() {
        let update = json!({
            "message": { "id": 1, "sender_id": { "@type": "messageSenderChat", "chat_id": -100 } }
        });
        assert!(sender_of(&update).is_none());
    }
}
