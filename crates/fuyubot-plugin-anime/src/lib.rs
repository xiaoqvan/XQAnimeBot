//! XQAnimeBot 的宿主桥。
//!
//! 这个 crate 只做四件事：
//!
//! 1. 把宿主事件翻成 `anime-core` 的调用（`bot`）；
//! 2. 把 `anime-core` 的结果翻成 TDLib 请求（`td`，实际映射在 `fuyubot-simplify`）；
//! 3. 维护命令 / 回调路由；
//! 4. 持有运行时状态（配置、连接、任务句柄）；
//! 5. **养一条后台 RSS 轮询线程**（`poll`）。
//!
//! 第 5 条是插件的“心跳”：对应用户看不到的那部分。TS 里它是 `anime/index.ts`
//! 的 `while (true)` 主循环，不是任何命令——用户不敲任何东西，它也在抓。
//!
//! **业务判断一律不写在这里**。宿主层能做的只有「收到什么、回什么」，
//! 「这条种子属于哪部番、该不该投」是 `anime-core` 的事——那边能纯 `cargo test`。

mod bot;
mod callback;
mod card;
mod chain;
mod fetch;
mod media;
mod nav;
mod poll;
mod qb;
mod review;
mod runtime;
mod store;
mod td;

use std::sync::Arc;

use fuyubot_abi::{HostApi, LogLevel, Plugin, define_plugin};
use fuyubot_simplify::{CommandSubscription, on_command};

use crate::poll::Poller;
use crate::runtime::{Bootstrap, Runtime};

/// 插件标识，宿主日志里用它区分来源。
pub const PLUGIN_NAME: &str = "XQAnimeBot";

/// 日志前缀，和 `PLUGIN_NAME` 分开：前者是身份，后者是给人看的短名。
const TAG: &str = "anime";

struct AnimePlugin {
    host: HostApi,
    /// 启动成功后才有值；`init` 阶段还没读配置。
    runtime: Option<Arc<Runtime>>,
    /// 订阅守卫：`drop` 就退订，所以 `stop` 里只要把它置空。
    subscription: Option<CommandSubscription>,
    /// 后台 RSS 轮询。`drop` = 请求停止，所以 `stop` 里只要把它置空。
    poller: Option<Poller>,
    /// 内联键盘回调订阅。`drop` 就退订，同上。
    callback: Option<callback::CallbackSubscription>,
}

impl Plugin for AnimePlugin {
    fn init(host: HostApi) -> Result<Self, String> {
        host.log(LogLevel::Info, &format!("[{TAG}] init｜{PLUGIN_NAME}"));
        Ok(Self {
            host,
            runtime: None,
            subscription: None,
            poller: None,
            callback: None,
        })
    }

    fn start(&mut self, config_json: &str) -> Result<(), String> {
        let (config, source) = Bootstrap::parse(config_json).resolve()?;
        let runtime = Runtime::new(self.host, config, source);
        runtime.log_boot();

        // 回调闭包持有 `Arc<Runtime>`：插件实例本身不可克隆，而回调要求 `'static`。
        let shared = Arc::clone(&runtime);
        let subscription = on_command(self.host, move |command| {
            // 不认识的命令静默放过——同一进程里可能还有别的插件在处理。
            let _ = bot::dispatch(&shared, command);
        })
        .ok_or_else(|| "订阅 updateNewMessage 失败（宿主没接事件总线？）".to_owned())?;

        let poller = Poller::spawn(Arc::clone(&runtime), Arc::clone(&runtime.poll_stats));
        // 回调订阅失败不致命：没有审核按钮，其余功能照跑。
        let callback = callback::spawn(self.host, Arc::clone(&runtime));
        if callback.is_none() {
            self.host
                .log(LogLevel::Warn, "订阅 updateNewCallbackQuery 失败，审核按钮不可用");
        }

        self.subscription = Some(subscription);
        self.runtime = Some(runtime);
        self.poller = Some(poller);
        self.callback = callback;
        self.host.log(
            LogLevel::Info,
            &format!(
                "[{TAG}] 就绪｜{} 条命令已注册，RSS 轮询已启动",
                bot::COMMANDS.len()
            ),
        );
        Ok(())
    }

    fn stop(&mut self) {
        // 顺序：先退订（不再接新消息），再停轮询，最后丢运行时。
        // 丢在最前面会让还在跑的回调拿到 `None`。
        self.subscription = None;
        self.callback = None;
        self.poller = None;
        self.runtime = None;
        self.host.log(LogLevel::Info, &format!("[{TAG}] stop"));
    }
}

define_plugin!(AnimePlugin, "xq.anime");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_is_stable() {
        // 宿主日志与文档都引用这个字符串，改名等于换插件身份。
        assert_eq!(PLUGIN_NAME, "XQAnimeBot");
    }

    #[test]
    fn command_table_is_not_empty() {
        assert!(!bot::COMMANDS.is_empty(), "至少要有 /ping 证明链路是通的");
    }

    #[test]
    fn rss_polling_is_background_not_a_command() {
        // 这条断言锁的是「形态」：原项目里 RSS 抓取是 `anime/index.ts` 的后台主循环，
        // 用户不敲命令它也在跑。曾经这里有个手动的 `/rss`，属实是把后台任务
        // 降级成了手动触发器——别再把它加回来。
        assert!(
            bot::spec_for("rss").is_none(),
            "RSS 是后台轮询，不该做成命令"
        );
    }
}
