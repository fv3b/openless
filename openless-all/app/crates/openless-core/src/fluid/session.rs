//! FluidSession：一次听写会话的流式缓冲与最终拼装。
//!
//! 自适应缓冲：不依赖 [`TranscriptAccumulator`]（绝对替换位语义，
//! 会把火山/讯飞的后缀增量 partial 拼碎），自持完整缓冲文本，
//! 按 delta 形态判型：
//! - final：全量替换（offset 忽略）；
//! - partial offset:0 且缓冲是 text 前缀：全量快照型 → 整体替换；
//! - partial offset:0 其余：后缀增量型（火山/讯飞实测形态）→ 追加；
//! - partial offset>0：替换式修订 → 保留前缀后写入。
//! 说话中缓冲始终是完整累计文本，segmenter 断句可用。
//! M2 起：段润色、口头命令、动作、注入都挂在段与缓冲之上，
//! `assembled_text` 届时基于「命令已应用」的缓冲拼装。

use crate::errors::BackendError;
use crate::types::TranscriptDelta;

use super::segmenter::{Segment, Segmenter};

/// 无标点尾巴的硬切阈值（字符数）：ASR 长时间不出标点时按长度断段。
const DEFAULT_MAX_FORCE_CHARS: usize = 120;

/// 会话配置（字段 Task 2 起补充）。
#[derive(Default, Clone, Debug)]
pub struct FluidConfig {}

/// 一次 [`FluidSession::feed`] 的结果（Task 6 填充）。
#[derive(Debug, Clone, Default)]
pub struct FeedOutcome {}

pub struct FluidSession {
    /// 说话中的完整累计文本（自适应缓冲）。
    buffer: String,
    /// 最近一次 partial 应用后缓冲的字符长度。
    #[allow(dead_code)]
    last_partial_len: usize,
    segmenter: Segmenter,
    segments: Vec<Segment>,
}

impl Default for FluidSession {
    fn default() -> Self {
        Self::new(FluidConfig::default())
    }
}

impl FluidSession {
    pub fn new(_config: FluidConfig) -> Self {
        Self {
            buffer: String::new(),
            last_partial_len: 0,
            segmenter: Segmenter::new(DEFAULT_MAX_FORCE_CHARS),
            segments: Vec::new(),
        }
    }

    /// 喂入一条转写增量。缓冲按 delta 形态自适应（见模块注释）；
    /// 每次缓冲变化后驱动断句器收集新完成的段。
    pub fn feed(&mut self, delta: &TranscriptDelta) -> Result<FeedOutcome, BackendError> {
        self.apply_delta(delta)?;
        let text = self.buffer.clone();
        let completed = self.segmenter.update(&text);
        if !completed.is_empty() {
            log::debug!(
                "[fluid] segmenter: {} segment(s) completed (buffer={} chars)",
                completed.len(),
                text.chars().count()
            );
        }
        self.segments.extend(completed);
        Ok(FeedOutcome {})
    }

    /// 按 delta 形态应用一条转写增量（判型规则见模块注释）。
    fn apply_delta(&mut self, delta: &TranscriptDelta) -> Result<(), BackendError> {
        if delta.is_final {
            self.buffer = delta.text.clone(); // final 全量替换，offset 忽略
            return Ok(());
        }
        let cur_chars = self.buffer.chars().count();
        let offset = usize::try_from(delta.offset).map_err(|_| {
            crate::errors::BackendError::new(
                crate::errors::BackendErrorCode::InvalidArgument,
                "transcript offset exceeds this platform's address space",
            )
        })?;
        if offset > cur_chars {
            return Err(crate::errors::BackendError::new(
                crate::errors::BackendErrorCode::InvalidArgument,
                "transcript delta starts after the current text",
            ));
        }
        if offset == 0
            && cur_chars <= delta.text.chars().count()
            && self.buffer.chars().zip(delta.text.chars()).all(|(a, b)| a == b)
        {
            // 全量快照型：text 以当前缓冲为前缀 → 整体替换
            self.buffer = delta.text.clone();
        } else if offset == 0 {
            // 后缀增量型 → 追加
            self.buffer.push_str(&delta.text);
        } else {
            // 替换式修订（offset>0）→ 照旧语义
            let kept: String = self.buffer.chars().take(offset).collect();
            self.buffer = format!("{kept}{}", delta.text);
        }
        self.last_partial_len = self.buffer.chars().count();
        Ok(())
    }

    /// 说话中的完整缓冲（原样，未 trim）：测试与 M1′ 验证探针。
    pub fn debug_text(&self) -> &str {
        &self.buffer
    }

    /// [`Self::debug_text`] 的别名：实时缓冲探针。
    pub fn debug_buffer(&self) -> &str {
        &self.buffer
    }

    /// 本次会话最终要插入的文本。M1：完整缓冲去首尾空白；
    /// M2 起改为基于「命令已应用」的缓冲拼装主文本＋附注＋动作块。
    pub fn assembled_text(&self) -> String {
        self.buffer.trim().to_string()
    }

    pub fn is_empty(&self) -> bool {
        self.assembled_text().is_empty()
    }

    /// 已完成的段（M2 段润色的输入；M1 仅供观测）。
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(text: &str, offset: u64, is_final: bool) -> TranscriptDelta {
        TranscriptDelta {
            text: text.to_string(),
            offset,
            is_final,
        }
    }

    #[test]
    fn suffix_partial_deltas_accumulate_and_final_replaces() {
        // 后缀增量型 provider（火山/讯飞实测形态）：partial 全部 offset:0
        let mut s = FluidSession::new(FluidConfig::default());
        s.feed(&delta("你好", 0, false)).unwrap();
        s.feed(&delta("，", 0, false)).unwrap();
        assert_eq!(s.debug_buffer(), "你好，");
        assert_eq!(s.last_partial_len, 3);
        // final 全量替换
        s.feed(&delta("你好，这是测试。", 0, true)).unwrap();
        assert_eq!(s.debug_text(), "你好，这是测试。");
        assert_eq!(s.last_partial_len, 3);
    }

    #[test]
    fn full_snapshot_partial_deltas_replace_not_append() {
        // 全量快照型 partial：text 是累计全文
        let mut s = FluidSession::new(FluidConfig::default());
        s.feed(&delta("你好", 0, false)).unwrap();
        s.feed(&delta("你好，", 0, false)).unwrap();
        assert_eq!(s.debug_text(), "你好，");
    }

    #[test]
    fn offset_revision_still_replaces() {
        // 替换式修订（offset>0）语义保留
        let mut s = FluidSession::new(FluidConfig::default());
        s.feed(&delta("ABC", 0, false)).unwrap();
        s.feed(&delta("X", 1, false)).unwrap();
        assert_eq!(s.debug_text(), "AX");
        assert_eq!(s.last_partial_len, 2);
    }

    #[test]
    fn rejects_offset_beyond_the_buffer() {
        let mut session = FluidSession::new(FluidConfig::default());
        let error = session.feed(&delta("越界", 5, false)).unwrap_err();
        assert_eq!(error.code, crate::errors::BackendErrorCode::InvalidArgument);
    }

    #[test]
    fn collects_completed_segments_from_the_buffer() {
        let mut session = FluidSession::new(FluidConfig::default());
        session.feed(&delta("第一句。第二句还没说完", 0, false)).unwrap();
        assert_eq!(session.segments().len(), 1);
        assert_eq!(session.segments()[0].text, "第一句。");
        // 句末标点停在缓冲末尾时不触发断句（ASR 可能继续追加）；
        // 尾巴留给会话结束时的补润（M2 用 Segmenter::tail 读取）。
        session.feed(&delta("第一句。第二句还没说完。", 0, true)).unwrap();
        assert_eq!(session.segments().len(), 1);
        assert_eq!(session.assembled_text(), "第一句。第二句还没说完。");
    }

    #[test]
    fn empty_session_assembles_to_empty() {
        let session = FluidSession::new(FluidConfig::default());
        assert!(session.is_empty());
        assert_eq!(session.assembled_text(), "");
    }

    #[test]
    fn assembled_text_trims_surrounding_whitespace() {
        let mut session = FluidSession::new(FluidConfig::default());
        session.feed(&delta("  两边空白  ", 0, true)).unwrap();
        assert_eq!(session.assembled_text(), "两边空白");
    }
}
