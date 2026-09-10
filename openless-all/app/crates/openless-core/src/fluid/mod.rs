//! Fluid 层：说话期间的流式会话（断句、增量润色、口头命令、注入、动作、建议）。
//!
//! 设计原则：全部为新增模块，不改动既有管线文件；上游 merge 时零冲突。
//! 接入点（M2 起）：`api.rs` 的 progress 循环把 `TranscriptDelta` 喂给
//! [`crate::fluid::session::FluidSession`]，见实施计划。

pub mod segmenter;
pub mod session;
