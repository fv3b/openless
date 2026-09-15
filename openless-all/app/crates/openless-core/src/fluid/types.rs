//! Fluid 层跨模块共享的轻量值类型。

use serde::{Deserialize, Serialize};

/// 听写上下文捕获时冻结的 Fluid 开关快照：润色流开关 + 当前会话是否走 Fluid 浮框。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FluidSnapshot {
    pub polish_enabled: bool,
    pub active: bool,
}

/// 会话配置：润色流开关（false＝机械模式：不产润色段，命中与材料追加照常）。
#[derive(Debug, Clone, Copy)]
pub struct FluidConfig {
    pub polish_enabled: bool,
}

impl Default for FluidConfig {
    fn default() -> Self {
        Self { polish_enabled: true }
    }
}

/// 一段待润色的生转写：段序号＋已润前文尾部＋本段文本＋随段转移的材料。
#[derive(Debug, Clone)]
pub struct PolishableSegment {
    /// 段在会话中的序号（apply_polished 的对位索引）。
    pub index: usize,
    /// 已润前文尾部（≤200 字符，截断自 polished 缓冲）。
    pub prior: String,
    /// 本段生转写（或尾巴补润时的尾巴原文）。
    pub text: String,
    /// 本段挂着的 inline 常用语材料（随产出转移，待融队列清空）。
    pub materials: Vec<String>,
}

/// 一次 [`crate::fluid::session::FluidSession::feed`] 的结果：
/// 新产出的润色段与新生效的常用语命中。
#[derive(Debug, Clone, Default)]
pub struct FeedOutcome {
    pub new_segments: Vec<PolishableSegment>,
    pub new_hits: Vec<FluidSnippetHit>,
}

/// 指令预览的实时结果：此刻停下将贴给 AI 的完整文本与递增修订号。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FluidPreviewChanged {
    pub text: String,
    pub revision: u64,
}

/// 常用语命中确认：命中哪条常用语、其标题与贴位模式（"inline"|"footnote"）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FluidSnippetHit {
    pub snippet_id: String,
    pub title: String,
    pub mode: String,
}

/// 面向浮框的用户级提示：文案与级别（"info"|"error"）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FluidNotice {
    pub message: String,
    pub level: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fluid_event_payloads_serialize_camel_case() {
        let p = FluidPreviewChanged { text: "你好".into(), revision: 3 };
        let v: serde_json::Value = serde_json::to_value(&p).unwrap();
        assert_eq!(v["revision"], 3);
        let h = FluidSnippetHit { snippet_id: "s1".into(), title: "翻译".into(), mode: "footnote".into() };
        let v: serde_json::Value = serde_json::to_value(&h).unwrap();
        assert_eq!(v["snippetId"], "s1");
    }
}
