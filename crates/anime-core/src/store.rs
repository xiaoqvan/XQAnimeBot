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
    ///
    /// 宿主接口没有 findOneAndUpdate，拿不到原子自增序列，这里退一步用「当前最大 id + 1」。
    /// 单写者（插件只有一个轮询线程）下与 TS 的 getNextSequence 等价；多写者并发时
    /// 可能撞 id，这是当前 trait 能力的已知边界，等宿主补原子序列后再换。
    fn cache_item_add(&self, item: &AnimeItem) -> Result<i64> {
        let latest = self.find(
            "cacheItem",
            json!({}),
            Some(json!({ "_id": 0, "id": 1 })),
            Some(json!({ "id": -1 })),
            Some(1),
        )?;
        let next_id = latest
            .first()
            .and_then(|doc| doc.get("id"))
            .and_then(value_as_i64)
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| Error::Store("cacheItem 的 id 已用尽（i64 溢出）".to_owned()))?;

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
                let ordering =
                    compare_values(left.get(field.as_str()), right.get(field.as_str()));
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
}
