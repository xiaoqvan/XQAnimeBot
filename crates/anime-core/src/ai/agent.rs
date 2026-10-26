//! Bangumi 匹配 Agent：OpenAI **Responses API** + 工具调用。
//!
//! 对应 TS 的 `bangumi/bangumiAgent.ts`（1102 行）。照搬它的协议与决策流程，包括：
//!
//! - 三个工具：`searchAnimeCandidates` / `getRelatedSubjects` / `getEpisodeDetail`；
//! - **迭代循环**：模型调工具 → 我们执行 → 把结果作为 `function_call_output` 追加回
//!   `input` → 再次请求，直到模型不再调工具（给出最终 JSON）或达到迭代上限；
//! - 输出带 `subjectId` / `episodeId` / `episodeSort` / `confidence` / `reason`；
//! - **去抖动**：同一个关键词 / 同一个 subjectId / 同一个集数编号不重复查，否则模型
//!   会在两个工具之间无限交替。
//!
//! ## 为什么是 Responses API 而不是 chat/completions
//!
//! TS 用的是 `client.responses.create`，工具调用走的是 `output[]` 里的
//! `function_call` 项 + `input[]` 里的 `function_call_output` 项，和
//! chat/completions 的 `messages[].tool_calls` 是两套格式。要跟 TS 互通就得照搬
//! 这一套——这也是本模块存在的唯一理由（[`crate::ai::AiClient`] 那边仍是
//! chat/completions 的两次单发，职责是「AI 兜底」，不是 Agent）。
//!
//! ## 设计约束
//!
//! - **Agent 只做匹配，不做业务判断**。它返回 `MatchResult`，要不要下载、发不发频道
//!   由调用方（插件的 poll/chain）决定。
//! - **任何异常都降级成 `confidence: 0`**，而不是把错误抛上去：匹配失败只是「这条要
//!   人工审核」，不该让一轮 RSS 轮询挂掉。
//! - **规则优先**：唯一候选、或集数范围能唯一定位时直接返回（不烧 token）；只有当
//!   规则无法区分时才进 LLM 决策循环。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ai::AiConfig;
use crate::bangumi::{AnimeCandidate, BgmTools};
use crate::rss::Http;
use crate::Result;

/// Agent 请求超时。工具循环最多 15 轮，单轮给 60 秒。
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// `llmDecision` 的迭代上限（TS 的同名常量是 15）。
const MAX_DECISION_ITERATIONS: usize = 15;

/// `aiEpisodeSearch` 的迭代上限（TS 是 8）。
const MAX_EPISODE_ITERATIONS: usize = 8;

/// 工具调用时的采样温度。匹配要稳定，别让模型发挥。
const TEMPERATURE: f64 = 0.1;

/// 匹配输入（TS 的 `bangumiAgent.ts::animeItem`）。
#[derive(Debug, Clone, Default)]
pub struct MatchInput {
    /// BT 标题。
    pub title: String,
    /// 标题解析出的番剧名（按优先级）。
    pub names: Vec<String>,
    /// 解析出的集数字符串（`"05"` / `"SP1"` / `None`）。
    pub episode: Option<String>,
}

/// 匹配结果（TS 的 `MatchResult`，字段名逐字对齐以便落库互通）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchResult {
    /// 命中的 Bangumi 条目 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<u64>,
    /// 命中的章节 id（精确到集）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<u64>,
    /// 命中的集数编号（章节的 `sort`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_sort: Option<u64>,
    /// 置信度 0~1。**不是** 0~100。
    pub confidence: f64,
    /// 判定理由（含迭代流程，直接进日志/审核消息）。
    pub reason: String,
}

impl MatchResult {
    /// 匹配失败（置信度 0）。所有异常路径都落到这里。
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            confidence: 0.0,
            reason: reason.into(),
            ..Default::default()
        }
    }

    /// 是否足够自信到「先发后审」（TS 的 `confidence >= 0.9`）。
    pub fn is_confident(&self) -> bool {
        self.confidence >= 0.9 && self.subject_id.is_some()
    }
}

/// 工具定义（TS 的 `TOOL_DEFINITIONS`）。`strict: false`：不是所有模型都支持严格模式。
fn tool_definitions() -> Value {
    json!([
        {
            "type": "function",
            "name": "searchAnimeCandidates",
            "strict": false,
            "description": "使用关键词搜索 Bangumi 番剧候选列表。返回 AnimeCandidate[]，包含 id、名称、中文名、放送日期、集数范围和简介。在搜索别名或模糊名称时调用此工具。",
            "parameters": {
                "type": "object",
                "properties": {
                    "keyword": {
                        "type": "string",
                        "description": "搜索关键词，可以是番剧原名、中文名、别名等"
                    },
                    "limit": {
                        "type": "number",
                        "description": "返回结果数量上限（1-50），默认 10",
                        "default": 10
                    }
                },
                "required": ["keyword"]
            }
        },
        {
            "type": "function",
            "name": "getRelatedSubjects",
            "strict": false,
            "description": "获取指定 Bangumi 条目的关联作品列表（如前传、续集、番外篇等）。仅返回 type=2（动画）的关联条目。当需要区分多季番剧或查找关联作品时调用此工具。",
            "parameters": {
                "type": "object",
                "properties": {
                    "subjectId": {
                        "type": "number",
                        "description": "Bangumi 条目 ID"
                    }
                },
                "required": ["subjectId"]
            }
        },
        {
            "type": "function",
            "name": "getEpisodeDetail",
            "strict": false,
            "description": "获取指定 Bangumi 条目中某集的具体详情，包括章节 ID（id）、集数编号（sort）和放送日期。当确定了 subjectId 并且输入包含集数信息时，调用此工具获取精确的章节 ID。",
            "parameters": {
                "type": "object",
                "properties": {
                    "subjectId": {
                        "type": "number",
                        "description": "Bangumi 条目 ID"
                    },
                    "episodeSort": {
                        "type": "number",
                        "description": "集数编号，如第 5 集则传入 5"
                    }
                },
                "required": ["subjectId", "episodeSort"]
            }
        }
    ])
}

/// 番剧匹配的系统提示（TS 的 `SYSTEM_PROMPT`）。
const SYSTEM_PROMPT: &str = r#"你是 Bangumi 动画匹配 Agent。

目标：
根据动漫名称、别名、集数、候选条目和关联条目找到最准确的 Bangumi Subject ID，
并尽可能精确到具体的章节 ID（集数）。

优先级：
1. episode_range - 集数范围匹配（如第5集落在1-12范围内）
2. 中文名称 (name_cn)
3. 原名 (name)
4. 放送日期 (date)
5. 前传/续集关系 (relation)

可用工具：
- searchAnimeCandidates: 搜索番剧候选项
- getRelatedSubjects: 获取关联条目（前传/续集/番外）
- getEpisodeDetail: 获取指定条目中某集的具体信息（包括集数 ID）

匹配流程建议：
1. 先调用 searchAnimeCandidates 搜索候选
2. 如果候选能确定 subjectId，且输入包含集数信息，调用 getEpisodeDetail 查找匹配的集数 ID
3. 如果仍不确定，调用 getRelatedSubjects 查看关联条目

限制：
- 禁止在 searchAnimeCandidates、getRelatedSubjects 和 getEpisodeDetail 之间无限交替调用。
- 如果已经对某个条目调过 getRelatedSubjects，不要再次对相同条目重复调用。
- 优先根据已有信息得出结论，而不是无限制地探索。
- 只有当 episodeSort 完全匹配时才填入 episodeId。

confidence 必须是 0~1 之间的小数（如 0.97），不是 0~100。
- confidence = 1 表示完全确定
- confidence = 0 表示完全无法匹配

最终必须返回 JSON，格式如下：
{
  "subjectId": 123,
  "episodeId": 456,
  "episodeSort": 5,
  "confidence": 0.97,
  "reason": "..."
}

如果能确定条目但无法确定具体集数，只返回 subjectId：
{
  "subjectId": 123,
  "confidence": 0.97,
  "reason": "..."
}

匹配不上时返回：
{
  "confidence": 0,
  "reason": "..."
}

禁止输出 markdown 代码块标记或其他包装文本。"#;

/// 集数匹配的系统提示（TS 的 `AI_EPISODE_SYSTEM_PROMPT`）。
const EPISODE_SYSTEM_PROMPT: &str = r#"你是 Bangumi 集数匹配助手。
根据番剧信息和集数字符串，通过工具调用迭代查找最准确的集数编号（sort 值）和章节 ID（id）。

番剧信息包含原名、中文名、放送日期和集数范围。
集数字符串可能包含"第X集"、"EP X"、"X話"等各种格式。

可用工具：
- searchAnimeCandidates: 搜索番剧候选项。当需要从当前候选之外搜索更多相关信息时调用。
- getRelatedSubjects: 获取关联条目（前传/续集/番外）。当集数可能属于关联条目时调用。
- getEpisodeDetail: 获取指定条目中某集的具体详情。调用此工具验证集数是否存在并获取章节 ID。

流程建议：
1. 根据番剧信息和集数字符串确定目标条目
2. 调用 getEpisodeDetail 验证集数是否存在
3. 如果不在当前条目中，可调用 getRelatedSubjects 查看关联条目
4. 需要更多信息时调用 searchAnimeCandidates
5. 重复直到找到匹配的集数或确定无法匹配

限制：
- 不要对相同的集数编号重复调用 getEpisodeDetail
- 优先根据已有信息得出结论，而不是无限制地探索

最终必须返回 JSON，格式如下：
{
  "episodeSort": 5,
  "episodeId": 12345,
  "reason": "解释为什么是这个集数以及验证过程"
}

如果完全无法确定，返回：
{
  "episodeSort": null,
  "episodeId": null,
  "reason": "..."
}

禁止输出 markdown 代码块标记或其他包装文本。"#;

/// 一次工具调用（Responses API `output[]` 里的 `function_call` 项）。
#[derive(Debug, Clone)]
struct FunctionCall {
    call_id: String,
    name: String,
    /// 原始参数字符串（模型给的是字符串化的 JSON，可能不合法）。
    arguments: String,
}

/// 去重追踪：防止模型在两个工具之间无限交替。
#[derive(Debug, Default)]
struct Dedup {
    keywords: Vec<String>,
    subject_ids: Vec<u64>,
    episode_sorts: Vec<u64>,
}

impl Dedup {
    fn mark_keyword(&mut self, keyword: &str) -> bool {
        let key = keyword.trim().to_lowercase();
        if self.keywords.contains(&key) {
            return false;
        }
        self.keywords.push(key);
        true
    }

    fn mark_subject(&mut self, id: u64) -> bool {
        if self.subject_ids.contains(&id) {
            return false;
        }
        self.subject_ids.push(id);
        true
    }

    fn mark_episode_sort(&mut self, sort: u64) -> bool {
        if self.episode_sorts.contains(&sort) {
            return false;
        }
        self.episode_sorts.push(sort);
        true
    }
}

/// Bangumi 匹配 Agent。
pub struct BangumiAgent {
    config: AiConfig,
    tools: BgmTools,
    http: Http,
}

impl BangumiAgent {
    pub fn new(config: AiConfig, tools: BgmTools) -> Self {
        Self {
            config,
            tools,
            http: Http::new(TIMEOUT),
        }
    }

    /// 配置齐了没（enabled + api_key）。没配时 [`Self::match_subject`] 直接返回
    /// `confidence: 0`，调用方不必自己判断。
    pub fn usable(&self) -> bool {
        self.config.usable()
    }

    /// 主入口：把一条 BT 条目匹配到 Bangumi 条目（TS 的 `matchAnimeSubject`）。
    ///
    /// 流程与 TS 一致：
    /// 1. 遍历 `title` + `names` 搜候选，按 id 合并去重；
    /// 2. 没候选 → `confidence: 0`；
    /// 3. 候选数 ≤ `llm_threshold` → 规则匹配（唯一候选 / 集数范围命中）；
    /// 4. 集数范围能缩小范围 → 用缩小后的候选；
    /// 5. 仍不确定 → 进 LLM 工具调用循环。
    pub fn match_subject(&self, input: &MatchInput, llm_threshold: usize) -> MatchResult {
        if !self.usable() {
            return MatchResult::failed("未配置 AI（ai.enabled=false 或缺少 api_key）");
        }
        if input.title.trim().is_empty() {
            return MatchResult::failed("输入数据无效：缺少 title");
        }

        // ── 第一阶段：搜索合并 ──────────────────────────────────────────
        let mut candidates: Vec<AnimeCandidate> = Vec::new();
        for name in std::iter::once(&input.title).chain(input.names.iter()) {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            // 单个搜索失败不影响其他搜索（TS 也是这个策略）。
            if let Ok(found) = self.tools.search_candidates(name, 10) {
                for candidate in found {
                    if !candidates.iter().any(|existing| existing.id == candidate.id) {
                        candidates.push(candidate);
                    }
                }
            }
        }

        if candidates.is_empty() {
            return MatchResult::failed("未找到候选");
        }

        // ── 第三阶段：候选数 ≤ 阈值，规则匹配 ───────────────────────────
        if candidates.len() <= llm_threshold {
            if let Some(result) = self.rule_match(&candidates, input) {
                return result;
            }
            return MatchResult::failed(format!(
                "候选数 {} 未超过阈值 {llm_threshold}，但规则无法决策",
                candidates.len()
            ));
        }

        // ── 第四阶段：集数范围缩小候选 ──────────────────────────────────
        let narrowed = match (input.episode.as_deref(), extract_episode_number(input.episode.as_deref().unwrap_or_default())) {
            (Some(raw), Some(number)) => {
                let matched: Vec<AnimeCandidate> = candidates
                    .iter()
                    .filter(|candidate| is_episode_in_range(number, candidate.episode_range.as_deref()))
                    .cloned()
                    .collect();
                // 唯一命中直接定案；多个命中就缩小范围交给 LLM。
                if matched.len() == 1 {
                    return self.settle_subject(&matched[0], raw, number, "集数范围命中");
                }
                if matched.len() > 1 {
                    matched
                } else {
                    candidates
                }
            }
            _ => candidates,
        };

        // ── 第五阶段：LLM 决策 ──────────────────────────────────────────
        self.llm_decision(input, &narrowed)
    }

    /// 规则匹配：唯一候选 / 集数范围唯一命中（TS 的第三阶段）。
    ///
    /// 返回 `None` 表示规则区分不了，交给 LLM。
    fn rule_match(&self, candidates: &[AnimeCandidate], input: &MatchInput) -> Option<MatchResult> {
        let raw_episode = input.episode.as_deref()?;
        let number = extract_episode_number(raw_episode)?;

        if candidates.len() == 1 {
            return Some(self.settle_subject(&candidates[0], raw_episode, number, "唯一候选"));
        }

        let matched: Vec<AnimeCandidate> = candidates
            .iter()
            .filter(|candidate| is_episode_in_range(number, candidate.episode_range.as_deref()))
            .cloned()
            .collect();
        if matched.len() == 1 {
            return Some(self.settle_subject(&matched[0], raw_episode, number, "集数范围命中"));
        }
        None
    }

    /// 条目已定，补集数（先规则取章节，失败再用 AI 辅助），然后给出置信度。
    ///
    /// 置信度沿用 TS 的取值：集数也匹配上 0.97~0.98，只有条目 0.95，AI 辅助 0.95~0.96。
    fn settle_subject(
        &self,
        candidate: &AnimeCandidate,
        raw_episode: &str,
        number: u64,
        why: &str,
    ) -> MatchResult {
        match self.tools.episode_detail(candidate.id, number as f64) {
            Ok(Some(detail)) => MatchResult {
                subject_id: Some(candidate.id),
                episode_id: Some(detail.id),
                episode_sort: Some(number),
                confidence: 0.97,
                reason: format!("{why} + 集数 {number} 匹配（章节 ID: {}）", detail.id),
            },
            Ok(None) => {
                // 规则取不到（比如集数是「剧场版」或番剧章节还没录入），让 AI 辅助。
                match self.ai_episode_search(candidate.id, raw_episode, Some(candidate)) {
                    Some((episode_id, episode_sort)) => MatchResult {
                        subject_id: Some(candidate.id),
                        episode_id: Some(episode_id),
                        episode_sort: Some(episode_sort),
                        confidence: 0.95,
                        reason: format!(
                            "{why} + AI 辅助集数匹配（集数 {episode_sort}，章节 ID: {episode_id}）"
                        ),
                    },
                    None => MatchResult {
                        subject_id: Some(candidate.id),
                        episode_id: None,
                        episode_sort: None,
                        confidence: 0.95,
                        reason: format!("{why}（集数 {raw_episode} 未匹配到章节）"),
                    },
                }
            }
            Err(error) => MatchResult {
                subject_id: Some(candidate.id),
                episode_id: None,
                episode_sort: None,
                confidence: 0.95,
                reason: format!("{why}（查章节失败：{error}）"),
            },
        }
    }

    /// LLM 决策循环（TS 的 `llmDecision`，迭代上限 15）。
    fn llm_decision(&self, input: &MatchInput, candidates: &[AnimeCandidate]) -> MatchResult {
        let mut items: Vec<Value> = vec![json!({
            "role": "user",
            "type": "message",
            "content": build_prompt(input, candidates),
        })];
        let mut dedup = Dedup::default();
        let mut log = vec![format!("LLM 决策开始，候选数: {}", candidates.len())];

        for iteration in 1..=MAX_DECISION_ITERATIONS {
            let response = match self.responses(SYSTEM_PROMPT, &items) {
                Ok(response) => response,
                Err(error) => return MatchResult::failed(format!("LLM 调用失败: {error}")),
            };

            let calls = extract_function_calls(&response);
            if calls.is_empty() {
                log.push(format!("第 {iteration} 轮迭代: LLM 给出最终决策"));
                let text = extract_output_text(&response);
                let mut result = parse_match_result(&text);
                result.reason.push_str(&format!("\n\n迭代流程:\n{}", log.join("\n")));
                return result;
            }

            // assistant 的 function_call 先入 input，再逐个补 function_call_output。
            for call in &calls {
                items.push(json!({
                    "type": "function_call",
                    "call_id": call.call_id,
                    "name": call.name,
                    "arguments": call.arguments,
                }));
            }
            for call in &calls {
                let output = self.execute(call, &mut dedup);
                items.push(json!({
                    "type": "function_call_output",
                    "call_id": call.call_id,
                    "output": output,
                }));
            }
            let names: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
            log.push(format!("第 {iteration} 轮迭代: 调用工具 [{}]", names.join(", ")));
        }

        MatchResult::failed(format!(
            "LLM 工具调用达到最大迭代次数，未能得出结果\n\n迭代流程:\n{}",
            log.join("\n")
        ))
    }

    /// AI 辅助集数匹配（TS 的 `aiEpisodeSearch`，迭代上限 8）。
    ///
    /// 返回 `(episode_id, episode_sort)`；无法确定返回 `None`。
    pub fn ai_episode_search(
        &self,
        subject_id: u64,
        episode: &str,
        candidate: Option<&AnimeCandidate>,
    ) -> Option<(u64, u64)> {
        if !self.usable() {
            return None;
        }
        let mut items: Vec<Value> = vec![json!({
            "role": "user",
            "type": "message",
            "content": build_episode_prompt(subject_id, episode, candidate),
        })];
        let mut dedup = Dedup::default();

        for _ in 1..=MAX_EPISODE_ITERATIONS {
            let response = match self.responses(EPISODE_SYSTEM_PROMPT, &items) {
                Ok(response) => response,
                Err(_) => return None,
            };

            let calls = extract_function_calls(&response);
            if calls.is_empty() {
                let text = extract_output_text(&response);
                let value = serde_json::from_str::<Value>(&extract_json_from_text(&text)).ok()?;
                let sort = value.get("episodeSort")?.as_u64()?;
                let id = value.get("episodeId")?.as_u64()?;
                if sort > 0 && id > 0 {
                    return Some((id, sort));
                }
                return None;
            }

            for call in &calls {
                items.push(json!({
                    "type": "function_call",
                    "call_id": call.call_id,
                    "name": call.name,
                    "arguments": call.arguments,
                }));
            }
            for call in &calls {
                let output = self.execute(call, &mut dedup);
                items.push(json!({
                    "type": "function_call_output",
                    "call_id": call.call_id,
                    "output": output,
                }));
            }
        }
        None
    }

    /// 调一次 Responses API。
    fn responses(&self, instructions: &str, input: &[Value]) -> Result<Value> {
        let url = format!(
            "{}/responses",
            self.config.base_url.trim_end_matches('/')
        );
        let body = json!({
            "model": self.config.model,
            "instructions": instructions,
            "input": input,
            "tools": tool_definitions(),
            "temperature": TEMPERATURE,
        });
        self.http
            .post_json_auth(&url, &body, Some(&self.config.api_key))
    }

    /// 执行一个工具调用，返回给模型的字符串结果。
    ///
    /// 参数不合法 / 工具执行异常都**返回错误 JSON 而不是报错**——模型看到错误会自己
    /// 换一种方式重试，比整条链断掉好（TS 的 `executeFunctionCall` 也是这个策略）。
    fn execute(&self, call: &FunctionCall, dedup: &mut Dedup) -> String {
        let args: Value = match serde_json::from_str(&call.arguments) {
            Ok(args) => args,
            Err(_) => {
                return json!({ "error": format!("无法解析工具参数 JSON: {}", call.arguments) })
                    .to_string()
            }
        };

        match call.name.as_str() {
            "searchAnimeCandidates" => {
                let Some(keyword) = args.get("keyword").and_then(Value::as_str) else {
                    return json!({ "error": "缺少 keyword" }).to_string();
                };
                if !dedup.mark_keyword(keyword) {
                    return json!({
                        "_notice": format!("关键词 \"{keyword}\" 已搜索过，跳过重复调用"),
                        "data": []
                    })
                    .to_string();
                }
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|limit| limit.clamp(1, 50) as u32)
                    .unwrap_or(10);
                match self.tools.search_candidates(keyword, limit) {
                    Ok(list) => serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_owned()),
                    Err(error) => json!({ "error": format!("工具执行异常: {error}") }).to_string(),
                }
            }
            "getRelatedSubjects" => {
                let Some(subject_id) = args.get("subjectId").and_then(Value::as_u64) else {
                    return json!({ "error": "缺少 subjectId" }).to_string();
                };
                if !dedup.mark_subject(subject_id) {
                    return json!({
                        "_notice": format!("条目 {subject_id} 的关联作品已查过，跳过重复调用"),
                        "data": []
                    })
                    .to_string();
                }
                match self.tools.related_subjects(subject_id) {
                    Ok(list) => serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_owned()),
                    Err(error) => json!({ "error": format!("工具执行异常: {error}") }).to_string(),
                }
            }
            "getEpisodeDetail" => {
                let Some(subject_id) = args.get("subjectId").and_then(Value::as_u64) else {
                    return json!({ "error": "缺少 subjectId" }).to_string();
                };
                let Some(sort) = args.get("episodeSort").and_then(Value::as_u64) else {
                    return json!({ "error": "缺少 episodeSort" }).to_string();
                };
                if !dedup.mark_episode_sort(sort) {
                    return json!({
                        "_notice": format!("集数 {sort} 已查过，跳过重复调用"),
                        "data": null
                    })
                    .to_string();
                }
                match self.tools.episode_detail(subject_id, sort as f64) {
                    Ok(detail) => serde_json::to_string(&detail).unwrap_or_else(|_| "null".to_owned()),
                    Err(error) => json!({ "error": format!("工具执行异常: {error}") }).to_string(),
                }
            }
            other => json!({ "error": format!("未知工具: {other}") }).to_string(),
        }
    }
}

/// 从 Responses API 的输出里取出所有 `function_call` 项。
fn extract_function_calls(response: &Value) -> Vec<FunctionCall> {
    response
        .get("output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
                .filter_map(|item| {
                    Some(FunctionCall {
                        call_id: item.get("call_id")?.as_str()?.to_owned(),
                        name: item.get("name")?.as_str()?.to_owned(),
                        arguments: item.get("arguments")?.as_str().unwrap_or_default().to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 从 Responses API 的输出里拼接所有文本（TS 的 `extractOutputText`）。
fn extract_output_text(response: &Value) -> String {
    response
        .get("output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
                .filter_map(|item| item.get("content"))
                .filter_map(Value::as_array)
                .flatten()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
                .filter_map(|part| part.get("text"))
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// 从模型文本里剥出 JSON（支持 ```json 代码块 / 裸 `{...}` / 文字混排）。
fn extract_json_from_text(raw: &str) -> String {
    // ```json ... ```
    if let Some(start) = raw.find("```") {
        let rest = &raw[start + 3..];
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        if let Some(end) = rest.find("```") {
            return rest[..end].trim().to_owned();
        }
    }
    // 最外层 { ... }
    if let (Some(start), Some(end)) = (raw.find('{'), raw.rfind('}')) {
        if start <= end {
            return raw[start..=end].to_owned();
        }
    }
    raw.trim().to_owned()
}

/// 解析最终 JSON；失败则正则兜底，再失败给 `confidence: 0`。
///
/// TS 的 `safeParseMatchResult`：先标准 JSON，再 `extractFieldsViaRegex`。
fn parse_match_result(raw: &str) -> MatchResult {
    let json_text = extract_json_from_text(raw);
    if let Ok(value) = serde_json::from_str::<Value>(&json_text)
        && let Ok(result) = serde_json::from_value::<MatchResult>(value)
    {
        // confidence 超出 0~1 的当无效（模型偶尔给 97）。
        if (0.0..=1.0).contains(&result.confidence) {
            return result;
        }
    }
    // 正则兜底：模型输出不标准时至少把关键字段捞出来。
    if let Some(result) = extract_fields_via_regex(&json_text) {
        return result;
    }
    MatchResult::failed(format!("LLM 返回结果解析失败，原始输出: {raw}"))
}

/// 正则兜底（TS 的 `extractFieldsViaRegex`）。
fn extract_fields_via_regex(raw: &str) -> Option<MatchResult> {
    let subject_id = capture_number(raw, r#""subjectId"\s*:\s*(\d+)"#);
    let episode_id = capture_number(raw, r#""episodeId"\s*:\s*(\d+)"#);
    let episode_sort = capture_number(raw, r#""episodeSort"\s*:\s*(\d+)"#);
    let confidence = raw
        .find(r#""confidence""#)
        .and_then(|start| {
            let rest = &raw[start..];
            let colon = rest.find(':')?;
            let after = rest[colon + 1..].trim_start();
            let end = after
                .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
                .unwrap_or(after.len());
            after[..end].parse::<f64>().ok()
        })
        .filter(|value| (0.0..=1.0).contains(value))?;
    let reason = capture_string(raw).unwrap_or_else(|| "（正则兜底提取）".to_owned());
    Some(MatchResult {
        subject_id,
        episode_id,
        episode_sort,
        confidence,
        reason,
    })
}

fn capture_number(raw: &str, pattern: &str) -> Option<u64> {
    let regex = regex::Regex::new(pattern).ok()?;
    regex
        .captures(raw)
        .and_then(|captures| captures.get(1))
        .and_then(|matched| matched.as_str().parse().ok())
}

fn capture_string(raw: &str) -> Option<String> {
    let regex = regex::Regex::new(r#""reason"\s*:\s*"([^"]*)""#).ok()?;
    regex
        .captures(raw)
        .and_then(|captures| captures.get(1))
        .map(|matched| matched.as_str().trim().to_owned())
}

/// 从集数字符串里抽数字（TS 的 `extractEpisodeNumber`）：`"37"`/`"第37集"`/`"EP37"` → 37。
pub fn extract_episode_number(raw: &str) -> Option<u64> {
    let digits: String = raw
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// 集数是否落在 `episode_range` 内（TS 的 `isEpisodeInRange`）。
///
/// 支持 `"26"`（单值）与 `"1-26"`（区间）两种写法；范围未知（`None`）时返回 `false`
/// —— 未知不该被当成「命中」，否则会把所有候选都留下。
pub fn is_episode_in_range(episode: u64, range: Option<&str>) -> bool {
    let Some(range) = range else {
        return false;
    };
    let range = range.trim();
    if range.is_empty() {
        return false;
    }
    if let Some((low, high)) = range.split_once('-') {
        let (Ok(low), Ok(high)) = (low.trim().parse::<u64>(), high.trim().parse::<u64>()) else {
            return false;
        };
        return episode >= low && episode <= high;
    }
    range.parse::<u64>().map(|total| episode <= total).unwrap_or(false)
}

/// 构建决策提示（TS 的 `buildLLMPrompt`）。简介截断到 120 字，别烧 token。
fn build_prompt(input: &MatchInput, candidates: &[AnimeCandidate]) -> String {
    let mut lines = vec![
        "请匹配以下番剧到 Bangumi 条目：".to_owned(),
        String::new(),
        format!("标题: {}", input.title),
    ];
    if !input.names.is_empty() {
        lines.push(format!("别名: {}", input.names.join("、")));
    }
    if let Some(episode) = input.episode.as_deref() {
        lines.push(format!("集数: {episode}"));
    }
    lines.push(String::new());
    lines.push(format!("当前候选列表 ({} 个):", candidates.len()));
    lines.push(String::new());
    for candidate in candidates {
        lines.push(format!("- ID: {}", candidate.id));
        lines.push(format!("  原名: {}", candidate.name));
        lines.push(format!(
            "  中文名: {}",
            empty_or(&candidate.name_cn, "(无)")
        ));
        lines.push(format!("  放送日期: {}", empty_or(&candidate.date, "(未知)")));
        lines.push(format!("  集数范围: {}", candidate.episode_range.as_deref().unwrap_or("(未知)")));
        lines.push(format!("  简介: {}", truncate(&candidate.summary, 120).unwrap_or_else(|| "(无)".to_owned())));
        lines.push(String::new());
    }
    lines.push("如果需要更多信息，请调用 searchAnimeCandidates、getRelatedSubjects 或 getEpisodeDetail 工具。".to_owned());
    lines.push("当你能确定 subjectId 并且输入包含集数信息时，调用 getEpisodeDetail 获取精确的章节 ID（episodeId）。".to_owned());
    lines.push("确定结果后，请直接返回 JSON，不要包含 markdown 代码块标记。".to_owned());
    lines.join("\n")
}

/// 构建集数匹配的提示（TS 的 `aiEpisodeSearch` 里的 `candidateLines`）。
fn build_episode_prompt(
    subject_id: u64,
    episode: &str,
    candidate: Option<&AnimeCandidate>,
) -> String {
    let mut lines = Vec::new();
    if let Some(candidate) = candidate {
        lines.push("番剧信息：".to_owned());
        lines.push(format!("- 原名: {}", candidate.name));
        lines.push(format!("- 中文名: {}", empty_or(&candidate.name_cn, "(无)")));
        lines.push(format!("- 放送日期: {}", empty_or(&candidate.date, "(未知)")));
        lines.push(format!("- 集数范围: {}", candidate.episode_range.as_deref().unwrap_or("(未知)")));
        lines.push(format!("- 简介: {}", empty_or(&candidate.summary, "(无)")));
        lines.push(String::new());
    } else {
        lines.push(format!("条目 ID: {subject_id}"));
        lines.push(String::new());
    }
    lines.push(format!("集数字符串: \"{episode}\""));
    lines.push(String::new());
    lines.push("可用工具：searchAnimeCandidates（搜索更多候选项）、getRelatedSubjects（查看关联条目）、getEpisodeDetail（验证集数）。".to_owned());
    lines.push("请先确定目标条目，然后调用 getEpisodeDetail 验证集数。".to_owned());
    lines.join("\n")
}

fn empty_or(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

fn truncate(text: &str, max_len: usize) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_len {
        return Some(text.to_owned());
    }
    Some(format!("{}...", chars[..max_len].iter().collect::<String>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_episode_number_handles_common_forms() {
        assert_eq!(extract_episode_number("37"), Some(37));
        assert_eq!(extract_episode_number("第37集"), Some(37));
        assert_eq!(extract_episode_number("EP37"), Some(37));
        assert_eq!(extract_episode_number("ep.37"), Some(37));
        assert_eq!(extract_episode_number("SP"), None);
        assert_eq!(extract_episode_number(""), None);
    }

    #[test]
    fn episode_range_matches_single_and_interval() {
        assert!(is_episode_in_range(5, Some("12")));
        assert!(!is_episode_in_range(13, Some("12")));
        assert!(is_episode_in_range(5, Some("1-12")));
        assert!(!is_episode_in_range(13, Some("1-12")));
        // 未知范围不算命中：否则会把所有候选都留下。
        assert!(!is_episode_in_range(5, None));
        assert!(!is_episode_in_range(5, Some("")));
        assert!(!is_episode_in_range(5, Some("abc")));
    }

    #[test]
    fn json_is_extracted_from_code_blocks_and_prose() {
        assert_eq!(
            extract_json_from_text("```json\n{\"confidence\":0.5}\n```"),
            "{\"confidence\":0.5}"
        );
        assert_eq!(extract_json_from_text("说明：\n{\"confidence\":0.5}\n结束"), "{\"confidence\":0.5}");
        assert_eq!(extract_json_from_text("no json here"), "no json here");
    }

    #[test]
    fn parse_match_result_accepts_plain_json() {
        let result = parse_match_result(r#"{"subjectId":123,"episodeId":456,"episodeSort":5,"confidence":0.97,"reason":"ok"}"#);
        assert_eq!(result.subject_id, Some(123));
        assert_eq!(result.episode_id, Some(456));
        assert_eq!(result.episode_sort, Some(5));
        assert!((result.confidence - 0.97).abs() < 1e-9);
        assert!(result.is_confident());
    }

    #[test]
    fn parse_match_result_falls_back_to_regex() {
        // 模型输出不标准（多了尾逗号）：至少把字段捞出来。
        let result = parse_match_result("我认为：{\"subjectId\":123, \"confidence\":0.9, \"reason\":\"x\",}");
        assert_eq!(result.subject_id, Some(123));
        assert!((result.confidence - 0.9).abs() < 1e-9);
    }

    #[test]
    fn parse_match_result_rejects_out_of_range_confidence() {
        // 模型偶尔给 0~100。超范围的置信度不能当真。
        let result = parse_match_result(r#"{"subjectId":1,"confidence":97,"reason":"x"}"#);
        assert!(result.confidence <= 1.0, "{result:?}");
    }

    #[test]
    fn unparsable_output_fails_softly() {
        let result = parse_match_result("完全不是 JSON");
        assert_eq!(result.confidence, 0.0);
        assert!(result.subject_id.is_none());
    }

    #[test]
    fn function_calls_and_text_are_extracted_from_response() {
        let response = json!({
            "output": [
                {"type":"function_call","call_id":"c1","name":"searchAnimeCandidates","arguments":"{\"keyword\":\"x\"}"},
                {"type":"message","content":[{"type":"output_text","text":"最终结果"}]}
            ]
        });
        let calls = extract_function_calls(&response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "searchAnimeCandidates");
        assert_eq!(extract_output_text(&response), "最终结果");
    }

    #[test]
    fn dedup_blocks_repeated_calls() {
        let mut dedup = Dedup::default();
        assert!(dedup.mark_keyword("Frieren"));
        assert!(!dedup.mark_keyword("frieren"), "大小写不同也算同一个");
        assert!(dedup.mark_subject(400602));
        assert!(!dedup.mark_subject(400602));
        assert!(dedup.mark_episode_sort(5));
        assert!(!dedup.mark_episode_sort(5));
    }

    #[test]
    fn disabled_agent_never_calls_out() {
        // 没配 api_key 时 match_subject 直接返回 confidence 0，不发任何请求。
        let tools = BgmTools::new(crate::bangumi::BgmClient::new(Default::default()));
        let agent = BangumiAgent::new(AiConfig::default(), tools);
        assert!(!agent.usable());
        let result = agent.match_subject(
            &MatchInput {
                title: "[组] 某番 - 01".to_owned(),
                names: vec!["某番".to_owned()],
                episode: Some("01".to_owned()),
            },
            1,
        );
        assert_eq!(result.confidence, 0.0);
        assert!(result.subject_id.is_none());
    }

    #[test]
    fn empty_title_is_rejected_before_any_call() {
        let tools = BgmTools::new(crate::bangumi::BgmClient::new(Default::default()));
        let agent = BangumiAgent::new(
            AiConfig {
                enabled: true,
                api_key: "test".to_owned(),
                ..Default::default()
            },
            tools,
        );
        let result = agent.match_subject(&MatchInput::default(), 1);
        assert_eq!(result.confidence, 0.0);
        assert!(result.reason.contains("缺少 title"), "{}", result.reason);
    }
}
