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
/// 新产出的润色段与新生效的常用语命中。
/// 候选与推荐均纯展示（2026-09-17 裁决），口头命令/点选选中子系统已移除。
#[derive(Debug, Clone, Default)]
pub struct FeedOutcome {
    pub new_segments: Vec<PolishableSegment>,
    pub new_hits: Vec<GhostwriterSnippetHit>,
}

/// live 批次里的一条推荐常用语（assist 现场产出，未入库）。
#[derive(Debug, Clone, PartialEq)]
pub struct LiveRecommendation {
    pub snippet_id: String,
    pub title: String,
    pub text: String,
}

/// 撤销最近一次动作的结果：常用语命中。
#[derive(Debug, Clone, PartialEq)]
pub enum LastAction {
    Hit(GhostwriterSnippetHit),
}

/// assist 批次视图：浮框候选区的一组同类候选。
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateGroupView {
    /// 组别："term"|"naming"。
    pub kind: String,
    pub items: Vec<CandidateItemView>,
}

/// 候选批次里的一条候选数据：名字＋展示注释（term=白话注释，回指说话人的
/// 说法；naming=起名理由）。注释仅展示用，进待融队列的材料只取名字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateItem {
    pub name: String,
    pub note: Option<String>,
}

impl From<&str> for CandidateItem {
    fn from(name: &str) -> Self {
        Self { name: name.to_string(), note: None }
    }
}

impl From<String> for CandidateItem {
    fn from(name: String) -> Self {
        Self { name, note: None }
    }
}

/// 批次视图里的一条候选：1-based 全局序号＋名字＋注释（纯展示）。
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateItemView {
    pub index: usize,
    pub text: String,
    pub note: Option<String>,
}

/// 批次视图里的一条推荐常用语。
#[derive(Debug, Clone, PartialEq)]
pub struct RecommendationView {
    pub snippet_id: String,
    pub title: String,
}

/// 实时助手批次视图：浮框候选区渲染的唯一依据。
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

/// 常用语命中确认：命中哪条常用语、其标题与贴位（"inline"|"head"|"tail"）。
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

/// 实时助手批次变化事件：候选组＋推荐，浮框候选区整体替换渲染。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterAssistChanged {
    pub candidate_groups: Vec<GhostwriterCandidateGroup>,
    pub recommendations: Vec<GhostwriterRecommendationItem>,
}

/// 事件载荷里的一组同类候选。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterCandidateGroup {
    /// 组别："term"|"naming"。
    pub kind: String,
    pub items: Vec<GhostwriterCandidateItem>,
}

/// 事件载荷里的一条候选：批次内 1-based 全局序号＋名字＋白话注释（纯展示）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterCandidateItem {
    pub index: u32,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 事件载荷里的一条推荐常用语。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterRecommendationItem {
    pub snippet_id: String,
    pub title: String,
}

/// 按需批量提取产出的一条候选常用语草稿（编辑与勾选都在管理页，确认后才入库）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetDraft {
    /// 整理好的说法。
    pub phrase: String,
    /// 便于口头触发的短触发词（提取侧缺省回落 phrase）。
    pub suggested_trigger: String,
    /// 原话例句。
    pub example: Option<String>,
}

/// 聊天记录行角色：用户（【我】）或助手（【助手】）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

/// 聊天记录里的一条发言：角色＋原话（对话会话维护，行语法由代码固定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTurn {
    pub role: ChatRole,
    pub text: String,
}

impl ChatTurn {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: ChatRole::User, text: text.into() }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self { role: ChatRole::Assistant, text: text.into() }
    }
}

/// 自动回话门控判定（机制级，不靠模型自觉）：放行／冷却中（等用户新段）／
/// 封顶（自动回话已达全会话上限）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyGate {
    Allow,
    Cooldown,
    Cap,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghostwriter_event_payloads_serialize_camel_case() {
        let p = GhostwriterPreviewChanged { text: "你好".into(), revision: 3 };
        let v: serde_json::Value = serde_json::to_value(&p).unwrap();
        assert_eq!(v["revision"], 3);
        let h = GhostwriterSnippetHit { snippet_id: "s1".into(), title: "翻译".into(), mode: "tail".into() };
        let v: serde_json::Value = serde_json::to_value(&h).unwrap();
        assert_eq!(v["snippetId"], "s1");
        let a = GhostwriterAssistChanged {
            candidate_groups: vec![GhostwriterCandidateGroup {
                kind: "term".into(),
                items: vec![GhostwriterCandidateItem { index: 1, text: "精准词".into(), note: Some("注".into()) }],
            }],
            recommendations: vec![GhostwriterRecommendationItem {
                snippet_id: "s2".into(),
                title: "触发词".into(),
            }],
        };
        let v: serde_json::Value = serde_json::to_value(&a).unwrap();
        assert_eq!(v["candidateGroups"][0]["items"][0]["index"], 1);
        assert_eq!(v["recommendations"][0]["snippetId"], "s2");
    }
}
