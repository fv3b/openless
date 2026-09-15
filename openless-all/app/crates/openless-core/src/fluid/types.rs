//! Fluid 层跨模块共享的轻量值类型。

use serde::{Deserialize, Serialize};

/// 听写上下文捕获时冻结的 Fluid 开关快照：润色流开关 + 当前会话是否走 Fluid 浮框。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FluidSnapshot {
    pub polish_enabled: bool,
    pub active: bool,
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
