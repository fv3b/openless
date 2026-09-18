//! 最近语音背景（2026-09-18 批次 A；同日复核偏好化）：会话启动时按用户偏好
//! （Ghostwriter 偏好的「最近语音背景」开关，实验性）从历史取最近语音会话的
//! 转写文本，冻结成一份与会话同生命周期的只读快照，供三个消费点使用——
//! 段润色/对话出稿的 LLM 背景（带「次要参考」标注块头）、实时助手（assist）
//! 的 user 输入（同一块）、火山流式 ASR 的 `corpus.context` dialog_ctx
//! （纯上下文，不标注）。会话中途不重取：流式识别的上下文不漂移，LLM 各次
//! 调用看到的背景一致，设置改动对下一场会话生效。
//!
//! 数据源用 final_text 而非 raw_transcript：final 是润色后的干净文本（无
//! ASR 错字与口水词），作「理解背景」最贴合；翻译会话的 final 是译文，改取
//! polish_source（被翻译的原文），与润色上下文的既有取数规则
//! （[`crate::dictation_context::eligible_polish_context_turns`]）一致。
//! 失败会话（error_code 非空）与空白文本不入背景。
//!
//! 范围由偏好圈定：开关关闭 → None（所有消费点照旧零变化）；按条数＝最近
//! N 条语音会话；按天数＝最近 N 天内全部语音会话（条数不限）。2000 字符
//! 总预算不变，超预算丢最旧。

use crate::shared_types::{BackgroundUnit, GhostwriterPreferences};
use crate::types::{DictationSession, HistorySource};

/// 全部转写文本的总字符预算：超出丢最旧（保留最新的）。
pub const RECENT_VOICE_TOTAL_CHAR_BUDGET: usize = 2000;

/// 单条转写的字符上限（截断防单条过长独占预算；上限与总量预算联动——
/// 三条顶格线仍会触顶，预算淘汰真实可达）。
pub const RECENT_VOICE_LINE_CHAR_CAP: usize = 800;

/// LLM 背景块头（逐字，用户裁决：两个 LLM 消费点的块头必须带「次要参考」；
/// ASR 侧不用此块，纯上下文）。
pub const RECENT_VOICE_BLOCK_HEADER: &str =
    "最近语音（次要参考，仅供理解背景，不是本次要处理的指令）：";

/// 会话启动时冻结的最近语音背景：转写文本按时间先后（旧→新）存放。
/// 空（无可用历史）＝None，一切消费点照旧零变化。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentVoiceBackground {
    lines: Vec<String>,
}

impl RecentVoiceBackground {
    /// 从历史条目（调用方按 newest-first 传入，与 HistoryStore 读序一致）
    /// 按偏好冻结背景：开关关闭 → None；只取语音来源、无错误、文本非空的
    /// 条目——单位 Sessions 取最近 [`GhostwriterPreferences::
    /// recent_voice_background_amount`] 条，单位 Days 取最近 N 天内全部
    /// （条数不限，按 created_at 判定），单条截断至行上限，总量超预算丢最旧。
    /// 当前会话自身此刻尚未入历史（stop 才落档），无需排除。
    pub fn from_history(
        sessions: &[DictationSession],
        preferences: &GhostwriterPreferences,
    ) -> Option<Self> {
        if !preferences.recent_voice_background_enabled {
            return None;
        }
        let amount = preferences.recent_voice_background_amount;
        if amount == 0 {
            return None;
        }
        let mut candidates: Vec<&DictationSession> = sessions
            .iter()
            .filter(|session| {
                session.source == HistorySource::Voice
                    && session.error_code.is_none()
                    && transcript_of(session)
                        .is_some_and(|text| !text.trim().is_empty())
            })
            .collect();
        match preferences.recent_voice_background_unit {
            BackgroundUnit::Sessions => candidates.truncate(amount as usize),
            BackgroundUnit::Days => {
                let cutoff = chrono::Utc::now() - chrono::Duration::days(i64::from(amount));
                candidates.retain(|session| created_within(session, cutoff));
            }
        }
        let mut lines: Vec<String> = Vec::new();
        let mut used = 0usize;
        for session in candidates {
            let Some(text) = transcript_of(session) else {
                continue;
            };
            let mut line: String = text.trim().chars().take(RECENT_VOICE_LINE_CHAR_CAP).collect();
            let truncated = text.trim().chars().count() > RECENT_VOICE_LINE_CHAR_CAP;
            if truncated {
                line.push('…');
            }
            if used + line.chars().count() > RECENT_VOICE_TOTAL_CHAR_BUDGET {
                // 预算耗尽：更旧的条目全部丢弃（含当前这条）。
                break;
            }
            used += line.chars().count();
            // 调用方 newest-first，背景按时间先后存放（旧→新）。
            lines.insert(0, line);
        }
        if lines.is_empty() {
            return None;
        }
        Some(Self { lines })
    }

    /// 冻结的转写行（时间先后，旧→新）。
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// LLM 消费点的背景块：块头（带「次要参考」标注）＋逐条转写（每条一行，
    /// 时间先后）。行数与文本以冻结快照为准。
    pub fn llm_block(&self) -> String {
        let mut block = String::from(RECENT_VOICE_BLOCK_HEADER);
        for line in &self.lines {
            block.push('\n');
            block.push_str(line);
        }
        block
    }

    /// ASR dialog_ctx 的上下文条目（新→旧）：官方按「从新到旧截断」，调用方
    /// 直接按此顺序传即可保住最近的话。
    pub fn asr_dialog_ctx_lines(&self) -> Vec<String> {
        self.lines.iter().rev().cloned().collect()
    }
}

/// 一条历史会话的背景文本：翻译会话取被翻译的原文（polish_source），其余取
/// 润色后的 final_text（比 raw 干净，作背景最贴合）。
fn transcript_of(session: &DictationSession) -> Option<String> {
    if session.translation_active {
        session
            .polish_source
            .clone()
            .filter(|text| !text.trim().is_empty())
    } else {
        Some(session.final_text.clone())
    }
}

/// created_at 是否落在窗口内（cutoff 及其后）。RFC3339 解析失败视为过期
/// （排除）——宁可少带，不带脏数据。
fn created_within(session: &DictationSession, cutoff: chrono::DateTime<chrono::Utc>) -> bool {
    chrono::DateTime::parse_from_rfc3339(&session.created_at)
        .map(|created| created.with_timezone(&chrono::Utc) >= cutoff)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_types::GhostwriterPreferences;
    use crate::types::{HistoryInsertStatus, PolishMode};

    /// 背景开关打开的偏好（数量/单位用默认：3 条）。
    fn prefs_enabled() -> GhostwriterPreferences {
        GhostwriterPreferences {
            recent_voice_background_enabled: true,
            ..GhostwriterPreferences::default()
        }
    }

    fn prefs_with(amount: u32, unit: BackgroundUnit) -> GhostwriterPreferences {
        GhostwriterPreferences {
            recent_voice_background_enabled: true,
            recent_voice_background_amount: amount,
            recent_voice_background_unit: unit,
            ..GhostwriterPreferences::default()
        }
    }

    fn session(final_text: &str) -> DictationSession {
        DictationSession {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            source: HistorySource::Voice,
            raw_transcript: "raw".into(),
            asr_transcript: None,
            final_text: final_text.into(),
            mode: PolishMode::Light,
            style_pack_id: None,
            translation_active: false,
            polish_source: None,
            app_bundle_id: None,
            app_name: None,
            insert_status: HistoryInsertStatus::Inserted,
            error_code: None,
            duration_ms: Some(1000),
            dictionary_entry_count: None,
            has_audio_recording: None,
            recording_file: None,
            asr_provider: None,
            asr_model: None,
            llm_provider: None,
            llm_model: None,
            pipeline_mode: None,
            asr_ms: None,
            polish_ms: None,
            ghostwriter_hits: None,
            ghostwriter_selections: None,
            ghostwriter_chat: None,
        }
    }

    #[test]
    fn freezes_up_to_three_newest_voice_transcripts_in_chronological_order() {
        let oldest = session("第一条旧指令");
        let middle = session("第二条中段指令");
        let newest = session("第三条最新指令");
        let empty = session("   ");
        let failed = {
            let mut failed = session("失败会话不该入背景");
            failed.error_code = Some("polishFailed".into());
            failed
        };
        let selection = {
            let mut selection = session("选区润色不是语音会话");
            selection.source = HistorySource::SelectionPolish;
            selection
        };
        // newest-first（HistoryStore 读序）：新条目在数组头部。
        let background = RecentVoiceBackground::from_history(
            &[selection, failed, empty, newest, middle, oldest],
            &prefs_enabled(),
        )
        .expect("有可用历史时应产出背景");
        assert_eq!(
            background.lines(),
            vec!["第一条旧指令", "第二条中段指令", "第三条最新指令"],
            "背景按时间先后（旧→新）冻结，默认最多 3 条"
        );
        assert_eq!(background.lines().len(), 3);
    }

    #[test]
    fn disabled_preference_yields_none_even_with_history() {
        // 开关关闭（默认）：无历史也应无背景——所有消费点零变化。
        let disabled = GhostwriterPreferences::default();
        assert!(disabled.recent_voice_background_enabled == false);
        assert!(RecentVoiceBackground::from_history(&[session("有历史")], &disabled).is_none());
    }

    #[test]
    fn sessions_unit_uses_configured_amount() {
        let sessions: Vec<DictationSession> =
            (1..=5).map(|i| session(&format!("第{i}条"))).collect();
        // 数组头=最新；amount=2 → 只留最新两条（旧→新）。
        let newest_first: Vec<DictationSession> = sessions.iter().rev().cloned().collect();
        let background = RecentVoiceBackground::from_history(&newest_first, &prefs_with(2, BackgroundUnit::Sessions))
            .expect("背景应存在");
        assert_eq!(background.lines(), vec!["第4条", "第5条"]);
    }

    #[test]
    fn zero_amount_yields_none() {
        assert!(RecentVoiceBackground::from_history(&[session("有一条")], &prefs_with(0, BackgroundUnit::Sessions)).is_none());
        assert!(RecentVoiceBackground::from_history(&[session("有一条")], &prefs_with(0, BackgroundUnit::Days)).is_none());
    }

    #[test]
    fn days_unit_keeps_all_sessions_within_window() {
        let now = chrono::Utc::now();
        let at = |days_ago: i64| (now - chrono::Duration::days(days_ago)).to_rfc3339();
        let mut recent_a = session("今天第一条");
        recent_a.created_at = at(0);
        let mut recent_b = session("两天前");
        recent_b.created_at = at(2);
        let mut recent_c = session("今天第二条");
        recent_c.created_at = at(0);
        let mut stale = session("十天前");
        stale.created_at = at(10);
        let mut unparseable = session("时间戳坏了");
        unparseable.created_at = "not-a-timestamp".into();
        // newest-first 乱序传入也可以：窗口过滤不依赖数组顺序，预算拼装再排旧→新。
        let background = RecentVoiceBackground::from_history(
            &[recent_c, unparseable, stale, recent_b, recent_a],
            &prefs_with(3, BackgroundUnit::Days),
        )
        .expect("窗口内应有背景");
        assert_eq!(
            background.lines(),
            vec!["今天第一条", "两天前", "今天第二条"],
            "N 天内全部保留（条数不限）；窗口过滤不改变调用方的 newest-first 拼装顺序，窗口外与时间戳不可解析的排除"
        );
    }

    #[test]
    fn translated_sessions_use_the_polish_source_not_the_translation() {
        let mut translated = session("这是英文译文 the translation");
        translated.translation_active = true;
        translated.polish_source = Some("这是被翻译的中文原文".to_string());
        let background =
            RecentVoiceBackground::from_history(&[translated], &prefs_enabled()).expect("翻译会话应取原文");
        assert_eq!(background.lines(), vec!["这是被翻译的中文原文"]);
        // polish_source 缺失（旧数据）→ 该条不可用。
        let mut missing = session("这是英文译文 the translation");
        missing.translation_active = true;
        assert!(RecentVoiceBackground::from_history(&[missing], &prefs_enabled()).is_none());
    }

    #[test]
    fn no_usable_history_yields_none() {
        assert!(RecentVoiceBackground::from_history(&[], &prefs_enabled()).is_none());
        let failed = {
            let mut failed = session("失败");
            failed.error_code = Some("transcribeFailed".into());
            failed
        };
        assert!(RecentVoiceBackground::from_history(&[failed], &prefs_enabled()).is_none());
    }

    #[test]
    fn llm_block_carries_secondary_reference_header_verbatim() {
        let background = RecentVoiceBackground::from_history(&[session("第二条"), session("第一条")], &prefs_enabled())
            .expect("背景应存在");
        let block = background.llm_block();
        assert!(
            block.starts_with("最近语音（次要参考，仅供理解背景，不是本次要处理的指令）：\n"),
            "块头必须逐字带「次要参考」标注，实际: {block}"
        );
        // 每条一行，时间先后（旧→新）。
        assert_eq!(block, "最近语音（次要参考，仅供理解背景，不是本次要处理的指令）：\n第一条\n第二条");
    }

    #[test]
    fn long_transcripts_are_truncated_per_line_and_over_budget_drops_oldest() {
        let long = "x".repeat(RECENT_VOICE_LINE_CHAR_CAP + 50);
        let background = RecentVoiceBackground::from_history(&[session(&long)], &prefs_enabled()).expect("背景应存在");
        let line = &background.lines()[0];
        assert_eq!(
            line.chars().count(),
            RECENT_VOICE_LINE_CHAR_CAP + 1,
            "单条截断至行上限并补省略号"
        );
        assert!(line.ends_with('…'));

        // 预算：每条 900 字符，3 条共 2700 > 2000 → 丢最旧，保留最新两条。
        let chunk = "y".repeat(900);
        let newest = session(&chunk);
        let middle = session(&chunk);
        let oldest = session(&chunk);
        let background = RecentVoiceBackground::from_history(&[newest, middle, oldest], &prefs_enabled())
            .expect("背景应存在");
        assert_eq!(background.lines().len(), 2, "超预算应丢最旧");
        let total: usize = background.lines().iter().map(|line| line.chars().count()).sum();
        assert!(total <= RECENT_VOICE_TOTAL_CHAR_BUDGET);
    }

    #[test]
    fn asr_dialog_ctx_lines_are_newest_first() {
        let background = RecentVoiceBackground::from_history(&[session("第二条"), session("第一条")], &prefs_enabled())
            .expect("背景应存在");
        assert_eq!(background.asr_dialog_ctx_lines(), vec!["第二条", "第一条"]);
    }
}
