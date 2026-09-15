//! Ghostwriter 层命令：常用语（snippet）CRUD 与浮框撤销最近命中。

use super::*;

use openless_core::ghostwriter::snippet_store::Snippet;

/// ghostwriter_cancel_last 的返回：是否撤销了命中＋撤销后的指令预览（拼装文本＋
/// 后端权威修订号，前端凭它丢弃撤销前在途的旧预览）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterCancelLastResult {
    pub cancelled: bool,
    pub assembled: Option<String>,
    pub revision: u64,
}

#[tauri::command]
pub fn list_ghostwriter_snippets(core: CoreState<'_>) -> Result<Vec<Snippet>, String> {
    Ok(core.list_snippets())
}

#[tauri::command]
pub fn create_ghostwriter_snippet(core: CoreState<'_>, snippet: Snippet) -> Result<Snippet, String> {
    core.create_snippet(snippet).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_ghostwriter_snippet(core: CoreState<'_>, snippet: Snippet) -> Result<Snippet, String> {
    core.save_snippet(snippet).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_ghostwriter_snippet(core: CoreState<'_>, id: String) -> Result<(), String> {
    core.delete_snippet(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_ghostwriter_snippet_enabled(
    core: CoreState<'_>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    core.set_snippet_enabled(&id, enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ghostwriter_cancel_last(
    core: CoreState<'_>,
    session_id: String,
) -> Result<GhostwriterCancelLastResult, String> {
    let parsed = uuid::Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let session_id = openless_core::SessionId::from_uuid(parsed);
    let (cancelled, revision) = match core
        .cancel_ghostwriter_last_hit(session_id)
        .map_err(|e| e.to_string())?
    {
        Some((_, revision)) => (true, revision),
        None => (false, 0),
    };
    let assembled = core.ghostwriter_assembled_text(session_id);
    Ok(GhostwriterCancelLastResult {
        cancelled,
        assembled,
        revision,
    })
}
