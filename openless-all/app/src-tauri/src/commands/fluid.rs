//! Fluid 层命令：常用语（snippet）CRUD 与浮框撤销最近命中。

use super::*;

use openless_core::fluid::snippet_store::Snippet;

/// fluid_cancel_last 的返回：是否撤销了命中＋撤销后的指令预览拼装文本。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FluidCancelLastResult {
    pub cancelled: bool,
    pub assembled: Option<String>,
}

#[tauri::command]
pub fn list_fluid_snippets(core: CoreState<'_>) -> Result<Vec<Snippet>, String> {
    Ok(core.list_snippets())
}

#[tauri::command]
pub fn create_fluid_snippet(core: CoreState<'_>, snippet: Snippet) -> Result<Snippet, String> {
    core.create_snippet(snippet).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_fluid_snippet(core: CoreState<'_>, snippet: Snippet) -> Result<Snippet, String> {
    core.save_snippet(snippet).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_fluid_snippet(core: CoreState<'_>, id: String) -> Result<(), String> {
    core.delete_snippet(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_fluid_snippet_enabled(
    core: CoreState<'_>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    core.set_snippet_enabled(&id, enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn fluid_cancel_last(
    core: CoreState<'_>,
    session_id: String,
) -> Result<FluidCancelLastResult, String> {
    let parsed = uuid::Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let session_id = openless_core::SessionId::from_uuid(parsed);
    let cancelled = core
        .cancel_fluid_last_hit(session_id)
        .map_err(|e| e.to_string())?
        .is_some();
    let assembled = core.fluid_assembled_text(session_id);
    Ok(FluidCancelLastResult {
        cancelled,
        assembled,
    })
}
