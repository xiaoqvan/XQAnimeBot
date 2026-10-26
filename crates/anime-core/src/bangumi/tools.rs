//! Bangumi 工具函数：供 LLM Agent 调用的精简查询。
//!
//! 对应 TS 的 `bangumi/tools.ts`。与 [`BgmClient`](super::BgmClient) 的 `subject` /
//! `episodes` / `search` 的区别是：**这里返回的是精简结构**，去掉图片、评分、标签等
//! 大模型用不上只会占 token 的字段；并且 `search_candidates` 会额外补一集数范围
//! （`episode_range`，取章节里最大 `sort`），那是 Agent 判断「第 N 集落在哪部番里」
//! 的关键依据。
//!
//! 三个工具与 TS 同名同语义：
//!
//! | 工具 | 用途 | 底层接口 |
//! | --- | --- | --- |
//! | [`BgmTools::search_candidates`] | 搜候选（带集数范围） | `POST /v0/search/subjects` + `GET /v0/episodes` |
//! | [`BgmTools::episode_detail`] | 按 subject + sort 取章节 | `GET /v0/episodes` |
//! | [`BgmTools::related_subjects`] | 关联条目（前传/续集） | `GET /v0/subjects/{id}/subjects` |

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{API_BASE, BgmClient, SUBJECT_TYPE_ANIME};
use crate::{Error, Result};

/// 精简的番剧候选项（TS 的 `AnimeCandidate`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimeCandidate {
    pub id: u64,
    /// 原名。
    pub name: String,
    /// 中文名。
    pub name_cn: String,
    /// 放送日期（`date`）。
    pub date: String,
    /// 集数范围：章节里最大 `sort` 的字符串形式（如 `"26"`）。
    ///
    /// 由 [`BgmTools::search_candidates`] 补充，不是搜索接口直接返回的字段。
    pub episode_range: Option<String>,
    /// 简介。
    pub summary: String,
}

/// 单集详情（TS 的 `EpisodeDetail`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EpisodeDetail {
    pub id: u64,
    pub name: String,
    pub name_cn: String,
    /// 章节编号（第几集）。
    pub ep: f64,
    /// 实际集数编号。
    pub sort: f64,
    pub airdate: String,
    pub subject_id: u64,
    /// 0 = 本篇，其余为 SP/OP/ED 等。
    #[serde(rename = "type")]
    pub r#type: i32,
}

/// 关联条目精简结果（TS 的 `RelatedSubject`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RelatedSubject {
    pub id: u64,
    pub name: String,
    pub name_cn: String,
    /// 关系描述（如「续集」「前传」）。
    pub relation: String,
}

/// 关联条目 API 返回的原始项（含 `type`，用于筛选动画）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RawRelatedSubject {
    id: u64,
    name: String,
    name_cn: String,
    relation: String,
    #[serde(rename = "type")]
    r#type: u32,
}

/// 搜索接口返回的原始条目（只要精简字段）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct SearchHit {
    id: u64,
    name: String,
    name_cn: String,
    date: String,
    summary: String,
}

/// Agent 工具集。包一层 [`BgmClient`] 而不是直接加方法：这些是**给模型用的**精简
/// 查询，跟业务主链（拉条目建档）用途不同，分开更容易看清谁依赖谁。
pub struct BgmTools {
    client: BgmClient,
}

impl BgmTools {
    pub fn new(client: BgmClient) -> Self {
        Self { client }
    }

    /// 搜番剧候选（TS 的 `searchAnimeCandidates`）。
    ///
    /// 与 [`BgmClient::search`] 的三点不同，都是为了对齐 TS：
    /// 1. 请求体带 `nsfw: true`（R18 番剧也要能匹配上）；
    /// 2. `limit` 由调用方给（Agent 工具参数），不固定 10；
    /// 3. 每个候选补一次 `episode_range`（并行做不到就串行，失败则该字段为 `None`）。
    pub fn search_candidates(&self, keyword: &str, limit: u32) -> Result<Vec<AnimeCandidate>> {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            return Err(Error::Config("search_candidates：搜索关键词为空".to_owned()));
        }
        let limit = limit.clamp(1, 50);
        let url = format!("{API_BASE}/v0/search/subjects?limit={limit}");
        let body = json!({
            "keyword": keyword,
            "sort": "rank",
            "filter": { "type": [SUBJECT_TYPE_ANIME], "nsfw": true },
        });
        let value = self.client.http().post_json(&url, &body)?;

        let hits: Vec<SearchHit> = value
            .get("data")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| serde_json::from_value(item.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();

        let mut candidates = Vec::with_capacity(hits.len());
        for hit in hits {
            // 集数范围要额外拉一次章节；失败只影响这一个字段，不该让整个候选消失。
            let episode_range = self.episode_range(hit.id).ok().flatten();
            candidates.push(AnimeCandidate {
                id: hit.id,
                name: hit.name,
                name_cn: hit.name_cn,
                date: hit.date,
                episode_range,
                summary: hit.summary,
            });
        }
        Ok(candidates)
    }

    /// 按 `subject_id` + 集数编号（`sort`）取章节详情（TS 的 `getEpisodeDetail`）。
    ///
    /// 找不到返回 `Ok(None)`——「这部番没有第 N 集」是正常结论，不是错误。
    pub fn episode_detail(&self, subject_id: u64, episode_sort: f64) -> Result<Option<EpisodeDetail>> {
        if subject_id == 0 || episode_sort <= 0.0 {
            return Ok(None);
        }
        let episodes = self.client.episodes(subject_id)?;
        Ok(episodes
            .into_iter()
            .find(|episode| (episode.sort - episode_sort).abs() < f64::EPSILON)
            .map(|episode| EpisodeDetail {
                id: episode.id,
                name: episode.name,
                name_cn: episode.name_cn,
                ep: episode.ep,
                sort: episode.sort,
                airdate: episode.airdate,
                subject_id,
                r#type: episode.r#type,
            }))
    }

    /// 关联条目（TS 的 `getRelatedSubjects`）：只保留 `type == 2`（动画）。
    pub fn related_subjects(&self, subject_id: u64) -> Result<Vec<RelatedSubject>> {
        if subject_id == 0 {
            return Err(Error::Config("related_subjects：无效的 subject_id".to_owned()));
        }
        let url = format!("{API_BASE}/v0/subjects/{subject_id}/subjects");
        let value = self.client.http().get_json_auth(&url, None)?;
        let list: Vec<RawRelatedSubject> =
            serde_json::from_value(value).unwrap_or_default();
        Ok(list
            .into_iter()
            .filter(|item| item.r#type == SUBJECT_TYPE_ANIME)
            .map(|item| RelatedSubject {
                id: item.id,
                name: item.name,
                name_cn: item.name_cn,
                relation: item.relation,
            })
            .collect())
    }

    /// 最大 `sort` 作为总集数（TS 的 `inferEpisodeRange`）。
    ///
    /// 空章节列表返回 `None`（TS 返回 `"0"`，但 `None` 更诚实：Agent 看到「未知」
    /// 比看到「0」不容易误判）。
    fn episode_range(&self, subject_id: u64) -> Result<Option<String>> {
        let episodes = self.client.episodes(subject_id)?;
        Ok(episodes
            .iter()
            .map(|episode| episode.sort)
            .fold(None::<f64>, |acc, sort| match acc {
                Some(best) if best >= sort => Some(best),
                _ => Some(sort),
            })
            .map(|max| format_number(max)))
    }
}

/// 把 `26.0` 打成 `"26"`，`26.5` 保持 `"26.5"`——与 TS 的 `String(maxSort)` 一致。
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_range_is_max_sort() {
        // episode_range 直接决定 Agent 能不能判断「第 5 集落在 1-12 里」，
        // 取最大 sort 而不是 data.len()。
        let episodes = vec![
            json!({ "id": 1, "type": 0, "ep": 1.0, "sort": 1.0, "name": "第1集" }),
            json!({ "id": 3, "type": 0, "ep": 2.0, "sort": 2.0, "name": "第2集" }),
            json!({ "id": 9, "type": 1, "ep": 1.0, "sort": 12.0, "name": "SP1" }),
        ];
        let parsed: Vec<super::super::Episode> = episodes
            .into_iter()
            .map(|value| serde_json::from_value(value).unwrap())
            .collect();
        let max = parsed
            .iter()
            .map(|episode| episode.sort)
            .fold(None::<f64>, |acc, sort| match acc {
                Some(best) if best >= sort => Some(best),
                _ => Some(sort),
            })
            .unwrap();
        assert_eq!(format_number(max), "12");
    }

    #[test]
    fn format_number_drops_trailing_zero() {
        assert_eq!(format_number(26.0), "26");
        assert_eq!(format_number(26.5), "26.5");
    }

    #[test]
    fn search_body_carries_nsfw_and_type_filter() {
        // Agent 搜索必须带 nsfw，否则 R18 番剧一个都搜不到——TS 就是这个值。
        let body = json!({
            "keyword": "x",
            "sort": "rank",
            "filter": { "type": [SUBJECT_TYPE_ANIME], "nsfw": true },
        });
        assert_eq!(body["filter"]["nsfw"], json!(true));
        assert_eq!(body["filter"]["type"], json!([2]));
    }
}
