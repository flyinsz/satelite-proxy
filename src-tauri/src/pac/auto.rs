//! PAC 名单自动更新后台任务。
//!
//! 按 `pac_update_interval_hours` 间隔检查 `pac_last_update`，到期则拉取
//! 配置的更新地址。仿 `subscription_auto`：进程内退避，失败后 30 分钟
//! 再试，不落盘（重启即重置）。
//!
//! 拉取用同步 `fetch_gfwlist_blocking`，因此整体丢到
//! `spawn_blocking` 里跑，不占 tokio worker。

use crate::state::AppState;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 检查间隔：每 5 分钟看一次是否需要更新。
const TICK_SECS: u64 = 300;
/// 首次检查延迟，等应用 setup 完成。
const INITIAL_DELAY_SECS: u64 = 45;
/// 失败后的退避时长（秒）。
const RETRY_BACKOFF_SECS: u64 = 30 * 60;

/// 下次允许尝试更新的时间戳（unix 秒）；0 表示无限制。
static RETRY_AT: AtomicU64 = AtomicU64::new(0);

/// 启动后台自动更新循环。仅当设置里开启自动更新时才会真正拉取。
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(INITIAL_DELAY_SECS)).await;
        loop {
            run_once(&app).await;
            tokio::time::sleep(Duration::from_secs(TICK_SECS)).await;
        }
    });
}

async fn run_once(app: &AppHandle) {
    // 锁内只读取配置并判断是否到期，网络请求放到锁外。
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let due = state
        .with_store(|store| {
            if !store.settings.pac_auto_update {
                return Ok(None);
            }
            let interval = store.settings.pac_update_interval_hours.max(1) as u64;
            let last = store.settings.pac_last_update.unwrap_or(0);
            let now = now_secs();
            // last == 0（从未更新）视为立即到期。
            let due = last == 0 || now.saturating_sub(last) >= interval * 3600;
            Ok(due.then(|| (store.settings.pac_source_url.clone(), interval)))
        })
        .ok()
        .flatten();

    // `with_store` 只返回所有权数据，不借用 store 锁。
    let Some((source_url, interval)) = due else {
        return;
    };
    let now = now_secs();
    let retry_at = RETRY_AT.load(Ordering::Relaxed);
    if retry_at > now {
        return;
    }

    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        // 在阻塞线程里重新取 state：AppState 由 Tauri 持有，不能跨线程搬。
        let state = worker_app
            .try_state::<AppState>()
            .ok_or_else(|| crate::error::AppError::Core("app state unavailable".into()))?;
        state.refresh_gfwlist_from(&source_url)
    })
    .await;

    match result {
        Ok(Ok(list)) => {
            RETRY_AT.store(0, Ordering::Relaxed);
            crate::app_log::info(
                "pac",
                format!(
                    "auto-updated gfwlist (interval {interval}h): {} upstream domains",
                    list.gfwlist_domains.len()
                ),
            );
            // 前端刷新计数 / 时间戳；PAC 脚本内容已在 refresh 里重建。
            let _ = app.emit("pac-list-updated", &list);
        }
        Ok(Err(error)) => {
            RETRY_AT.store(now.saturating_add(RETRY_BACKOFF_SECS), Ordering::Relaxed);
            crate::app_log::warn(
                "pac",
                format!("auto-update gfwlist failed ({error}); retry in 30m"),
            );
        }
        Err(error) => {
            RETRY_AT.store(now.saturating_add(RETRY_BACKOFF_SECS), Ordering::Relaxed);
            crate::app_log::warn("pac", format!("auto-update task panicked: {error}"));
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn due_logic_handles_never_updated_and_intervals() {
        let due = |last: u64, interval_hours: u64, now: u64| {
            last == 0 || now.saturating_sub(last) >= interval_hours * 3600
        };
        // 从未更新 → 立刻到期。
        assert!(due(0, 24, 1_000_000));
        // 24h 间隔：刚更新过 → 不到期；刚好超期 → 到期。
        assert!(!due(1_000_000, 24, 1_000_000 + 3600));
        assert!(due(1_000_000, 24, 1_000_000 + 24 * 3600));
    }
}