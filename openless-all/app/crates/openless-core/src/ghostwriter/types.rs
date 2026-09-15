//! Ghostwriter 层跨模块共享的轻量值类型。

use serde::{Deserialize, Serialize};

/// 听写上下文捕获时冻结的 Ghostwriter 快照：当前会话是否走 Ghostwriter 浮框。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterSnapshot {
    pub active: bool,
}

/// 一段待润色的生转写：段序号＋已润前文尾部＋本段文本＋随段转移的材料。
#[derive(Debug, Clone)]
pub struct PolishableSegment {
    /// 段在会话中的序号（apply_polished 的对位索引）。尾巴补润请求
    /// （tail_polish_input）复用本结构时取 segments.len() 作哨兵——
    /// 对位不存在，apply_polished 对它一律 false，须走 apply_tail_polished。
    pub index: usize,
    /// 已润前文尾部（≤200 字符，截断自 polished 缓冲）。
    pub prior: String,
    /// 本段生转写（或尾巴补润时的尾巴原文）。
    pub text: String,
    /// 本段挂着的 inline 常用语材料（随产出转移，待融队列清空）。
    pub materials: Vec<String>,
}

/// 一次 [`crate::ghostwriter::session::GhostwriterSession::feed`] 的结果：
/// 新产出的润色段、新生效的常用语命中与口头命令新生效的选中。
#[derive(Debug, Clone, Default)]
pub struct FeedOutcome {
    pub new_segments: Vec<PolishableSegment>,
    pub new_hits: Vec<GhostwriterSnippetHit>,
    /// 本次 feed 中口头命令（用候选N/用常用语N）新生效的选中，材料已进待融队列。
    pub new_selections: Vec<Selection>,
}

/// 口头/点选原的类别：现场候选或推荐常用语。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    /// 现场候选（LLM 说话中现场生成的表述选项）。
    Candidate,
    /// 推荐常用语（从已启用库里挑出的条目）。
    Recommendation,
}

/// live 批次里的一条推荐常用语（assist 现场产出，未入库）。
#[derive(Debug, Clone, PartialEq)]
pub struct LiveRecommendation {
    pub snippet_id: String,
    pub title: String,
    pub text: String,
}

/// 一次生效的选中原：批次内 1-based 全局序号＋进待融队列的材料文本。
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub kind: SelectionKind,
    /// 批次内 1-based 全局序号（候选跨组连续计数；推荐按列表序）。
    pub index: usize,
    /// 推荐常用语对应的常用语 id；现场候选无。
    pub snippet_id: Option<String>,
    /// 进待融队列的材料文本。
    pub text: String,
}

/// 撤销最近一次动作的结果：常用语命中或口头/点选选中。
#[derive(Debug, Clone, PartialEq)]
pub enum LastAction {
    Hit(GhostwriterSnippetHit),
    Selection(Selection),
}

/// assist 批次视图：浮框候选区的一组同类候选。
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateGroupView {
    /// 组别："term"|"phrase"|"naming"。
    pub kind: String,
    pub items: Vec<CandidateItemView>,
}

/// 批次视图里的一条候选：1-based 全局序号＋文本＋选中态。
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateItemView {
    pub index: usize,
    pub text: String,
    pub selected: bool,
}

/// 批次视图里的一条推荐常用语。
#[derive(Debug, Clone, PartialEq)]
pub struct RecommendationView {
    pub snippet_id: String,
    pub title: String,
    pub selected: bool,
}

/// 实时助手批次视图：浮框候选区渲染与口头选中范围判定的唯一依据。
#[derive(Debug, Clone, PartialEq)]
pub struct AssistSnapshot {
    pub candidate_groups: Vec<CandidateGroupView>,
    pub recommendations: Vec<RecommendationView>,
}

/// 指令预览的实时结果：此刻停下将贴给 AI 的完整文本与递增修订号。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterPreviewChanged {
    pub text: String,
    pub revision: u64,
}

/// 常用语命中确认：命中哪条常用语、其标题与贴位模式（"inline"|"footnote"）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterSnippetHit {
    pub snippet_id: String,
    pub title: String,
    pub mode: String,
}

/// 面向浮框的用户级提示：文案与级别（"info"|"error"）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterNotice {
    pub message: String,
    pub level: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghostwriter_event_payloads_serialize_camel_case() {
        let p = GhostwriterPreviewChanged { text: "你好".into(), revision: 3 };
        let v: serde_json::Value = serde_json::to_value(&p).unwrap();
        assert_eq!(v["revision"], 3);
        let h = GhostwriterSnippetHit { snippet_id: "s1".into(), title: "翻译".into(), mode: "footnote".into() };
        let v: serde_json::Value = serde_json::to_value(&h).unwrap();
        assert_eq!(v["snippetId"], "s1");
    }
}
