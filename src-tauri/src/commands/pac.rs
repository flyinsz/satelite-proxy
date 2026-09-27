//! PAC 反向代理的命令层：实现方式切换、名单读写、gfwlist 刷新、状态查询。

use crate::runtime::ProxyStatus;
use crate::state::{AppState, PacStatus};
use tauri::{AppHandle, Manager, State};

/// 切换 system 代理实现方式 manual|pac。若 system 代理当前开启则重新应用。
#[tauri::command]
pub async fn set_system_proxy_kind(
    app: AppHandle,
    kind: String,
) -> Result<ProxyStatus, String> {
    let resource_dir = app.path().resource_dir().ok();
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = worker_app
            .try_state::<AppState>()
            .ok_or_else(|| "app state unavailable".to_string())?;
        state
            .set_system_proxy_kind(&kind, resource_dir.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("system proxy kind task: {e}"))?;
    crate::tray::refresh_icon(&app);
    result
}

/// 当前 PAC 名单。
#[tauri::command]
pub async fn get_pac_list(state: State<'_, AppState>) -> Result<crate::pac::PacList, String> {
    state.pac_list().map_err(|e| e.to_string())
}

/// 保存 PAC 名单；PAC 服务运行中则重建内容使其立即生效。
#[tauri::command]
pub async fn update_pac_list(
    app: AppHandle,
    state: State<'_, AppState>,
    list: crate::pac::PacList,
) -> Result<crate::pac::PacList, String> {
    let _ = &state;
    // set_pac_list 在 pac_active 时会重建本地 PAC 服务（block_on 嵌套 runtime），
    // 必须在 spawn_blocking 线程里执行；锁顺序由 AppState 内部保证。
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = worker_app
            .try_state::<AppState>()
            .ok_or_else(|| "app state unavailable".to_string())?;
        state.set_pac_list(list).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("update pac list task: {e}"))?;
    crate::tray::refresh_icon(&app);
    result
}

/// 拉取 gfwlist 并合并进 PAC 名单（网络操作，走 spawn_blocking）。
#[tauri::command]
pub async fn refresh_gfwlist(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<crate::pac::PacList, String> {
    let _ = &state;
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = worker_app
            .try_state::<AppState>()
            .ok_or_else(|| "app state unavailable".to_string())?;
        state.refresh_gfwlist().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("refresh gfwlist task: {e}"))?;
    crate::tray::refresh_icon(&app);
    result
}

/// PAC 功能状态快照（实现方式 / 是否启用 / URL / 域名数 / 刷新时间）。
#[tauri::command]
pub async fn get_pac_status(state: State<'_, AppState>) -> Result<PacStatus, String> {
    state.pac_status().map_err(|e| e.to_string())
}
