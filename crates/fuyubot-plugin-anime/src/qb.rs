//! `download.qbittorrent` 的薄客户端。
//!
//! 只包业务真的会用的四个动作：加种子、查列表、查文件、查属性。
//! 暂停/继续/删除留给调用方按需加——现在链路里没有「取消」的需求。
//!
//! 响应形状：qb 插件把 qBittorrent 的原始 JSON 直接返回，数组会被 [fuyubot_service::ok]
//! 包成 `{"ok":true,"value":[...]}`，对象则平铺。所以取值统一走 [field_array]。

use anime_core::{Error, Result};
use fuyubot_abi::HostApi;
use fuyubot_service::{call_service, service_available};
use serde_json::{Value, json};

const SERVICE: &str = "download.qbittorrent";

/// qB 客户端。`HostApi` 是 `Copy`，整个结构可以随手复制进线程。
#[derive(Debug, Clone, Copy)]
pub struct Qb {
    host: HostApi,
}

impl Qb {
    pub fn new(host: HostApi) -> Self {
        Self { host }
    }

    /// 插件装没装、起没起。
    pub fn available(&self) -> bool {
        service_available(&self.host, SERVICE, "list")
    }

    fn call(&self, method: &str, input: Value) -> Result<Value> {
        call_service(&self.host, SERVICE, method, &input)
            .map_err(|message| Error::Service {
                service: SERVICE.to_owned(),
                message,
            })
    }

    /// 加一个磁力。`save_path` 为空时用 qB 的默认目录。
    pub fn add(&self, magnet: &str, save_path: &str, category: &str) -> Result<()> {
        let mut input = json!({ "urls": [magnet] });
        if !save_path.trim().is_empty() {
            input["savepath"] = json!(save_path);
        }
        if !category.trim().is_empty() {
            input["category"] = json!(category);
        }
        self.call("add", input)?;
        Ok(())
    }

    /// 按 hash 过滤的种子列表。qB 的 `hashes` 参数用 `|` 连接。
    pub fn list(&self, hashes: &[String]) -> Result<Vec<Value>> {
        let mut input = json!({});
        if !hashes.is_empty() {
            input["hashes"] = json!(hashes.join("|"));
        }
        let reply = self.call("list", input)?;
        Ok(field_array(&reply))
    }

    /// 删除种子；`delete_files` = 连磁盘文件一起删。
    ///
    /// 发送完成后调用：不删的话 qB 会一直做种占着磁盘，而且源文件被我们删掉后
    /// 会变成 missingFiles 状态。
    pub fn delete(&self, hash: &str, delete_files: bool) -> Result<()> {
        self.call(
            "delete",
            json!({ "hashes": hash, "delete_files": delete_files }),
        )?;
        Ok(())
    }

    /// 种子里的文件列表（`name` 是相对路径，`size` 是字节）。
    pub fn files(&self, hash: &str) -> Result<Vec<Value>> {
        let reply = self.call("files", json!({ "hash": hash }))?;
        Ok(field_array(&reply))
    }
}

/// 从服务响应里取数组：数组被包在 `value` 里，对象平铺。
fn field_array(reply: &Value) -> Vec<Value> {
    if let Some(items) = reply.as_array() {
        return items.clone();
    }
    for key in ["value", "data", "docs"] {
        if let Some(items) = reply.get(key).and_then(Value::as_array) {
            return items.clone();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_is_read_from_value_wrapper() {
        let reply = json!({ "ok": true, "value": [{ "hash": "a" }, { "hash": "b" }] });
        assert_eq!(field_array(&reply).len(), 2);
    }

    #[test]
    fn plain_array_and_missing_field_are_handled() {
        assert_eq!(field_array(&json!([1, 2])).len(), 2);
        assert!(field_array(&json!({ "ok": true })).is_empty());
    }
}
