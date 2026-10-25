//! 命令路由。
//!
//! 一张静态表决定「有哪些命令、谁处理、`/help` 里怎么描述」。表是唯一真相：
//! 加命令只改 `COMMANDS`，`/help` 自动跟着变，不会出现「文档里写了但没实现」。
//!
//! 认不出的命令**静默忽略**——宿主里可能同时装着别的插件，抢别人的命令是更糟的错。

use std::sync::Arc;

use chrono::Utc;
use anime_core::bangumi::BgmClient;
use anime_core::card::bangumi::BangumiCard;
use anime_core::store::Storage;
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
        name: "about",
        summary: "这个机器人是干什么的",
        handler: cmd_about,
    },
    CommandSpec {
        name: "addanime",
        summary: "按 Bangumi 条目 id 导入/更新番剧与章节：/addanime <id>",
        handler: cmd_addanime,
    },
    CommandSpec {
        name: "animeblacklist",
        summary: "查看 RSS 黑名单关键词",
        handler: cmd_blacklist,
    },
    CommandSpec {
        name: "bgm",
        summary: "按关键词搜 Bangumi，给出可 /addanime 的条目 id",
        handler: cmd_bgm,
    },
    CommandSpec {
        name: "bindbangumi",
        summary: "绑定 Bangumi 账号：/bindbangumi <access_token>",
        handler: cmd_bindbangumi,
    },
    CommandSpec {
        name: "canceltask",
        summary: "取消下载（当前没有队列，仅报告状态）",
        handler: cmd_canceltask,
    },
    CommandSpec {
        name: "collect",
        summary: "给条目/章节标记 Bangumi 收藏状态",
        handler: cmd_collect,
    },
    CommandSpec {
        name: "exitbangumi",
        summary: "解绑当前账号的 Bangumi 绑定",
        handler: cmd_exitbangumi,
    },
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
        name: "progress",
        summary: "轮询与下载进度",
        handler: cmd_progress,
    },
    CommandSpec {
        name: "s",
        summary: "/searchanime 的短命令",
        handler: cmd_search,
    },
    CommandSpec {
        name: "searchanime",
        summary: "按关键词搜库里的番剧",
        handler: cmd_search,
    },
    CommandSpec {
        name: "selftest",
        summary: "一条命令自检：服务、库、RSS、匹配各跑一遍（不发消息）",
        handler: cmd_selftest,
    },
    CommandSpec {
        name: "setanimer18",
        summary: "设置番剧的 r18 标记：/setanimer18 <id> <true|false>",
        handler: cmd_set_r18,
    },
    CommandSpec {
        name: "status",
        summary: "运行状态：依赖的服务在不在、配置从哪来",
        handler: cmd_status,
    },
    CommandSpec {
        name: "testbgimg",
        summary: "用 Bangumi 条目 id 出一张番剧卡（调试出图）",
        handler: cmd_testbgimg,
    },
    CommandSpec {
        name: "updateanime",
        summary: "按 Bangumi 条目 id 刷新番剧与章节：/updateanime <id>",
        handler: cmd_updateanime,
    },
    CommandSpec {
        name: "web",
        summary: "查看管理端地址（配置里没开就没有）",
        handler: cmd_web,
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

thread_local! {
    /// 当前命令的发送者。命令处理函数是同步的，所以线程局部足够传递。
    static CURRENT_SENDER: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// 当前命令是谁发的（`None` = 频道身份或取不到）。
///
/// 处理函数用它做「这是不是我的 Bangumi 绑定」这类判断，不必改 16 个函数的签名。
pub fn current_sender() -> Option<i64> {
    CURRENT_SENDER.with(std::cell::Cell::get)
}

/// 只有主人能敲的命令（`telegram.owner_id`；为 0 时不校验，保持旧行为）。
const ADMIN_COMMANDS: &[&str] = &[
    "addanime",
    "updateanime",
    "setanimer18",
    "animeblacklist",
    "canceltask",
    "testbgimg",
    "bindbangumi",
    "exitbangumi",
];

/// 分发一条命令。返回 `true` 表示这次消息被本插件处理了。
pub fn dispatch(runtime: &Arc<Runtime>, command: &Command, sender: Option<i64>) -> bool {
    let Some(spec) = spec_for(&command.name) else {
        return false;
    };
    let owner = runtime.config.telegram.owner_id;
    if ADMIN_COMMANDS.contains(&spec.name) && owner != 0 && sender != Some(owner) {
        // 不回复：群里可能还有别的机器人在管同一批命令，沉默比报错干净。
        runtime.host.log(
            fuyubot_abi::LogLevel::Warn,
            &format!("[anime] 非主人执行管理命令 /{}（sender {sender:?}）", spec.name),
        );
        return true;
    }
    CURRENT_SENDER.with(|cell| cell.set(sender));
    (spec.handler)(runtime, command);
    CURRENT_SENDER.with(|cell| cell.set(None));
    true
}

/// `/updateanime <id>`：只刷新库里的条目与章节，不动导航消息。
///
/// 和 `/addanime` 的区别：那条会顺手发导航卡（第一次入库时有用），
/// 这条是维护命令——番剧信息变了（评分/集数/简介）刷一遍即可。
fn cmd_updateanime(runtime: &Arc<Runtime>, command: &Command) {
    let target = Target::new(command.chat_id, command.message_id);
    let Some(id) = command
        .args
        .split_whitespace()
        .next()
        .and_then(|raw| raw.parse::<u64>().ok())
    else {
        runtime
            .td
            .reply_text(target, "用法：/updateanime <bangumi 条目 id>");
        return;
    };
    let runtime = Arc::clone(runtime);
    std::thread::spawn(move || {
        let text = match refresh_anime(&runtime, id) {
            Ok((name, total)) => format!("已刷新「{name}」（{id}），章节 {total} 条"),
            Err(error) => format!("刷新失败：{error}"),
        };
        runtime.td.reply_text(target, text);
    });
}
/// `/addanime <bangumi_id>`：拉条目 + 章节写库。
///
/// TS 里 `addanime` 是「给已有番剧加种子」、`addnewanime` 是「新建番剧」。
/// 这里先合并成一条「按 id 导入/更新」：章节按 id upsert，重复执行只刷新内容。
fn cmd_addanime(runtime: &Arc<Runtime>, command: &Command) {
    let target = Target::new(command.chat_id, command.message_id);
    let Some(id) = command
        .args
        .split_whitespace()
        .next()
        .and_then(|raw| raw.parse::<u64>().ok())
    else {
        runtime
            .td
            .reply_text(target, "用法：/addanime <bangumi 条目 id>");
        return;
    };
    let runtime = Arc::clone(runtime);
    std::thread::spawn(move || {
        let text = match import_and_publish(&runtime, id) {
            Ok((name, total, nav)) => match nav {
                Some(nav) => format!(
                    "已导入「{name}」（{id}），章节 {total} 条；导航消息 {}（chat {}）",
                    nav.message_id, nav.chat_id
                ),
                None => format!("已导入「{name}」（{id}），章节 {total} 条；导航频道未配置，跳过发卡"),
            },
            Err(error) => format!("导入失败：{error}"),
        };
        runtime.td.reply_text(target, text);
    });
}

/// 拉 Bangumi 条目与章节写库。返回（展示名, 章节数）——不碰导航。
fn refresh_anime(runtime: &Runtime, id: u64) -> anime_core::Result<(String, usize)> {
    let (anime, _subject, _episodes, written) = save_subject(runtime, id)?;
    Ok((anime.display_name().to_owned(), written))
}

/// 拉取 + 写库的公共部分：返回写好的 anime 与章节数。
fn save_subject(
    runtime: &Runtime,
    id: u64,
) -> anime_core::Result<(
    anime_core::models::Anime,
    anime_core::bangumi::Subject,
    Vec<anime_core::bangumi::Episode>,
    usize,
)> {
    let client = BgmClient::new(runtime.config.bangumi.clone());
    let subject = client.subject(id)?;
    let episodes = client.episodes(id)?;
    let anime = subject.to_anime();
    runtime.store.save_anime(&anime)?;
    let metas: Vec<_> = episodes.iter().map(|episode| episode.to_meta(id)).collect();
    let written = runtime.store.upsert_episodes(id as i64, &metas)?;
    Ok((anime, subject, episodes, written))
}

/// 拉 Bangumi 条目与章节写库，然后发/复用导航消息。
///
/// 返回（展示名, 章节数, 导航消息）。导航频道没配时返回 None——这不是失败，
/// 只是这台机器上没开导航。
fn import_and_publish(
    runtime: &Runtime,
    id: u64,
) -> anime_core::Result<(String, usize, Option<crate::nav::NavRef>)> {
    let (anime, subject, episodes, written) = save_subject(runtime, id)?;

    // 已经有导航消息就复用：重复 /addanime 不该在频道里刷第二张卡。
    let nav = match runtime.store.anime_by_id(id as i64)? {
        Some(anime) => anime.nav_message,
        None => None,
    }
    .map(|nav| crate::nav::NavRef {
        chat_id: nav.chat_id,
        message_id: nav.message_id,
    });
    let nav = match nav {
        Some(nav) => Some(nav),
        None if runtime.config.telegram.nav_channel == 0 => None,
        None => Some(crate::nav::publish(runtime, &subject, &episodes)?),
    };
    Ok((anime.display_name().to_owned(), written, nav))
}
/// `/testbgimg <subject_id>`：拉 Bangumi 条目 → 出图 → 发回当前会话。
/// 网络 + 渲染都是慢操作，放线程里跑。
fn cmd_testbgimg(runtime: &Arc<Runtime>, command: &Command) {
    let target = Target::new(command.chat_id, command.message_id);
    let Some(id) = command
        .args
        .split_whitespace()
        .next()
        .and_then(|raw| raw.parse::<u64>().ok())
    else {
        runtime
            .td
            .reply_text(target, "用法：/testbgimg <bangumi 条目 id>");
        return;
    };
    let runtime = Arc::clone(runtime);
    std::thread::spawn(move || {
        match build_and_render(&runtime, id) {
            Ok((caption, path)) => {
                let sent = runtime
                    .td
                    .send_photo(target, path.display().to_string(), caption.clone());
                if !sent {
                    runtime
                        .td
                        .reply_text(target, format!("图片已生成但发送失败：{}", path.display()));
                }
            }
            Err(error) => {
                runtime
                    .td
                    .reply_text(target, format!("出图失败：{error}"));
            }
        }
    });
}

/// 拉条目 + 章节，转成卡片并渲染。返回（文案, 图片路径）。
fn build_and_render(
    runtime: &Runtime,
    id: u64,
) -> anime_core::Result<(String, std::path::PathBuf)> {
    let client = BgmClient::new(runtime.config.bangumi.clone());
    let subject = client.subject(id)?;
    let episodes = client.episodes(id)?;
    let card = crate::card::with_emoji(BangumiCard::from_subject(&subject, &episodes));
    let caption = format!("{} / {}（{}）", card.name_cn, card.name, card.id);
    let path = crate::card::render(runtime, &card)?;
    Ok((caption, path))
}
/// `/about`：TS 里这是 `/start` 的一大段欢迎语；宿主已有内置 start 插件，
/// 抢它的命令只会让用户收到两条回复，所以换个名字。
fn cmd_about(runtime: &Arc<Runtime>, command: &Command) {
    runtime.td.reply_text(
        Target::new(command.chat_id, command.message_id),
        "XQAnimeBot｜把种子发到群里，或等 RSS 自动抓\n\
         抓到后会解析番名、对齐集数，命中库里的番就直接下载并投到频道。\n\
         管理命令：/progress /searchanime /status /help",
    );
}

/// `/web`：管理端默认不启动（`http.listen` 为空），所以这里只说事实。
fn cmd_web(runtime: &Arc<Runtime>, command: &Command) {
    let http = &runtime.config.http;
    let text = if http.listen.trim().is_empty() {
        "Web 管理端未启用（配置里 http.listen 为空）".to_owned()
    } else {
        let token = if http.token.trim().is_empty() {
            "未设置访问令牌".to_owned()
        } else {
            "令牌已配置（这里不回显）".to_owned()
        };
        format!("管理端：http://{}\n{token}", http.listen.trim())
    };
    runtime
        .td
        .reply_text(Target::new(command.chat_id, command.message_id), text);
}

/// `/progress`：轮询累计 + 当前在下载的条目数。
fn cmd_progress(runtime: &Arc<Runtime>, command: &Command) {
    let (rounds, items, usable, failures) = runtime.poll_stats.snapshot();
    let text = format!(
        "轮询：{rounds} 轮｜{items} 条｜可解析 {usable} 条｜源失败 {failures} 次\n\
         下载中：{} 条（上限 3）",
        crate::chain::active()
    );
    runtime
        .td
        .reply_text(Target::new(command.chat_id, command.message_id), text);
}

/// `/searchanime`、`/s`：查库是 IO，放到线程里，别堵住命令回调线程。
fn cmd_search(runtime: &Arc<Runtime>, command: &Command) {
    let keyword = command.args.trim().to_owned();
    if keyword.chars().count() < 2 {
        runtime.td.reply_text(
            Target::new(command.chat_id, command.message_id),
            "用法：/searchanime 关键词（至少 2 个字）",
        );
        return;
    }
    let runtime = Arc::clone(runtime);
    let target = Target::new(command.chat_id, command.message_id);
    std::thread::spawn(move || {
        let text = match runtime.store.search_anime(&keyword) {
            Ok(list) if list.is_empty() => format!("没有找到「{keyword}」"),
            Ok(list) => {
                let mut text = format!("找到 {} 部：\n", list.len());
                for anime in list.iter().take(10) {
                    let episode = anime.episode.as_deref().unwrap_or("-");
                    text.push_str(&format!(
                        "· {}（{}）共 {} 集\n",
                        anime.display_name(),
                        anime.id,
                        episode
                    ));
                }
                text
            }
            Err(error) => format!("搜索失败：{error}"),
        };
        runtime.td.reply_text(target, text);
    });
}

/// `/bindbangumi <access_token>`：校验 token（调 `/v0/me`）后写 `bangumi_users`。
///
/// TS 版走 OAuth 授权页 + 回调；那要求一个能接 redirect 的 HTTP 端点。这里先用
/// 「粘贴 access token」的等价做法：token 从 https://next.bgm.tv/demo/access-token 生成。
fn cmd_bindbangumi(runtime: &Arc<Runtime>, command: &Command) {
    let target = Target::new(command.chat_id, command.message_id);
    let token = command.args.trim().to_owned();
    if token.is_empty() {
        runtime.td.reply_text(
            target,
            "用法：/bindbangumi <bangumi access_token>\n可在 https://next.bgm.tv/demo/access-token 生成",
        );
        return;
    }
    let Some(tg_user_id) = current_sender() else {
        runtime
            .td
            .reply_text(target, "无法识别你的 Telegram 账号：请在私聊里绑定");
        return;
    };
    let runtime = Arc::clone(runtime);
    std::thread::spawn(move || {
        let mut config = runtime.config.bangumi.clone();
        config.access_token = token.clone();
        let client = BgmClient::new(config);
        let text = match client.me() {
            Ok(user) => {
                let doc = anime_core::models::BangumiUserDoc {
                    tg_user_id,
                    id: user.id as i64,
                    username: user.username.clone(),
                    nickname: user.nickname.clone(),
                    avatar: Some(user.avatar.best().to_owned()).filter(|url| !url.is_empty()),
                    access_token: token.clone(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                };
                match runtime.store.bangumi_user_upsert(&doc) {
                    Ok(()) => format!(
                        "已绑定 Bangumi 账号：{}（{}）",
                        if user.nickname.is_empty() { &user.username } else { &user.nickname },
                        user.id
                    ),
                    Err(error) => format!("写入绑定失败：{error}"),
                }
            }
            Err(error) => format!("校验 access token 失败：{error}"),
        };
        runtime.td.reply_text(target, text);
    });
}

/// `/exitbangumi`：删掉当前 Telegram 用户的绑定。
fn cmd_exitbangumi(runtime: &Arc<Runtime>, command: &Command) {
    let target = Target::new(command.chat_id, command.message_id);
    let Some(tg_user_id) = current_sender() else {
        runtime
            .td
            .reply_text(target, "无法识别你的 Telegram 账号：请在私聊里解绑");
        return;
    };
    let text = match runtime.store.bangumi_user_delete(tg_user_id) {
        Ok(0) => "你还没有绑定 Bangumi 账号".to_owned(),
        Ok(_) => "已解绑 Bangumi 账号".to_owned(),
        Err(error) => format!("解绑失败：{error}"),
    };
    runtime.td.reply_text(target, text);
}
/// `/bgm <关键词>`：搜 Bangumi 并列出 id，方便接着 `/addanime <id>`。
///
/// 本地 `/searchanime` 只搜已入库的番剧；要『先查外部再导入』就得走这条。
fn cmd_bgm(runtime: &Arc<Runtime>, command: &Command) {
    let target = Target::new(command.chat_id, command.message_id);
    let keyword = command.args.trim().to_owned();
    if keyword.is_empty() {
        runtime.td.reply_text(target, "用法：/bgm <关键词>");
        return;
    }
    let runtime = Arc::clone(runtime);
    std::thread::spawn(move || {
        let client = BgmClient::new(runtime.config.bangumi.clone());
        let text = match client.search(&keyword) {
            Ok(list) if list.is_empty() => format!("Bangumi 没有找到「{keyword}」"),
            Ok(list) => {
                let mut text = format!("Bangumi 找到 {} 部：\n", list.len());
                for subject in list.iter().take(8) {
                    text.push_str(&format!(
                        "· {} / {}（{}）\n",
                        subject.name_cn, subject.name, subject.id
                    ));
                }
                text.push_str("\n用 /addanime <id> 导入。");
                text
            }
            Err(error) => format!("搜索 Bangumi 失败：{error}"),
        };
        runtime.td.reply_text(target, text);
    });
}
/// `/animeblacklist`：只读展示。增删改需要写 config 集合，等审核链路一起做。
fn cmd_blacklist(runtime: &Arc<Runtime>, command: &Command) {
    let target = Target::new(command.chat_id, command.message_id);
    let text = match runtime.store.config_list("animeBlacklist") {
        Ok(list) if list.is_empty() => "黑名单为空".to_owned(),
        Ok(list) => format!("黑名单 {} 条：\n{}", list.len(), list.join("、")),
        Err(error) => format!("读取黑名单失败：{error}"),
    };
    runtime.td.reply_text(target, text);
}

/// `/collect <条目id> [章节id]`：弹一个五种收藏状态的键盘。
///
/// 对应 TS 视频消息下面那排收藏按钮——出图/导航还没接，先用命令把入口给出来。
fn cmd_collect(runtime: &Arc<Runtime>, command: &Command) {
    let target = Target::new(command.chat_id, command.message_id);
    let mut parts = command.args.split_whitespace();
    let subject = parts.next().and_then(|raw| raw.parse::<u64>().ok());
    let episode = parts.next().and_then(|raw| raw.parse::<u64>().ok());
    let Some(subject) = subject else {
        runtime.td.reply_text(
            target,
            "用法：/collect <bangumi 条目 id> [章节 id]",
        );
        return;
    };
    let text = match episode {
        Some(episode) => format!("给条目 {subject} / 章节 {episode} 标记收藏状态："),
        None => format!("给条目 {subject} 标记收藏状态："),
    };
    runtime.td.send_text_with_buttons(
        target,
        text,
        crate::callback::collection_buttons(subject, episode),
    );
}
/// `/canceltask`：目前没有任务队列（下载是 detached 线程），只能报告在跑的条数。
/// 真正的取消要等 checkpoint/队列落地——那时才可能「取消某一条」。
fn cmd_canceltask(runtime: &Arc<Runtime>, command: &Command) {
    let active = crate::chain::active();
    let text = if active == 0 {
        "当前没有下载中的任务".to_owned()
    } else {
        format!("有 {active} 条正在下载；暂不支持单独取消（等任务队列落地）")
    };
    runtime
        .td
        .reply_text(Target::new(command.chat_id, command.message_id), text);
}

/// `/selftest`：把「服务 / 库 / RSS / 匹配」四条线跑一遍，只在会话里回结果。
///
/// 存在的意义：真机联调时不用等 RSS 命中才能确认链路通——这一条命令就能看出
/// 是哪个环节坏了（插件没装？库空的？RSS 抓不到？匹配全落空？）。
/// **不发任何业务消息**，只回一条纯文本。
fn cmd_selftest(runtime: &Arc<Runtime>, command: &Command) {
    let runtime = Arc::clone(runtime);
    let target = Target::new(command.chat_id, command.message_id);
    std::thread::spawn(move || {
        let mut lines: Vec<String> = Vec::new();

        // 1) 四个基础服务在不在
        let services = ["storage.mongo", "download.qbittorrent", "media.ffmpeg", "image.render"]
            .iter()
            .map(|name| {
                let present = !runtime.host.get_service(name).is_null();
                format!("{} {name}", if present { "✓" } else { "✗" })
            })
            .collect::<Vec<_>>()
            .join("　");
        lines.push(format!("服务：{services}"));

        // 2) 库：候选番剧（只查一次：Mongo 挂着时每次查询要等 30s 超时）。
        let candidates = match runtime.store.anime_candidates() {
            Ok(candidates) => {
                lines.push(format!("库：{} 部番剧", candidates.len()));
                candidates
            }
            Err(error) => {
                lines.push(format!("库：读取失败（{error}）"));
                Vec::new()
            }
        };

        // 3) RSS：真抓一轮（不发消息）
        // 自检只取少量条目：bangumi 每条要打 3 次 API，全量跑一轮能到分钟级，
        // 自检的目的是「通不通」，不是「跑完整轮」。
        let mut probe_config = runtime.config.rss.clone();
        probe_config.max_items = probe_config.max_items.min(25);
        let mut seen = crate::fetch::Seen::default();
        let summary = crate::fetch::run(
            &probe_config,
            &runtime.config.ai,
            &runtime.logger,
            &mut seen,
        );
        lines.push(format!(
            "RSS：{} 条，可解析 {} 条，源失败 {}",
            summary.total,
            summary.usable,
            summary.sources.iter().filter(|line| line.error.is_some()).count()
        ));

        // 4) 匹配：拿库里已有番剧跑一遍，看有几条能对上
        let mut matched = 0usize;
        let mut samples: Vec<String> = Vec::new();
        for item in &summary.items {
            if let anime_core::matching::SeasonDecision::Matched(hit) =
                anime_core::matching::decide_season_with(
                    &item.names,
                    &candidates,
                    anime_core::matching::MatchPolicy::ts(),
                )
            {
                matched += 1;
                if samples.len() < 3 {
                    samples.push(format!("{} → {}", item.title, hit.matched_name));
                }
            }
        }
        lines.push(format!("匹配：命中 {matched} 条"));
        for sample in samples {
            lines.push(format!("  · {sample}"));
        }

        runtime.td.reply_text(target, lines.join("\n"));
    });
}
/// `/setanimer18 <id> <true|false>`：只改标记，不动其它字段。
fn cmd_set_r18(runtime: &Arc<Runtime>, command: &Command) {
    let target = Target::new(command.chat_id, command.message_id);
    let mut parts = command.args.split_whitespace();
    let id = parts.next().and_then(|raw| raw.parse::<i64>().ok());
    let flag = parts.next().and_then(|raw| match raw.to_ascii_lowercase().as_str() {
        "true" | "1" | "on" => Some(true),
        "false" | "0" | "off" => Some(false),
        _ => None,
    });
    let (Some(id), Some(flag)) = (id, flag) else {
        runtime.td.reply_text(
            target,
            "用法：/setanimer18 <番剧 id> <true|false>",
        );
        return;
    };
    let update = serde_json::json!({
        "$set": { "r18": flag, "updatedAt": chrono::Utc::now() }
    });
    let text = match runtime
        .store
        .update("anime", serde_json::json!({ "id": id }), update, false, false)
    {
        Ok(_) => format!("已设置 r18={flag}（id {id}）"),
        Err(error) => format!("更新失败：{error}"),
    };
    runtime.td.reply_text(target, text);
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
    fn admin_commands_are_declared() {
        // 名单里写错一个名字不会报错，只会让那条命令悄悄变成「谁都能敲」——用测试钉住。
        for name in ADMIN_COMMANDS {
            assert!(
                spec_for(name).is_some(),
                "管理命令 {name} 不在 COMMANDS 表里"
            );
        }
    }
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
