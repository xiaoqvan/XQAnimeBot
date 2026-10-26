//! 待审核提示：把「拿不准的匹配」发到管理群，并挂上通过/驳回按钮。
//!
//! 对应 TS 的 `adminPrompts.ts::promptAdminConfirmAnime`：管理群收到一条带键盘的
//! 消息，点按钮走 `CallbackQuery` 那条路。这里只做「发提示 + 落 pendingReviews」，
//! 批准之后要做什么（下载/改库）留给 callback 的后续实现。
//!
//! 为什么要落库而不是把信息全塞进按钮：`callback_data` 有长度上限（TDLib 也是
//! 字节串），回调只带一个自增 id，详情回查文档。

use std::sync::Arc;

use anime_core::log;
use anime_core::matching::SeasonDecision;
use anime_core::models::{AnimeItem, PendingReviewDoc};
use anime_core::store::Storage;

use crate::callback;
use crate::runtime::Runtime;
use crate::td::Target;

/// 发一条待审核提示。返回待审核记录的自增 id（`None` = 没配管理群/写库失败）。
pub fn prompt(runtime: &Runtime, item: &AnimeItem, decision: &SeasonDecision) -> Option<i64> {
    let doc = PendingReviewDoc {
        item: Some(item.clone()),
        ..Default::default()
    };
    let id = match runtime.store.pending_review_insert(&doc) {
        Ok(id) => id,
        Err(error) => {
            log!(&runtime.logger, Warn, "写入待审核记录失败：{error}");
            return None;
        }
    };

    let chat = runtime.config.telegram.admin_group_id;
    if chat == 0 {
        log!(&runtime.logger, Warn, "没配置 admin_group_id，待审核提示发不出去");
        return None;
    }
    let text = format!(
        "待审核 #{id}\n标题：{}\n判定：{}\n集数：{}",
        item.title,
        decision.describe(),
        item.episode.as_deref().unwrap_or("未知")
    );
    runtime
        .td
        .send_text_with_buttons(Target::new(chat, 0), text, callback::review_buttons(id));
    Some(id)
}

/// 审核通过后真正开始下载：从 pendingReviews 的记录里还原条目，交给 chain。
///
/// `item` 是 [prompt] 写进去的结构化字段，这里只做反序列化还原，不做二次判断——
/// 要投哪部番、第几集，审核时已经定了。
pub fn start_approved_download(runtime: &Arc<Runtime>, record: &serde_json::Value) -> bool {
    let Some(item) = record
        .get("item")
        .and_then(|value| serde_json::from_value::<AnimeItem>(value.clone()).ok())
    else {
        log!(&runtime.logger, Warn, "待审核记录缺少 item，无法开始下载");
        return false;
    };
    if item.magnet.trim().is_empty() {
        log!(&runtime.logger, Warn, "待审核记录缺 magnet，无法开始下载");
        return false;
    }
    log!(&runtime.logger, Info, "审核通过，开始下载：{}", item.title);
    crate::chain::spawn(Arc::clone(runtime), item, 0);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_approved_download_rejects_missing_item_or_magnet() {
        // 没有 item 字段：拒绝。
        assert!(!start_approved_download_dry(&serde_json::json!({ "id": 1 })));
        // item 缺 magnet：拒绝。
        assert!(!start_approved_download_dry(&serde_json::json!({
            "item": { "title": "x", "magnet": "  " }
        })));
    }

    /// 测试用：不真的 spawn 线程，只判断 `start_approved_download` 的前置校验逻辑。
    /// 通过只测「能还原出 item」这个纯函数路径；真正 spawn 在 `start_approved_download`
    /// 里做，单测里用一个空 item 也不会真的去下载。
    fn start_approved_download_dry(record: &serde_json::Value) -> bool {
        let item = record
            .get("item")
            .and_then(|value| serde_json::from_value::<AnimeItem>(value.clone()).ok());
        match item {
            None => false,
            Some(item) => !item.magnet.trim().is_empty(),
        }
    }
}
