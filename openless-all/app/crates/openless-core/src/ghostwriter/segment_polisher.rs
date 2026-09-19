//! 段润色：把一段已完成的生转写用临时 context 调 LLM 整理成指令。
//!
//! 每个完成的段独立一次润色调用：provider 照 selection-voice 的既有
//! 解析路径（[`crate::provider_resolution::resolve_session_provider`]）
//! 解析 LLM 通道，context 用 [`DictationContext::capture`] 现场捕获后
//! 逐项覆写（mode=Light、指令化任务书正文（随请求携带，dispatcher 从
//! 任务书存储取）、清空热词/前文轮次/光标上下文、关翻译）。user 输入＝
//! 已润前文（若有）＋本段生转写＋本段常用语材料，材料逐条前缀
//! 「参考材料：」交给 LLM 融合进指令。

use std::sync::Arc;

use crate::credentials::{CredentialStore, ProviderSlot};
use crate::dictation_context::{
    DictationContext, DictationProviderInvocations, DictationStartOptions, ProviderInvocation,
};
use crate::errors::{BackendError, BackendErrorCode};
use crate::ports::{TextPolisher, TextStreamChunk, TextStreamSink};
use crate::types::{PolishMode, SessionId};

/// 段润色的指令化 system prompt：正文迁往 [`crate::ghostwriter::prompts`]（任务书注册表），
/// 这里保留 re-export 兼容既有调用点。
pub use crate::ghostwriter::prompts::GHOSTWRITER_INSTRUCTION_PROMPT;

/// 对话终稿出稿调用的固定会话 id（uuid5 确定性）：fixture 按「精确等于此 id」
/// 路由 canned 出稿文本，照 [`crate::ghostwriter::assist::assist_session_id`] 的
/// 裁决机制；dispatcher 调 [`polish_segment`] 出稿时也传它（生产与测试共享）。
pub fn conversation_finalize_session_id() -> SessionId {
    SessionId::from_uuid(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        b"openless.ghostwriter.conversation-finalize",
    ))
}

/// 一次段润色的请求（由 dispatcher 从缓冲与段状态组装）。
#[derive(Debug, Clone)]
pub struct SegmentPolishRequest {
    /// 段会话 id：dispatcher 以 `SessionId::new()` 现生成，与听写会话 id 无关联。
    pub session_id: SessionId,
    /// 该段在会话中的序号。
    pub segment_index: usize,
    /// 已润前文尾部（≤200 字符，截断自 polished 缓冲）；空表示无前文。
    pub prior: String,
    /// 本段生转写。
    pub segment: String,
    /// 本段挂着的 inline 常用语材料。
    pub materials: Vec<String>,
    /// 指令化任务书正文（dispatcher 从任务书存储取，保存即生效）。
    pub instruction: String,
    /// 最近语音背景块（2026-09-18 批次 A；带「次要参考」块头，dispatcher 从
    /// 会话冻结快照现算）；None＝无历史，输入零变化。只是背景注入，指令主体
    /// 不变——对话出稿任务书「只整理【我】的内容」的语义不受影响。
    pub recent_voice_block: Option<String>,
}

struct DiscardTextStream;

impl TextStreamSink for DiscardTextStream {
    fn publish(&self, _chunk: TextStreamChunk) -> Result<(), BackendError> {
        Ok(())
    }
}

/// 对一段已完成的生转写做指令化润色，返回整理后的指令文本。
pub async fn polish_segment(
    polisher: &Arc<dyn TextPolisher>,
    credential_store: &Arc<dyn CredentialStore>,
    active_llm_provider: &str,
    request: &SegmentPolishRequest,
) -> Result<String, BackendError> {
    let llm = crate::provider_resolution::resolve_session_provider(
        credential_store,
        ProviderSlot::Llm,
        active_llm_provider,
    )
    .await?;
    let preferences = crate::shared_types::UserPreferences::default();
    let style_pack = crate::style_packs::builtin_style_pack_for_mode(PolishMode::Light);
    let mut context = DictationContext::capture(
        &preferences,
        &style_pack,
        DictationProviderInvocations::new(
            ProviderInvocation::for_provider("ghostwriter-segment-unused-asr"),
            llm,
            ProviderInvocation::for_provider("ghostwriter-segment-unused-omni"),
        ),
        Vec::new(),
        Vec::new(),
        &DictationStartOptions::default(),
    );
    context.asr.prompt = None;
    context.polish.mode = PolishMode::Light;
    context.polish.style_system_prompt = request.instruction.clone();
    context.polish.hotwords.clear();
    context.polish.translation_active = false;
    context.polish.cursor_context = None;
    context.polish.prior_turns.clear();
    let output = polisher
        .polish(
            request.session_id,
            Arc::new(context),
            compose_user_input(request),
            Arc::new(DiscardTextStream),
        )
        .await?;
    let text = output.text.trim().to_string();
    if text.is_empty() {
        return Err(BackendError::new(
            BackendErrorCode::Provider,
            "ghostwriter segment polisher returned empty text",
        ));
    }
    Ok(text)
}

/// user 输入拼装：前文（若有，前缀「（前文：…）」）＋本段＋材料（每条前缀
/// 「参考材料：」）＋最近语音背景块（若有，垫底——只是理解背景，不是指令）。
fn compose_user_input(request: &SegmentPolishRequest) -> String {
    let mut parts = Vec::new();
    if !request.prior.trim().is_empty() {
        parts.push(format!("（前文：{}）", request.prior.trim()));
    }
    parts.push(request.segment.clone());
    for material in &request.materials {
        parts.push(format!("参考材料：{}", material));
    }
    if let Some(block) = request
        .recent_voice_block
        .as_deref()
        .filter(|block| !block.trim().is_empty())
    {
        parts.push(block.to_string());
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::InMemoryCredentialStore;
    use crate::ports::PolishOutput;
    use futures_util::future::BoxFuture;
    use std::sync::Mutex;

    #[derive(Clone)]
    struct PolishCall {
        session_id: SessionId,
        context: Arc<DictationContext>,
        raw_text: String,
    }

    #[derive(Clone)]
    struct CapturingPolisher {
        result: Result<PolishOutput, BackendError>,
        calls: Arc<Mutex<Vec<PolishCall>>>,
    }

    impl CapturingPolisher {
        fn successful(text: impl Into<String>) -> Self {
            Self {
                result: Ok(PolishOutput::text(text)),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<PolishCall> {
            self.calls.lock().expect("polish call lock poisoned").clone()
        }
    }

    impl TextPolisher for CapturingPolisher {
        fn polish(
            &self,
            session_id: SessionId,
            context: Arc<DictationContext>,
            raw_text: String,
            _partials: Arc<dyn TextStreamSink>,
        ) -> BoxFuture<'static, Result<PolishOutput, BackendError>> {
            let result = self.result.clone();
            self.calls
                .lock()
                .expect("polish call lock poisoned")
                .push(PolishCall {
                    session_id,
                    context,
                    raw_text,
                });
            Box::pin(async move { result })
        }

        fn cancel(&self, _session_id: SessionId) -> BoxFuture<'static, Result<(), BackendError>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn request() -> SegmentPolishRequest {
        SegmentPolishRequest {
            session_id: SessionId::new(),
            segment_index: 3,
            prior: "已经把环境配好了。".to_string(),
            segment: "把那个临时文件删了嗯就是那种缓存".to_string(),
            materials: vec![
                "/tmp/cache 目录".to_string(),
                "ls -la 输出贴进去".to_string(),
            ],
            instruction: "任务书覆写后的指令化正文".to_string(),
            recent_voice_block: None,
        }
    }

    fn store() -> Arc<dyn CredentialStore> {
        Arc::new(InMemoryCredentialStore::default())
    }

    #[tokio::test]
    async fn polish_segment_sends_instruction_prompt_and_materials() {
        let fixture = CapturingPolisher::successful("  删除临时缓存文件。  ");
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        let request = request();

        let text = polish_segment(&polisher, &store(), "test-llm", &request)
            .await
            .expect("segment polish should succeed");

        assert_eq!(text, "删除临时缓存文件。");
        let calls = fixture.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].session_id, request.session_id);
        let context = &calls[0].context;
        assert_eq!(context.llm.provider_id, "test-llm");
        // system prompt＝随请求携带的任务书正文（dispatcher 侧覆写即生效）。
        assert_eq!(context.polish.style_system_prompt, request.instruction);
        assert_eq!(context.polish.mode, PolishMode::Light);
        assert!(!context.polish.translation_active);
        assert!(context.polish.hotwords.is_empty());
        assert!(context.polish.cursor_context.is_none());
        assert!(context.polish.prior_turns.is_empty());
        assert_eq!(
            calls[0].raw_text.split('\n').collect::<Vec<_>>(),
            vec![
                "（前文：已经把环境配好了。）",
                "把那个临时文件删了嗯就是那种缓存",
                "参考材料：/tmp/cache 目录",
                "参考材料：ls -la 输出贴进去",
            ]
        );
    }

    #[tokio::test]
    async fn polish_segment_empty_output_is_provider_error() {
        let fixture = CapturingPolisher::successful("   \n");
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture);
        let request = request();

        let error = polish_segment(&polisher, &store(), "test-llm", &request)
            .await
            .expect_err("blank output must be a provider error");

        assert_eq!(error.code, BackendErrorCode::Provider);
    }

    #[tokio::test]
    async fn recent_voice_block_is_appended_with_secondary_reference_header() {
        let fixture = CapturingPolisher::successful("好的");
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        let mut polish_request = request();
        polish_request.recent_voice_block = Some(
            crate::ghostwriter::recent_voice::RecentVoiceBackground::from_history(
                &[
                    session_with_final("第二条"),
                    session_with_final("第一条"),
                ],
                &crate::shared_types::GhostwriterPreferences {
                    recent_voice_background_enabled: true,
                    ..crate::shared_types::GhostwriterPreferences::default()
                },
            )
            .expect("背景应存在")
            .llm_block(),
        );

        polish_segment(&polisher, &store(), "test-llm", &polish_request)
            .await
            .expect("segment polish should succeed");

        let raw = fixture.calls()[0].raw_text.clone();
        let lines: Vec<&str> = raw.split('\n').collect();
        assert_eq!(
            lines.last(),
            Some(&"第二条"),
            "背景块垫底（指令主体在前），逐条一行、时间先后（最新在末）"
        );
        assert!(
            raw.contains("最近语音（次要参考，仅供理解背景，不是本次要处理的指令）：\n第一条\n第二条"),
            "块头必须逐字带「次要参考」标注，实际: {raw}"
        );

        // 无历史（None）：输入与旧路径逐字一致，零变化。
        let baseline = CapturingPolisher::successful("好的");
        let request = request();
        polish_segment(
            &(Arc::new(baseline.clone()) as Arc<dyn TextPolisher>),
            &store(),
            "test-llm",
            &request,
        )
        .await
        .unwrap();
        assert!(
            !baseline.calls()[0]
                .raw_text
                .contains("最近语音"),
            "无历史时不应出现背景块"
        );
    }

    fn session_with_final(final_text: &str) -> crate::types::DictationSession {
        crate::types::DictationSession {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            source: crate::types::HistorySource::Voice,
            raw_transcript: String::new(),
            asr_transcript: None,
            final_text: final_text.to_string(),
            mode: PolishMode::Light,
            style_pack_id: None,
            translation_active: false,
            polish_source: None,
            app_bundle_id: None,
            app_name: None,
            insert_status: crate::types::HistoryInsertStatus::Inserted,
            error_code: None,
            duration_ms: None,
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
            extracted_at: None,
        }
    }
}
