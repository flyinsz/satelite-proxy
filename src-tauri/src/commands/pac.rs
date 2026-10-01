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
///
/// 使用设置里配置的更新地址；如需临时指定地址请先用
/// [`update_pac_settings`]。
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

/// 拉取指定分组的远程地址并替换该分组条目（网络操作，走 spawn_blocking）。
#[tauri::command]
pub async fn refresh_pac_group(
    app: AppHandle,
    state: State<'_, AppState>,
    group_id: String,
) -> Result<crate::pac::PacList, String> {
    let _ = &state;
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = worker_app
            .try_state::<AppState>()
            .ok_or_else(|| "app state unavailable".to_string())?;
        state
            .refresh_pac_group(&group_id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("refresh pac group task: {e}"))?;
    crate::tray::refresh_icon(&app);
    result
}

/// PAC 功能状态快照（实现方式 / 是否启用 / URL / 域名数 / 刷新时间）。
#[tauri::command]
pub async fn get_pac_status(state: State<'_, AppState>) -> Result<PacStatus, String> {
    state.pac_status().map_err(|e| e.to_string())
}

/// gfwlist 更新地址的内置预设（GitHub raw / jsDelivr / Fastly）。
#[tauri::command]
pub fn list_pac_source_presets() -> Vec<crate::pac::PacSourcePreset> {
    crate::pac::source_presets()
}

/// 更新 PAC 设置：更新地址 / 自动更新 / 间隔 / 端口。
///
/// 端口变更且 PAC 服务运行中会重建服务并重指系统代理，因此在
/// spawn_blocking 线程里执行（内部会 block_on 停启 PAC 服务）。
#[tauri::command]
pub async fn update_pac_settings(
    app: AppHandle,
    source_url: Option<String>,
    auto_update: Option<bool>,
    update_interval_hours: Option<u32>,
    pac_port: Option<u16>,
) -> Result<PacStatus, String> {
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = worker_app
            .try_state::<AppState>()
            .ok_or_else(|| "app state unavailable".to_string())?;
        state
            .update_pac_settings(source_url, auto_update, update_interval_hours, pac_port)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("update pac settings task: {e}"))?;
    crate::tray::refresh_icon(&app);
    result
}

/// 当前生效的 PAC 脚本文本（UI 预览 / 排障）。
#[tauri::command]
pub async fn get_pac_preview(state: State<'_, AppState>) -> Result<String, String> {
    state.pac_preview().map_err(|e| e.to_string())
}

/// 置空 gfwlist 上游域名（保留用户自定义域名），用于清空后重新拉取。
#[tauri::command]
pub async fn clear_gfwlist(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<crate::pac::PacList, String> {
    let _ = &state;
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = worker_app
            .try_state::<AppState>()
            .ok_or_else(|| "app state unavailable".to_string())?;
        state.clear_gfwlist().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("clear gfwlist task: {e}"))?;
    crate::tray::refresh_icon(&app);
    result
}
