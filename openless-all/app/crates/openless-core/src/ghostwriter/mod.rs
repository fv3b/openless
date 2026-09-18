//! Ghostwriter 层：说话期间的流式会话（断句、增量润色、口头命令、注入、动作、建议）。
//!
//! 设计原则：以新增模块为主；接入既有管线所必需的少量触点（`api.rs` 的
//! progress 循环把 `TranscriptDelta` 喂给 [`crate::ghostwriter::session::GhostwriterSession`]、
//! `events.rs` 的事件登记、`dictation_context.rs` 的 ghostwriter 偏好携带）随 M1/M2
//! 一并落地，上游 merge 时以这些小 diff 为冲突面。

pub mod assist;
pub mod dispatcher;
pub mod prompts;
pub mod recent_voice;
pub mod segment_polisher;
pub mod segmenter;
pub mod snippet_extractor;
pub mod session;
pub mod snippet_store;
pub mod task_brief_store;
pub mod types;
