//! `storage.mongo` 服务的 [Storage] 实现。
//!
//! 领域层（`anime-core`）只定义「要什么数据」，这里负责把它翻译成
//! `storage.mongo` 的 JSON 调用。两边唯一的耦合是方法名与集合名，
//! 所以换存储实现（内存、远程）只要换这个文件。
//!
//! 错误统一包成 [anime_core::Error::Store]，消息里带集合名，日志能直接定位。

use anime_core::Error;
use anime_core::Result;
use anime_core::store::Storage;
use fuyubot_abi::HostApi;
use fuyubot_service::{call_service, service_available};
use serde_json::{Value, json};

/// 服务名。宿主里四个基础插件之一。
const SERVICE: &str = "storage.mongo";

/// 用宿主服务实现的存储。
///
/// `HostApi` 是 `Copy` 的，所以这个结构也能随手复制进后台线程。
#[derive(Debug, Clone, Copy)]
pub struct HostStore {
    host: HostApi,
}

impl HostStore {
    pub fn new(host: HostApi) -> Self {
        Self { host }
    }

    /// 服务在不在。轮询线程启动时探一次，缺了就降级成「只抓不存」，
    /// 而不是每轮都打一条同样的错误日志。
    pub fn available(&self) -> bool {
        service_available(&self.host, SERVICE, "find")
    }

    fn call(&self, method: &str, input: Value) -> Result<Value> {
        call_service(&self.host, SERVICE, method, &input)
            .map_err(|error| Error::Store(format!("{SERVICE}.{method}：{error}")))
    }
}

impl Storage for HostStore {
    fn find(
        &self,
        collection: &str,
        filter: Value,
        projection: Option<Value>,
        sort: Option<Value>,
        limit: Option<usize>,
    ) -> Result<Vec<Value>> {
        let mut input = json!({ "collection": collection, "filter": filter });
        if let Some(projection) = projection {
            input["projection"] = projection;
        }
        if let Some(sort) = sort {
            input["sort"] = sort;
        }
        if let Some(limit) = limit {
            input["limit"] = json!(limit);
        }
        let reply = self.call("find", input)?;
        Ok(reply
            .get("docs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    fn find_one(
        &self,
        collection: &str,
        filter: Value,
        projection: Option<Value>,
    ) -> Result<Option<Value>> {
        let mut input = json!({ "collection": collection, "filter": filter });
        if let Some(projection) = projection {
            input["projection"] = projection;
        }
        let reply = self.call("find_one", input)?;
        // 服务端用 null 表示没命中（不是缺字段），两种情况都当 None。
        Ok(reply.get("doc").filter(|doc| !doc.is_null()).cloned())
    }

    fn insert(&self, collection: &str, docs: Value) -> Result<()> {
        self.call("insert", json!({ "collection": collection, "docs": docs }))?;
        Ok(())
    }

    fn update(
        &self,
        collection: &str,
        filter: Value,
        update: Value,
        upsert: bool,
        many: bool,
    ) -> Result<u64> {
        let reply = self.call(
            "update",
            json!({
                "collection": collection,
                "filter": filter,
                "update": update,
                "upsert": upsert,
                "many": many,
            }),
        )?;
        // modified 是真正改动的条数；没有它时退回 matched（upsert 插入算 0 条修改）。
        Ok(reply
            .get("modified")
            .or_else(|| reply.get("matched"))
            .and_then(Value::as_u64)
            .unwrap_or(0))
    }

    fn count(&self, collection: &str, filter: Value) -> Result<u64> {
        let reply = self.call("count", json!({ "collection": collection, "filter": filter }))?;
        Ok(reply.get("count").and_then(Value::as_u64).unwrap_or(0))
    }

    fn delete(&self, collection: &str, filter: Value, many: bool) -> Result<u64> {
        let reply = self.call(
            "delete",
            json!({ "collection": collection, "filter": filter, "many": many }),
        )?;
        Ok(reply.get("deleted").and_then(Value::as_u64).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_is_stable() {
        // 改名等于换契约：所有业务查询都会落空，但不会报错——所以用测试钉住。
        assert_eq!(SERVICE, "storage.mongo");
    }
}
