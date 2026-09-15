//! Fluid 层跨模块共享的轻量值类型。

use serde::{Deserialize, Serialize};

/// 听写上下文捕获时冻结的 Fluid 开关快照：润色流开关 + 当前会话是否走 Fluid 浮框。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FluidSnapshot {
    pub polish_enabled: bool,
    pub active: bool,
}
