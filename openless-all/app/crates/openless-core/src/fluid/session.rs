//! FluidSession：一次听写会话的流式缓冲与最终拼装。
//!
//! M1 骨架：缓冲 `TranscriptDelta`（offset 语义统一走
//! [`TranscriptAccumulator`]）并驱动 [`Segmenter`] 收集完整段；
//! `assembled_text` 返回最终将插入的文本。
//! M2 起：段润色、口头命令、动作、注入都挂在段与缓冲之上，
//! `assembled_text` 届时基于「命令已应用」的缓冲拼装。

use crate::errors::BackendError;
use crate::types::{TranscriptAccumulator, TranscriptDelta};

use super::segmenter::{Segment, Segmenter};

/// 无标点尾巴的硬切阈值（字符数）：ASR 长时间不出标点时按长度断段。
const DEFAULT_MAX_FORCE_CHARS: usize = 120;

pub struct FluidSession {
    transcript: TranscriptAccumulator,
    segmenter: Segmenter,
    segments: Vec<Segment>,
}

impl Default for FluidSession {
    fn default() -> Self {
        Self::new()
    }
}

impl FluidSession {
    pub fn new() -> Self {
        Self {
            transcript: TranscriptAccumulator::default(),
            segmenter: Segmenter::new(DEFAULT_MAX_FORCE_CHARS),
            segments: Vec::new(),
        }
    }

    /// 喂入一条转写增量。修订（offset 回退）由 accumulator 统一处理；
    /// 每次缓冲变化后驱动断句器收集新完成的段。
    pub fn feed(&mut self, delta: &TranscriptDelta) -> Result<(), BackendError> {
        self.transcript.apply(delta)?;
        let text = self.transcript.text().to_string();
        let completed = self.segmenter.update(&text);
        if !completed.is_empty() {
            log::debug!(
                "[fluid] segmenter: {} segment(s) completed (buffer={} chars)",
                completed.len(),
                text.chars().count()
            );
        }
        self.segments.extend(completed);
        Ok(())
    }

    /// 本次会话最终要插入的文本。M1：完整缓冲去首尾空白；
    /// M2 起改为基于「命令已应用」的缓冲拼装主文本＋附注＋动作块。
    pub fn assembled_text(&self) -> String {
        self.transcript.text().trim().to_string()
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
    fn assembled_text_joins_partial_and_final_deltas() {
        let mut session = FluidSession::new();
        session.feed(&delta("你好，", 0, false)).unwrap();
        session.feed(&delta("这是测试。", 3, true)).unwrap();
        assert_eq!(session.assembled_text(), "你好，这是测试。");
        assert!(!session.is_empty());
    }

    #[test]
    fn offset_revision_replaces_from_the_offset() {
        let mut session = FluidSession::new();
        session.feed(&delta("ABC", 0, false)).unwrap();
        session.feed(&delta("X", 1, false)).unwrap();
        assert_eq!(session.assembled_text(), "AX");
    }

    #[test]
    fn rejects_offset_beyond_the_buffer() {
        let mut session = FluidSession::new();
        let error = session.feed(&delta("越界", 5, false)).unwrap_err();
        assert_eq!(error.code, crate::errors::BackendErrorCode::InvalidArgument);
    }

    #[test]
    fn collects_completed_segments_from_the_buffer() {
        let mut session = FluidSession::new();
        session.feed(&delta("第一句。第二句还没说完", 0, false)).unwrap();
        assert_eq!(session.segments().len(), 1);
        assert_eq!(session.segments()[0].text, "第一句。");
        // 句末标点停在缓冲末尾时不触发断句（ASR 可能继续追加）；
        // 尾巴留给会话结束时的补润（M2 用 Segmenter::tail 读取）。
        session.feed(&delta("。", 11, true)).unwrap();
        assert_eq!(session.segments().len(), 1);
        assert_eq!(session.assembled_text(), "第一句。第二句还没说完。");
    }

    #[test]
    fn empty_session_assembles_to_empty() {
        let session = FluidSession::new();
        assert!(session.is_empty());
        assert_eq!(session.assembled_text(), "");
    }

    #[test]
    fn assembled_text_trims_surrounding_whitespace() {
        let mut session = FluidSession::new();
        session.feed(&delta("  两边空白  ", 0, true)).unwrap();
        assert_eq!(session.assembled_text(), "两边空白");
    }
}
