//! Ghostwriter 层命令：常用语（snippet）CRUD、浮框撤销最近动作、候选/推荐点选、
//! 沉淀建议存取与任务书管理。

use super::*;

use openless_core::ghostwriter::snippet_store::Snippet;
use openless_core::ghostwriter::task_brief_store::TaskBriefInfo;
use openless_core::ghostwriter::types::SelectionKind;

/// ghostwriter_cancel_last 的返回：撤销结果（action："hit"|"selection"|"none"）＋
/// 撤销后的指令预览（拼装文本＋后端权威修订号，前端凭它丢弃撤销前在途的旧预览）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterCancelLastResult {
    pub cancelled: bool,
    pub action: String,
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
    let cancelled = core
        .cancel_ghostwriter_last_action(session_id)
        .map_err(|e| e.to_string())?;
    let (cancelled, action, revision) = match cancelled {
        Some((action, revision)) => {
            let action = match action {
                openless_core::ghostwriter::types::LastAction::Hit(_) => "hit",
                openless_core::ghostwriter::types::LastAction::Selection(_) => "selection",
            };
            (true, action.to_string(), revision)
        }
        None => (false, "none".to_string(), 0),
    };
    let assembled = core.ghostwriter_assembled_text(session_id);
    Ok(GhostwriterCancelLastResult {
        cancelled,
        action,
        assembled,
        revision,
    })
}

/// 点选/取消候选区一条（index 为 1-based 全局序号；kind："candidate"|"recommendation"）。
#[tauri::command]
pub fn ghostwriter_toggle_selection(
    core: CoreState<'_>,
    session_id: String,
    kind: String,
    index: usize,
) -> Result<(), String> {
    let parsed = uuid::Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let session_id = openless_core::SessionId::from_uuid(parsed);
    let kind = match kind.as_str() {
        "candidate" => SelectionKind::Candidate,
        "recommendation" => SelectionKind::Recommendation,
        other => return Err(format!("unknown ghostwriter selection kind: {other}")),
    };
    core.toggle_ghostwriter_selection(session_id, kind, index)
        .map_err(|e| e.to_string())
}

/// 沉淀建议存为常用语；无建议时返回 null。
#[tauri::command]
pub fn ghostwriter_save_suggestion(
    core: CoreState<'_>,
    session_id: String,
) -> Result<Option<Snippet>, String> {
    let parsed = uuid::Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let session_id = openless_core::SessionId::from_uuid(parsed);
    core.save_ghostwriter_suggestion(session_id)
        .map_err(|e| e.to_string())
}

/// 忽略沉淀建议（本次会话不再提）。
#[tauri::command]
pub fn ghostwriter_dismiss_suggestion(
    core: CoreState<'_>,
    session_id: String,
) -> Result<(), String> {
    let parsed = uuid::Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let session_id = openless_core::SessionId::from_uuid(parsed);
    core.dismiss_ghostwriter_suggestion(session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_ghostwriter_task_briefs(core: CoreState<'_>) -> Result<Vec<TaskBriefInfo>, String> {
    core.list_ghostwriter_task_briefs().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_ghostwriter_task_brief(
    core: CoreState<'_>,
    id: String,
    body: String,
) -> Result<TaskBriefInfo, String> {
    core.save_ghostwriter_task_brief(&id, &body)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reset_ghostwriter_task_brief(
    core: CoreState<'_>,
    id: String,
) -> Result<TaskBriefInfo, String> {
    core.reset_ghostwriter_task_brief(&id)
        .map_err(|e| e.to_string())
}
