//! 单集纠正（`/jz`）：把某部番里**某一集**（可窄到一个字幕组）改挂到正确的番上。
//!
//! 为什么不是 TS 那套「番剧 id + 缓存 id」
//! --------------------------------------
//! TS 的 `/jz` 是 `ConAnimeInformation`：把一条 `cacheItem`（还没确定是哪部番的
//! 暂存条目）接到某个 Bangumi 条目上。那套依赖「先有缓存条目，再谈番剧」的
//! 流程——我们现在的链路在投放前就把番剧定下来了（`newanime` 建档、审核、缓冲
//! 频道），缓存条目不再是纠正的入口，而是**历史包袱**：真正要纠正的是
//! 「已经发进正片频道、但挂错了番」的那一集。
//!
//! 所以这里改成按**资源**纠正：
//!
//! ```text
//! /jz <源番剧 id> <集数> <目标番剧 id> [字幕组]
//!     ↑从哪部    ↑哪一集   ↑改挂到哪部     ↑只动这一个组（不填=整集）
//! ```
//!
//! 一次纠正做四件事（顺序有讲究）：
//!
//! ```text
//! 1. 找出 resources 里「源番 + 该集（+ 该字幕组）」的记录
//! 2. 按目标番的章节列表重新解析 episodeId（番换了，章节 id 必然是另一套）
//! 3. 写新记录 → 删旧记录（upsert 键是 anime_id+episode+groups，改键必须插+删）
//! 4. 两边导航各刷一次 + 把已发视频的文案改成目标番的
//! ```
//!
//! 第 4 步的「改文案」是这条命令真正的价值所在：导航刷完只保证**导航卡**对，
//! 频道里那条已发的视频还挂着旧番的名字和旧导航链接——不一起改，等于纠正了
//! 一半。

use std::collections::BTreeSet;

use anime_core::log;
use anime_core::models::{AnimeItem, EpisodeMeta, EpisodeResourceDoc};
use anime_core::store::Storage;
use serde_json::json;

use crate::runtime::Runtime;

/// 一次纠正请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectRequest {
    /// 现在挂在哪部番上（Bangumi subject id）。
    pub from_anime: i64,
    /// 应该挂到哪部番上。
    pub to_anime: i64,
    /// 集数（`"03"` / `"SP1"` / `"剧场版"`）。
    pub episode: String,
    /// 只纠正这个字幕组；`None` 表示整集一起搬。
    pub group: Option<String>,
}

impl CorrectRequest {
    /// 一句话描述，进预览和结果文案。
    fn describe(&self) -> String {
        match &self.group {
            Some(group) => format!("第 {} 集（字幕组 {}）", self.episode, group),
            None => format!("第 {} 集（全部字幕组）", self.episode),
        }
    }
}

/// 解析 `/jz` 的参数：`from episode to [group...]`。
///
/// 顺序按「从哪部、哪一集、到哪部」排——纠正时脑子里是这个顺序，命令行也就
/// 是这个顺序。字幕组放最后且**整段剩余都算它**：组名带空格（`Loli House`）
/// 是常有的事。
///
/// 认不出返回 `None`：参数个数不对、id 不是整数、集数是空串。
pub fn parse_request(args: &str) -> Option<CorrectRequest> {
    let mut parts = args.split_whitespace();
    let from_anime = parts.next()?.parse::<i64>().ok()?;
    let episode = parts.next()?.to_owned();
    let to_anime = parts.next()?.parse::<i64>().ok()?;
    let group: Vec<&str> = parts.collect();
    let group = if group.is_empty() {
        None
    } else {
        let name = group.join(" ");
        (!name.trim().is_empty()).then_some(name)
    };
    if episode.trim().is_empty() {
        return None;
    }
    Some(CorrectRequest {
        from_anime,
        to_anime,
        episode,
        group,
    })
}

/// 在目标番的章节列表里找「第 N 集」对应的章节 id。
///
/// 集数字符串先按 [`crate::text::parse_episode`] 归一（同一套规则，导航排序也是它），
/// 再按 `sort` 找：`sort` 是 Bangumi 的播出序号，`SP1` 这类排在 `type=1` 里。
///
/// 找不到返回 `None`——调用方据此决定要不要照样搬（资源本身可以搬，只是这一集
/// 的「标记看过」链接会缺）。
pub fn episode_id_of(episodes: &[EpisodeMeta], episode: &str) -> Option<i64> {
    let find = |sort: f64, kind: Option<i32>| -> Option<i64> {
        episodes
            .iter()
            .find(|meta| {
                (meta.sort - sort).abs() < f64::EPSILON
                    && kind.is_none_or(|kind| meta.r#type.unwrap_or(0) == kind)
            })
            // `sort` 缺字段的老数据退一步看 `ep`：两者在 Bangumi 里通常相同。
            .or_else(|| {
                episodes.iter().find(|meta| {
                    (meta.ep - sort).abs() < f64::EPSILON
                        && kind.is_none_or(|kind| meta.r#type.unwrap_or(0) == kind)
                })
            })
            .map(|meta| meta.id)
            .filter(|id| *id != 0)
    };

    match crate::text::parse_episode(episode) {
        crate::text::ParsedEpisode::Num { number, .. } => find(number, Some(0)),
        crate::text::ParsedEpisode::Sp { number } => find(number as f64, Some(1)),
        // 剧场版之类没有编号，章节表里也对不上任何一条。
        crate::text::ParsedEpisode::Other => None,
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

/// upsert 键（与 [`anime_core::store::Storage::resource_upsert`] 一致）：
/// 番剧 + 集数 + 字幕组。判断「搬过去会不会顶掉东西」用它。
fn resource_key(resource: &EpisodeResourceDoc) -> String {
    format!("{}|{}", resource.episode, resource.groups.join(","))
}

/// 纠正结果。字段都是给结果文案用的，所以直接存渲染好的名字。
#[derive(Debug, Clone, Default)]
pub struct CorrectOutcome {
    pub from_name: String,
    pub to_name: String,
    /// 实际搬走的资源条数（= 字幕组数）。
    pub moved: usize,
    /// 目标番里这一集对应的章节 id。
    pub episode_id: Option<i64>,
    /// 改掉的已发消息文案条数。
    pub captions: usize,
    /// 目标番这一集同一字幕组**原本就有**资源，被本次覆盖掉的条数。
    pub overwritten: usize,
    /// 恰好没匹配上章节 id（集数是「剧场版」之类，或目标番章节表里没有）。
    pub episode_id_missing: bool,
}

impl CorrectOutcome {
    /// 结果文案。
    pub fn render(&self, request: &CorrectRequest) -> String {
        let mut out = format!(
            "已把「{}」的{} 改挂到「{}」",
            self.from_name,
            request.describe(),
            self.to_name
        );
        out.push_str(&format!("\n搬走 {} 条资源", self.moved));
        match self.episode_id {
            Some(id) => out.push_str(&format!("，新章节 id {id}")),
            None if self.episode_id_missing => {
                out.push_str("，未找到对应章节 id（“标记看过”链接会缺失）")
            }
            None => {}
        }
        if self.captions > 0 {
            out.push_str(&format!("，改掉 {} 条已发消息文案", self.captions));
        }
        if self.overwritten > 0 {
            // upsert 键是「番剧+集数+字幕组」，所以搬过去必然顶掉目标番原有的
            // 那一条。这是设计使然（同一集同一组不该有两条），但要说出来。
            out.push_str(&format!("；覆盖了目标番原有的 {} 条", self.overwritten));
        }
        out
    }
}

/// 执行一次纠正。
///
/// 校验顺序即「最便宜的先做」：先比参数、再读两边的番剧文档、最后才动库。
/// 任何一步不满足就返回 `Err` 且**不写任何东西**——搬错了再搬回来比现在报错麻烦得多。
pub fn run(runtime: &Runtime, request: &CorrectRequest) -> Result<CorrectOutcome, String> {
    if request.from_anime == request.to_anime {
        return Err("源番剧和目标番剧是同一个，没有要纠正的".to_owned());
    }

    let from = runtime
        .store
        .anime_by_id(request.from_anime)
        .map_err(|error| format!("读源番剧失败：{error}"))?
        .ok_or_else(|| format!("库里没有 id 为 {} 的番剧", request.from_anime))?;
    let to = runtime
        .store
        .anime_by_id(request.to_anime)
        .map_err(|error| format!("读目标番剧失败：{error}"))?
        .ok_or_else(|| {
            format!(
                "库里没有 id 为 {} 的番剧，请先 /updateanime {} 建档",
                request.to_anime, request.to_anime
            )
        })?;

    // 1. 找源番这一集的资源。
    let rows = runtime
        .store
        .find(
            "resources",
            json!({ "anime_id": request.from_anime, "episode": request.episode }),
            None,
            None,
            None,
        )
        .map_err(|error| format!("读资源失败：{error}"))?;
    let mut matched: Vec<EpisodeResourceDoc> = rows
        .into_iter()
        .filter_map(|row| serde_json::from_value::<EpisodeResourceDoc>(row).ok())
        .filter(|resource| in_scope(resource, request.group.as_deref()))
        .collect();
    if matched.is_empty() {
        // 报出这一集实际有哪些字幕组：管理员常常记不清组名，比干说「没找到」有用。
        let groups: BTreeSet<String> = runtime
            .store
            .find(
                "resources",
                json!({ "anime_id": request.from_anime }),
                None,
                None,
                None,
            )
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| serde_json::from_value::<EpisodeResourceDoc>(row).ok())
            .filter(|resource| resource.episode == request.episode)
            .flat_map(|resource| resource.groups)
            .collect();
        let hint = if groups.is_empty() {
            format!("「{}」没有第 {} 集的资源", from.display_name(), request.episode)
        } else {
            format!(
                "「{}」第 {} 集的字幕组有：{}",
                from.display_name(),
                request.episode,
                groups.iter().cloned().collect::<Vec<_>>().join("、")
            )
        };
        return Err(hint);
    }

    // 2. 目标番的章节 id。库里没有章节就现拉一次并写回（TS 也是用到才拉）。
    let mut episodes = runtime
        .store
        .episodes_by_subject(request.to_anime)
        .map_err(|error| format!("读章节失败：{error}"))?;
    if episodes.is_empty() {
        let client = anime_core::bangumi::BgmClient::new(runtime.config.bangumi.clone());
        let subject_id = request.to_anime as u64;
        match client.episodes(subject_id) {
            Ok(fresh) => {
                let metas: Vec<EpisodeMeta> =
                    fresh.iter().map(|episode| episode.to_meta(subject_id)).collect();
                if let Err(error) = runtime.store.upsert_episodes(request.to_anime, &metas) {
                    log!(&runtime.logger, Warn, "写章节失败：{error}");
                }
                episodes = runtime
                    .store
                    .episodes_by_subject(request.to_anime)
                    .unwrap_or_default();
            }
            Err(error) => log!(&runtime.logger, Warn, "拉章节失败：{error}"),
        }
    }
    let episode_id = episode_id_of(&episodes, &request.episode);
    let episode_id_missing = episode_id.is_none();

    // 目标番这一集的现有资源：upsert 键相同就必然顶掉，得先数清楚再告诉管理员。
    let existing: BTreeSet<String> = runtime
        .store
        .find(
            "resources",
            json!({ "anime_id": request.to_anime }),
            None,
            None,
            None,
        )
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| serde_json::from_value::<EpisodeResourceDoc>(row).ok())
        .map(|resource| resource_key(&resource))
        .collect();

    // 3. 插新的、删旧的。顺序不能反：删了再插，中途失败就丢了。
    let mut moved = 0usize;
    let mut overwritten = 0usize;
    for resource in &mut matched {
        let old_key = json!({
            "anime_id": request.from_anime,
            "episode": resource.episode,
            "groups": resource.groups,
        });
        if existing.contains(&resource_key(resource)) {
            overwritten += 1;
        }
        let mut fresh = resource.clone();
        fresh.anime_id = request.to_anime;
        fresh.episode_id = episode_id;
        // 旧的 ObjectId 不能带过去：那是**另一条**文档的标识，写进去会撞主键。
        fresh.id = None;
        match runtime.store.resource_upsert(&fresh) {
            Ok(_) => moved += 1,
            Err(error) => {
                log!(&runtime.logger, Warn, "写新资源失败：{error}");
                continue;
            }
        }
        let mut filter = old_key;
        if let Some(id) = resource.episode_id {
            filter["episodeId"] = json!(id);
        }
        // many：同键的历史重复文档（早期 insert 留下的）一并清掉，否则源番里
        // 这一集会阴魂不散地出现在导航上。
        if let Err(error) = runtime.store.delete("resources", filter, true) {
            log!(&runtime.logger, Warn, "删旧资源失败：{error}");
        }
    }
    if moved == 0 {
        return Err("写入新资源失败，没有改动".to_owned());
    }

    // 4. 两边导航各刷一次：源番少了这一集，目标番多了这一集。
    for id in [request.from_anime, request.to_anime] {
        if let Err(error) = crate::nav::refresh(runtime, id) {
            log!(&runtime.logger, Warn, "刷新导航失败（{id}）：{error}");
        }
    }

    // 5. 已发视频的文案还写着旧番的名字和旧导航链接，一起改掉。
    let mut captions = 0usize;
    for resource in &matched {
        let Some(message) = resource.message.as_ref() else {
            continue;
        };
        if message.message_id == 0 {
            continue;
        }
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
            episode: Some(resource.episode.clone()),
        };
        let text = crate::text::anime_text(&to, &item, episode_id.unwrap_or(0));
        if runtime
            .td
            .edit_caption(message.chat_id, message.message_id, text)
        {
            captions += 1;
        }
    }

    Ok(CorrectOutcome {
        from_name: from.display_name().to_owned(),
        to_name: to.display_name().to_owned(),
        moved,
        episode_id,
        captions,
        overwritten,
        episode_id_missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episodes() -> Vec<EpisodeMeta> {
        vec![
            EpisodeMeta {
                id: 1001,
                sort: 1.0,
                ep: 1.0,
                r#type: Some(0),
                ..Default::default()
            },
            EpisodeMeta {
                id: 1003,
                sort: 3.0,
                ep: 3.0,
                r#type: Some(0),
                ..Default::default()
            },
            EpisodeMeta {
                id: 1010,
                sort: 1.0,
                ep: 1.0,
                r#type: Some(1),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn parse_request_reads_from_episode_then_to() {
        let request = parse_request("123 03 456").expect("四段参数");
        assert_eq!(request.from_anime, 123);
        assert_eq!(request.episode, "03");
        assert_eq!(request.to_anime, 456);
        assert_eq!(request.group, None);
    }

    #[test]
    fn parse_request_keeps_group_names_with_spaces() {
        // 组名带空格：剩下的整段都归它，不能只取第一个词。
        let request = parse_request("123 03 456 Loli House").expect("组名带空格");
        assert_eq!(request.group.as_deref(), Some("Loli House"));
    }

    #[test]
    fn parse_request_rejects_incomplete_args() {
        assert!(parse_request("").is_none());
        assert!(parse_request("123").is_none());
        assert!(parse_request("123 03").is_none());
        assert!(parse_request("abc 03 456").is_none(), "id 必须是整数");
        assert!(parse_request("123  456").is_none(), "集数不能是空串");
    }

    #[test]
    fn episode_id_matches_by_sort_and_type() {
        let list = episodes();
        assert_eq!(episode_id_of(&list, "03"), Some(1003));
        assert_eq!(episode_id_of(&list, "3"), Some(1003), "前导零不影响");
        assert_eq!(episode_id_of(&list, "SP1"), Some(1010), "SP 走 type=1");
    }

    #[test]
    fn episode_id_returns_none_when_absent_or_unnumbered() {
        let list = episodes();
        assert_eq!(episode_id_of(&list, "99"), None);
        // 剧场版没有编号，章节表里也对不上：按「没找到」处理，不是错误。
        assert_eq!(episode_id_of(&list, "剧场版"), None);
        assert_eq!(episode_id_of(&[], "01"), None);
    }

    #[test]
    fn scope_is_group_name_or_everything() {
        let resource = EpisodeResourceDoc {
            groups: vec!["喵萌奶茶屋".to_owned()],
            ..Default::default()
        };
        assert!(in_scope(&resource, None), "不指定字幕组 = 整集");
        assert!(in_scope(&resource, Some("喵萌奶茶屋")));
        assert!(!in_scope(&resource, Some("LoliHouse")));
    }

    #[test]
    fn outcome_render_names_both_sides() {
        let outcome = CorrectOutcome {
            from_name: "A".to_owned(),
            to_name: "B".to_owned(),
            moved: 2,
            episode_id: Some(1003),
            captions: 1,
            overwritten: 1,
            episode_id_missing: false,
        };
        let request = parse_request("1 03 2 组").expect("带组");
        let text = outcome.render(&request);
        assert!(text.contains("「A」"), "{text}");
        assert!(text.contains("「B」"), "{text}");
        assert!(text.contains("搬走 2 条资源"), "{text}");
        assert!(text.contains("新章节 id 1003"), "{text}");
        assert!(text.contains("改掉 1 条已发消息文案"), "{text}");
        assert!(text.contains("覆盖了目标番原有的 1 条"), "{text}");
    }
}
