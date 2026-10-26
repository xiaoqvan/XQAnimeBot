//! Bangumi 账号：取可用的 access token（必要时自动刷新）。
//!
//! 对应 TS 的 `utils/getBgmToken.ts`。所有要调 Bangumi 写接口的地方
//! （收藏条目 / 标记章节看过）都先过这一步，避免各自实现一套「过期了怎么办」。
//!
//! ## 为什么提前一分钟刷新
//!
//! `authTime + expiresIn` 算出的过期时刻是**服务端**视角的，网络往返和我们自己
//! 的时钟偏差都会吃掉一点。TS 提前 60 秒刷新，照搬：宁可多刷一次（refresh token
//! 换一次不影响用户），也别让用户在「还差 3 秒过期」时收到一个 401。

use anime_core::bangumi::BgmClient;
use anime_core::log;
use anime_core::models::BangumiUserDoc;
use anime_core::store::Storage;
use chrono::Utc;

use crate::runtime::Runtime;

/// 提前多久刷新。见模块注释。
const REFRESH_MARGIN_SECS: i64 = 60;

/// 取 token 的结果：要么拿到，要么带着**给用户看的原因**失败。
///
/// 用枚举而不是 `Result` 是因为失败原因要原样发给用户（TS 也是把 `message`
/// 直接发回去）——「没绑定 / 没授权 / 刷新失败」三种提示各不相同，
/// 收成一个 `Error` 再在 UI 层拆解反而更绕。
pub enum BgmToken {
    /// 可用的 access token。
    Ready(String),
    /// 拿不到，附带给用户看的说明（已是可直接发送的话术）。
    Failed(String),
}

impl BgmToken {
    /// 拿到就走 `then`，拿不到就把原因交给 `otherwise`。
    pub fn either<T>(self, then: impl FnOnce(String) -> T, otherwise: impl FnOnce(String) -> T) -> T {
        match self {
            Self::Ready(token) => then(token),
            Self::Failed(message) => otherwise(message),
        }
    }
}

/// 取这个 Telegram 用户可用的 Bangumi access token，过期就自动刷新。
///
/// 刷新成功后**写回库**（新的 access/refresh token、新的 `authTime`），
/// 否则下一次调用会拿着过期 token 再刷一次——refresh token 是滚动的，
/// 旧的那次用过就作废，不落库等于把用户踢下线。
pub fn get_bgm_token(runtime: &Runtime, tg_user_id: i64) -> BgmToken {
    let user = match runtime.store.bangumi_user_by_tg(tg_user_id) {
        Ok(Some(user)) => user,
        Ok(None) => {
            return BgmToken::Failed(
                "未绑定 Bangumi 账户\n请先使用 /bindbangumi 绑定后再操作".to_owned(),
            )
        }
        Err(error) => {
            log!(&runtime.logger, Warn, "读 Bangumi 绑定失败：{error}");
            return BgmToken::Failed(format!("读取绑定信息失败：{error}"));
        }
    };
    let Some(access_token) = user
        .access_token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
    else {
        return BgmToken::Failed(
            "Bangumi 账户未授权\n请先使用 /bindbangumi 完成授权后再操作".to_owned(),
        );
    };

    if !needs_refresh(&user) {
        return BgmToken::Ready(access_token.to_owned());
    }
    let Some(refresh_token) = user
        .refresh_token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
    else {
        return BgmToken::Failed(
            "授权已过期且没有 refresh token\n请用 /exitbangumi 退出后重新 /bindbangumi 绑定".to_owned(),
        );
    };

    let client = BgmClient::new(runtime.config.bangumi.clone());
    match client.refresh_token(refresh_token) {
        Ok(token) => {
            let mut updated = user.clone();
            updated.access_token = Some(token.access_token.clone());
            // refresh 响应里可能不带新的 refresh token（Bangumi 会给，但可能是 null）：
            // 没有就保留旧的，别把它清成空——那就再也刷不动了。
            if let Some(next) = token.refresh_token.filter(|next| !next.trim().is_empty()) {
                updated.refresh_token = Some(next);
            }
            if token.expires_in > 0 {
                updated.expires_in = Some(token.expires_in as i64);
            }
            if !token.token_type.is_empty() {
                updated.token_type = Some(token.token_type);
            }
            if let Some(scope) = token.scope.filter(|scope| !scope.trim().is_empty()) {
                updated.scope = Some(scope);
            }
            updated.auth_time = Some(Utc::now());
            if let Err(error) = runtime.store.bangumi_user_upsert(&updated) {
                log!(&runtime.logger, Warn, "写回刷新后的 token 失败：{error}");
                // 库没写成功但 token 已经拿到：这次照样能用，下次会再刷一次。
            }
            BgmToken::Ready(token.access_token)
        }
        Err(error) => {
            log!(
                &runtime.logger,
                Warn,
                "刷新 Bangumi token 失败（tg {tg_user_id}）：{error}"
            );
            BgmToken::Failed(format!(
                "授权已过期且刷新失败：{error}\n可尝试 /exitbangumi 后重新 /bindbangumi 绑定"
            ))
        }
    }
}

/// 这个 token 需要刷新吗（`authTime + expiresIn` 已经过了/快到了）。
///
/// 缺 `authTime` 或缺 `expiresIn` 时**不刷**：那是老数据（比如用 `/bindbangumi
/// <access_token>` 手工绑的），没有刷新依据，硬刷只会拿到 400；直接拿去用，
/// 真过期了会在调用时报错，用户看到的是具体接口的错误而不是这里的猜测。
fn needs_refresh(user: &BangumiUserDoc) -> bool {
    let (Some(auth_time), Some(expires_in)) = (user.auth_time, user.expires_in) else {
        return false;
    };
    if expires_in <= 0 {
        return false;
    }
    let deadline = auth_time + chrono::Duration::seconds(expires_in - REFRESH_MARGIN_SECS);
    Utc::now() >= deadline
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(auth_time: Option<chrono::DateTime<Utc>>, expires_in: Option<i64>) -> BangumiUserDoc {
        BangumiUserDoc {
            auth_time,
            expires_in,
            access_token: Some("access".to_owned()),
            refresh_token: Some("refresh".to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn expired_tokens_need_refresh() {
        let past = Utc::now() - chrono::Duration::days(30);
        assert!(needs_refresh(&user(Some(past), Some(604_800))));
    }

    #[test]
    fn fresh_tokens_do_not() {
        assert!(!needs_refresh(&user(Some(Utc::now()), Some(604_800))));
    }

    #[test]
    fn the_margin_is_one_minute_not_exactly_at_expiry() {
        // 还剩 30 秒就该刷：边界上刷新比等到 401 安全。
        let almost = Utc::now() - chrono::Duration::seconds(604_800 - 30);
        assert!(needs_refresh(&user(Some(almost), Some(604_800))));
        // 还剩两分钟则不必。
        let plenty = Utc::now() - chrono::Duration::seconds(604_800 - 120);
        assert!(!needs_refresh(&user(Some(plenty), Some(604_800))));
    }

    #[test]
    fn without_auth_time_we_cannot_judge_so_we_do_not_refresh() {
        // 手工绑的旧记录没有 authTime/expiresIn，硬刷只会拿到 400。
        assert!(!needs_refresh(&user(None, Some(604_800))));
        assert!(!needs_refresh(&user(Some(Utc::now()), None)));
        assert!(!needs_refresh(&user(Some(Utc::now()), Some(0))));
    }
}
