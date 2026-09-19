//! Ghostwriter 层命令：常用语（snippet）CRUD、浮框撤销最近动作、
//! 候选常用语按需提取与任务书管理。
//! 候选与推荐纯展示（2026-09-17 裁决），点选命令已移除。

use super::*;

use openless_core::ghostwriter::snippet_store::Snippet;
use openless_core::ghostwriter::task_brief_store::TaskBriefInfo;

/// fit 的高度下限（逻辑 px）：防内容测量异常把窗口钳成 0 高。
pub(crate) const GHOSTWRITER_MIN_FIT_HEIGHT_LOGICAL: f64 = 120.0;

/// 浮框窗口贴合内容的纯几何计算（物理坐标进出）。
///
/// 输入当前 outer position/size（物理像素）、scale_factor 与目标逻辑宽高；
/// 输出新物理尺寸与位置：新物理尺寸 = 逻辑值 × scale（高度先按
/// `min_height_logical` 钳制），高度差全部从顶边收（底边锚定，屏幕上卡片
/// 位置不动），宽度差取半居中。scale 非法（≤0 / NaN）按 1 处理。
#[cfg_attr(mobile, allow(dead_code))]
pub(crate) fn fit_window_geometry(
    position: (i32, i32),
    size: (u32, u32),
    scale_factor: f64,
    width_logical: f64,
    height_logical: f64,
    min_height_logical: f64,
) -> ((i32, i32), (u32, u32)) {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let width = if width_logical.is_finite() {
        (width_logical * scale).round().max(1.0) as u32
    } else {
        1
    };
    let height = if height_logical.is_finite() {
        height_logical.max(min_height_logical)
    } else {
        min_height_logical
    };
    let new_h = ((height * scale).round().max(1.0)) as u32;
    let new_x = position.0 + (size.0 as i32 - width as i32) / 2;
    // 高度差全部从顶边收：new_y + new_h == 旧底边，i32 整数运算精确锚定。
    let new_y = position.1 + size.1 as i32 - new_h as i32;
    ((new_x, new_y), (width, new_h))
}

#[cfg(test)]
mod tests {
    use super::{fit_window_geometry, GHOSTWRITER_MIN_FIT_HEIGHT_LOGICAL};

    #[test]
    fn fit_geometry_scales_logical_target_by_scale_factor_two() {
        // Retina（scale=2）：旧窗 560×420 物理 1120×840，目标逻辑 520×300。
        let ((x, y), (w, h)) = fit_window_geometry((100, 200), (1120, 840), 2.0, 520.0, 300.0, 120.0);
        assert_eq!((w, h), (1040, 600));
        assert_eq!((x, y), (140, 440));
        // 底边锚定：新旧底边重合。
        assert_eq!(y + h as i32, 200 + 840);
    }

    #[test]
    fn fit_geometry_recenters_horizontally_on_width_change() {
        // 宽度变化（560→520，scale=1）：x 右移宽度差的一半，高度不变则 y 不动。
        let ((x, y), (w, h)) = fit_window_geometry((10, 20), (560, 300), 1.0, 520.0, 300.0, 120.0);
        assert_eq!((w, h), (520, 300));
        assert_eq!((x, y), (30, 20));
    }

    #[test]
    fn fit_geometry_clamps_height_to_minimum() {
        // 高度下限钳制：目标 10 逻辑 px < 120，按 120 生效（scale=2 → 240 物理），
        // 底边仍锚定。
        let ((x, y), (w, h)) = fit_window_geometry((0, 0), (1040, 800), 2.0, 520.0, 10.0, GHOSTWRITER_MIN_FIT_HEIGHT_LOGICAL);
        assert_eq!(w, 1040);
        assert_eq!(h, 240);
        assert_eq!(y + h as i32, 800);
    }

    #[test]
    fn fit_geometry_falls_back_to_scale_one_on_invalid_scale() {
        let ((x, y), (w, h)) = fit_window_geometry((0, 0), (560, 420), 0.0, 520.0, 300.0, 120.0);
        assert_eq!((w, h), (520, 300));
        let ((x2, y2), _) = fit_window_geometry((0, 0), (560, 420), f64::NAN, 520.0, 300.0, 120.0);
        assert_eq!((x2, y2), (x, y));
    }
}

/// ghostwriter_cancel_last 的返回：撤销结果（action："hit"|"none"）＋
/// 撤销后的指令预览（拼装文本＋后端权威修订号，前端凭它丢弃撤销前在途的旧预览）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterCancelLastResult {
    pub cancelled: bool,
    pub action: String,
    pub assembled: Option<String>,
    pub revision: u64,
}

/// 浮框窗口动态贴合卡片（2026-09-18 裁决，替代轮询穿透）：把 ghostwriter
/// 窗口改成内容逻辑尺寸，同一次主线程 pass 里顺序 set_size + set_position
///（无跨 IPC 中间帧）；高度差从顶边收（底边锚定）、宽度水平居中。窗口贴合
/// 卡片后，窗口矩形之外的点击/悬停原生落到后面的软件，无需任何穿透逻辑。
/// 查询/设置失败只 log::warn，不打断会话。
#[cfg(not(mobile))]
#[tauri::command]
pub fn ghostwriter_fit_window(app: AppHandle, width_logical: f64, height_logical: f64) {
    use tauri::{Manager, PhysicalPosition, PhysicalSize};

    let Some(window) = app.get_webview_window("ghostwriter") else {
        return;
    };
    let position = match window.outer_position() {
        Ok(p) => (p.x, p.y),
        Err(error) => {
            log::warn!("[ghostwriter] fit window: outer_position failed: {error}");
            return;
        }
    };
    let size = match window.outer_size() {
        Ok(s) => (s.width, s.height),
        Err(error) => {
            log::warn!("[ghostwriter] fit window: outer_size failed: {error}");
            return;
        }
    };
    let scale_factor = match window.scale_factor() {
        Ok(scale) => scale,
        Err(error) => {
            log::warn!("[ghostwriter] fit window: scale_factor failed: {error}");
            1.0
        }
    };
    let ((new_x, new_y), (new_w, new_h)) = fit_window_geometry(
        position,
        size,
        scale_factor,
        width_logical,
        height_logical,
        GHOSTWRITER_MIN_FIT_HEIGHT_LOGICAL,
    );
    // 两条写入放进同一个主线程闭包：下一次绘制前一并生效。
    let window_for_main = window.clone();
    if let Err(error) = window.run_on_main_thread(move || {
        if let Err(error) = window_for_main.set_size(PhysicalSize::new(new_w, new_h)) {
            log::warn!("[ghostwriter] fit window: set_size failed: {error}");
        }
        if let Err(error) = window_for_main.set_position(PhysicalPosition::new(new_x, new_y)) {
            log::warn!("[ghostwriter] fit window: set_position failed: {error}");
        }
    }) {
        log::warn!("[ghostwriter] fit window: schedule on main thread failed: {error}");
    }
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



/// 按需批量提取候选常用语（管理页触发）：选中的历史会话 id 交给 Core 提取，
/// 返回可编辑草稿（不落库，保存由前端逐条 create）。
#[tauri::command]
pub async fn ghostwriter_extract_snippet_candidates(
    core: CoreState<'_>,
    session_ids: Vec<String>,
) -> Result<Vec<openless_core::ghostwriter::types::SnippetDraft>, String> {
    core.extract_ghostwriter_snippet_candidates(session_ids)
        .await
        .map_err(|e| e.to_string())
}

/// 按需提取候选热词（词典页向导触发）：选中的历史会话 **raw 原文**交给 Core
/// 提取，返回可编辑草稿（不落库，保存由前端逐条 add_vocab）。
#[tauri::command]
pub async fn ghostwriter_extract_hotword_candidates(
    core: CoreState<'_>,
    session_ids: Vec<String>,
) -> Result<Vec<openless_core::ghostwriter::types::HotwordDraft>, String> {
    core.extract_ghostwriter_hotword_candidates(session_ids)
        .await
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
    core.reset_ghostwriter_task_brief(&id).map_err(|e| e.to_string())
}

/// 对话热键入口（会话未开时按下）：与普通代笔会话同路径开局，回话时机与
/// 追问深度由后端按偏好冻结；返回新会话 id。
#[tauri::command]
pub async fn ghostwriter_start_conversation(core: CoreState<'_>) -> Result<String, String> {
    super::dictation::ensure_core_started(&core).await?;
    let session_id = core
        .start_ghostwriter_conversation()
        .await
        .map_err(|e| e.to_string())?;
    Ok(session_id.to_string())
}

/// 对话热键入口（会话开着且回话时机＝显式交话时按下）：显式交话一次，
/// 后端校验会话存在且为对话会话，绕过冷却。
#[tauri::command]
pub fn ghostwriter_trigger_reply(core: CoreState<'_>, session_id: String) -> Result<(), String> {
    let parsed = uuid::Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    core.trigger_ghostwriter_reply(openless_core::SessionId::from_uuid(parsed))
        .map_err(|e| e.to_string())
}
