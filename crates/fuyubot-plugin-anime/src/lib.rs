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
mod commands;
mod card;
mod chain;
mod fetch;
mod media;
mod nav;
mod poll;
mod newanime;
mod prompts;
mod reply;
mod qb;
mod review;
mod runtime;
mod store;
mod td;
mod text;
mod web;

use std::sync::Arc;

use fuyubot_abi::{HostApi, LogLevel, Plugin, define_plugin};


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
    subscription: Option<commands::CommandSubscription>,
    /// 后台 RSS 轮询。`drop` = 请求停止，所以 `stop` 里只要把它置空。
    poller: Option<Poller>,
    /// 内联键盘回调订阅。`drop` 就退订，同上。
    callback: Option<callback::CallbackSubscription>,
    /// 管理 API。`drop` 就停监听。
    web: Option<web::WebServer>,
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
            web: None,
        })
    }

    fn start(&mut self, config_json: &str) -> Result<(), String> {
        // 配置三层来源：内联 config_toml → 宿主全局 [plugin.xq.anime] → 内置默认值。
        // 全局配置经 host.get_config 读整段 JSON，再用环境变量覆盖凭据。
        let global_json = self.host.get_config("xq.anime", "");
        let (config, source) =
            Bootstrap::parse(config_json)
                .resolve_with_env(global_json.as_deref())
                .map_err(|error| {
                    // 宿主只会说「plugin_start 返回 -1」，把真正的原因写进日志。
                    self.host.log(
                        LogLevel::Error,
                        &format!("[{TAG}] 启动失败（配置）：{error}"),
                    );
                    error
                })?;
        let runtime = Runtime::new(self.host, config, source);
        runtime.log_boot();

        // 自建订阅而不是用 simplify 的 on_command：管理命令要知道「谁发的」。
        let subscription = commands::spawn(self.host, Arc::clone(&runtime))
            .ok_or_else(|| "订阅 updateNewMessage 失败（宿主没接事件总线？）".to_owned())?;

        let poller = Poller::spawn(Arc::clone(&runtime), Arc::clone(&runtime.poll_stats));
        // 管理 API 没配 listen 时 spawn 返回 None，属于正常情况。
        let web = web::spawn(Arc::clone(&runtime));
        // 回调订阅失败不致命：没有审核按钮，其余功能照跑。
        let callback = callback::spawn(self.host, Arc::clone(&runtime));
        if callback.is_none() {
            self.host
                .log(LogLevel::Warn, "订阅 updateNewCallbackQuery 失败，审核按钮不可用");
        }

        // 用本插件的 [mongodb] 驱动 storage.mongo（云端连接串走这里）。
        // 最多等 3 秒让它生效：否则第一轮轮询可能落在切换前的空库上。
        configure_storage(Arc::clone(&runtime));
        for _ in 0..30 {
            if STORAGE_READY.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // 恢复上次没跑完的任务（检查点）。放在轮询启动之后：恢复是补作业，
        // 不该挡住插件进入就绪状态。
        recover_tasks(Arc::clone(&runtime));

        self.subscription = Some(subscription);
        self.runtime = Some(runtime);
        self.poller = Some(poller);
        self.callback = callback;
        self.web = web;
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
        self.web = None;
        self.poller = None;
        self.runtime = None;
        self.host.log(LogLevel::Info, &format!("[{TAG}] stop"));
    }
}

define_plugin!(AnimePlugin, "xq.anime");

/// `storage.mongo` 是否已经按本插件配置就绪（启动时最多等 3 秒）。
static STORAGE_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 用本插件的 `[mongodb]` 配置去驱动 `storage.mongo`。
///
/// 为什么要重试：插件加载顺序不保证（实测 anime 常先于 mongo 起来），一次性调用
/// 会找不到服务。这里最多等 30 秒，等到就配、配完退出。
fn configure_storage(runtime: Arc<Runtime>) {
    use fuyubot_service::{call_service, service_available};
    let mongo = runtime.config.mongodb.clone();
    if mongo.uri.trim().is_empty() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("xq-anime-mongo".to_owned())
        .spawn(move || {
            for _ in 0..15 {
                if service_available(&runtime.host, "storage.mongo", "configure") {
                    let input = serde_json::json!({
                        "uri": mongo.uri,
                        "db": mongo.database,
                        "timeout_ms": mongo.timeout_ms,
                    });
                    let _ = call_service(&runtime.host, "storage.mongo", "configure", &input);
                    // 读回真实状态：mongo 插件自己的 start 可能晚一步把库改回去，
                    // 所以「调用成功」不算数，以 ping 回来的 db 为准。
                    if let Ok(info) = call_service(&runtime.host, "storage.mongo", "ping", &serde_json::json!({})) {
                        let db = info["db"].as_str().unwrap_or_default();
                        let uri = info["uri"].as_str().unwrap_or_default();
                        if db == mongo.database {
                            if uri != mongo.uri {
                                runtime.host.log(
                                    LogLevel::Warn,
                                    &format!(
                                        "[{TAG}] storage.mongo 的连接串是 {uri}（由宿主配置建立），[mongodb].uri={} 不生效；库已切到 {db}",
                                        mongo.uri
                                    ),
                                );
                            }
                            runtime.host.log(
                                LogLevel::Info,
                                &format!("[{TAG}] storage.mongo 就绪：库 {db}（来自 [mongodb] 配置）"),
                            );
                            STORAGE_READY.store(true, std::sync::atomic::Ordering::Release);
                            return;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            runtime.host.log(
                LogLevel::Warn,
                &format!(
                    "[{TAG}] 15 秒内没能把 storage.mongo 切到库 {}，仍用宿主配置（可用 MONGO_URI/MONGO_DB）",
                    mongo.database
                ),
            );
            // 失败也要放行：不让第一轮轮询无限等下去。
            STORAGE_READY.store(true, std::sync::atomic::Ordering::Release);
        });

    if spawned.is_err() {
        // 起不来就退回宿主配置，不影响插件本身。
    }
}


/// 同一个任务最多重放几次。超过就判失败：崩溃—重启—再崩溃的死循环比丢一条种子更糟。
const MAX_RESUME_ATTEMPTS: i32 = 3;

/// 把 `task_checkpoints` 里没跑完的 rss 任务重新塞回下载队列。
///
/// 只认 kind=rss 的检查点：web/mkv 队列还没实现，恢复它们只会造出「假装在跑」的任务。
/// 读库是同步调用，放线程里；失败只记日志——恢复不了不该让插件起不来。
fn recover_tasks(runtime: Arc<Runtime>) {
    let spawned = std::thread::Builder::new()
        .name("xq-anime-recover".to_owned())
        .spawn(move || {
            use anime_core::models::{AnimeItem, CheckpointKind, CheckpointStatus};
            use anime_core::store::Storage;

            let checkpoints = match runtime.store.checkpoint_resumable() {
                Ok(checkpoints) => checkpoints,
                Err(error) => {
                    runtime
                        .host
                        .log(LogLevel::Warn, &format!("读取检查点失败，跳过恢复：{error}"));
                    return;
                }
            };
            for checkpoint in checkpoints {
                if checkpoint.kind != CheckpointKind::Rss {
                    continue;
                }
                if checkpoint.attempt >= MAX_RESUME_ATTEMPTS {
                    let _ = runtime
                        .store
                        .checkpoint_finish(&checkpoint.key, CheckpointStatus::Failed);
                    runtime.host.log(
                        LogLevel::Warn,
                        &format!(
                            "任务 {} 已重放 {} 次，放弃",
                            checkpoint.key, checkpoint.attempt
                        ),
                    );
                    continue;
                }
                let item = checkpoint
                    .payload
                    .get("item")
                    .cloned()
                    .and_then(|value| serde_json::from_value::<AnimeItem>(value).ok());
                let Some(item) = item else {
                    let _ = runtime
                        .store
                        .checkpoint_finish(&checkpoint.key, CheckpointStatus::Failed);
                    runtime.host.log(
                        LogLevel::Warn,
                        &format!("检查点 {} 的 payload 还原失败，放弃", checkpoint.key),
                    );
                    continue;
                };
                if let Err(error) = runtime.store.checkpoint_bump_attempt(&checkpoint.key) {
                    runtime
                        .host
                        .log(LogLevel::Warn, &format!("增加重放次数失败：{error}"));
                }
                runtime.host.log(
                    LogLevel::Info,
                    &format!(
                        "恢复任务：{}（第 {} 次重放，阶段 {}）",
                        checkpoint.key,
                        checkpoint.attempt + 1,
                        checkpoint.stage
                    ),
                );
                chain::spawn(Arc::clone(&runtime), item, 0);
            }
        });
    if spawned.is_err() {
        // 线程起不来就是「不恢复」，插件照常启动。
    }
}

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
