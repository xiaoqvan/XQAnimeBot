//! MongoDB 插件：注册 `storage.mongo` 服务。
//!
//! 定位是**通用存储**，不认识番剧：集合名、过滤条件、管道全部由调用方给。
//! 动漫插件把 `anime` / `torrents` / `episodes_meta` / `resources` / `cache_resources`
//! / `task_checkpoints` 六个集合放在这里，字段结构由调用方自己维护——
//! 数据库插件只保证「JSON 进、JSON 出，语义与 Mongo Shell 一致」。
//!
//! ```text
//! 业务插件 ──get_service("storage.mongo")──▶ 本插件 ──官方 mongodb 驱动──▶ MongoDB
//! ```
//!
//! 三个刻意的设计：
//!
//! 1. 业务逻辑在 [`api`] 里，`unsafe extern "C"` 只是薄薄一层壳——这样同一套代码
//!    既能被宿主调用，也能被示例/集成测试直接调用（否则脱离宿主就只能测编译）。
//! 2. **服务方法是同步的**，而驱动是异步的。这里在专用线程上跑一个 tokio 运行时，
//!    用 `Handle::block_on` 把异步调用摊平，业务侧不用关心 async 传染。
//! 3. **不缓存游标**：所有查询一次取完再返回。分页靠 `limit`/`skip`，
//!    大集合请用 `aggregate` 让服务端先筛，别把 10 万条拉进内存。

use std::ffi::{c_char, c_void};
use std::sync::OnceLock;

use futures_util::TryStreamExt;
use fuyubot_abi::{HostApi, LogLevel, Plugin};
use fuyubot_service::{
    Handler, Service, Settings, emit, error, ok, opt_bool, opt_str, opt_u64, parse, require_str,
};
use mongodb::bson::{Bson, Document, doc};
use mongodb::options::IndexOptions;
use mongodb::{Client, Collection, IndexModel};
use serde_json::{Value, json};
use tokio::runtime::{Handle, Runtime};

const SERVICE: &str = "storage.mongo";
const PLUGIN: &str = "mongo";

/// 启动配置：
///
/// ```json
/// { "uri": "mongodb://127.0.0.1:27017", "db": "xqanime" }
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    pub uri: String,
    pub db: String,
}

static SETTINGS: Settings<Config> = Settings::new();
static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static CLIENT: OnceLock<Client> = OnceLock::new();
static HOST: OnceLock<HostApi> = OnceLock::new();

struct MongoPlugin {
    host: HostApi,
}

impl Plugin for MongoPlugin {
    fn init(host: HostApi) -> Result<Self, String> {
        Service::new(
            SERVICE,
            vec![
                ("ping", svc_ping as Handler),
                ("find", svc_find as Handler),
                ("find_one", svc_find_one as Handler),
                ("insert", svc_insert as Handler),
                ("update", svc_update as Handler),
                ("delete", svc_delete as Handler),
                ("count", svc_count as Handler),
                ("aggregate", svc_aggregate as Handler),
                ("distinct", svc_distinct as Handler),
                ("ensure_index", svc_ensure_index as Handler),
                ("collections", svc_collections as Handler),
            ],
        )
        .register(&host)?;
        let _ = HOST.set(host.clone());
        Ok(Self { host })
    }

    fn start(&mut self, config_json: &str) -> Result<(), String> {
        let raw: Value = serde_json::from_str(config_json).unwrap_or(Value::Null);
        let uri = opt_str(&raw, "uri")
            .unwrap_or("mongodb://127.0.0.1:27017")
            .to_owned();
        let db = opt_str(&raw, "db").unwrap_or("xqanime").to_owned();

        api::configure(&uri, &db)?;

        // 预热：真的 ping 一次，配置写错在 start 阶段就暴露，而不是等第一条业务查询。
        match api::ping() {
            Ok(info) => self.host.log(
                LogLevel::Info,
                &format!(
                    "[{PLUGIN}] 已连接 {uri}/{db}（集合 {} 个）",
                    info["collections"].as_u64().unwrap_or(0)
                ),
            ),
            Err(err) => self
                .host
                .log(LogLevel::Warn, &format!("[{PLUGIN}] 预热 ping 失败：{err}")),
        }
        Ok(())
    }
}

fuyubot_abi::define_plugin!(MongoPlugin, "fuyubot.mongo");

// ─── 业务实现（可被示例/测试直接调用） ──────────────────────────────────────

pub mod api {
    use super::*;

    /// 建运行时 + 客户端。重复调用是安全的（第二次直接返回）。
    pub fn configure(uri: &str, db: &str) -> Result<(), String> {
        if CLIENT.get().is_none() {
            let runtime = Runtime::new().map_err(|err| format!("建 tokio 运行时失败：{err}"))?;
            let client = runtime
                .block_on(Client::with_uri_str(uri))
                .map_err(|err| format!("连接 MongoDB 失败（{uri}）：{err}"))?;
            let _ = RUNTIME.set(runtime);
            let _ = CLIENT.set(client);
        }
        SETTINGS.set(Config {
            uri: uri.to_owned(),
            db: db.to_owned(),
        });
        Ok(())
    }

    /// 按方法名分派。入参/出参与服务协议完全一致。
    pub fn call(method: &str, input: Value) -> Result<Value, String> {
        match method {
            "ping" => ping(),
            "find" => find(&input),
            "find_one" => find_one(&input),
            "insert" => insert(&input),
            "update" => update(&input),
            "delete" => delete(&input),
            "count" => count(&input),
            "aggregate" => aggregate(&input),
            "distinct" => distinct(&input),
            "ensure_index" => ensure_index(&input),
            "collections" => collections(&input),
            other => Err(format!("未知方法：{other}")),
        }
    }

    pub fn ping() -> Result<Value, String> {
        let (_handle, client, db) = connection()?;
        block_on(client.database(&db).run_command(doc! { "ping": 1 }))
            .map_err(|err| format!("ping 失败：{err}"))?;
        let names = block_on(client.database(&db).list_collection_names())
            .map_err(|err| format!("取集合列表失败：{err}"))?;
        let uri = SETTINGS.get().map(|config| config.uri).unwrap_or_default();
        Ok(json!({ "db": db, "uri": uri, "collections": names.len() }))
    }

    fn find(input: &Value) -> Result<Value, String> {
        let (collection, _db) = target(input)?;
        let filter = opt_document(input, "filter")?;
        let projection = opt_document(input, "projection")?;
        let sort = opt_document(input, "sort")?;

        let mut find = collection.find(filter);
        if !projection.is_empty() {
            find = find.projection(projection);
        }
        if !sort.is_empty() {
            find = find.sort(sort);
        }
        if let Some(limit) = opt_u64(input, "limit") {
            find = find.limit(limit.min(10_000) as i64);
        }
        if let Some(skip) = opt_u64(input, "skip") {
            find = find.skip(skip);
        }
        // 游标必须在同一个 async 块里读完：出了 block_on 就没有运行时了。
        let docs: Vec<Document> = block_on(async move {
            let cursor = find.await.map_err(|err| format!("find 失败：{err}"))?;
            cursor
                .try_collect::<Vec<Document>>()
                .await
                .map_err(|err| format!("读游标失败：{err}"))
        })?;
        let count = docs.len();
        Ok(json!({ "docs": docs_to_json(docs), "count": count }))
    }

    fn find_one(input: &Value) -> Result<Value, String> {
        let (collection, _db) = target(input)?;
        let filter = opt_document(input, "filter")?;
        let projection = opt_document(input, "projection")?;
        let mut find = collection.find_one(filter);
        if !projection.is_empty() {
            find = find.projection(projection);
        }
        let doc = block_on(find).map_err(|err| format!("find_one 失败：{err}"))?;
        Ok(json!({ "doc": doc.map(to_json) }))
    }

    fn insert(input: &Value) -> Result<Value, String> {
        let (collection, _db) = target(input)?;
        let mut docs: Vec<Document> = match input.get("docs") {
            Some(Value::Array(items)) => items.iter().map(to_document).collect::<Result<_, _>>()?,
            Some(value @ Value::Object(_)) => vec![to_document(value)?],
            _ => return Err("缺少 docs（对象或对象数组）".to_owned()),
        };
        if docs.is_empty() {
            return Ok(json!({ "inserted": 0, "ids": [] }));
        }
        let ids: Vec<Value> = if docs.len() > 1 {
            let result = block_on(collection.insert_many(docs))
                .map_err(|err| format!("insert_many 失败：{err}"))?;
            result
                .inserted_ids
                .into_values()
                .map(bson_to_json)
                .collect()
        } else {
            let doc = docs.pop().expect("上面判过非空");
            let id = block_on(collection.insert_one(doc))
                .map_err(|err| format!("insert_one 失败：{err}"))?
                .inserted_id;
            vec![bson_to_json(id)]
        };
        Ok(json!({ "inserted": ids.len(), "ids": ids }))
    }

    fn update(input: &Value) -> Result<Value, String> {
        let (collection, _db) = target(input)?;
        let filter = opt_document(input, "filter")?;
        // `update` 支持完整操作符（`$set` / `$inc`…）；`patch` 是快捷方式，自动包 `$set`。
        let update = match input.get("update") {
            Some(value @ Value::Object(_)) => to_document(value)?,
            _ => {
                let patch = opt_document(input, "patch")?;
                if patch.is_empty() {
                    return Err("缺少 update 或 patch".to_owned());
                }
                doc! { "$set": patch }
            }
        };
        let upsert = opt_bool(input, "upsert").unwrap_or(false);
        let many = opt_bool(input, "many").unwrap_or(false);

        if many {
            let result = block_on(collection.update_many(filter, update).upsert(upsert))
                .map_err(|err| format!("update_many 失败：{err}"))?;
            Ok(json!({
                "matched": result.matched_count,
                "modified": result.modified_count,
            }))
        } else {
            let result = block_on(collection.update_one(filter, update).upsert(upsert))
                .map_err(|err| format!("update_one 失败：{err}"))?;
            Ok(json!({
                "matched": result.matched_count,
                "modified": result.modified_count,
                "upserted": result.upserted_id.map(bson_to_json),
            }))
        }
    }

    fn delete(input: &Value) -> Result<Value, String> {
        let (collection, _db) = target(input)?;
        let filter = opt_document(input, "filter")?;
        if filter.is_empty() && !opt_bool(input, "allow_empty").unwrap_or(false) {
            // 空过滤 = 删全表。TS 版也踩过这个坑，这里强制显式确认。
            return Err("拒绝执行空过滤的删除（确实要清空请传 allow_empty: true）".to_owned());
        }
        let many = opt_bool(input, "many").unwrap_or(true);
        let deleted = if many {
            block_on(collection.delete_many(filter))
                .map_err(|err| format!("delete_many 失败：{err}"))?
                .deleted_count
        } else {
            block_on(collection.delete_one(filter))
                .map_err(|err| format!("delete_one 失败：{err}"))?
                .deleted_count
        };
        Ok(json!({ "deleted": deleted }))
    }

    fn count(input: &Value) -> Result<Value, String> {
        let (collection, _db) = target(input)?;
        let filter = opt_document(input, "filter")?;
        let count = block_on(collection.count_documents(filter))
            .map_err(|err| format!("count 失败：{err}"))?;
        Ok(json!({ "count": count }))
    }

    fn aggregate(input: &Value) -> Result<Value, String> {
        let (collection, _db) = target(input)?;
        let pipeline: Vec<Document> = match input.get("pipeline") {
            Some(Value::Array(stages)) => {
                stages.iter().map(to_document).collect::<Result<_, _>>()?
            }
            _ => return Err("缺少 pipeline（阶段数组）".to_owned()),
        };
        let docs: Vec<Document> = block_on(async move {
            let cursor = collection
                .aggregate(pipeline)
                .await
                .map_err(|err| format!("aggregate 失败：{err}"))?;
            cursor
                .try_collect::<Vec<Document>>()
                .await
                .map_err(|err| format!("读游标失败：{err}"))
        })?;
        let count = docs.len();
        Ok(json!({ "docs": docs_to_json(docs), "count": count }))
    }

    fn distinct(input: &Value) -> Result<Value, String> {
        let (collection, _db) = target(input)?;
        let field = require_str(input, "field")?.to_owned();
        let filter = opt_document(input, "filter")?;
        let values: Vec<Bson> = block_on(collection.distinct(field, filter))
            .map_err(|err| format!("distinct 失败：{err}"))?;
        Ok(json!({ "values": values.into_iter().map(bson_to_json).collect::<Vec<_>>() }))
    }

    fn ensure_index(input: &Value) -> Result<Value, String> {
        let (collection, _db) = target(input)?;
        let keys = opt_document(input, "keys")?;
        if keys.is_empty() {
            return Err("缺少 keys（如 {\"id\": 1}）".to_owned());
        }
        let mut options = IndexOptions::default();
        options.unique = opt_bool(input, "unique");
        options.sparse = opt_bool(input, "sparse");
        options.name = input.get("name").and_then(Value::as_str).map(str::to_owned);
        if let Some(seconds) = opt_u64(input, "ttl_seconds") {
            options.expire_after = Some(std::time::Duration::from_secs(seconds));
        }
        let model = IndexModel::builder().keys(keys).options(options).build();
        let name =
            block_on(collection.create_index(model)).map_err(|err| format!("建索引失败：{err}"))?;
        Ok(json!({ "index": name.index_name }))
    }

    fn collections(input: &Value) -> Result<Value, String> {
        let (_handle, client, default_db) = connection()?;
        let db = opt_str(input, "db").unwrap_or(&default_db).to_owned();
        let names = block_on(client.database(&db).list_collection_names())
            .map_err(|err| format!("取集合列表失败：{err}"))?;
        Ok(json!({ "db": db, "collections": names }))
    }

    // ── 内部 ──────────────────────────────────────────────────────────────

    /// 取集合（`db` 可选，覆盖默认库）。
    fn target(input: &Value) -> Result<(Collection<Document>, String), String> {
        let name = require_str(input, "collection")?.to_owned();
        let (_handle, client, default_db) = connection()?;
        let db = opt_str(input, "db").unwrap_or(&default_db).to_owned();
        let collection = client.database(&db).collection::<Document>(&name);
        Ok((collection, db))
    }

    fn connection() -> Result<(Handle, Client, String), String> {
        let config = SETTINGS.require().map_err(|_| {
            "storage.mongo 未初始化：请先调 api::configure 或在插件配置里给 uri/db".to_owned()
        })?;
        let client = CLIENT
            .get()
            .cloned()
            .ok_or_else(|| "MongoDB 客户端未就绪".to_owned())?;
        let handle = RUNTIME
            .get()
            .ok_or_else(|| "tokio 运行时未就绪".to_owned())?
            .handle()
            .clone();
        Ok((handle, client, config.db))
    }

    /// 在运行时上把异步调用跑完。调用方线程是宿主回调线程，不在运行时里，`block_on` 安全。
    ///
    /// 参数取 `IntoFuture`：mongodb 3.x 的 action（`find` / `run_command`…）本身不是 Future，
    /// 要等它转成 Future 才能 `block_on`。运行时的存在性由 `connection()` 保证。
    fn block_on<T>(future: impl std::future::IntoFuture<Output = T>) -> T {
        let handle = RUNTIME
            .get()
            .expect("configure 已建好运行时；未初始化的调用会先在 connection() 处报错")
            .handle()
            .clone();
        handle.block_on(async move { future.await })
    }

    fn to_document(value: &Value) -> Result<Document, String> {
        match value {
            Value::Null => Ok(Document::new()),
            Value::Object(_) => serde_json::from_value::<Document>(value.clone())
                .map_err(|err| format!("不是合法 BSON：{err}")),
            other => Err(format!("期望对象，收到 {other}")),
        }
    }

    fn opt_document(input: &Value, key: &str) -> Result<Document, String> {
        match input.get(key) {
            None | Some(Value::Null) => Ok(Document::new()),
            Some(value) => to_document(value),
        }
    }

    /// BSON → JSON。用 relaxed Extended JSON：`ObjectId` / `Date` 变成 `{"$oid":..}`，
    /// 数字与字符串保持自然形态，业务侧好读也好回写。
    fn to_json(doc: Document) -> Value {
        Bson::Document(doc).into_relaxed_extjson()
    }

    fn docs_to_json(docs: Vec<Document>) -> Value {
        Value::Array(docs.into_iter().map(to_json).collect())
    }

    fn bson_to_json(value: Bson) -> Value {
        value.into_relaxed_extjson()
    }
}

// ─── 服务壳（薄层） ────────────────────────────────────────────────────────

fn dispatch(method: &str, input: *const c_char) -> *const c_char {
    let result = parse(input).and_then(|value| api::call(method, value));
    match result {
        Ok(fields) => ok(fields),
        Err(message) => {
            if let Some(host) = HOST.get() {
                emit(
                    host,
                    "mongo.error",
                    json!({ "method": method, "error": message }),
                );
            }
            error(message)
        }
    }
}

unsafe extern "C" fn svc_ping(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("ping", input)
}
unsafe extern "C" fn svc_find(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("find", input)
}
unsafe extern "C" fn svc_find_one(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("find_one", input)
}
unsafe extern "C" fn svc_insert(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("insert", input)
}
unsafe extern "C" fn svc_update(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("update", input)
}
unsafe extern "C" fn svc_delete(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("delete", input)
}
unsafe extern "C" fn svc_count(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("count", input)
}
unsafe extern "C" fn svc_aggregate(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("aggregate", input)
}
unsafe extern "C" fn svc_distinct(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("distinct", input)
}
unsafe extern "C" fn svc_ensure_index(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("ensure_index", input)
}
unsafe extern "C" fn svc_collections(_ctx: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch("collections", input)
}

#[cfg(test)]
mod tests {
    use super::api::call;
    use mongodb::bson::{Bson, Document};
    use serde_json::json;

    /// JSON ↔ BSON 的转换逻辑不依赖数据库，单独验证。
    #[test]
    fn extended_json_is_accepted_for_filters() {
        // 这里走的是 api 内部同一套 serde_json → Document 转换。
        let value = json!({ "_id": { "$oid": "507f1f77bcf86cd799439011" }, "id": 42 });
        let doc: Document = serde_json::from_value(value).unwrap();
        assert!(matches!(doc.get("_id"), Some(Bson::ObjectId(_))));
        assert_eq!(doc.get_i32("id").unwrap(), 42);
    }

    #[test]
    fn unconfigured_calls_fail_with_a_clear_message() {
        // 没调 configure 时不能 panic，要给业务侧可读的错误。
        let err = call("find", json!({ "collection": "anime" })).unwrap_err();
        assert!(err.contains("未初始化") || err.contains("未就绪"), "{err}");
    }

    #[test]
    fn unknown_method_is_rejected() {
        let err = call("drop_everything", json!({})).unwrap_err();
        assert!(err.contains("未知方法"), "{err}");
    }
}
