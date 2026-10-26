//! 数据访问层：把「业务要什么」和「宿主怎么存」隔开。
//!
//! 对应 TS 的 database/query.ts 与 database/create.ts，但这里**只定义接口和查询构造**：
//! 真正发请求的是宿主注册的 storage.mongo 服务。anime-core 不认识宿主，也不链接
//! MongoDB 驱动，所以本模块既能在没有数据库的环境里跑单测，也不会把宿主的连接生命周期
//! 带进领域层。
//!
//! ## 为什么是同步 trait
//!
//! 宿主的服务调用本身是同步的（插件壳在服务线程上把异步驱动摊平），TS 业务代码也是
//! 同步写法。这里保持同步，避免把 async 传染给匹配、投递整条链路；将来真需要异步
//! 存储，再加适配层比现在先铺一层 async_trait 更省事。
//!
//! ## 约定
//!
//! - 集合名与 TS 完全一致：anime / episodes_meta / torrents / cacheItem / config。
//! - 过滤、投影、排序都是 Mongo Shell 语义的原样 JSON，交给宿主解释；本层不拼聚合管道。
//! - 时间字段用 chrono::Utc::now()，序列化交给 serde（RFC 3339 字符串），不手写
//!   Extended JSON 的 $date：宿主用 serde 把 JSON 转 BSON，业务层多认识一种日期编码
//!   只会多一处漂移。
//! - 所有失败统一包成 [Error::Store]，消息里带集合名和动作，日志能直接定位。
//!
//! ## 字段对齐
//!
//! 文档结构见 [crate::models]。投影只取业务需要的列，反序列化后仍然走
//! [crate::models::Anime] / [crate::models::EpisodeMeta]，避免两套字段名各自漂移。

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::matching::AnimeCandidate;
use crate::models::{Anime, AnimeItem, EpisodeMeta, TorrentDoc, TorrentStatus};
use crate::title::clean_title;
use crate::{Error, Result};

/// `pendingReviews` 里 `status` 的初始值。
///
/// 动作后的终态由调用方约定（插件侧用 `"approved"` / `"rejected"`）；本层只保证
/// 「新写入一定是 pending」，不替业务定义状态机。
pub const PENDING_REVIEW_STATUS: &str = "pending";

/// 宿主 storage.mongo 服务的最小接口。
///
/// 刻意**不**暴露驱动类型（Collection / Document / Cursor），只保留 JSON 进出的
/// 五个动作。这样插件壳可以用服务表实现它，单测可以用内存实现它，而业务查询只依赖
/// 这个 trait——不会因为换存储（Mongo / 内存 / 远程）改一遍业务代码。
///
/// 集合不存在时按 MongoDB 语义视为空集合，不要报错。
pub trait Storage {
    /// 查询多条文档。
    ///
    /// sort 形如 {"updatedAt": -1}；limit 为 None 表示不限条数（由宿主决定上限）。
    fn find(
        &self,
        collection: &str,
        filter: Value,
        projection: Option<Value>,
        sort: Option<Value>,
        limit: Option<usize>,
    ) -> Result<Vec<Value>>;

    /// 查询一条文档；没有命中返回 Ok(None)，而不是错误。
    fn find_one(
        &self,
        collection: &str,
        filter: Value,
        projection: Option<Value>,
    ) -> Result<Option<Value>>;

    /// 插入一条或多条文档（docs 可以是对象或对象数组）。
    fn insert(&self, collection: &str, docs: Value) -> Result<()>;

    /// 更新文档，返回受影响条数。
    ///
    /// upsert / many 与驱动语义一致；返回值只用于观测，业务逻辑不应依赖它的精确含义
    /// （例如「匹配到但字段没变」算不算受影响由宿主实现决定）。
    fn update(
        &self,
        collection: &str,
        filter: Value,
        update: Value,
        upsert: bool,
        many: bool,
    ) -> Result<u64>;

    /// 统计命中条数。
    fn count(&self, collection: &str, filter: Value) -> Result<u64>;

    /// 删除文档，返回删除条数。空 filter 由实现方决定是否拒绝（Mongo 侧要求显式确认）。
    fn delete(&self, collection: &str, filter: Value, many: bool) -> Result<u64>;

    // ── 业务查询（默认实现，全部建立在上面五个动作之上） ──────────────────

    /// 拉取番剧库的匹配视图，供 [crate::matching] 使用。
    ///
    /// 按 updatedAt 倒序，与 TS 的 hasAnimeSend 遍历顺序一致；同时间戳的相对顺序
    /// 不保证，调用方不要依赖。投影只取匹配需要的列，避免把 summary / 图片等大字段
    /// 从数据库搬到内存。
    fn anime_candidates(&self) -> Result<Vec<AnimeCandidate>> {
        let rows = self.find(
            "anime",
            json!({}),
            Some(json!({
                "_id": 0,
                "id": 1,
                "name": 1,
                "name_cn": 1,
                "names": 1,
                "episode": 1,
                "airingStart": 1,
                "updatedAt": 1,
            })),
            Some(json!({ "updatedAt": -1 })),
            None,
        )?;

        rows.into_iter()
            .map(|mut row| {
                // 老库的 updatedAt 是 BSON Date，宿主以 {"$date": ...} 返回；
                // 先折成 RFC3339 字符串，models 里的 DateTime<Utc> 才反序列化得动。
                normalize_date_field(&mut row, "updatedAt");
                let anime: Anime = decode("anime", row)?;
                Ok(AnimeCandidate::from_anime(&anime))
            })
            .collect()
    }

    /// 取某部番的章节元数据。
    ///
    /// sort 先按 sort 再按 id：Bangumi 的 sort 可能重复（SP / 本篇混排），
    /// 用 id 兜底才能保证每次返回的顺序稳定，匹配结果不会因为游标顺序而抖动。
    fn episodes_by_subject(&self, id: i64) -> Result<Vec<EpisodeMeta>> {
        let rows = self.find(
            "episodes_meta",
            json!({ "subject_id": id }),
            None,
            Some(json!({ "sort": 1, "id": 1 })),
            None,
        )?;

        rows.into_iter()
            .map(|row| decode("episodes_meta", row))
            .collect()
    }

    /// torrents 里是否已有这个标题。
    ///
    /// 写作前统一走 [clean_title]——TS 在入库时清洗、查询时却不洗，结果同一条种子
    /// 换个空格就查不到；这里两边用同一套归一化，避免重复投递。
    fn has_torrent_title(&self, title: &str) -> Result<bool> {
        let title = clean_title(title);
        if title.is_empty() {
            // 空标题不可能命中真实种子，直接返回 false，省一次集合扫描。
            return Ok(false);
        }
        Ok(self.count("torrents", json!({ "title": title }))? > 0)
    }

    /// torrents 里是否已有这个磁力（按 infoHash 去重）。
    ///
    /// 同一个种子在不同站点标题会变（字幕组后缀、tracker 参数），但 btih 不变，
    /// 所以跨标题变体的重复判断只能看 infoHash。磁力解析不出合法的 40 位 hex 时
    /// 返回 false：这不是「已存在」，只是没法判重。
    fn has_torrent_by_magnet(&self, magnet: &str) -> Result<bool> {
        let Some(info_hash) = btih_from_magnet(magnet) else {
            return Ok(false);
        };
        Ok(self.count("torrents", json!({ "infoHash": info_hash }))? > 0)
    }

    /// 写入一条种子记录。
    ///
    /// 状态用 [TorrentStatus] 而不是字符串，非法状态在类型层面就被挡住（TS 里是
    /// 运行时白名单校验）。标题清洗后再入库，与 [Self::has_torrent_title] 保持同一
    /// 套归一化；infoHash 能解析出来才写，避免留下空字段。
    fn add_torrent(&self, magnet: &str, status: TorrentStatus, title: &str) -> Result<()> {
        let title = clean_title(title);
        if title.is_empty() {
            return Err(Error::Store(
                "add_torrent 拒绝写入空标题（清洗后为空）".to_owned(),
            ));
        }

        let now = Utc::now();
        let doc = TorrentDoc {
            title,
            magnet_link: magnet.to_owned(),
            info_hash: btih_from_magnet(magnet),
            status,
            created_at: now,
            updated_at: now,
        };
        let value = serde_json::to_value(&doc)
            .map_err(|err| Error::Store(format!("torrents 文档序列化失败：{err}")))?;
        self.insert("torrents", value)
    }

    /// 按自增 id 取一条待审核缓存，返回其中的 item。
    ///
    /// 与 TS 的 getCacheItemById 一样只返回业务对象，不把 createdAt / _id 漏给
    /// 调用方。
    fn cache_item_get(&self, id: i64) -> Result<Option<AnimeItem>> {
        let Some(doc) = self.find_one(
            "cacheItem",
            json!({ "id": id }),
            Some(json!({ "_id": 0, "item": 1 })),
        )?
        else {
            return Ok(None);
        };

        let row: CacheItemRow = decode("cacheItem", doc)?;
        Ok(Some(row.item))
    }

    /// 新增一条待审核缓存，返回分配到的自增 id。
    fn cache_item_add(&self, item: &AnimeItem) -> Result<i64> {
        let next_id = self.next_sequence("cacheItemId")?;
        self.insert(
            "cacheItem",
            json!({
                "id": next_id,
                "item": item,
                "createdAt": Utc::now(),
            }),
        )?;
        Ok(next_id)
    }

    /// 从 `config` 集合取一个自增序列的下一个值（TS 的 `getNextSequence`）。
    ///
    /// 序列文档是 `{ type: name, seq: N }`（与黑名单的 `{ key, list }` 共存于 `config`，
    /// 用 `type` 字段区分）。三个序列与 TS 对齐：`cacheItemId` / `bangumi_user_id` /
    /// `pendingReviewId`——两边读写**同一个**序列文档，接老库后 id 不会撞号。
    ///
    /// ## 已知边界
    ///
    /// 宿主没有 `findOneAndUpdate`（拿不到原子递增后的返回值），这里用读-改-写：
    /// 单写者（插件只有一个轮询线程 + 命令线程）下与 TS 的 `$inc` 等价；多写者并发
    /// 可能撞号。等宿主补原子序列再换。
    fn next_sequence(&self, name: &str) -> Result<i64> {
        let doc = self.find_one(
            "config",
            json!({ "type": name }),
            Some(json!({ "_id": 0, "seq": 1 })),
        )?;
        let current = doc
            .and_then(|doc| doc.get("seq").cloned())
            .and_then(|seq| value_as_i64(&seq))
            .unwrap_or(0);
        let next = current
            .checked_add(1)
            .ok_or_else(|| Error::Store(format!("序列 {name} 已用尽（i64 溢出）")))?;
        self.update(
            "config",
            json!({ "type": name }),
            json!({
                "$set": { "seq": next },
                "$setOnInsert": { "type": name },
            }),
            true,
            false,
        )?;
        Ok(next)
    }

    /// 取 config 集合里某个 key 的 list 字段。
    ///
    /// TS 的 animeBlacklist / tagExcludeList 共用这套结构。文档或 list 缺失都按
    /// 空数组处理（黑名单为空是正常状态），但 list 存在却不是字符串数组时直接报错：
    /// 那是数据损坏，静默跳过会让过滤规则悄悄失效。
    fn config_list(&self, key: &str) -> Result<Vec<String>> {
        let Some(doc) = self.find_one(
            "config",
            json!({ "key": key }),
            Some(json!({ "_id": 0, "list": 1 })),
        )?
        else {
            return Ok(Vec::new());
        };

        match doc.get("list") {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(list) => serde_json::from_value(list.clone()).map_err(|err| {
                Error::Store(format!("config[{key}] 的 list 不是字符串数组：{err}"))
            }),
        }
    }

    /// 往 config 集合某个 key 的 list 里**幂等**加一个词（已存在就不重复加）。
    ///
    /// 与 TS 的 `$addToSet` 语义一致；这里用读-改-写实现，因为 `Storage::update`
    /// 的宿主实现不保证透传 `$addToSet`。黑名单是管理员手动操作、低并发，读-改-写
    /// 的竞态窗口可接受（同 `cache_item_add` 的单写者假设）。
    fn config_list_add(&self, key: &str, value: &str) -> Result<bool> {
        let mut list = self.config_list(key)?;
        if list.iter().any(|item| item == value) {
            return Ok(false);
        }
        list.push(value.to_owned());
        self.config_list_write(key, &list)?;
        Ok(true)
    }

    /// 从 config 集合某个 key 的 list 里移除一个词；不存在返回 `false`。
    fn config_list_remove(&self, key: &str, value: &str) -> Result<bool> {
        let mut list = self.config_list(key)?;
        let before = list.len();
        list.retain(|item| item != value);
        if list.len() == before {
            return Ok(false);
        }
        self.config_list_write(key, &list)?;
        Ok(true)
    }

    /// 清空 config 集合某个 key 的 list。
    fn config_list_clear(&self, key: &str) -> Result<()> {
        self.config_list_write(key, &[])?;
        Ok(())
    }

    /// 把整个 list 写回 config 集合（`$set` + upsert），是上面三个方法共用的落盘动作。
    fn config_list_write(&self, key: &str, list: &[String]) -> Result<()> {
        self.update(
            "config",
            json!({ "key": key }),
            json!({
                "$set": { "list": list },
                "$setOnInsert": { "key": key },
            }),
            true,
            false,
        )?;
        Ok(())
    }

    /// 按 id upsert 一部番剧，并刷新 updatedAt。
    ///
    /// 更新时**不**把 createdAt 放进 $set：老文档的创建时间要保留；新文档则由
    /// $setOnInsert 补上。Anime 的 Option / 空 Vec 字段会被 serde 跳过，
    /// 因此不会把老库里的值意外清空。
    fn save_anime(&self, anime: &Anime) -> Result<()> {
        if anime.id <= 0 {
            return Err(Error::Store(format!(
                "save_anime 需要有效的 anime.id，收到 {}",
                anime.id
            )));
        }

        let now = json!(Utc::now());
        let mut doc = serde_json::to_value(anime)
            .map_err(|err| Error::Store(format!("anime 文档序列化失败：{err}")))?;
        let fields = doc
            .as_object_mut()
            .ok_or_else(|| Error::Store("anime 序列化结果不是 JSON 对象".to_owned()))?;
        // 创建时间只允许在插入时写；更新时保留库里的原值。
        fields.remove("createdAt");
        fields.insert("updatedAt".to_owned(), now.clone());

        self.update(
            "anime",
            json!({ "id": anime.id }),
            json!({
                "$set": doc,
                "$setOnInsert": { "createdAt": now },
            }),
            true,
            false,
        )?;
        Ok(())
    }

    /// 把一部番剧写进 `cacheAnime` 集合（TS 的 `saveAnime(anime, cache=true)`）。
    ///
    /// 用于「拿不准、先缓存待审核」的番剧：等管理员确认后再从 `cacheAnime` 提升到
    /// `anime`。与 [Self::save_anime] 对称，只是落的目标集合不同。
    fn save_cached_anime(&self, anime: &Anime) -> Result<()> {
        if anime.id <= 0 {
            return Err(Error::Store(format!(
                "save_cached_anime 需要有效的 anime.id，收到 {}",
                anime.id
            )));
        }
        let now = json!(Utc::now());
        let mut doc = serde_json::to_value(anime)
            .map_err(|err| Error::Store(format!("cacheAnime 文档序列化失败：{err}")))?;
        let fields = doc
            .as_object_mut()
            .ok_or_else(|| Error::Store("cacheAnime 序列化结果不是 JSON 对象".to_owned()))?;
        fields.remove("createdAt");
        fields.insert("updatedAt".to_owned(), now.clone());
        self.update(
            "cacheAnime",
            json!({ "id": anime.id }),
            json!({
                "$set": doc,
                "$setOnInsert": { "createdAt": now },
            }),
            true,
            false,
        )?;
        Ok(())
    }

    /// 从 `cacheAnime` 取一部番剧（按 id）。
    fn cached_anime_by_id(&self, id: i64) -> Result<Option<Anime>> {
        let doc = self.find_one("cacheAnime", json!({ "id": id }), None)?;
        doc.map(|doc| decode("cacheAnime", doc)).transpose()
    }

    /// 把一条集数资源 upsert 进 `resources`（`cache=false`）或 `cache_resources`。
    ///
    /// 主键是 `anime_id + episode + groups (+ episodeId)`；`cache` 版还带 `cache_id`。
    /// 返回「是否真的写进去了」（匹配到但字段没变时宿主回 0，语义上不算新增）。
    fn resource_upsert(&self, resource: &crate::models::EpisodeResourceDoc) -> Result<bool> {
        let collection = if resource.cache_id.is_some() {
            "cache_resources"
        } else {
            "resources"
        };

        let mut filter = json!({
            "anime_id": resource.anime_id,
            "episode": resource.episode,
            "groups": resource.groups,
        });
        if let Some(episode_id) = resource.episode_id {
            filter["episodeId"] = json!(episode_id);
        }
        if resource.cache_id.is_some() {
            let cache_key = resource
                .cache_id
                .clone()
                .unwrap_or_else(|| json!(format!("{}-{}", resource.anime_id, resource.episode)));
            filter["cache_id"] = cache_key;
        }

        let now = json!(Utc::now());
        let mut doc = serde_json::to_value(resource)
            .map_err(|err| Error::Store(format!("资源文档序列化失败：{err}")))?;
        if let Some(fields) = doc.as_object_mut() {
            fields.remove("createdAt");
            fields.insert("updatedAt".to_owned(), now.clone());
        }

        let affected = self.update(
            collection,
            filter,
            json!({
                "$set": doc,
                "$setOnInsert": { "createdAt": now },
            }),
            true,
            false,
        )?;
        Ok(affected > 0)
    }

    /// 查一部番剧的某集资源（`resources`，`cache=false`；`cache_resources`，`cache=true`）。
    fn resource_find(
        &self,
        cache: bool,
        anime_id: i64,
        episode: &str,
    ) -> Result<Vec<crate::models::EpisodeResourceDoc>> {
        let collection = if cache { "cache_resources" } else { "resources" };
        let docs = self.find(
            collection,
            json!({ "anime_id": anime_id, "episode": episode }),
            Some(json!({ "_id": 0 })),
            None,
            None,
        )?;
        docs.into_iter()
            .map(|doc| decode("resources", doc))
            .collect()
    }

    /// 按关键词模糊搜番剧（对应 TS 的 `searchAnime`）。
    ///
    /// 先搜 name / name_cn，没结果再搜 names 数组——TS 也是这个顺序，
    /// 因为 names 里混着中日英三语名，直接全搜会让结果变吵。
    /// 关键词至少 2 个字符：单字搜索在几万条番剧里没有意义。
    fn search_anime(&self, keyword: &str) -> Result<Vec<Anime>> {
        let keyword = keyword.trim();
        if keyword.chars().count() < 2 {
            return Err(Error::Store("搜索关键词至少 2 个字符".to_owned()));
        }
        let pattern = regex::escape(keyword);
        let regex = json!({ "$regex": pattern, "$options": "i" });
        let projection = json!({ "_id": 0 });
        let sort = json!({ "updatedAt": -1 });

        let mut docs = self.find(
            "anime",
            json!({ "$or": [{ "name": regex.clone() }, { "name_cn": regex }] }),
            Some(projection.clone()),
            Some(sort.clone()),
            Some(20),
        )?;
        if docs.is_empty() {
            docs = self.find(
                "anime",
                json!({ "names": regex }),
                Some(projection),
                Some(sort),
                Some(20),
            )?;
        }
        docs.into_iter().map(|doc| decode("anime", doc)).collect()
    }

// ── Bangumi 账号绑定（bangumi_users）──────────────────────────────

    /// 写入/更新一条绑定（按 `tgUserId` upsert，`createdAt` 只写一次）。
    ///
    /// `id` 是内部自增主键：新绑定时分配 `bangumi_user_id` 序列，已绑定时保留原 id
    /// （更新凭证 / 刷新 token 不该换主键）。Bangumi 用户 id 存在 `bgmUserId`。
    fn bangumi_user_upsert(&self, user: &crate::models::BangumiUserDoc) -> Result<i64> {
        // 已绑定则沿用原内部 id，否则分配新序列。
        let id = match self.bangumi_user_by_tg(user.tg_user_id)? {
            Some(existing) => existing.id,
            None => self.next_sequence("bangumi_user_id")?,
        };

        let mut doc = serde_json::to_value(user)
            .map_err(|err| Error::Store(format!("绑定文档序列化失败：{err}")))?;
        if let Some(fields) = doc.as_object_mut() {
            fields.insert("id".to_owned(), json!(id));
            fields.remove("createdAt");
            fields.insert("updatedAt".to_owned(), json!(Utc::now()));
        }
        self.update(
            "bangumi_users",
            json!({ "tgUserId": user.tg_user_id }),
            json!({ "$set": doc, "$setOnInsert": { "createdAt": json!(Utc::now()) } }),
            true,
            false,
        )?;
        Ok(id)
    }

    /// 按 Telegram 用户 id 取绑定。
    fn bangumi_user_by_tg(
        &self,
        tg_user_id: i64,
    ) -> Result<Option<crate::models::BangumiUserDoc>> {
        let doc = self.find_one(
            "bangumi_users",
            json!({ "tgUserId": tg_user_id }),
            Some(json!({ "_id": 0 })),
        )?;
        doc.map(|doc| decode("bangumi_users", doc)).transpose()
    }

    /// 解绑（删除该用户的绑定），返回删除条数。
    fn bangumi_user_delete(&self, tg_user_id: i64) -> Result<u64> {
        self.delete(
            "bangumi_users",
            json!({ "tgUserId": tg_user_id }),
            true,
        )
    }
    /// 按 Bangumi id 取一部番剧。没命中返回 `Ok(None)`。
    fn anime_by_id(&self, id: i64) -> Result<Option<Anime>> {
        let doc = self.find_one("anime", json!({ "id": id }), None)?;
        doc.map(|doc| decode("anime", doc)).transpose()
    }

    /// 写入/更新一部番剧的章节（按 `subject_id + id` upsert）。
    ///
    /// TS 的 `replaceEpisodeMetas` 是先删后插；这个 trait 没有 delete（只有
    /// find/find_one/insert/update/count），所以用 upsert：章节 id 是 Bangumi 的稳定
    /// 主键，重复导入不会产生重复文档。代价是「Bangumi 删掉的章节」会留在库里——
    /// 这种脏数据只影响统计，不影响匹配。
    fn upsert_episodes(
        &self,
        subject_id: i64,
        episodes: &[crate::models::EpisodeMeta],
    ) -> Result<usize> {
        let mut written = 0usize;
        for episode in episodes {
            let doc = serde_json::to_value(episode)
                .map_err(|err| Error::Store(format!("章节序列化失败：{err}")))?;
            self.update(
                "episodes_meta",
                json!({ "subject_id": subject_id, "id": episode.id }),
                json!({ "$set": doc }),
                true,
                false,
            )?;
            written += 1;
        }
        Ok(written)
    }

    // ── 待审核（pendingReviews）──────────────────────────────────────────

    /// 写入一条待审核记录，返回分配到的自增 id（TS 的 `createPendingReview`）。
    ///
    /// 主键是自增 `id`（`pendingReviewId` 序列），回调只带 `r=<id>`，详情回查文档。
    /// `id` / `status` / `createdAt` 由本方法补，调用方只需给业务字段（item/anime/…）。
    fn pending_review_insert(&self, doc: &crate::models::PendingReviewDoc) -> Result<i64> {
        let id = self.next_sequence("pendingReviewId")?;
        let mut value = serde_json::to_value(doc)
            .map_err(|err| Error::Store(format!("待审核文档序列化失败：{err}")))?;
        if let Some(fields) = value.as_object_mut() {
            fields.insert("id".to_owned(), json!(id));
            fields.insert("status".to_owned(), json!(PENDING_REVIEW_STATUS));
            fields.insert("createdAt".to_owned(), json!(Utc::now()));
        }
        self.insert("pendingReviews", value)?;
        Ok(id)
    }

    /// 改一条待审核记录的状态（按自增 `id`），并刷新 `updatedAt`。
    ///
    /// 状态取值由调用方约定（审核动作就是 `"approved"` / `"rejected"`）；这里只拒绝
    /// 空串，不维护白名单——集合里可能还有别的业务写进来的历史状态。
    ///
    /// **返回 `Ok(())` 不代表一定命中**：宿主回的是「真正改动」的条数，状态没变化
    /// 时也是 0，和「id 不存在」区分不开。要确认存在，先调 [Self::pending_review_get]。
    fn pending_review_set_status(&self, id: i64, status: &str) -> Result<()> {
        let status = status.trim();
        if status.is_empty() {
            return Err(Error::Store(format!(
                "pending_review_set_status 拒绝空状态（id={id}）"
            )));
        }
        self.update(
            "pendingReviews",
            json!({ "id": id }),
            json!({ "$set": { "status": status, "updatedAt": Utc::now() } }),
            false,
            false,
        )?;
        Ok(())
    }

    /// 按自增 `id` 取一条待审核记录（整份文档）。
    ///
    /// `id <= 0` 直接当「没有」返回，不去查库——自增序列从 1 起，非正数不可能是主键。
    /// 投影掉 `_id`：业务层不该拿宿主的主键当定位方式，自增 `id` 才是。
    fn pending_review_get(&self, id: i64) -> Result<Option<Value>> {
        if id <= 0 {
            return Ok(None);
        }
        self.find_one(
            "pendingReviews",
            json!({ "id": id }),
            Some(json!({ "_id": 0 })),
        )
    }

    // ── 任务检查点（task_checkpoints）──────────────────────────────────

    /// 起一条任务时写检查点（按 `key` upsert，状态 running，attempt 归零）。
    ///
    /// `payload` 要能还原出「从哪继续」：rss 任务存 `{"kind":"rss","item":{...}}`，
    /// 恢复时直接反序列化回 [crate::models::AnimeItem]。
    fn checkpoint_start(
        &self,
        key: &str,
        kind: crate::models::CheckpointKind,
        stage: &str,
        payload: &Value,
    ) -> Result<()> {
        let key = key.trim();
        if key.is_empty() {
            return Err(Error::Store("checkpoint_start 拒绝空 key".to_owned()));
        }
        let now = json!(Utc::now());
        self.update(
            "task_checkpoints",
            json!({ "key": key }),
            json!({
                "$set": {
                    "kind": kind,
                    "payload": payload,
                    "stage": stage,
                    "status": crate::models::CheckpointStatus::Running,
                    "attempt": 0,
                    "updatedAt": now,
                },
                "$setOnInsert": { "createdAt": now },
            }),
            true,
            false,
        )?;
        Ok(())
    }

    /// 推进阶段（下载 → 转码 → 发送）。不改 attempt。
    fn checkpoint_stage(&self, key: &str, stage: &str) -> Result<()> {
        self.update(
            "task_checkpoints",
            json!({ "key": key.trim() }),
            json!({ "$set": { "stage": stage, "status": crate::models::CheckpointStatus::Running, "updatedAt": Utc::now() } }),
            false,
            false,
        )?;
        Ok(())
    }

    /// 收尾：done / failed / canceled。
    fn checkpoint_finish(&self, key: &str, status: crate::models::CheckpointStatus) -> Result<()> {
        self.update(
            "task_checkpoints",
            json!({ "key": key.trim() }),
            json!({ "$set": { "status": status, "updatedAt": Utc::now() } }),
            false,
            false,
        )?;
        Ok(())
    }

    /// 按 key 取一条检查点（恢复逻辑和排查都用它）。
    fn checkpoint_get(&self, key: &str) -> Result<Option<crate::models::TaskCheckpoint>> {
        let doc = self.find_one(
            "task_checkpoints",
            json!({ "key": key.trim() }),
            Some(json!({ "_id": 0 })),
        )?;
        doc.map(|doc| decode("task_checkpoints", doc)).transpose()
    }

    /// 列出重启后要恢复的任务（queued / running），按更新时间升序。
    fn checkpoint_resumable(&self) -> Result<Vec<crate::models::TaskCheckpoint>> {
        let docs = self.find(
            "task_checkpoints",
            json!({ "status": { "$in": ["queued", "running"] } }),
            Some(json!({ "_id": 0 })),
            Some(json!({ "updatedAt": 1 })),
            Some(100),
        )?;
        docs.into_iter()
            .map(|doc| decode("task_checkpoints", doc))
            .collect()
    }

    /// 给恢复次数 +1（防止崩溃循环里无限重放），返回新值。
    fn checkpoint_bump_attempt(&self, key: &str) -> Result<i32> {
        let key = key.trim();
        let Some(doc) = self.find_one("task_checkpoints", json!({ "key": key }), None)? else {
            return Err(Error::Store(format!("检查点不存在：{key}")));
        };
        let next = doc
            .get("attempt")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .saturating_add(1) as i32;
        self.update(
            "task_checkpoints",
            json!({ "key": key }),
            json!({ "$set": { "attempt": next, "updatedAt": Utc::now() } }),
            false,
            false,
        )?;
        Ok(next)
    }

    /// 清理已结束的检查点（done / failed / canceled），返回删除条数。
    ///
    /// 宿主接口没有 delete，只能查出来再逐条……没有删除能力，所以这里改成把
    /// 已结束的记录状态保持原样、只返回条数，真正的清理等宿主补 delete。
    fn checkpoint_finished_count(&self) -> Result<u64> {
        self.count(
            "task_checkpoints",
            json!({ "status": { "$in": ["done", "failed", "canceled"] } }),
        )
    }
}

/// cacheItem 的投影壳：只取 item，忽略 _id / id / createdAt。
#[derive(Debug, Deserialize)]
struct CacheItemRow {
    item: AnimeItem,
}

/// 反序列化一个文档，失败时带上集合名。
///
/// 不直接把 serde 的报错往上抛，是因为调用方看到「missing field subject_id」时
/// 还需要知道查的是哪个集合；集合名+字段才是可定位的日志。
fn decode<T>(collection: &str, doc: Value) -> Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(doc).map_err(|err| {
        Error::Store(format!(
            "{collection} 集合的文档反序列化失败（字段可能与 models.rs 不一致）：{err}"
        ))
    })
}

/// 把宿主返回的 Extended JSON 日期折成 RFC3339 字符串。
///
/// storage.mongo 的 find 用 relaxed Extended JSON 出数据，BSON Date 会变成
/// {"$date": "2026-01-01T00:00:00Z"}（canonical 形式则是 {"$date":{"$numberLong":"..."}}）；
/// 而 models 里的 DateTime<Utc> 只认 RFC3339 字符串。不折这一下，TS 写入的老数据
/// 一条都读不出来。字段不存在或已经是字符串时保持原样。
fn normalize_date_field(doc: &mut Value, field: &str) {
    let Some(object) = doc.as_object_mut() else {
        return;
    };
    let Some(value) = object.get_mut(field) else {
        return;
    };
    if let Some(text) = extjson_date_string(value) {
        *value = Value::String(text);
    }
}

/// 从 {"$date": ...} 里取出日期字符串；无法识别时返回 None。
fn extjson_date_string(value: &Value) -> Option<String> {
    let date = value.as_object()?.get("$date")?;
    let millis = match date {
        // relaxed：{"$date": "2026-01-01T00:00:00Z"}
        Value::String(text) => return Some(text.clone()),
        // canonical v2：{"$date": {"$numberLong": "1767225600000"}}
        Value::Object(inner) => match inner.get("$numberLong")? {
            Value::String(text) => text.parse::<i64>().ok()?,
            Value::Number(number) => number.as_i64()?,
            _ => return None,
        },
        // Extended JSON v1 的历史写法：{"$date": 1767225600000}
        Value::Number(number) => number.as_i64()?,
        _ => return None,
    };
    DateTime::<Utc>::from_timestamp_millis(millis)
        .map(|date| date.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// 从磁力链接里取 btih（40 位十六进制），统一返回小写。
///
/// 只认 xt=urn:btih: 这个参数，不搜整串文本：dn 里也可能出现 urn:btih: 字样，
/// 按参数解析才不会把文件名当哈希。前缀和哈希都做大小写折叠；长度不是 40 或含非 hex
/// 字符时返回 None（base32 的 btih 不在当前数据里出现，不猜）。
pub fn btih_from_magnet(magnet: &str) -> Option<String> {
    const PREFIX: &str = "urn:btih:";

    let query = magnet.split_once('?').map_or(magnet, |(_, query)| query);
    for param in query.split('&') {
        let Some((key, value)) = param.split_once('=') else {
            continue;
        };
        if !key.eq_ignore_ascii_case("xt") {
            continue;
        }
        // 用 get 而不是切片：value 可能含非 ASCII 字符，切片在非字符边界会 panic。
        let Some(prefix) = value.get(..PREFIX.len()) else {
            continue;
        };
        if !prefix.eq_ignore_ascii_case(PREFIX) {
            continue;
        }
        let Some(hash) = value.get(PREFIX.len()..) else {
            continue;
        };
        if hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

/// JSON 数字转 i64；同时兼容 as_i64 与超出 i64 的 as_u64（后者按溢出处理）。
fn value_as_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::cmp::Ordering;
    use std::collections::HashMap;

    use serde_json::{Map, json};

    use super::*;

    /// 40 位小写 hex，固定值方便手算断言。
    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
    const HASH_UPPER: &str = "0123456789ABCDEF0123456789ABCDEF01234567";
    const OTHER_HASH: &str = "fedcba9876543210fedcba9876543210fedcba98";

    /// 内存版 Storage：只实现够测试用的 Mongo 语义。
    ///
    /// 过滤支持顶层字段精确匹配，排序支持数字/字符串/布尔，投影支持 inclusion。
    /// 它**不是**通用 Mongo 模拟器——足够验证「业务查询构造了哪些 filter/sort/projection」
    /// 以及数据往返即可。
    #[derive(Debug, Default)]
    struct FakeStorage {
        collections: RefCell<HashMap<String, Vec<Value>>>,
        finds: RefCell<Vec<FindCall>>,
        inserts: RefCell<Vec<(String, Value)>>,
        updates: RefCell<Vec<UpdateCall>>,
    }

    #[derive(Debug, Clone)]
    struct FindCall {
        collection: String,
        filter: Value,
        projection: Option<Value>,
        sort: Option<Value>,
        limit: Option<usize>,
    }

    #[derive(Debug, Clone)]
    struct UpdateCall {
        collection: String,
        filter: Value,
        update: Value,
        upsert: bool,
        many: bool,
    }

    impl FakeStorage {
        fn with_docs(rows: &[(&str, Vec<Value>)]) -> Self {
            let mut collections = HashMap::new();
            for (collection, docs) in rows {
                collections.insert((*collection).to_owned(), docs.clone());
            }
            Self {
                collections: RefCell::new(collections),
                ..Self::default()
            }
        }

        fn docs(&self, collection: &str) -> Vec<Value> {
            self.collections
                .borrow()
                .get(collection)
                .cloned()
                .unwrap_or_default()
        }

        fn last_find(&self) -> FindCall {
            self.finds
                .borrow()
                .last()
                .cloned()
                .expect("预期至少发生一次 find 调用")
        }

        fn last_update(&self) -> UpdateCall {
            self.updates
                .borrow()
                .last()
                .cloned()
                .expect("预期至少发生一次 update 调用")
        }
    }

    impl Storage for FakeStorage {
        fn find(
            &self,
            collection: &str,
            filter: Value,
            projection: Option<Value>,
            sort: Option<Value>,
            limit: Option<usize>,
        ) -> crate::Result<Vec<Value>> {
            self.finds.borrow_mut().push(FindCall {
                collection: collection.to_owned(),
                filter: filter.clone(),
                projection: projection.clone(),
                sort: sort.clone(),
                limit,
            });

            let mut docs: Vec<Value> = self
                .collections
                .borrow()
                .get(collection)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|doc| matches_filter(doc, &filter))
                .collect();

            if let Some(sort) = &sort {
                apply_sort(&mut docs, sort);
            }
            if let Some(limit) = limit {
                docs.truncate(limit);
            }
            if let Some(projection) = &projection {
                docs = docs
                    .into_iter()
                    .map(|doc| project_doc(doc, projection))
                    .collect();
            }
            Ok(docs)
        }

        fn find_one(
            &self,
            collection: &str,
            filter: Value,
            projection: Option<Value>,
        ) -> crate::Result<Option<Value>> {
            let mut docs = self.find(collection, filter, projection, None, Some(1))?;
            Ok(docs.pop())
        }

        fn insert(&self, collection: &str, docs: Value) -> crate::Result<()> {
            self.inserts
                .borrow_mut()
                .push((collection.to_owned(), docs.clone()));

            let mut collections = self.collections.borrow_mut();
            let bucket = collections.entry(collection.to_owned()).or_default();
            match docs {
                Value::Array(items) => bucket.extend(items),
                other => bucket.push(other),
            }
            Ok(())
        }

        fn update(
            &self,
            collection: &str,
            filter: Value,
            update: Value,
            upsert: bool,
            many: bool,
        ) -> crate::Result<u64> {
            self.updates.borrow_mut().push(UpdateCall {
                collection: collection.to_owned(),
                filter: filter.clone(),
                update: update.clone(),
                upsert,
                many,
            });

            let mut collections = self.collections.borrow_mut();
            let bucket = collections.entry(collection.to_owned()).or_default();
            let mut affected = 0u64;
            for doc in bucket.iter_mut() {
                if matches_filter(doc, &filter) {
                    apply_set(doc, &update);
                    affected += 1;
                    if !many {
                        break;
                    }
                }
            }

            if affected == 0 && upsert {
                let mut doc = Map::new();
                if let Some(filter) = filter.as_object() {
                    for (key, value) in filter {
                        doc.insert(key.clone(), value.clone());
                    }
                }
                for operator in ["$set", "$setOnInsert"] {
                    if let Some(fields) = update.get(operator).and_then(Value::as_object) {
                        for (key, value) in fields {
                            doc.insert(key.clone(), value.clone());
                        }
                    }
                }
                bucket.push(Value::Object(doc));
                affected = 1;
            }

            Ok(affected)
        }

        fn delete(&self, collection: &str, filter: Value, many: bool) -> crate::Result<u64> {
            let mut collections = self.collections.borrow_mut();
            let Some(docs) = collections.get_mut(collection) else {
                return Ok(0);
            };
            let before = docs.len();
            if many {
                docs.retain(|doc| !matches_filter(doc, &filter));
            } else if let Some(index) = docs.iter().position(|doc| matches_filter(doc, &filter)) {
                docs.remove(index);
            }
            Ok((before - docs.len()) as u64)
        }

        fn count(&self, collection: &str, filter: Value) -> crate::Result<u64> {
            let collections = self.collections.borrow();
            Ok(collections.get(collection).map_or(0, |docs| {
                docs.iter()
                    .filter(|doc| matches_filter(doc, &filter))
                    .count() as u64
            }))
        }
    }

    fn matches_filter(doc: &Value, filter: &Value) -> bool {
        let Some(filter) = filter.as_object() else {
            return true;
        };
        filter
            .iter()
            .all(|(field, expected)| doc.get(field.as_str()) == Some(expected))
    }

    fn apply_set(doc: &mut Value, update: &Value) {
        let Some(fields) = update.get("$set").and_then(Value::as_object) else {
            return;
        };
        if !doc.is_object() {
            *doc = Value::Object(Map::new());
        }
        if let Some(target) = doc.as_object_mut() {
            for (key, value) in fields {
                target.insert(key.clone(), value.clone());
            }
        }
    }

    fn apply_sort(docs: &mut [Value], sort: &Value) {
        let Some(sort) = sort.as_object() else {
            return;
        };
        docs.sort_by(|left, right| {
            for (field, direction) in sort {
                let ordering = compare_values(left.get(field.as_str()), right.get(field.as_str()));
                if ordering != Ordering::Equal {
                    return if direction.as_i64().unwrap_or(1) < 0 {
                        ordering.reverse()
                    } else {
                        ordering
                    };
                }
            }
            Ordering::Equal
        });
    }

    fn compare_values(left: Option<&Value>, right: Option<&Value>) -> Ordering {
        match (left, right) {
            (Some(Value::Number(a)), Some(Value::Number(b))) => match (a.as_f64(), b.as_f64()) {
                (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
                _ => Ordering::Equal,
            },
            (Some(Value::String(a)), Some(Value::String(b))) => a.cmp(b),
            (Some(Value::Bool(a)), Some(Value::Bool(b))) => a.cmp(b),
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            _ => Ordering::Equal,
        }
    }

    fn project_doc(doc: Value, projection: &Value) -> Value {
        let Some(spec) = projection.as_object() else {
            return doc;
        };
        let inclusive = spec
            .iter()
            .any(|(field, value)| field.as_str() != "_id" && value.as_i64() == Some(1));
        if !inclusive {
            return doc;
        }

        let mut out = Map::new();
        if let Some(source) = doc.as_object() {
            for (field, value) in source {
                if spec.get(field.as_str()).and_then(Value::as_i64) == Some(1) {
                    out.insert(field.clone(), value.clone());
                }
            }
        }
        Value::Object(out)
    }

    #[test]
    fn magnet_lookup_ignores_case_and_extra_params() {
        let store = FakeStorage::with_docs(&[(
            "torrents",
            vec![json!({ "title": "某番 [01]", "infoHash": HASH })],
        )]);

        let with_trackers = format!(
            "magnet:?xt=urn:btih:{HASH_UPPER}&dn=%E6%9F%90%E7%95%AA&tr=udp%3A%2F%2Ftracker.example%3A80"
        );
        assert!(
            store.has_torrent_by_magnet(&with_trackers).unwrap(),
            "大小写不同、带 tracker 参数的同一个磁力必须命中"
        );

        // xt 不是第一个参数、前缀大写、hash 小写，仍然要命中。
        let reordered = format!("magnet:?dn=first&xt=urn:BTIH:{HASH}&tr=udp://tracker");
        assert!(store.has_torrent_by_magnet(&reordered).unwrap());

        let other = format!("magnet:?xt=urn:btih:{OTHER_HASH}");
        assert!(!store.has_torrent_by_magnet(&other).unwrap());

        assert!(!store.has_torrent_by_magnet("magnet:?dn=没有哈希").unwrap());
    }

    #[test]
    fn magnet_lookup_only_reads_xt_not_dn() {
        let store = FakeStorage::with_docs(&[("torrents", vec![json!({ "infoHash": HASH })])]);

        // dn 里伪造一个同值 btih，xt 才是真哈希（OTHER_HASH）——不能被 dn 带偏。
        let decoy = format!("magnet:?dn=urn:btih:{HASH}&xt=urn:btih:{OTHER_HASH}");
        assert!(!store.has_torrent_by_magnet(&decoy).unwrap());

        // 反向：dn 是干扰项，xt 命中。
        let real = format!("magnet:?dn=urn:btih:{OTHER_HASH}&xt=urn:btih:{HASH}");
        assert!(store.has_torrent_by_magnet(&real).unwrap());
    }

    #[test]
    fn btih_parser_requires_exactly_40_hex_digits() {
        assert_eq!(
            btih_from_magnet(&format!("magnet:?xt=urn:btih:{HASH_UPPER}")),
            Some(HASH.to_owned())
        );
        assert_eq!(btih_from_magnet("magnet:?xt=urn:btih:0123"), None);
        assert_eq!(
            btih_from_magnet(&format!("magnet:?xt=urn:btih:{HASH}0")),
            None,
            "41 位必须拒绝"
        );
        assert_eq!(
            btih_from_magnet(&format!("magnet:?xt=urn:btih:{}g", &HASH[..39])),
            None,
            "非 hex 字符必须拒绝"
        );
    }

    #[test]
    fn add_torrent_cleans_title_and_records_info_hash() {
        let store = FakeStorage::default();
        let raw_title = "  [喵萌奶茶屋]\u{3000}葬送的芙莉莲\u{200b}  [01]  ";
        let expected = clean_title(raw_title);
        assert_ne!(expected, raw_title, "测试数据必须真的需要清洗");

        let magnet = format!("magnet:?xt=urn:btih:{HASH_UPPER}&dn=demo");
        store
            .add_torrent(&magnet, TorrentStatus::PendingDownload, raw_title)
            .unwrap();

        let inserts = store.inserts.borrow();
        assert_eq!(inserts.len(), 1);
        let (collection, doc) = &inserts[0];
        assert_eq!(collection, "torrents");
        assert_eq!(doc["title"], json!(expected));
        assert_eq!(doc["title"], json!("[喵萌奶茶屋] 葬送的芙莉莲 [01]"));
        assert_eq!(doc["magnetLink"], json!(magnet));
        assert_eq!(doc["infoHash"], json!(HASH), "infoHash 必须写成小写");
        assert_eq!(doc["status"], json!("等待下载"));
        assert!(
            doc.get("createdAt").and_then(Value::as_str).is_some(),
            "createdAt 由 serde 序列化成 RFC3339 字符串"
        );
        assert!(doc.get("updatedAt").and_then(Value::as_str).is_some());
    }

    #[test]
    fn add_torrent_without_btih_omits_info_hash() {
        let store = FakeStorage::default();
        store
            .add_torrent("magnet:?dn=no-hash", TorrentStatus::Failed, "某番 [02]")
            .unwrap();

        let inserts = store.inserts.borrow();
        let (_, doc) = &inserts[0];
        assert!(doc.get("infoHash").is_none());
        assert_eq!(doc["status"], json!("失败"));
    }

    #[test]
    fn add_torrent_rejects_empty_title() {
        let store = FakeStorage::default();
        let err = store
            .add_torrent("magnet:?dn=x", TorrentStatus::Failed, " \u{3000} ")
            .unwrap_err();
        assert!(matches!(err, Error::Store(_)));
        assert!(store.inserts.borrow().is_empty(), "失败时不能写库");
    }

    #[test]
    fn episodes_use_stable_sort_and_filter_by_subject() {
        let store = FakeStorage::with_docs(&[(
            "episodes_meta",
            vec![
                json!({ "id": 200, "subject_id": 42, "sort": 2.0, "ep": 2.0 }),
                json!({ "id": 100, "subject_id": 42, "sort": 1.0, "ep": 1.0 }),
                json!({ "id": 300, "subject_id": 7, "sort": 0.0, "ep": 1.0 }),
            ],
        )]);

        let episodes = store.episodes_by_subject(42).unwrap();
        assert_eq!(
            episodes
                .iter()
                .map(|episode| episode.id)
                .collect::<Vec<_>>(),
            vec![100, 200]
        );

        let call = store.last_find();
        assert_eq!(call.collection, "episodes_meta");
        assert_eq!(call.filter, json!({ "subject_id": 42 }));
        assert_eq!(call.sort, Some(json!({ "sort": 1, "id": 1 })));
        assert_eq!(call.projection, None);
        assert_eq!(call.limit, None);
    }

    #[test]
    fn config_list_returns_string_array() {
        let store = FakeStorage::with_docs(&[(
            "config",
            vec![
                json!({ "key": "animeBlacklist", "list": ["甲", "乙"] }),
                json!({ "key": "tagExcludeList", "list": [] }),
                json!({ "key": "nullList", "list": null }),
            ],
        )]);

        assert_eq!(
            store.config_list("animeBlacklist").unwrap(),
            vec!["甲".to_owned(), "乙".to_owned()]
        );

        let call = store.last_find();
        assert_eq!(call.collection, "config");
        assert_eq!(call.filter, json!({ "key": "animeBlacklist" }));
        assert_eq!(call.projection, Some(json!({ "_id": 0, "list": 1 })));

        assert!(store.config_list("tagExcludeList").unwrap().is_empty());
        assert!(store.config_list("nullList").unwrap().is_empty());
        assert!(store.config_list("missing").unwrap().is_empty());
    }

    #[test]
    fn config_list_rejects_corrupted_list() {
        let store = FakeStorage::with_docs(&[(
            "config",
            vec![json!({ "key": "bad", "list": "不是数组" })],
        )]);
        assert!(matches!(store.config_list("bad"), Err(Error::Store(_))));
    }

    #[test]
    fn config_list_add_is_idempotent() {
        let store = FakeStorage::with_docs(&[(
            "config",
            vec![json!({ "key": "animeBlacklist", "list": ["甲"] })],
        )]);

        // 新词加进去，返回 true。
        assert!(store.config_list_add("animeBlacklist", "乙").unwrap());
        assert_eq!(
            store.config_list("animeBlacklist").unwrap(),
            vec!["甲".to_owned(), "乙".to_owned()]
        );

        // 重复词不加，返回 false。
        assert!(!store.config_list_add("animeBlacklist", "乙").unwrap());
        assert_eq!(
            store.config_list("animeBlacklist").unwrap(),
            vec!["甲".to_owned(), "乙".to_owned()]
        );

        // 不存在的 key：先 upsert 出文档再加词。
        assert!(store.config_list_add("newKey", "词").unwrap());
        assert_eq!(store.config_list("newKey").unwrap(), vec!["词".to_owned()]);
    }

    #[test]
    fn config_list_remove_and_clear() {
        let store = FakeStorage::with_docs(&[(
            "config",
            vec![json!({ "key": "animeBlacklist", "list": ["甲", "乙"] })],
        )]);

        assert!(store.config_list_remove("animeBlacklist", "乙").unwrap());
        assert_eq!(store.config_list("animeBlacklist").unwrap(), vec!["甲".to_owned()]);

        // 不存在的词返回 false，列表不变。
        assert!(!store.config_list_remove("animeBlacklist", "丙").unwrap());

        store.config_list_clear("animeBlacklist").unwrap();
        assert!(store.config_list("animeBlacklist").unwrap().is_empty());
    }

    #[test]
    fn has_torrent_title_matches_after_cleaning() {
        let raw = "  [组]\u{3000}某番  [01] ";
        let store =
            FakeStorage::with_docs(&[("torrents", vec![json!({ "title": clean_title(raw) })])]);

        assert!(store.has_torrent_title(raw).unwrap());
        assert!(!store.has_torrent_title("[组] 别的番 [01]").unwrap());
        assert!(!store.has_torrent_title(" \u{3000} ").unwrap());
    }

    #[test]
    fn anime_candidates_projects_and_maps_to_matching_view() {
        let store = FakeStorage::with_docs(&[(
            "anime",
            vec![
                json!({
                    "id": 1,
                    "name": "older",
                    "name_cn": "旧番",
                    "names": ["older"],
                    "episode": "12",
                    "airingStart": "2025年1月1日",
                    "updatedAt": "2025-01-01T00:00:00Z"
                }),
                json!({
                    "id": 2,
                    "name": "newer",
                    "name_cn": "新番",
                    "names": ["newer", "别名"],
                    "episode": "24",
                    "airingStart": "2026年1月1日",
                    "updatedAt": "2026-01-01T00:00:00Z",
                    // 不该被投影带出来，但即使带出来也必须被 models::Anime 正确忽略。
                    "navMessage": { "chat_id": -100 }
                }),
            ],
        )]);

        let candidates = store.anime_candidates().unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].id, 2, "按 updatedAt 倒序");
        assert_eq!(candidates[0].name_cn.as_deref(), Some("新番"));
        assert_eq!(candidates[0].airing_start.as_deref(), Some("2026年1月1日"));
        assert_eq!(
            candidates[0].names,
            vec!["newer".to_owned(), "别名".to_owned()]
        );
        assert!(candidates[0].updated_at.is_some());
        assert_eq!(candidates[1].id, 1);

        let call = store.last_find();
        assert_eq!(call.collection, "anime");
        assert_eq!(call.filter, json!({}));
        assert_eq!(call.sort, Some(json!({ "updatedAt": -1 })));
        assert_eq!(call.limit, None);

        let projection = call.projection.expect("anime_candidates 必须带投影");
        assert_eq!(projection["_id"], json!(0));
        for field in [
            "id",
            "name",
            "name_cn",
            "names",
            "episode",
            "airingStart",
            "updatedAt",
        ] {
            assert_eq!(projection[field], json!(1), "投影必须包含 {field}");
        }
    }

    #[test]
    fn anime_candidates_accepts_extjson_dates_from_host() {
        // storage.mongo 的 find 用 relaxed Extended JSON 出数据；TS 写入的老库
        // updatedAt 是 BSON Date，读回来是 {"$date": ...}，必须能转回 chrono。
        let store = FakeStorage::with_docs(&[(
            "anime",
            vec![
                json!({
                    "id": 1,
                    "name": "relaxed",
                    "updatedAt": { "$date": "2026-01-01T00:00:00Z" }
                }),
                json!({
                    "id": 2,
                    "name": "canonical",
                    "updatedAt": { "$date": { "$numberLong": "1767225600000" } }
                }),
            ],
        )]);

        let candidates = store.anime_candidates().unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.updated_at.is_some())
        );
        for candidate in &candidates {
            assert_eq!(
                candidate.updated_at.unwrap().to_rfc3339(),
                "2026-01-01T00:00:00+00:00"
            );
        }
    }

    #[test]
    fn save_anime_upserts_by_id_and_keeps_created_at() {
        let store = FakeStorage::with_docs(&[(
            "anime",
            vec![json!({
                "id": 7,
                "name": "旧名",
                "createdAt": "2020-01-01T00:00:00Z",
                "updatedAt": "2020-01-01T00:00:00Z"
            })],
        )]);
        let anime = Anime {
            id: 7,
            name: Some("新名".to_owned()),
            ..Anime::default()
        };

        store.save_anime(&anime).unwrap();

        let call = store.last_update();
        assert_eq!(call.collection, "anime");
        assert_eq!(call.filter, json!({ "id": 7 }));
        assert!(call.upsert);
        assert!(!call.many);

        let set = call.update.get("$set").expect("必须是 $set 更新");
        assert_eq!(set["name"], json!("新名"));
        assert!(set.get("updatedAt").is_some());
        assert!(set.get("createdAt").is_none(), "$set 不能覆盖创建时间");
        assert!(
            call.update["$setOnInsert"].get("createdAt").is_some(),
            "新文档要补 createdAt"
        );

        let docs = store.docs("anime");
        assert_eq!(docs[0]["name"], json!("新名"));
        assert_eq!(docs[0]["createdAt"], json!("2020-01-01T00:00:00Z"));
    }

    #[test]
    fn save_anime_inserts_created_at_for_new_document() {
        let store = FakeStorage::default();
        store
            .save_anime(&Anime {
                id: 8,
                name: Some("新番".to_owned()),
                ..Anime::default()
            })
            .unwrap();

        let docs = store.docs("anime");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0]["id"], json!(8));
        assert!(docs[0].get("createdAt").and_then(Value::as_str).is_some());
        assert!(docs[0].get("updatedAt").and_then(Value::as_str).is_some());
    }

    #[test]
    fn save_anime_rejects_invalid_id() {
        let store = FakeStorage::default();
        let err = store.save_anime(&Anime::default()).unwrap_err();
        assert!(matches!(err, Error::Store(_)));
    }

    #[test]
    fn cache_item_round_trips_and_allocates_next_id() {
        let store = FakeStorage::default();
        let item = AnimeItem {
            title: "某番 [01]".to_owned(),
            pub_date: "2026-01-01T00:00:00Z".to_owned(),
            magnet: format!("magnet:?xt=urn:btih:{HASH}"),
            link: "https://example.invalid/1".to_owned(),
            team: "喵萌奶茶屋".to_owned(),
            fansub: vec!["喵萌奶茶屋".to_owned()],
            names: vec!["某番".to_owned()],
            source: Some("Baha".to_owned()),
            episode: Some("01".to_owned()),
        };

        assert_eq!(store.cache_item_add(&item).unwrap(), 1);
        assert_eq!(store.cache_item_add(&item).unwrap(), 2);

        let loaded = store.cache_item_get(1).unwrap().expect("缓存应存在");
        assert_eq!(loaded.title, item.title);
        assert_eq!(loaded.team, item.team);
        assert_eq!(loaded.names, item.names);
        assert_eq!(loaded.episode, item.episode);
        assert!(store.cache_item_get(99).unwrap().is_none());
    }

    // ── 待审核（pendingReviews）──────────────────────────────────────────

#[test]
    fn checkpoint_records_stage_and_finishes() {
        let store = FakeStorage::default();
        store
            .checkpoint_start(
                "[组] 某番 - 05",
                crate::models::CheckpointKind::Rss,
                "下载",
                &json!({ "kind": "rss", "item": { "title": "x" } }),
            )
            .unwrap();
        let checkpoint = store
            .checkpoint_get("[组] 某番 - 05")
            .unwrap()
            .expect("刚写的检查点应当能读回来");
        assert_eq!(checkpoint.stage, "下载");
        assert_eq!(checkpoint.attempt, 0);
        assert_eq!(checkpoint.status, crate::models::CheckpointStatus::Running);

        store.checkpoint_stage("[组] 某番 - 05", "转码").unwrap();
        assert_eq!(
            store
                .checkpoint_get("[组] 某番 - 05")
                .unwrap()
                .unwrap()
                .stage,
            "转码"
        );

        store
            .checkpoint_finish(
                "[组] 某番 - 05",
                crate::models::CheckpointStatus::Done,
            )
            .unwrap();
        assert_eq!(
            store
                .checkpoint_get("[组] 某番 - 05")
                .unwrap()
                .unwrap()
                .status,
            crate::models::CheckpointStatus::Done
        );
    }

    #[test]
    fn checkpoint_bump_attempt_counts_and_rejects_blank_key() {
        let store = FakeStorage::default();
        store
            .checkpoint_start(
                "k",
                crate::models::CheckpointKind::Rss,
                "下载",
                &json!({}),
            )
            .unwrap();
        assert_eq!(store.checkpoint_bump_attempt("k").unwrap(), 1);
        assert_eq!(store.checkpoint_bump_attempt("k").unwrap(), 2);
        assert!(store.checkpoint_start("  ", crate::models::CheckpointKind::Rss, "下载", &json!({})).is_err());
    }
    #[test]
    fn pending_review_insert_allocates_id_and_stamps_times() {
        let store = FakeStorage::default();
        let doc = crate::models::PendingReviewDoc {
            item: Some(AnimeItem {
                title: "某番 [01]".to_owned(),
                pub_date: String::new(),
                magnet: format!("magnet:?xt=urn:btih:{HASH}"),
                link: String::new(),
                team: String::new(),
                fansub: vec![],
                names: vec![],
                source: None,
                episode: Some("01".to_owned()),
            }),
            ..Default::default()
        };

        let id1 = store.pending_review_insert(&doc).unwrap();
        let id2 = store.pending_review_insert(&doc).unwrap();
        assert_eq!(id1, 1, "自增 id 从 1 起");
        assert_eq!(id2, 2, "每次插入分配新 id");

        let docs = store.docs("pendingReviews");
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0]["id"], json!(1));
        assert_eq!(docs[0]["status"], json!(PENDING_REVIEW_STATUS));
        assert_eq!(docs[0]["item"]["title"], json!("某番 [01]"));
        assert!(docs[0]["createdAt"].as_str().is_some(), "createdAt 是 RFC3339 字符串");
    }

    #[test]
    fn pending_review_set_status_by_id_keeps_payload() {
        let store = FakeStorage::default();
        let doc = crate::models::PendingReviewDoc {
            item: Some(AnimeItem {
                title: "某番 [01]".to_owned(),
                pub_date: String::new(),
                magnet: format!("magnet:?xt=urn:btih:{HASH}"),
                link: String::new(),
                team: String::new(),
                fansub: vec![],
                names: vec![],
                source: None,
                episode: Some("01".to_owned()),
            }),
            ..Default::default()
        };
        let id = store.pending_review_insert(&doc).unwrap();

        store.pending_review_set_status(id, "approved").unwrap();

        let docs = store.docs("pendingReviews");
        assert_eq!(docs[0]["status"], json!("approved"));
        assert_eq!(docs[0]["item"]["title"], json!("某番 [01]"), "业务字段不能被覆盖");
        assert!(docs[0]["updatedAt"].as_str().is_some());

        // 不存在的 id 不算错误：调用方先 get 再 set，0 条命中只说明记录没了。
        store.pending_review_set_status(999, "rejected").unwrap();
    }

    #[test]
    fn pending_review_get_by_id_and_never_leaks_mongo_id() {
        let store = FakeStorage::default();
        let doc = crate::models::PendingReviewDoc::default();
        let id = store.pending_review_insert(&doc).unwrap();

        let got = store
            .pending_review_get(id)
            .unwrap()
            .expect("应能取到待审核记录");
        assert_eq!(got["id"], json!(id));
        assert_eq!(got["status"], json!(PENDING_REVIEW_STATUS));
        assert!(got.get("_id").is_none(), "_id 不该漏给业务层");

        let call = store.last_find();
        assert_eq!(call.collection, "pendingReviews");
        assert_eq!(call.filter, json!({ "id": id }));
        assert_eq!(call.projection, Some(json!({ "_id": 0 })));

        assert!(store.pending_review_get(999).unwrap().is_none());
        assert!(store.pending_review_get(0).unwrap().is_none());
        assert!(store.pending_review_get(-1).unwrap().is_none());
    }

    #[test]
    fn pending_review_rejects_blank_status() {
        let store = FakeStorage::default();
        let doc = crate::models::PendingReviewDoc::default();
        let id = store.pending_review_insert(&doc).unwrap();

        assert!(matches!(
            store.pending_review_set_status(id, "   "),
            Err(Error::Store(_))
        ));
    }

    #[test]
    fn sequence_is_shared_between_cache_item_and_pending_review() {
        // 三个序列各自独立：cacheItemId / bangumi_user_id / pendingReviewId 互不干扰，
        // 但同名序列在多次调用间单调递增（对齐 TS 的 getNextSequence）。
        let store = FakeStorage::default();
        assert_eq!(store.next_sequence("cacheItemId").unwrap(), 1);
        assert_eq!(store.next_sequence("cacheItemId").unwrap(), 2);
        assert_eq!(store.next_sequence("pendingReviewId").unwrap(), 1);
        assert_eq!(store.next_sequence("bangumi_user_id").unwrap(), 1);
    }
}
