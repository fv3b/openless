//! 段润色：把一段已完成的生转写用临时 context 调 LLM 整理成指令。
//!
//! 每个完成的段独立一次润色调用：provider 照 selection-voice 的既有
//! 解析路径（[`crate::provider_resolution::resolve_session_provider`]）
//! 解析 LLM 通道，context 用 [`DictationContext::capture`] 现场捕获后
//! 逐项覆写（mode=Light、指令化 prompt、清空热词/前文轮次/光标上下文、
//! 关翻译）。user 输入＝已润前文（若有）＋本段生转写＋本段常用语材料，
//! 材料逐条前缀「参考材料：」交给 LLM 融合进指令。

use std::sync::Arc;

use crate::credentials::{CredentialStore, ProviderSlot};
use crate::dictation_context::{
    DictationContext, DictationProviderInvocations, DictationStartOptions, ProviderInvocation,
};
use crate::errors::{BackendError, BackendErrorCode};
use crate::ports::{TextPolisher, TextStreamChunk, TextStreamSink};
use crate::types::{PolishMode, SessionId};

/// 段润色的指令化 system prompt（逐字使用，勿改动）。
pub const FLUID_INSTRUCTION_PROMPT: &str = "你是语音指令整理器。用户在用语音给 AI 助手下指令，下面是一段口语转写。\n把它整理成清晰、直接、结构清楚的指令：\n- 去掉口头语、重复、语气词（嗯、啊、就是那种、类似什么的）\n- 理顺语句顺序，需要时整理成简短要点\n- 把口语化的说法换成准确表述，但绝不改变用户的意思，绝不添加用户没说的要求\n- 原话里的具体信息（名字、数字、路径、代码、命令）一字不改\n- 如果给了「参考材料」，把材料内容自然融合进指令对应的位置\n只输出整理后的指令文本，不要任何解释或前缀。";

/// 一次段润色的请求（由 dispatcher 从缓冲与段状态组装）。
#[derive(Debug, Clone)]
pub struct SegmentPolishRequest {
    /// 段会话 id：独立前缀 `fluid-segment-{index}`，dispatcher 负责补唯一后缀。
    pub session_id: SessionId,
    /// 该段在会话中的序号。
    pub segment_index: usize,
    /// 已润前文尾部（≤200 字符，截断自 polished 缓冲）；空表示无前文。
    pub prior: String,
    /// 本段生转写。
    pub segment: String,
    /// 本段挂着的 inline 常用语材料。
    pub materials: Vec<String>,
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
            ProviderInvocation::for_provider("fluid-segment-unused-asr"),
            llm,
            ProviderInvocation::for_provider("fluid-segment-unused-omni"),
        ),
        Vec::new(),
        Vec::new(),
        &DictationStartOptions::default(),
    );
    context.asr.prompt = None;
    context.polish.mode = PolishMode::Light;
    context.polish.style_system_prompt = FLUID_INSTRUCTION_PROMPT.to_string();
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
            "fluid segment polisher returned empty text",
        ));
    }
    Ok(text)
}

/// user 输入拼装：前文（若有，前缀「（前文：…）」）＋本段＋材料（每条前缀「参考材料：」）。
fn compose_user_input(request: &SegmentPolishRequest) -> String {
    let mut parts = Vec::new();
    if !request.prior.trim().is_empty() {
        parts.push(format!("（前文：{}）", request.prior.trim()));
    }
    parts.push(request.segment.clone());
    for material in &request.materials {
        parts.push(format!("参考材料：{}", material));
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
        assert_eq!(context.polish.style_system_prompt, FLUID_INSTRUCTION_PROMPT);
        assert!(context
            .polish
            .style_system_prompt
            .contains("语音指令整理器"));
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
}
