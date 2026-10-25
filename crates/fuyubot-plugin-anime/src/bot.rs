//! 命令路由。
//!
//! 一张静态表决定「有哪些命令、谁处理、`/help` 里怎么描述」。表是唯一真相：
//! 加命令只改 `COMMANDS`，`/help` 自动跟着变，不会出现「文档里写了但没实现」。
//!
//! 认不出的命令**静默忽略**——宿主里可能同时装着别的插件，抢别人的命令是更糟的错。

use std::sync::Arc;

use chrono::Utc;
use fuyubot_simplify::Command;

use crate::poll;
use crate::runtime::Runtime;
use crate::td::Target;

/// 命令处理函数。同步：TDLib 的投递接口本身不阻塞，真需要等待就自己开线程。
pub type Handler = fn(&Arc<Runtime>, &Command);

/// 一条命令的声明。
pub struct CommandSpec {
    /// 命令名，不带 `/`，全小写。
    pub name: &'static str,
    /// `/help` 里的一行说明。
    pub summary: &'static str,
    pub handler: Handler,
}

/// 全部业务命令。**按字母序**排，`/help` 直接照这个顺序输出。
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "help",
        summary: "看看我能做什么",
        handler: cmd_help,
    },
    CommandSpec {
        name: "ping",
        summary: "测活：插件还在不在、跑了多久",
        handler: cmd_ping,
    },
    CommandSpec {
        name: "status",
        summary: "运行状态：依赖的服务在不在、配置从哪来",
        handler: cmd_status,
    },
];

/// 按名字查命令声明（不区分大小写）。
///
/// 单独拆出来是因为「这条命令归不归我」是纯查表，没有 `Runtime` 也能测；
/// `dispatch` 只是在这之上补一次执行。
pub fn spec_for(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS
        .iter()
        .find(|spec| spec.name.eq_ignore_ascii_case(name))
}

/// 分发一条命令。返回 `true` 表示这次消息被本插件处理了。
pub fn dispatch(runtime: &Arc<Runtime>, command: &Command) -> bool {
    let Some(spec) = spec_for(&command.name) else {
        return false;
    };
    (spec.handler)(runtime, command);
    true
}

fn cmd_ping(runtime: &Arc<Runtime>, command: &Command) {
    // 这里**不报延迟**：TDLib 的投递是异步的，插件侧拿不到真实往返时间，
    // 硬编一个数字比不报更误导。回答「我还活着、活了多久」就够。
    runtime.td.reply_text(
        Target::new(command.chat_id, command.message_id),
        format!("pong｜插件已运行 {}", runtime.uptime()),
    );
}

fn cmd_status(runtime: &Arc<Runtime>, command: &Command) {
    let config = &runtime.config;
    let source = runtime.config_source.as_ref().map_or_else(
        || "内置默认值".to_owned(),
        |path| path.display().to_string(),
    );

    // 依赖服务是否已在宿主里注册——比任何自检都直接：缺了就说明对应插件没装/没起来。
    let services = [
        "storage.mongo",
        "download.qbittorrent",
        "media.ffmpeg",
        "image.render",
    ]
    .iter()
    .map(|name| {
        let present = !runtime.host.get_service(name).is_null();
        format!("{} {name}", if present { "✓" } else { "✗" })
    })
    .collect::<Vec<_>>()
    .join("　");

    let (rounds, items, usable, failures) = runtime.poll_stats.snapshot();
    let mut tally = format!("{rounds} 轮 / {items} 条，可解析 {usable} 条");
    if failures > 0 {
        tally.push_str(&format!("，{failures} 次源失败"));
    }

    let text = format!(
        "XQAnimeBot 运行中\n\
         ── 版本：{}\n\
         ── 运行：{}\n\
         ── 配置：{source}\n\
         ── RSS：{}\n\
         ── 累计：{tally}\n\
         ── 频道：番剧 {}｜导航 {}\n\
         ── 服务：{services}",
        env!("CARGO_PKG_VERSION"),
        runtime.uptime(),
        poll::interval_label(config.rss.interval_secs, Utc::now()),
        config.telegram.anime_channel,
        config.telegram.nav_channel
    );
    runtime
        .td
        .reply_text(Target::new(command.chat_id, command.message_id), text);
}

fn cmd_help(runtime: &Arc<Runtime>, command: &Command) {
    let mut text = String::from("可用命令：\n");
    for spec in COMMANDS {
        text.push_str(&format!("/{} —— {}\n", spec.name, spec.summary));
    }
    text.push_str("\n先把种子发到群里，剩下的交给它。");
    runtime
        .td
        .reply_text(Target::new(command.chat_id, command.message_id), text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_command_is_found() {
        for spec in COMMANDS {
            assert!(
                spec_for(spec.name).is_some(),
                "表里的命令要能被查到：{}",
                spec.name
            );
            assert!(
                spec_for(&spec.name.to_uppercase()).is_some(),
                "命令名不该区分大小写：{}",
                spec.name
            );
        }
    }

    #[test]
    fn unknown_command_is_not_claimed() {
        assert!(spec_for("definitely_not_ours").is_none());
        assert!(spec_for("").is_none());
    }

    #[test]
    fn builtin_plugins_keep_their_commands() {
        // 同一进程里 /start /help /plugins 都可能有别的插件在管。`/help` 我们自己也提供，
        // 但语义必须一致（列命令），所以这里只锁「不抢别人的」。
        assert!(spec_for("start").is_none(), "/start 归 start 插件");
        assert!(spec_for("plugins").is_none(), "/plugins 归 manage 插件");
    }

    #[test]
    fn command_table_is_sorted_and_unique() {
        let names: Vec<&str> = COMMANDS.iter().map(|spec| spec.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "命令表按字母序排，/help 才不会乱");
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "命令名不能重复");
    }

    #[test]
    fn every_command_has_a_summary() {
        for spec in COMMANDS {
            assert!(!spec.summary.is_empty(), "命令 {} 缺说明", spec.name);
        }
    }
}
