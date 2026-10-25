//! 待审核提示：把「拿不准的匹配」发到管理群，并挂上通过/驳回按钮。
//!
//! 对应 TS 的 `adminPrompts.ts::promptAdminConfirmAnime`：管理群收到一条带键盘的
//! 消息，点按钮走 `CallbackQuery` 那条路。这里只做「发提示 + 落 pendingReviews」，
//! 批准之后要做什么（下载/改库）留给 callback 的后续实现。
//!
//! 为什么要落库而不是把信息全塞进按钮：`callback_data` 有长度上限（TDLib 也是
//! 字节串），key 里只放一个短标识，详情回查。

use std::sync::Arc;

use anime_core::log;
use anime_core::matching::SeasonDecision;
use anime_core::models::AnimeItem;
use anime_core::store::Storage;
use anime_core::title;
use serde_json::json;

use crate::callback;
use crate::runtime::Runtime;
use crate::td::Target;

/// 发一条待审核提示。返回是否真的发出去了（没配管理群/写库失败都算没发）。
pub fn prompt(runtime: &Runtime, item: &AnimeItem, decision: &SeasonDecision) -> bool {
    let key = format!("rss:{}", title::clean_title(&item.title));
    let payload = json!({
        "title": item.title,
        "magnet": item.magnet,
        "names": item.names,
        "episode": item.episode,
        "fansub": item.fansub,
        "decision": decision.describe(),
    });
    if let Err(error) = runtime.store.pending_review_insert(&key, &payload) {
        log!(&runtime.logger, Warn, "写入待审核记录失败：{error}");
        return false;
    }

    let chat = runtime.config.telegram.admin_group_id;
    if chat == 0 {
        log!(&runtime.logger, Warn, "没配置 admin_group_id，待审核提示发不出去");
        return false;
    }
    let text = format!(
        "待审核\n标题：{}\n判定：{}\n集数：{}",
        item.title,
        decision.describe(),
        item.episode.as_deref().unwrap_or("未知")
    );
    runtime
        .td
        .send_text_with_buttons(Target::new(chat, 0), text, callback::review_buttons(&key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_key_uses_cleaned_title() {
        // key 必须稳定：同一条种子第二轮还要能查到同一条记录。
        let item = AnimeItem {
            title: "[组] 某番  -  05".to_owned(),
            pub_date: String::new(),
            magnet: String::new(),
            link: String::new(),
            team: String::new(),
            fansub: vec![],
            names: vec![],
            source: None,
            episode: None,
        };
        let key = format!("rss:{}", title::clean_title(&item.title));
        assert!(key.ends_with("某番 - 05"), "{key}");
        assert!(!key.contains("  "), "多余空白要被压掉：{key}");
    }
}
/// 审核通过后真正开始下载：从 pendingReviews 的记录里还原条目，交给 chain。
///
/// payload 是 [prompt] 写进去的（title/magnet/names/episode/fansub），这里只做还原，
/// 不做二次判断——要投哪部番、第几集，审核时已经定了。
pub fn start_approved_download(runtime: &Arc<Runtime>, record: &serde_json::Value) -> bool {
    let payload = record.get("payload").cloned().unwrap_or(serde_json::Value::Null);
    let Some(item) = item_from_payload(&payload) else {
        log!(&runtime.logger, Warn, "待审核记录缺少 magnet/title，无法开始下载");
        return false;
    };
    log!(&runtime.logger, Info, "审核通过，开始下载：{}", item.title);
    crate::chain::spawn(Arc::clone(runtime), item, 0);
    true
}

/// 从 payload 还原 [AnimeItem]；缺 magnet 或 title 就放弃。
fn item_from_payload(payload: &serde_json::Value) -> Option<AnimeItem> {
    let title = payload.get("title")?.as_str()?.to_owned();
    let magnet = payload.get("magnet").and_then(serde_json::Value::as_str)?.to_owned();
    if magnet.trim().is_empty() {
        return None;
    }
    let strings = |key: &str| {
        payload
            .get(key)
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let fansub = strings("fansub");
    Some(AnimeItem {
        title,
        pub_date: String::new(),
        magnet,
        link: String::new(),
        team: fansub.first().cloned().unwrap_or_default(),
        fansub,
        names: strings("names"),
        source: None,
        episode: payload
            .get("episode")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    })
}

#[cfg(test)]
mod payload_tests {
    use super::*;

    #[test]
    fn payload_round_trips_into_anime_item() {
        let payload = serde_json::json!({
            "title": "[组] 某番 - 05",
            "magnet": "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
            "names": ["某番"],
            "episode": "05",
            "fansub": ["组"]
        });
        let item = item_from_payload(&payload).expect("应当能还原");
        assert_eq!(item.team, "组");
        assert_eq!(item.episode.as_deref(), Some("05"));
        assert!(item.magnet.starts_with("magnet:"));
    }

    #[test]
    fn payload_without_magnet_is_rejected() {
        assert!(item_from_payload(&serde_json::json!({ "title": "x" })).is_none());
        assert!(item_from_payload(&serde_json::json!({ "title": "x", "magnet": "  " })).is_none());
    }
}
