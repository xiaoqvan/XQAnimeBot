//! 权限：谁能敲管理命令。
//!
//! 对应 TS 的 `isUserAdmin(client, ADMIN_GROUP_ID, sender)` + `config.super_admin`：
//! 两个条件**任一命中**即可——
//!
//! ```text
//! super_admin（配置里的 user id）           → 是管理员
//! admin_group_id 里是 creator/administrator → 是管理员
//! owner_id（本仓旧配置的兜底）             → 是管理员
//! ```
//!
//! ## 为什么要开线程
//!
//! `getChatMember` 是**异步** TDLib 方法，宿主这边要 `invoke` + 等响应，也就是
//! 会阻塞调用线程。命令是从更新回调里进来的（那条线程就是接收循环），
//! 在那里等网络请求会把整个插件的收消息卡住。所以校验只在 worker 线程里跑，
//! [`dispatch_admin`] 负责开线程，[`is_admin`] 自己不做线程方面的事。

use anime_core::config::TelegramConfig;
use anime_core::log;
use fuyubot_abi::LogLevel;
use fuyubot_tdlib_ext::TdExt;
use serde_json::Value;

use crate::runtime::Runtime;

/// TDLib 的 `chatMemberStatus` 里算「管理员」的两个 `@type`。
///
/// `chatMemberStatusMember` 不算：管理群里普通成员一大堆，把管理命令开放给他们
/// 等于没做权限。
const ADMIN_STATUSES: [&str; 2] = ["chatMemberStatusCreator", "chatMemberStatusAdministrator"];

/// 这个 Telegram 用户能敲管理命令吗？
///
/// **会阻塞**（见模块注释）：调用方必须在 worker 线程里调它。
/// 取不到成员信息（群没配 / 机器人不在群里 / 超时）一律按「不是管理员」处理，
/// 并把原因写进日志——权限判错是安全事故，但**判断失败**时保守更安全。
pub fn is_admin(runtime: &Runtime, user_id: i64) -> bool {
    let telegram = &runtime.config.telegram;
    if is_super_admin(telegram, user_id) {
        return true;
    }
    let group = telegram.admin_group_id;
    if group == 0 {
        // 没配管理群：只剩上面那两条 id 白名单。这里不能退化成「放行」。
        return false;
    }
    match TdExt::new(runtime.host).get_chat_member(group, user_id) {
        Ok(member) => {
            let status = member
                .pointer("/status/@type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            // `left` / `kicked` / `banned` 都会被 TDLib 塞进 status，
            // 所以是白名单而不是「非 member 就算管理员」。
            ADMIN_STATUSES.contains(&status)
        }
        Err(error) => {
            log!(
                &runtime.logger,
                Warn,
                "查管理群成员失败（group {group}，user {user_id}）：{error}"
            );
            false
        }
    }
}

/// 配置里直接点名的两个 id：`super_admin`（TS 有）与 `owner_id`（本仓兜底）。
///
/// 单独拆出来：`is_admin` 的群成员查询要开线程，而这两条是纯配置比较，
/// 同步就能判——能不走网络就不走。
pub fn is_super_admin(telegram: &TelegramConfig, user_id: i64) -> bool {
    (telegram.super_admin != 0 && telegram.super_admin == user_id)
        || (telegram.owner_id != 0 && telegram.owner_id == user_id)
}

/// 记一条「非管理员试图执行管理命令」。
///
/// 不回复对方（TS 也是直接 `return`）：群里可能还有别的机器人在管同一批命令，
/// 沉默比「你的权限不足」干净。
pub fn log_denied(runtime: &Runtime, command: &str, sender: Option<i64>) {
    runtime.host.log(
        LogLevel::Warn,
        &format!("[anime] 非管理员执行管理命令 /{command}（sender {sender:?}）"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn telegram(super_admin: i64, owner_id: i64) -> TelegramConfig {
        TelegramConfig {
            super_admin,
            owner_id,
            ..Default::default()
        }
    }

    #[test]
    fn super_admin_and_owner_are_recognized_by_config_alone() {
        // 这两条是纯配置比较，不必查群成员：能不走网络就不走。
        let config = telegram(111, 222);
        assert!(is_super_admin(&config, 111));
        assert!(is_super_admin(&config, 222));
        assert!(!is_super_admin(&config, 333));
    }

    #[test]
    fn zero_ids_mean_unset_not_everyone() {
        // 两个都留 0 时谁都不是超管：0 不是合法的 Telegram user id，
        // 退化成「放行」会让没配权限的部署门户大开。
        let config = telegram(0, 0);
        assert!(!is_super_admin(&config, 0));
        assert!(!is_super_admin(&config, 1));
    }

    #[test]
    fn admin_statuses_are_a_whitelist() {
        // 普通成员、已退群、被踢都不算管理员。
        assert!(ADMIN_STATUSES.contains(&"chatMemberStatusCreator"));
        assert!(ADMIN_STATUSES.contains(&"chatMemberStatusAdministrator"));
        assert!(!ADMIN_STATUSES.contains(&"chatMemberStatusMember"));
        assert!(!ADMIN_STATUSES.contains(&"chatMemberStatusLeft"));
    }
}
