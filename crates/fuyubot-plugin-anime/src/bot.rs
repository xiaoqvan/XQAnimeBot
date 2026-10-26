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
use anime_core::models::AnimeItem;
use anime_core::store::Storage;
use fuyubot_simplify::Command;

use crate::perm;
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
        summary: "给已有番剧补种：/addanime <章节 id> <bangumi.moe/dmhy 链接>",
        handler: cmd_addanime,
    },
    CommandSpec {
        name: "addnewanime",
        summary: "新建番剧并补种：/addnewanime <章节 id> <bangumi.moe/dmhy 链接>",
        handler: cmd_addnewanime,
    },
    CommandSpec {
        name: "aimatch",
        summary: "用 LLM Agent 匹配番剧（调试用）：/aimatch <标题> [集数]",
        handler: cmd_aimatch,
    },
    CommandSpec {
        name: "animeblacklist",
        summary: "RSS 黑名单关键词（list/add/remove/clear）",
        handler: cmd_blacklist,
    },
    CommandSpec {
        name: "bgm",
        summary: "按关键词搜 Bangumi，给出可 /addanime 的条目 id",
        handler: cmd_bgm,
    },
    CommandSpec {
        name: "bindbangumi",
        summary: "绑定 Bangumi 账号：不带参数走 OAuth 授权；也可 /bindbangumi <access_token>",
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
        name: "jz",
        summary: "单集纠正：/jz <源番剧 id> <集数> <目标番剧 id> [字幕组]",
        handler: cmd_jz,
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
        name: "start",
        summary: "深链入口：Bangumi 授权回调 / 收藏 / 标记看过（/start 后带参数才响应）",
        handler: cmd_start,
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
    "jz",
    "testbgimg",
    "bindbangumi",
    "exitbangumi",
];

/// 分发一条命令。返回 `true` 表示这次消息被本插件处理了。
pub fn dispatch(runtime: &Arc<Runtime>, command: &Command, sender: Option<i64>) -> bool {
    let Some(spec) = spec_for(&command.name) else {
        return false;
    };
    if !ADMIN_COMMANDS.contains(&spec.name) {
        run(runtime, spec, command, sender);
        return true;
    }
    // 管理命令：先认发送者。取不到（频道身份）直接拒——TS 也只认
    // `messageSenderUser`，宁可漏判也不放行一个身份不明的来源。
    let Some(user_id) = sender else {
        perm::log_denied(runtime, spec.name, sender);
        return true;
    };
    // 配置里点名的两个 id 不必查群成员，省一次 TDLib 往返。
    if perm::is_super_admin(&runtime.config.telegram, user_id) {
        run(runtime, spec, command, sender);
        return true;
    }
    // 查群成员要等 TDLib 响应，而这里跑在接收循环那条线程上——开线程做。
    let worker = Arc::clone(runtime);
    let command = command.clone();
    let spawned = std::thread::Builder::new()
        .name("xq-anime-cmd".to_owned())
        .spawn(move || {
            if !perm::is_admin(&worker, user_id) {
                perm::log_denied(&worker, spec.name, sender);
                return;
            }
            run(&worker, spec, &command, sender);
        });
    if let Err(error) = spawned {
        runtime.host.log(
            fuyubot_abi::LogLevel::Error,
            &format!("[anime] 管理命令线程起不来（/{}）：{error}", spec.name),
        );
    }
    true
}

/// 真正执行一条命令：把发送者放进线程局部，跑完再清空。
///
/// 拆出来是因为 `dispatch` 有三条分支都要做同样这件事，而 `CURRENT_SENDER`
/// 是**线程局部**——所以「设置 → 执行 → 清空」必须在同一个线程里，
/// 管理命令那条分支因此把整个 `run` 搬进了 worker 线程。
fn run(runtime: &Arc<Runtime>, spec: &'static CommandSpec, command: &Command, sender: Option<i64>) {
    CURRENT_SENDER.with(|cell| cell.set(sender));
    (spec.handler)(runtime, command);
    CURRENT_SENDER.with(|cell| cell.set(None));
}

/// 从用户给的链接里取出一条种子（TS `anime/get.ts` + `parseInfo`）。
///
/// 两家的字段来源不同，但产出同一个 [`AnimeItem`]：后面的下载 / 发送 / 记资源
/// 只认这个结构，不关心它是从哪抓来的。
fn fetch_bt_item(url: &str) -> std::result::Result<AnimeItem, String> {
    use anime_core::torrent::{self, TorrentSource};

    let (title, magnet, team, extra_names, pub_date) = match torrent::parse_source(url) {
        None => return Err("目前仅支持 bangumi.moe 和 dmhy 的链接".to_owned()),
        Some(TorrentSource::Bangumi { id }) => {
            let info = torrent::fetch_bangumi_torrent(&id)
                .map_err(|error| format!("取种子信息失败：{error}"))?;
            let fansub = torrent::fansub_from_title(&info.title);
            if fansub.is_empty() {
                return Err(format!("标题里没有字幕组标记：{}", info.title));
            }
            // 发布组：有 team_id 就问 API，没有就用标题里的第一个字幕组
            // （TS 同逻辑——自建种子往往没有 team）。
            let team = match info.team_id.as_deref() {
                Some(team_id) if !team_id.is_empty() => torrent::fetch_bangumi_team(team_id)
                    .ok()
                    .and_then(|teams| teams.into_iter().next())
                    .map(|team| team.name)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| fansub[0].clone()),
                _ => fansub[0].clone(),
            };
            let extra_names = if info.tag_ids.is_empty() {
                Vec::new()
            } else {
                torrent::fetch_bangumi_tags(&info.tag_ids)
                    .map(|tags| torrent::locale_names(&tags))
                    .unwrap_or_default()
            };
            (info.title, info.magnet, team, extra_names, info.pub_date)
        }
        Some(TorrentSource::Dmhy { url }) => {
            let info = torrent::fetch_dmhy_torrent(&url)
                .map_err(|error| format!("取种子信息失败：{error}"))?;
            (info.title, info.magnet, info.team, Vec::new(), None)
        }
    };
    if magnet.trim().is_empty() {
        return Err("这个链接里没有磁力".to_owned());
    }
    let fansub = torrent::fansub_from_title(&title);
    if fansub.is_empty() {
        return Err(format!("标题里没有字幕组标记：{title}"));
    }
    // LoliHouse 的简繁标记会让番名解析命中不了规则：TS 建库前先换掉。
    let title = if team == "LoliHouse" {
        title.replace("[简繁内封字幕]", "[简体内嵌]")
    } else {
        title
    };
    let parsed = anime_core::title::parse_info(&title, Some(&team))
        .ok_or_else(|| format!("没有适用于发布组「{team}」的标题规则：{title}"))?;
    // 译名：解析出的名字 + 标签里的多语言名，去重去空（TS 同逻辑）。
    let mut names: Vec<String> = Vec::new();
    for name in parsed.names.iter().chain(extra_names.iter()) {
        let name = name.trim();
        if !name.is_empty() && !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    if names.is_empty() {
        return Err(format!("解析不出番剧名：{title}"));
    }
    Ok(AnimeItem {
        title,
        pub_date: pub_date.unwrap_or_default(),
        magnet,
        link: url.to_owned(),
        team,
        fansub,
        names,
        source: (!parsed.source.is_empty()).then_some(parsed.source.clone()),
        episode: (!parsed.episode.is_empty()).then_some(parsed.episode.clone()),
    })
}

/// `/addanime <章节 id> <链接>` / `/addnewanime <章节 id> <链接>` 的公共部分。
///
/// 两者只差一件事：`create` 为真时**先建档**（番剧还不在库里），
/// 之后下载 / 发送 / 记资源 / 刷导航完全一样——都由 `chain` 负责。
///
/// 先发一条「正在添加」的提示，最后再把那条消息改成结果：
/// 下载 + 上传要几十分钟，不给回执的话管理员会以为命令没生效。
fn add_bt(runtime: &Arc<Runtime>, command: &Command, create: bool) {
    let target = Target::new(command.chat_id, command.message_id);
    let mut parts = command.args.split_whitespace();
    let episode_id = parts.next().and_then(|raw| raw.parse::<u64>().ok());
    let url = parts.next().map(str::to_owned);
    let (Some(episode_id), Some(url)) = (episode_id, url) else {
        runtime.td.reply_text(
            target,
            format!(
                "用法：/{} <章节 id> <链接>\n示例：/{} 12345 https://bangumi.moe/torrent/xxxx",
                command.name, command.name
            ),
        );
        return;
    };

    let worker = Arc::clone(runtime);
    // `command` 是借用，线程要 'static：整条拷一份进去。
    let command = command.clone();
    let spawned = std::thread::Builder::new()
        .name("xq-anime-add".to_owned())
        .spawn(move || {
            let tips_id = worker
                .td
                .send_text_wait(target, "正在添加 BT 信息，请稍候…");
                    let result = add_bt_inner(&worker, episode_id, &url, create);
            let text = match result {
                Ok(summary) => summary,
                Err(error) => format!("添加失败：{error}"),
            };
            // 改掉那条「正在添加」；拿不到它的 id（没等确认）就新发一条。
            match tips_id {
                Some(id) if worker.td.edit_text(command.chat_id, id, &text) => {}
                _ => {
                    worker.td.reply_text(target, text);
                }
            }
        });
    if let Err(error) = spawned {
        runtime.host.log(
            fuyubot_abi::LogLevel::Error,
            &format!("[anime] /{} 线程起不来：{error}", command.name),
        );
    }
}

/// 真正干活：章节 → 条目 →（可选）建档 → 抓种子 → 交给 chain。
fn add_bt_inner(
    runtime: &Arc<Runtime>,
    episode_id: u64,
    url: &str,
    create: bool,
) -> std::result::Result<String, String> {
    use anime_core::store::Storage;

    let client = anime_core::bangumi::BgmClient::new(runtime.config.bangumi.clone());
    // 章节 id → 它属于哪部番：命令收的是章节 id（TS 同），不是条目 id。
    let episode = client
        .episode(episode_id)
        .map_err(|error| format!("取章节信息失败：{error}"))?;
    let subject_id = episode.subject_id as i64;
    let mut anime = runtime
        .store
        .anime_by_id(subject_id)
        .map_err(|error| format!("读库失败：{error}"))?;
    if anime.is_none() && !create {
        return Err(format!(
            "未找到 ID 为 {subject_id} 的动漫信息。请先 /addnewanime 建档，或确认章节 id 是否正确。"
        ));
    }
    if anime.is_none() {
        // 建档（TS 的 buildAndSaveAnimeFromInfo）：拉条目 + 章节一起写库。
        let (created, _subject, _episodes, _written) = save_subject(runtime, subject_id as u64)
            .map_err(|error| format!("建档失败：{error}"))?;
        anime = Some(created);
    }
    let anime = anime.expect("上面两条路必定填入");
    let display = crate::prompts::display(&anime).to_owned();

    let item = fetch_bt_item(url)?;
    // 下载 + 发送 + 记资源 + 刷导航全交给 chain：它有限流、有断点、会发封面。
    crate::chain::spawn_with(
        Arc::clone(runtime),
        item.clone(),
        crate::chain::Plan::for_episode(subject_id, episode_id as i64),
    );
    Ok(format!(
        "已为动漫 {display} 添加 BT 信息，正在下载并发送：{}\n\n如需修改，再用 /addanime。",
        item.title
    ))
}

/// `/addanime <章节 id> <链接>`：给**已在库里**的番剧补一条种子。
fn cmd_addanime(runtime: &Arc<Runtime>, command: &Command) {
    add_bt(runtime, command, false);
}

/// `/addnewanime <章节 id> <链接>`：番剧不在库里时用它——先建档再补种。
fn cmd_addnewanime(runtime: &Arc<Runtime>, command: &Command) {
    add_bt(runtime, command, true);
}

/// `/updateanime <bangumi 条目 id>`：拉最新条目与章节写库。
///
/// 库里还没有这部番时它同时也是**导入**命令：顺手把导航卡发出来
/// （`/addnewanime` 走的是「章节 id + 链接」，那条要先有种子）。
/// 已经有导航消息时复用，不会在频道里刷第二张卡。
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
        let text = match import_and_publish(&runtime, id) {
            Ok((name, total, nav)) => match nav {
                Some(nav) => format!(
                    "已刷新「{name}」（{id}），章节 {total} 条；导航消息 {}（chat {}）",
                    nav.message_id, nav.chat_id
                ),
                None => format!("已刷新「{name}」（{id}），章节 {total} 条；导航频道未配置，跳过发卡"),
            },
            Err(error) => format!("刷新失败：{error}"),
        };
        runtime.td.reply_text(target, text);
    });
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
    let mut anime = subject.to_anime();
    // 同 newanime：标签在写库前过滤（TS 的 extractFilteredTagNames）。
    anime.tags = crate::text::filter_tag_names(&anime.tags);
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
        // 不给参数 = 走 OAuth：发一条带「前往授权」按钮的消息（TS 同行为）。
        // 给了参数 = 老办法：直接粘一个 access token（手工生成的那种）。
        send_auth_button(runtime, target);
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
                // `id` 是内部自增主键（由 store 分配），Bangumi 用户 id 放 `bgmUserId`；
                // username/nickname/avatar 是 API 返回值，TS 不落库，这里也不存。
                let doc = anime_core::models::BangumiUserDoc {
                    tg_user_id,
                    bgm_user_id: Some(user.id as i64),
                    access_token: Some(token.clone()),
                    ..Default::default()
                };
                match runtime.store.bangumi_user_upsert(&doc) {
                    Ok(_) => format!(
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

/// `/bindbangumi` 的 OAuth 分支：建/取绑定记录 → 拼授权页 → 发 URL 按钮。
///
/// `state` 用库里的**内部自增 id**（`bangumi_user_upsert` 返回）：Bangumi 回调时
/// 把它原样带回来，才能知道这次授权属于哪个 Telegram 用户。
fn send_auth_button(runtime: &Arc<Runtime>, target: Target) {
    use anime_core::store::Storage;

    let Some(tg_user_id) = current_sender() else {
        runtime
            .td
            .reply_text(target, "无法识别你的 Telegram 账号：请在私聊里绑定");
        return;
    };
    if runtime.config.bangumi.app_id.trim().is_empty() {
        runtime.td.reply_text(
            target,
            "未配置 bangumi.app_id：请先在配置里填好 OAuth 应用凭据，\
             或直接 /bindbangumi <access_token> 手工绑定",
        );
        return;
    }
    // upsert 只为拿到（或复用）内部 id：此时还没有 token，只写 tgUserId。
    let doc = anime_core::models::BangumiUserDoc {
        tg_user_id,
        ..Default::default()
    };
    let state = match runtime.store.bangumi_user_upsert(&doc) {
        Ok(id) => id,
        Err(error) => {
            runtime
                .td
                .reply_text(target, format!("创建绑定记录失败：{error}"));
            return;
        }
    };
    let url = anime_core::bangumi::oauth::authorize_url(
        &runtime.config.bangumi.app_id,
        &runtime.config.bangumi.redirect_uri,
        &state.to_string(),
    );
    let text = format!(
        "请点击下方按钮进行 Bangumi 账户授权\n\n注：\n\
         > Bangumi 用于管理 ACG 收藏与收视进度\n\
         > Bangumi 不提供资源下载与观看\n\
         我们使用 Bangumi 的 API 接口进行收藏与进度管理"
    );
    let _ = runtime.td.send_text_with_buttons(
        target,
        text,
        vec![vec![fuyubot_simplify::ReplyButton::url("前往 Bangumi 授权页面", url)]],
    );
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
/// `/aimatch <标题> [集数]`：拿 LLM Agent 实跑一次番剧匹配（调试用）。
///
/// 为什么要这条命令：Agent 的匹配质量只有真跑才知道（规则命中率、LLM 迭代几轮、
/// 置信度分布），而这些在没有真 RSS 数据时看不出来。它同时也是「给了 API key 之后
/// 第一件事」——一条命令就能确认链路通不通。
fn cmd_aimatch(runtime: &Arc<Runtime>, command: &Command) {
    let target = Target::new(command.chat_id, command.message_id);
    let args = command.args.trim().to_owned();
    if args.is_empty() {
        runtime
            .td
            .reply_text(target, "用法：/aimatch <BT 标题> [集数]");
        return;
    }

    // 最后一个参数如果是纯数字，当成集数（其余都是标题）。
    let mut pieces: Vec<&str> = args.split_whitespace().collect();
    let trailing_is_number = pieces
        .last()
        .is_some_and(|last| !last.is_empty() && last.chars().all(|ch| ch.is_ascii_digit()));
    let explicit_episode = if trailing_is_number {
        pieces.pop().map(str::to_owned)
    } else {
        None
    };
    let title = pieces.join(" ");

        // 没启用 AI 就直说，别让管理员对着置信度 0 猜是配置没开还是模型不给面子。
        if !runtime.config.ai.usable() {
            let reason = if runtime.config.ai.enabled {
                "api_key 为空"
            } else {
                "ai.enabled = false"
            };
            runtime.td.reply_text(
                target,
                format!(
                    "AI 未启用（{reason}）。请在 `fuyubot.toml` 的 `[plugin.xq.anime.ai]` 段\n\
                     填写 enabled / api_key / base_url / model。"
                ),
            );
            return;
        }

        let runtime = Arc::clone(runtime);
        std::thread::spawn(move || {
            use anime_core::ai::{BangumiAgent, MatchInput};
            use anime_core::bangumi::{BgmClient, BgmTools};

        // 番名候选：按字幕组规则解析；没有组标签（或组不在规则表里）就退回整条标题。
        let team = anime_core::rss::fansub_of(&title).into_iter().next();
        let (names, episode) = match anime_core::title::parse_info(&title, team.as_deref()) {
            Some(info) if info.is_usable() => {
                let episode = (info.episode != "未知").then_some(info.episode);
                (info.names, episode)
            }
            _ => (vec![title.clone()], None),
        };
        let episode = explicit_episode.or(episode);

        let tools = BgmTools::new(BgmClient::new(runtime.config.bangumi.clone()));
        let agent = BangumiAgent::new(runtime.config.ai.clone(), tools);
        let input = MatchInput {
            title: title.clone(),
            names,
            episode: episode.clone(),
        };
        let result = agent.match_subject(&input, 1);

        let text = format!(
            "标题：{title}\n番名：{}\n集数：{}\n── 结果 ──\n条目：{}\n章节：{}（第 {} 集）\n置信度：{:.2}\n理由：{}",
            input.names.join("、"),
            episode.as_deref().unwrap_or("(未解析)"),
            result
                .subject_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "(无)".to_owned()),
            result
                .episode_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "(无)".to_owned()),
            result
                .episode_sort
                .map(|sort| sort.to_string())
                .unwrap_or_else(|| "-".to_owned()),
            result.confidence,
            result.reason
        );
        runtime.td.reply_text(target, text);
    });
}

/// `/animeblacklist`：黑名单关键词增删查。存在 MongoDB 的 config 集合
/// `{key:"animeBlacklist", list:[...]}`，与 TS 版完全一致，接老库直接互通。
fn cmd_blacklist(runtime: &Arc<Runtime>, command: &Command) {
    use anime_core::store::Storage;
    let target = Target::new(command.chat_id, command.message_id);

    let mut parts = command.args.split_whitespace();
    let sub = parts.next().unwrap_or("").to_lowercase();
    let keyword = parts.collect::<Vec<_>>().join(" ");

    let text = match sub.as_str() {
        "list" | "" => match runtime.store.config_list("animeBlacklist") {
            Ok(list) if list.is_empty() => "黑名单为空".to_owned(),
            Ok(list) => format!("黑名单 {} 条：\n{}", list.len(), list.join("、")),
            Err(error) => format!("读取黑名单失败：{error}"),
        },
        "add" => {
            if keyword.is_empty() {
                "用法：/animeblacklist add <关键词>".to_owned()
            } else {
                match runtime.store.config_list_add("animeBlacklist", &keyword) {
                    Ok(true) => format!("已添加黑名单关键词：{keyword}"),
                    Ok(false) => format!("关键词「{keyword}」已在黑名单中"),
                    Err(error) => format!("添加失败：{error}"),
                }
            }
        }
        "remove" | "delete" => {
            if keyword.is_empty() {
                "用法：/animeblacklist remove <关键词>".to_owned()
            } else {
                match runtime.store.config_list_remove("animeBlacklist", &keyword) {
                    Ok(true) => format!("已移除黑名单关键词：{keyword}"),
                    Ok(false) => format!("关键词「{keyword}」不在黑名单中"),
                    Err(error) => format!("移除失败：{error}"),
                }
            }
        }
        "clear" => match runtime.store.config_list_clear("animeBlacklist") {
            Ok(()) => "已清空黑名单".to_owned(),
            Err(error) => format!("清空失败：{error}"),
        },
        _ => "未知子命令。可用：list / add / remove / clear".to_owned(),
    };
    runtime.td.reply_text(target, text);
}

/// `/start <payload>`：三种深链（见 [`crate::deeplink`]）。
///
/// 认不出的载荷**不回复**：`/start` 是 Telegram 里最常见的命令，宿主还有自己的
/// start 插件（回欢迎语），我们再插一句「无法识别」会很吵。
fn cmd_start(runtime: &Arc<Runtime>, command: &Command) {
    let Some(link) = crate::deeplink::parse(&command.args) else {
        return;
    };
    let Some(tg_user_id) = current_sender() else {
        // 频道身份发来的 /start 没法确定身份（收藏/授权都要绑定用户）。
        return;
    };
    let target = Target::new(command.chat_id, command.message_id);
    let worker = Arc::clone(runtime);
    let spawned = std::thread::Builder::new()
        .name("xq-anime-start".to_owned())
        .spawn(move || {
            let (text, buttons) = match link.action {
                crate::deeplink::Action::BangumiCallback => {
                    let text = finish_bangumi_auth(&worker, tg_user_id, &link.values);
                    (text, Vec::new())
                }
                crate::deeplink::Action::Collection => {
                    collect_subject(&worker, tg_user_id, &link.values)
                }
                crate::deeplink::Action::EpisodeWatched => {
                    let text = watch_episode(&worker, tg_user_id, &link.values);
                    (text, Vec::new())
                }
            };
            if buttons.is_empty() {
                worker.td.reply_text(target, text);
            } else {
                worker.td.send_text_with_buttons(target, text, buttons);
            }
        });
    if let Err(error) = spawned {
        runtime.host.log(
            fuyubot_abi::LogLevel::Error,
            &format!("[anime] /start 线程起不来：{error}"),
        );
    }
}

/// `bgm_cb-<code>_<state>`：授权码换 token，写回绑定。
fn finish_bangumi_auth(runtime: &Runtime, tg_user_id: i64, values: &[String]) -> String {
    use anime_core::store::Storage;

    let short = values.first().map(String::as_str).unwrap_or_default();
    let state = values.get(1).map(String::as_str).unwrap_or_default();
    let Some(code) = crate::deeplink::base62_to_hex(short) else {
        return "授权失败：授权码格式不正确，请重新 /bindbangumi".to_owned();
    };
    let binding = match runtime.store.bangumi_user_by_tg(tg_user_id) {
        Ok(binding) => binding,
        Err(error) => return format!("授权失败，读取绑定记录出错：{error}"),
    };
    let Some(binding) = binding else {
        return "授权失败，找不到对应的用户记录！请重新使用 /bindbangumi 进行绑定。".to_owned();
    };
    // `state` 是库里的内部自增 id：对不上说明这个链接不是给当前用户签发的
    // （转别人的授权链接是常见误操作，Bangumi 也会拒，这里先挡一道）。
    if !state.is_empty() && state != binding.id.to_string() {
        return "授权失败，当前 Telegram 账户与绑定的账户不匹配！请使用正确的账户进行授权。"
            .to_owned();
    }

    let client = anime_core::bangumi::BgmClient::new(runtime.config.bangumi.clone());
    let token = match client.exchange_code(
        &code,
        &runtime.config.bangumi.redirect_uri,
        &binding.id.to_string(),
    ) {
        Ok(token) => token,
        Err(error) => return format!("授权失败，发生错误：{error}"),
    };
    let mut updated = binding.clone();
    updated.access_token = Some(token.access_token.clone());
    updated.refresh_token = token.refresh_token.filter(|next| !next.trim().is_empty());
    updated.expires_in = (token.expires_in > 0).then_some(token.expires_in as i64);
    updated.token_type = (!token.token_type.is_empty()).then_some(token.token_type.clone());
    updated.bgm_user_id = token.user_id.map(|id| id as i64);
    updated.scope = token.scope.filter(|scope| !scope.trim().is_empty());
    updated.auth_time = Some(Utc::now());
    if let Err(error) = runtime.store.bangumi_user_upsert(&updated) {
        return format!("授权已生效但写入失败：{error}");
    }

    // 报个账户名确认绑对了人：拿不到也不算失败（token 已经能用了）。
    let mut me_config = runtime.config.bangumi.clone();
    me_config.access_token = token.access_token;
    let who = anime_core::bangumi::BgmClient::new(me_config)
        .me()
        .map(|user| {
            format!(
                "账户名：{}\n用户ID：{}",
                if user.nickname.is_empty() {
                    &user.username
                } else {
                    &user.nickname
                },
                user.id
            )
        })
        .unwrap_or_else(|_| "（读取账户信息失败）".to_owned());

    format!(
        "账户授权成功！\n{who}\n你可以在 https://bgm.tv/ 中管理你的进度\n\n\
         使用 /exitbangumi 可以退出登录。\n\n如果需要更改绑定的账户，\
         使用 /exitbangumi 退出后重新授权流程即可"
    )
}

/// `collection-<animeId>`：把条目标成「在看」，再给一排改状态的按钮。
fn collect_subject(
    runtime: &Runtime,
    tg_user_id: i64,
    values: &[String],
) -> (String, Vec<Vec<fuyubot_simplify::ReplyButton>>) {
    let Some(anime_id) = values.first().and_then(|raw| raw.parse::<u64>().ok()) else {
        return ("收藏失败：链接里没有条目 id".to_owned(), Vec::new());
    };
    crate::bgmuser::get_bgm_token(runtime, tg_user_id).either(
        |access_token| {
            let client = anime_core::bangumi::BgmClient::new(runtime.config.bangumi.clone());
            if let Err(error) = client.set_subject_collection(&access_token, anime_id, 3) {
                return (format!("收藏失败：{error}"), Vec::new());
            }
            let name = anime_name(runtime, anime_id as i64);
            (
                format!("收藏动漫：{name} 成功！\n当前状态：在看\n\n点击下方按钮更改收藏状态："),
                // 五个状态都给（含当前的「在看」）：点错了也能点回来。
                crate::callback::collection_buttons(anime_id, None),
            )
        },
        |message| (message, Vec::new()),
    )
}

/// `eplook-<episodeId>`：把章节标成「看过」。
fn watch_episode(runtime: &Runtime, tg_user_id: i64, values: &[String]) -> String {
    use anime_core::store::Storage;

    let Some(episode_id) = values.first().and_then(|raw| raw.parse::<u64>().ok()) else {
        return "标记失败：链接里没有章节 id".to_owned();
    };
    let subject_id = match runtime.store.subject_id_of_episode(episode_id as i64) {
        Ok(Some(id)) => id,
        Ok(None) => return format!("标记失败，找不到对应的动漫信息，剧集ID：{episode_id}"),
        Err(error) => return format!("标记失败，读取章节信息出错：{error}"),
    };
    crate::bgmuser::get_bgm_token(runtime, tg_user_id).either(
        |access_token| {
            let client = anime_core::bangumi::BgmClient::new(runtime.config.bangumi.clone());
            if let Err(error) = client.set_episode_collection(&access_token, episode_id, 2) {
                // 没收藏条目就标不了章节：Bangumi 会 400，这里把原因说清楚。
                return format!(
                    "标记失败：{error}\n请先收藏 [{}](https://bgm.tv/subject/{subject_id}) 后再标记看过！",
                    anime_name(runtime, subject_id)
                );
            }
            format!(
                "动漫：{}\n集数：[{episode_id}](https://bgm.tv/ep/{episode_id}) 标记为 看过 成功！",
                anime_name(runtime, subject_id)
            )
        },
        |message| message,
    )
}

/// 番剧名：中文名优先（与提示语同一套规则）；读不到就退回 id。
fn anime_name(runtime: &Runtime, anime_id: i64) -> String {
    use anime_core::store::Storage;
    runtime
        .store
        .anime_by_id(anime_id)
        .ok()
        .flatten()
        .map(|anime| crate::prompts::display(&anime).to_owned())
        .unwrap_or_else(|| format!("{anime_id}"))
}

/// `/jz <源番剧 id> <集数> <目标番剧 id> [字幕组]`：把一集改挂到正确的番上。
///
/// 见 [`crate::correct`]——现在的链路在投放前就把番剧定下来了，要纠正的是
/// **已经发进正片频道但挂错番**的那一集，不再是缓存条目。
///
/// 干的是写库 + 刷两边导航 + 改已发消息文案，所以要开线程：一次 mongo 往返
/// 不该把更新流按住。
fn cmd_jz(runtime: &Arc<Runtime>, command: &Command) {
    let target = Target::new(command.chat_id, command.message_id);
    let Some(request) = crate::correct::parse_request(&command.args) else {
        runtime.td.reply_text(
            target,
            "用法：/jz <源番剧 id> <集数> <目标番剧 id> [字幕组]\n\
             示例：/jz 12345 03 67890\n\
             只纠正某个字幕组：/jz 12345 03 67890 喵萌奶茶屋",
        );
        return;
    };
    let worker = Arc::clone(runtime);
    let spawned = std::thread::Builder::new()
        .name("xq-anime-jz".to_owned())
        .spawn(move || {
            let text = match crate::correct::run(&worker, &request) {
                Ok(outcome) => outcome.render(&request),
                Err(error) => format!("纠正失败：{error}"),
            };
            worker.td.reply_text(target, text);
        });
    if let Err(error) = spawned {
        runtime.host.log(
            fuyubot_abi::LogLevel::Error,
            &format!("[anime] /jz 线程起不来：{error}"),
        );
    }
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
    let source = runtime.config_source.describe();

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
        // 同一进程里 /help /plugins 都可能有别的插件在管。`/help` 我们自己也提供，
        // 但语义必须一致（列命令），所以这里只锁「不抢别人的」。
        assert!(spec_for("plugins").is_none(), "/plugins 归 manage 插件");
    }

    #[test]
    fn start_only_reacts_to_its_own_deep_links() {
        // `/start` 我们**接管了**（三种深链），但只对认识的载荷出声：
        // 宿主自带的 start 插件会回欢迎语，我们再插一句「无法识别」会很吵。
        assert!(spec_for("start").is_some());
        assert!(crate::deeplink::parse("").is_none());
        assert!(crate::deeplink::parse("welcome").is_none());
        assert!(crate::deeplink::parse("bgm_cb-abc_42").is_some());
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
