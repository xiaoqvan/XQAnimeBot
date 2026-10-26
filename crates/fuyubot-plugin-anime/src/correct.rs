//! 单集纠正（`/jz`）：把已经发出去的**一集**从错挂的章节改挂到正确的章节上。
//!
//! 为什么不是 TS 那套「番剧 id + 缓存 id」
//! --------------------------------------
//! TS 的 `/jz` 是 `ConAnimeInformation`：把一条 `cacheItem`（还没确定是哪部番的
//! 暂存条目）接到某个 Bangumi 条目上。那套依赖「先有缓存条目，再谈番剧」的
//! 流程——我们现在的链路在投放前就把番剧定下来了（`newanime` 建档、审核、缓冲
//! 频道），缓存条目不再是纠正的入口，而是**历史包袱**：真正要纠正的是
//! 「已经发进正片频道、但章节挂错了」的那一集。
//!
//! 为什么参数是两个**章节 id** 而不是「番剧 id + 集数」
//! --------------------------------------------
//! 管理员手上有的往往就是错的那个章节 id（导航卡、Bangumi 页面、`bgm.tv/ep/<id>`
//! 上看得到），让他再回去查番剧 id 和集数编号是多一步容易错的翻译。所以直接收
//! 两个章节 id：
//!
//! ```text
//! /jz <源集数 id> <目标集数 id> [字幕组]
//!     ↑挂错的那个   ↑应该挂的那个   ↑只动这一个组（不填=整集）
//! ```
//!
//! 章节 id 先查本地 `episodes_meta`（里面带着 `subject_id`，等于顺便把番剧 id 也
//! 找出来了），查不到再走 Bangumi `GET /v0/episodes/{id}`——单章接口同样带
//! `subject_id`，拿到后顺手写回库，下次就不用联网了。
//!
//! 一次纠正做五件事（顺序有讲究）：
//!
//! ```text
//! 1. 两个章节 id 各自解析出「属于哪部番 + 第几集」
//! 2. 找出 resources 里挂在源章节上的记录（可按字幕组收窄）
//! 3. 目标章节那一侧若有同集同组的旧记录，先算清楚会顶掉哪几条
//! 4. 写新记录（anime_id、episodeId 都换成目标的）→ 删掉两侧旧记录
//! 5. 两边导航各刷一次 + 把已发视频的文案改成目标番的
//! ```
//!
//! 第 5 步的「改文案」是这条命令真正的价值所在：导航刷完只保证**导航卡**对，
//! 频道里那条已发的视频还挂着旧番的名字和旧导航链接——不一起改，等于纠正了
//! 一半。

use anime_core::log;
use anime_core::models::{AnimeItem, EpisodeMeta, EpisodeResourceDoc};
use anime_core::store::Storage;
use serde_json::json;

use crate::runtime::Runtime;

/// 一次纠正请求：两个章节 id + 可选的字幕组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectRequest {
    /// 现在错挂着的那个章节 id（数据库里的 `episodeId`）。
    pub from_episode: i64,
    /// 应该挂到的章节 id。
    pub to_episode: i64,
    /// 只纠正这个字幕组；`None` 表示整集一起搬。
    pub group: Option<String>,
}

impl CorrectRequest {
    /// 「动了哪些」的一句话，进结果文案。
    fn scope(&self) -> String {
        match &self.group {
            Some(group) => format!("（字幕组 {group}）"),
            None => "（全部字幕组）".to_owned(),
        }
    }
}

/// 解析 `/jz` 的参数：`from_episode to_episode [group...]`。
///
/// 字幕组放最后且**整段剩余都算它**：组名带空格（`Loli House`）是常有的事。
///
/// 认不出返回 `None`：参数个数不对、id 不是正整数。
pub fn parse_request(args: &str) -> Option<CorrectRequest> {
    let mut parts = args.split_whitespace();
    let from_episode = parts.next()?.parse::<i64>().ok()?;
    let to_episode = parts.next()?.parse::<i64>().ok()?;
    let group: Vec<&str> = parts.collect();
    let group = if group.is_empty() {
        None
    } else {
        let name = group.join(" ");
        (!name.trim().is_empty()).then_some(name)
    };
    if from_episode <= 0 || to_episode <= 0 {
        return None;
    }
    Some(CorrectRequest {
        from_episode,
        to_episode,
        group,
    })
}

/// 一个章节 id 解析出来的东西。
#[derive(Debug, Clone)]
pub struct ResolvedEpisode {
    /// 章节元数据，`subject_id` 一定有值（解析函数保证）。
    pub meta: EpisodeMeta,
    /// 从哪来的：结果文案要告诉管理员这次有没有联网。
    pub origin: &'static str,
}

impl ResolvedEpisode {
    /// 这一集所属的番剧（Bangumi subject id）。
    pub fn subject_id(&self) -> i64 {
        self.meta.subject_id
    }

    /// 集数编号：Bangumi 的 `sort` 是播出序号，`SP` 也排在它自己的序号上；
    /// 缺 `sort` 的老数据退回 `ep`（两者在 Bangumi 里通常相同）。
    pub fn number(&self) -> f64 {
        if self.meta.sort > 0.0 {
            self.meta.sort
        } else {
            self.meta.ep
        }
    }

    /// 资源表里这一集**可能写成**的字符串（同一集有 `03` / `3` 两种写法）。
    ///
    /// 没有编号的章节（剧场版 `sort = 0`）返回空：拿 `00` 去匹配资源只会误伤。
    pub fn labels(&self) -> Vec<String> {
        let number = self.number();
        let mut out = Vec::new();
        if number <= 0.0 {
            return out;
        }
        match self.meta.r#type.unwrap_or(0) {
            // 1 = SP：资源表里通常写作 `SP1` / `特别篇1`。
            1 => {
                let sp = number as i64;
                push_unique(&mut out, format!("SP{sp}"));
                push_unique(&mut out, format!("SP{sp:02}"));
                push_unique(&mut out, format!("特别篇{sp}"));
            }
            _ => {
                if number.fract() == 0.0 {
                    let episode = number as i64;
                    push_unique(&mut out, format!("{episode:02}"));
                    push_unique(&mut out, episode.to_string());
                } else {
                    // 5.5 这种半集：照抄数字，补零反而认不出。
                    push_unique(&mut out, format!("{number}"));
                }
            }
        }
        out
    }

    /// 展示用的集数标签（取第一种写法）。没有编号的章节退回章节名。
    pub fn label(&self) -> String {
        if let Some(first) = self.labels().first() {
            return first.clone();
        }
        let name = self
            .meta
            .name_cn
            .clone()
            .or_else(|| self.meta.name.clone())
            .unwrap_or_default();
        if name.trim().is_empty() {
            "未知".to_owned()
        } else {
            name
        }
    }
}

/// 收集候选集数字符串，去重且不收空串。
fn push_unique(out: &mut Vec<String>, text: String) {
    if !text.is_empty() && !out.contains(&text) {
        out.push(text);
    }
}

/// 解析一个章节 id：本地库优先，查不到再问 Bangumi。
///
/// 本地 `episodes_meta` 里就带着 `subject_id`——所以这一步同时把「番剧 id」也
/// 找出来了，不需要管理员再给一遍。本地记录缺 `subject_id`（老数据）时同样视为
/// 未命中，走网络补齐并写回。
pub fn resolve_episode(runtime: &Runtime, id: i64) -> Result<ResolvedEpisode, String> {
    if id <= 0 {
        return Err(format!("章节 id 得是正整数，收到 {id}"));
    }

    let local = runtime
        .store
        .find_one("episodes_meta", json!({ "id": id }), None)
        .ok()
        .flatten()
        .and_then(|row| serde_json::from_value::<EpisodeMeta>(row).ok());
    if let Some(meta) = local.filter(|meta| meta.subject_id != 0) {
        return Ok(ResolvedEpisode {
            meta,
            origin: "本地库",
        });
    }

    let client = anime_core::bangumi::BgmClient::new(runtime.config.bangumi.clone());
    let episode = client.episode(id as u64).map_err(|error| {
        format!("章节 {id} 本地库里没有，Bangumi 也查不到（{error}）")
    })?;
    if episode.subject_id == 0 {
        return Err(format!("Bangumi 没说章节 {id} 属于哪部番"));
    }
    let meta = episode.to_meta(episode.subject_id);
    // 写回：下次再纠正同一集就不用联网了，顺便给导航补上章节数据。
    if let Err(error) = runtime
        .store
        .upsert_episodes(episode.subject_id as i64, std::slice::from_ref(&meta))
    {
        log!(&runtime.logger, Warn, "写回章节失败：{error}");
    }
    Ok(ResolvedEpisode {
        meta,
        origin: "Bangumi",
    })
}

/// 一条资源是否挂在给定章节上。
///
/// **有 `episodeId` 就只认 id**：写着别的章节 id 的资源是别的那一集，哪怕集数
/// 字符串碰巧一样也不该被搬走。没有 `episodeId` 的老数据才退回集数字符串，
/// `03` / `3` 两种写法都算。
fn matches_episode(resource: &EpisodeResourceDoc, id: i64, labels: &[String]) -> bool {
    match resource.episode_id {
        Some(episode_id) => episode_id == id,
        None => labels.iter().any(|label| *label == resource.episode),
    }
}

/// 一条资源是否在这次纠正的范围内。
fn in_scope(resource: &EpisodeResourceDoc, group: Option<&str>) -> bool {
    let Some(group) = group else {
        return true;
    };
    let wanted = group.trim();
    resource
        .groups
        .iter()
        .any(|name| name.trim().eq_ignore_ascii_case(wanted))
}

/// 纠正结果。字段都是给结果文案用的，所以直接存渲染好的名字。
#[derive(Debug, Clone, Default)]
pub struct CorrectOutcome {
    pub from_name: String,
    pub to_name: String,
    /// 源 / 目标章节的集数标签（`03` / `SP1`）。
    pub from_label: String,
    pub to_label: String,
    /// 两个章节 id 各自的来源，管理员看得到这次有没有联网。
    pub from_origin: &'static str,
    pub to_origin: &'static str,
    /// 实际搬走的资源条数（= 字幕组数）。
    pub moved: usize,
    /// 改掉的已发消息文案条数。
    pub captions: usize,
    /// 目标章节那一侧同集同组**原本就有**的资源，被本次覆盖掉的条数。
    pub overwritten: usize,
    /// 两个章节属于同一部番：不是改挂，只是把这一集的章节 id 改对。
    pub same_subject: bool,
}

impl CorrectOutcome {
    /// 结果文案。
    pub fn render(&self, request: &CorrectRequest) -> String {
        let mut out = if self.same_subject {
            format!(
                "「{}」第 {} 集{}的章节 id 已改成 {}",
                self.from_name, self.from_label, request.scope(), request.to_episode
            )
        } else {
            format!(
                "已把「{}」第 {} 集{}改挂到「{}」第 {} 集",
                self.from_name,
                self.from_label,
                request.scope(),
                self.to_name,
                self.to_label
            )
        };
        out.push_str(&format!(
            "\n章节 id {} → {}（{} / {}）",
            request.from_episode, request.to_episode, self.from_origin, self.to_origin
        ));
        out.push_str(&format!("\n搬走 {} 条资源", self.moved));
        if self.captions > 0 {
            out.push_str(&format!("，改掉 {} 条已发消息文案", self.captions));
        }
        if self.overwritten > 0 {
            // 同一集同一组不该留两条，被顶掉的要说出来——管理员可能正是指望
            // 目标番那一条留着。
            out.push_str(&format!("；覆盖了目标番原有的 {} 条", self.overwritten));
        }
        out
    }

}

/// 执行一次纠正。
///
/// 校验顺序即「最便宜的先做」：先解析章节 id、再读番剧文档、最后才动库。
/// 任何一步不满足就返回 `Err` 且**不写任何东西**——搬错了再搬回来比现在报错麻烦得多。
pub fn run(runtime: &Runtime, request: &CorrectRequest) -> Result<CorrectOutcome, String> {
    // 1. 两个章节 id 各解析一次：这次纠正的「哪部番、第几集」全靠它们。
    let from = resolve_episode(runtime, request.from_episode)?;
    let to = resolve_episode(runtime, request.to_episode)?;
    if request.from_episode == request.to_episode {
        return Err("源章节和目标章节是同一个，没有要纠正的".to_owned());
    }
    let same_subject = from.subject_id() == to.subject_id();

    let from_name = anime_name(runtime, from.subject_id());
    let to_name = anime_name(runtime, to.subject_id());
    // 跨番纠正要求目标番已经建档：导航要刷、文案要按目标番渲染，没有文档就
    // 只能刷出个空导航。同番改 id 不受这条限制。
    if !same_subject && !matches!(runtime.store.anime_by_id(to.subject_id()), Ok(Some(_))) {
        return Err(format!(
            "库里没有 id 为 {} 的番剧，请先 /updateanime {} 建档",
            to.subject_id(),
            to.subject_id()
        ));
    }

    // 2. 源番这一集的资源。整部番一次捞出来：一季也就几十条，省得为「有没有这一集」
    //    再往返一次，找不着时还能把实际有哪些集报给管理员。
    let all_from: Vec<EpisodeResourceDoc> = runtime
        .store
        .find(
            "resources",
            json!({ "anime_id": from.subject_id() }),
            None,
            None,
            None,
        )
        .map_err(|error| format!("读资源失败：{error}"))?
        .into_iter()
        .filter_map(|row| serde_json::from_value::<EpisodeResourceDoc>(row).ok())
        .collect();
    let labels = from.labels();
    let matched: Vec<EpisodeResourceDoc> = all_from
        .iter()
        .filter(|resource| matches_episode(resource, request.from_episode, &labels))
        .filter(|resource| in_scope(resource, request.group.as_deref()))
        .cloned()
        .collect();
    if matched.is_empty() {
        // 报出这一集实际挂了哪些字幕组、以及这一整部番有哪些集：管理员常常记不清
        // 组名，比干说「没找到」有用。
        let groups: Vec<String> = all_from
            .iter()
            .filter(|resource| matches_episode(resource, request.from_episode, &labels))
            .flat_map(|resource| resource.groups.clone())
            .collect();
        let hint = if groups.is_empty() {
            let episodes: Vec<String> = all_from
                .iter()
                .map(|resource| resource.episode.clone())
                .take(10)
                .collect();
            let tail = if episodes.is_empty() {
                "库里没有这部番的资源".to_owned()
            } else {
                format!("这部番的资源集数有：{}", episodes.join("、"))
            };
            format!(
                "「{}」没有挂在章节 {}（第 {} 集）上的资源；{tail}",
                from_name,
                request.from_episode,
                from.label()
            )
        } else {
            format!(
                "「{}」第 {} 集的字幕组有：{}",
                from_name,
                from.label(),
                groups.join("、")
            )
        };
        return Err(hint);
    }

    // 3. 目标章节那一侧的现有资源：同集同组会被顶掉，得先数清楚再告诉管理员。
    let to_rows: Vec<EpisodeResourceDoc> = if same_subject {
        all_from.clone()
    } else {
        runtime
            .store
            .find(
                "resources",
                json!({ "anime_id": to.subject_id() }),
                None,
                None,
                None,
            )
            .unwrap_or_default()
            .into_iter()
            .filter_map(decode_resource)
            .collect()
    };

    // 4. 插新的 → 删旧的。顺序不能反：先插后删，中途失败最多留一条重复，
    //    反过来则是真丢数据。
    let mut moved = 0usize;
    let mut overwritten = 0usize;
    for resource in &matched {
        // 集数字符串也按目标章节重写：源标 `03` 而目标章节是第 5 集时，不改就会
        // 挂在目标番**第 3 集**的位置上，还会顶掉人家真正的第 3 集。
        // 只有 `v2` 这类版本号后缀跟着走——那是同一集的修订版，不是集数。
        let new_episode = target_episode(&resource.episode, &to);
        // 目标侧同集同组的现有记录：跨番纠正时它们会被顶掉，得先数清楚。
        // 同番时不数——那里的「现有记录」就是我们自己这条，报了只会让人以为
        // 覆盖了别的资源。
        let same_cell: Vec<&EpisodeResourceDoc> = to_rows
            .iter()
            .filter(|row| row.episode == new_episode && row.groups == resource.groups)
            .collect();
        if !same_subject {
            overwritten += same_cell.len();
        }
        // 要删的只有 episodeId 不是目标 id 的那些；已经是目标 id 的那条由 upsert
        // 直接更新，删了反而把刚写的内容丢掉。
        let stale: Vec<&EpisodeResourceDoc> = same_cell
            .into_iter()
            .filter(|row| row.episode_id != Some(request.to_episode))
            .collect();

        let mut fresh = resource.clone();
        fresh.anime_id = to.subject_id();
        fresh.episode = new_episode.clone();
        // 用户要的就是这个：集数 id 一起改过去，导航的「标记看过」才指得对。
        fresh.episode_id = Some(request.to_episode);
        // 旧的 ObjectId 不能带过去：那是**另一条**文档的标识，写进去会撞主键。
        fresh.id = None;
        if let Err(error) = runtime.store.resource_upsert(&fresh) {
            log!(&runtime.logger, Warn, "写新资源失败：{error}");
            continue;
        }
        moved += 1;

        // 目标侧旧记录：按 episodeId 精确删（缺失的用 null 匹配），
        // 刚写进去的那条 episodeId 是目标 id，不会被误伤。
        for row in stale {
            let mut filter = json!({
                "anime_id": to.subject_id(),
                "episode": row.episode,
                "groups": row.groups,
            });
            filter["episodeId"] = episode_id_filter(row.episode_id);
            if let Err(error) = runtime.store.delete("resources", filter, true) {
                log!(&runtime.logger, Warn, "删目标侧旧资源失败：{error}");
            }
        }

        // 源侧旧记录。many：同键的历史重复文档（早期 insert 留下的）一并清掉，
        // 否则源番里这一集会阴魂不散地出现在导航上。
        let mut filter = json!({
            "anime_id": from.subject_id(),
            "episode": resource.episode,
            "groups": resource.groups,
        });
        filter["episodeId"] = episode_id_filter(resource.episode_id);
        if let Err(error) = runtime.store.delete("resources", filter, true) {
            log!(&runtime.logger, Warn, "删旧资源失败：{error}");
        }
    }
    if moved == 0 {
        return Err("写入新资源失败，没有改动".to_owned());
    }

    // 5. 两边导航各刷一次：源番少了这一集，目标番多了这一集（同番时只刷一次）。
    let mut refreshed: Vec<i64> = Vec::new();
    for id in [from.subject_id(), to.subject_id()] {
        if refreshed.contains(&id) {
            continue;
        }
        refreshed.push(id);
        if let Err(error) = crate::nav::refresh(runtime, id) {
            log!(&runtime.logger, Warn, "刷新导航失败（{id}）：{error}");
        }
    }

    // 6. 已发视频的文案还写着旧番的名字和旧导航链接，一起改掉。
    let mut captions = 0usize;
    let to_anime = runtime.store.anime_by_id(to.subject_id()).ok().flatten();
    for resource in &matched {
        let Some(anime) = to_anime.as_ref() else {
            break;
        };
        let Some(message) = resource.message.as_ref() else {
            continue;
        };
        if message.message_id == 0 {
            continue;
        }
        let new_episode = target_episode(&resource.episode, &to);
        // 文案只需要「标题 + 字幕组 + 集数」：磁力、链接这些重建不出来，
        // 也用不上（anime_text 只在 title 有 link 时才给标题加跳转）。
        let item = AnimeItem {
            title: resource.title.clone(),
            pub_date: String::new(),
            magnet: String::new(),
            link: String::new(),
            team: resource.groups.first().cloned().unwrap_or_default(),
            fansub: resource.groups.clone(),
            names: resource.names.clone().unwrap_or_default(),
            source: resource.source.clone(),
            episode: Some(new_episode.clone()),
        };
        let text = crate::text::anime_text(anime, &item, request.to_episode);
        if runtime
            .td
            .edit_caption(message.chat_id, message.message_id, text)
        {
            captions += 1;
        }
    }

    Ok(CorrectOutcome {
        from_name,
        to_name,
        from_label: from.label(),
        to_label: to.label(),
        from_origin: from.origin,
        to_origin: to.origin,
        moved,
        captions,
        overwritten,
        same_subject,
    })
}

/// 反序列化一条资源（脏数据直接跳过，不让一条坏记录挡住整次纠正）。
fn decode_resource(row: serde_json::Value) -> Option<EpisodeResourceDoc> {
    serde_json::from_value::<EpisodeResourceDoc>(row).ok()
}

/// 按目标章节重写集数字符串，只保留源串里的版本号后缀（`03v2` → `05v2`）。
///
/// 其余一律用目标编号：纠正的目的就是让这一集站到目标番的正确位置上。
fn rename_episode(episode: &str, target: &str) -> String {
    match crate::text::parse_episode(episode) {
        crate::text::ParsedEpisode::Num { version, .. } if !version.is_empty() => {
            format!("{target}v{version}")
        }
        _ => target.to_owned(),
    }
}

/// 纠正后这条资源该用的集数：目标章节有编号就换成它的，没有（剧场版之类）就保留原样。
///
/// 目标编号是 `0` 意味着「这一章没有集数概念」，拿章节名去当集数是瞎改，排序也会乱。
fn target_episode(episode: &str, to: &ResolvedEpisode) -> String {
    if to.number() <= 0.0 {
        return episode.to_owned();
    }
    rename_episode(episode, &to.label())
}

/// 删除用的 `episodeId` 条件：没有就查 `null`（Mongo 里 `{f: null}` 命中缺失的字段）。
///
/// 不能干脆不带这个条件——那就分不清「episodeId 是 1001 的那条」和「还没章节 id
/// 的那条」，会把刚写进去的新记录一起删掉。
fn episode_id_filter(episode_id: Option<i64>) -> serde_json::Value {
    match episode_id {
        Some(id) => json!(id),
        None => serde_json::Value::Null,
    }
}

/// 番剧名：库里没有就退回 id（纠正照样能做，只是文案里显示数字）。
fn anime_name(runtime: &Runtime, id: i64) -> String {
    runtime
        .store
        .anime_by_id(id)
        .ok()
        .flatten()
        .map(|anime| anime.display_name().to_owned())
        .unwrap_or_else(|| format!("{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: i64, subject_id: i64, sort: f64, kind: i32) -> EpisodeMeta {
        EpisodeMeta {
            id,
            subject_id,
            sort,
            ep: sort,
            r#type: Some(kind),
            ..Default::default()
        }
    }

    fn resolved(meta: EpisodeMeta) -> ResolvedEpisode {
        ResolvedEpisode {
            meta,
            origin: "本地库",
        }
    }

    fn resource(episode: &str, group: &str, episode_id: Option<i64>) -> EpisodeResourceDoc {
        EpisodeResourceDoc {
            episode: episode.to_owned(),
            groups: vec![group.to_owned()],
            episode_id,
            ..Default::default()
        }
    }

    #[test]
    fn parse_request_reads_two_episode_ids() {
        let request = parse_request("1001 2005").expect("两个章节 id");
        assert_eq!(request.from_episode, 1001);
        assert_eq!(request.to_episode, 2005);
        assert_eq!(request.group, None);
    }

    #[test]
    fn parse_request_keeps_group_names_with_spaces() {
        // 组名带空格：剩下的整段都归它，不能只取第一个词。
        let request = parse_request("1001 2005 Loli House").expect("组名带空格");
        assert_eq!(request.group.as_deref(), Some("Loli House"));
    }

    #[test]
    fn parse_request_rejects_bad_args() {
        assert!(parse_request("").is_none());
        assert!(parse_request("1001").is_none());
        assert!(parse_request("abc 2005").is_none(), "id 必须是整数");
        assert!(parse_request("0 2005").is_none(), "id 必须为正");
        assert!(parse_request("1001 -3").is_none());
    }

    #[test]
    fn labels_cover_padded_and_plain_numbers() {
        // 资源表里 `03` 和 `3` 两种写法都有过，两边都得认。
        assert_eq!(resolved(meta(1, 1, 3.0, 0)).labels(), vec!["03", "3"]);
        assert_eq!(resolved(meta(1, 1, 12.0, 0)).labels(), vec!["12"]);
        assert_eq!(resolved(meta(1, 1, 5.5, 0)).labels(), vec!["5.5"]);
    }

    #[test]
    fn labels_handle_sp_episodes() {
        let labels = resolved(meta(1, 1, 2.0, 1)).labels();
        assert_eq!(labels[0], "SP2");
        assert!(labels.contains(&"特别篇2".to_owned()), "{labels:?}");
    }

    #[test]
    fn resource_matches_by_episode_id_or_label() {
        let labels = resolved(meta(1001, 1, 3.0, 0)).labels();
        assert!(matches_episode(
            &resource("03", "组", Some(1001)),
            1001,
            &labels
        ));
        assert!(matches_episode(&resource("3", "组", None), 1001, &labels));
        assert!(!matches_episode(&resource("04", "组", None), 1001, &labels));
        // episodeId 是别的一集：即便集数字符串碰上了也不该算。
        assert!(!matches_episode(
            &resource("03", "组", Some(9999)),
            1001,
            &labels
        ));
    }

    #[test]
    fn rename_episode_follows_target_and_keeps_version() {
        assert_eq!(rename_episode("03", "05"), "05");
        // 版本号是「同一集的修订版」，不是集数：跟着走。
        assert_eq!(rename_episode("03v2", "05"), "05v2");
        // SP / 剧场版这类没有编号的，纠正后也落到目标编号上。
        assert_eq!(rename_episode("剧场版", "05"), "05");
    }

    #[test]
    fn target_episode_keeps_original_when_target_has_no_number() {
        // 目标章节 sort = 0（剧场版之类的无编号章节）：没有集数概念，别瞎改。
        let to = resolved(meta(2005, 2, 0.0, 0));
        assert_eq!(to.number(), 0.0);
        assert!(to.labels().is_empty(), "{:?}", to.labels());
        assert_eq!(target_episode("03", &to), "03");
        // 有编号才换成目标编号。
        let to = resolved(meta(2005, 2, 5.0, 0));
        assert_eq!(target_episode("03", &to), "05");
    }

    #[test]
    fn episode_id_filter_matches_missing_field_with_null() {
        assert_eq!(episode_id_filter(Some(1001)), json!(1001));
        assert_eq!(episode_id_filter(None), serde_json::Value::Null);
    }

    #[test]
    fn scope_is_group_name_or_everything() {
        let resource = resource("03", "喵萌奶茶屋", None);
        assert!(in_scope(&resource, None), "不指定字幕组 = 整集");
        assert!(in_scope(&resource, Some("喵萌奶茶屋")));
        assert!(!in_scope(&resource, Some("LoliHouse")));
    }

    #[test]
    fn outcome_render_cross_subject() {
        let outcome = CorrectOutcome {
            from_name: "A".to_owned(),
            to_name: "B".to_owned(),
            from_label: "03".to_owned(),
            to_label: "05".to_owned(),
            from_origin: "本地库",
            to_origin: "Bangumi",
            moved: 2,
            captions: 1,
            overwritten: 1,
            same_subject: false,
        };
        let request = parse_request("1001 2005 组").expect("带组");
        let text = outcome.render(&request);
        assert!(text.contains("「A」"), "{text}");
        assert!(text.contains("「B」"), "{text}");
        assert!(text.contains("第 03 集"), "{text}");
        assert!(text.contains("第 05 集"), "{text}");
        assert!(text.contains("章节 id 1001 → 2005"), "{text}");
        assert!(text.contains("本地库 / Bangumi"), "{text}");
        assert!(text.contains("搬走 2 条资源"), "{text}");
        assert!(text.contains("覆盖了目标番原有的 1 条"), "{text}");
    }

    #[test]
    fn outcome_render_same_subject() {
        let outcome = CorrectOutcome {
            from_name: "A".to_owned(),
            to_name: "A".to_owned(),
            from_label: "03".to_owned(),
            to_label: "03".to_owned(),
            from_origin: "本地库",
            to_origin: "本地库",
            moved: 1,
            captions: 0,
            overwritten: 0,
            same_subject: true,
        };
        let request = parse_request("1001 2005").expect("两个 id");
        let text = outcome.render(&request);
        assert!(text.contains("章节 id 已改成 2005"), "{text}");
        assert!(!text.contains("改挂到"), "{text}");
    }
}
