//! 词典页的按需热词提取：把用户选中的历史语音 **raw 原文**（未经润色，是最
//! 接近 ASR 真实输出的证据）交给 LLM，找疑似识别混乱的词，产出候选热词
//! 草稿（`HotwordDraft`），由用户编辑勾选后逐条 add_vocab 进词典。
//!
//! 结构照抄 [`super::snippet_extractor`]：provider 解析、context 逐项覆写
//! （mode=Light、style_system_prompt＝提取热词任务书正文＋
//! [`super::prompts::HOTWORD_EXTRACTION_CONTRACT`]）、转写拼接（同一条
//! `compose_transcripts`）。**输入必须是 raw_transcript**：final_text 已被
//! 润色修正过，识别错误会被洗掉（用户裁决：raw 是证据）。
//!
//! fixture 路由（控制器裁决）：用 uuid5 确定性会话 id
//! （[`hotword_extraction_session_id`]），fixture 按「精确等于此 id」路由
//! canned 输出；dispatcher 用同一 helper 传参，生产与测试共享一个 id。

use std::sync::Arc;

use serde::Deserialize;

use crate::credentials::{CredentialStore, ProviderSlot};
use crate::dictation_context::{
    DictationContext, DictationProviderInvocations, DictationStartOptions, ProviderInvocation,
};
use crate::errors::BackendError;
use crate::ghostwriter::types::HotwordDraft;
use crate::ports::{TextPolisher, TextStreamChunk, TextStreamSink};
use crate::types::{PolishMode, SessionId};

/// 热词提取调用的固定会话 id（uuid5 确定性）：fixture 按「精确等于此 id」
/// 路由 canned 输出，dispatcher 调 [`extract_hotwords`] 时也传它。
pub fn hotword_extraction_session_id() -> SessionId {
    SessionId::from_uuid(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        b"openless.ghostwriter.hotword-extract",
    ))
}

struct DiscardTextStream;

impl TextStreamSink for DiscardTextStream {
    fn publish(&self, _chunk: TextStreamChunk) -> Result<(), BackendError> {
        Ok(())
    }
}

/// 跑一次热词提取，返回候选草稿（空 error/hotword 的条目丢弃，example 空白
/// 归 None）。全空输入直接返回空（不发 LLM 调用）。
pub async fn extract_hotwords(
    polisher: &Arc<dyn TextPolisher>,
    credential_store: &Arc<dyn CredentialStore>,
    active_llm_provider: &str,
    session_id: SessionId,
    transcripts: &[String],
    instruction_body: &str,
) -> Result<Vec<HotwordDraft>, BackendError> {
    let composed = super::snippet_extractor::compose_transcripts(transcripts);
    if composed.is_empty() {
        return Ok(Vec::new());
    }
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
            ProviderInvocation::for_provider("ghostwriter-hotword-unused-asr"),
            llm,
            ProviderInvocation::for_provider("ghostwriter-hotword-unused-omni"),
        ),
        Vec::new(),
        Vec::new(),
        &DictationStartOptions::default(),
    );
    context.asr.prompt = None;
    context.polish.mode = PolishMode::Light;
    context.polish.style_system_prompt =
        format!("{instruction_body}\n{}", super::prompts::HOTWORD_EXTRACTION_CONTRACT);
    context.polish.hotwords.clear();
    context.polish.translation_active = false;
    context.polish.cursor_context = None;
    context.polish.prior_turns.clear();
    let output = polisher
        .polish(
            session_id,
            Arc::new(context),
            composed,
            Arc::new(DiscardTextStream),
        )
        .await?;
    Ok(parse_drafts(&output.text))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftJson {
    error: String,
    hotword: String,
    #[serde(default)]
    example: Option<String>,
}

/// 解析 LLM 输出为草稿列表：`serde_json::from_str`；失败/空 → 空 Vec（合法
/// 返回，log::warn）。空 error/hotword 丢弃；example 空白归 None。
fn parse_drafts(text: &str) -> Vec<HotwordDraft> {
    let parsed: Vec<DraftJson> =
        match serde_json::from_str(super::assist::strip_json_fence(text)) {
            Ok(parsed) => parsed,
            Err(error) => {
                log::warn!(
                    "[ghostwriter] hotword extraction output is not valid JSON, treating as empty: {error}"
                );
                return Vec::new();
            }
        };
    parsed
        .into_iter()
        .filter(|draft| !draft.error.trim().is_empty() && !draft.hotword.trim().is_empty())
        .map(|draft| HotwordDraft {
            error: draft.error,
            hotword: draft.hotword,
            example: draft
                .example
                .filter(|example| !example.trim().is_empty()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::InMemoryCredentialStore;
    use crate::ghostwriter::prompts::{HOTWORD_EXTRACTION_CONTRACT, TaskBriefId};
    use crate::testing::FixtureTextPolisher;

    fn store() -> Arc<dyn CredentialStore> {
        Arc::new(InMemoryCredentialStore::default())
    }

    #[tokio::test]
    async fn extract_parses_canned_candidates_and_drops_blank_entries() {
        // error/hotword 都要有；空白条目丢弃；example 空白归 None（条目保留）。
        let json = r#"[{"error":"误词甲","hotword":"热词甲","example":"说到了误词甲"},{"error":"误词乙","hotword":"热词乙"},{"error":"  ","hotword":"热词丙"},{"error":"误词丁","hotword":"  "},{"error":"误词戊","hotword":"热词戊","example":"  "}]"#;
        let fixture = FixtureTextPolisher::successful("unused").with_hotword_json(json);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let drafts = extract_hotwords(
            &polisher,
            &store(),
            "test-llm",
            hotword_extraction_session_id(),
            &["说到了误词甲".to_string()],
            TaskBriefId::HotwordExtraction.default_body(),
        )
        .await
        .expect("extraction should succeed");

        assert_eq!(
            drafts,
            vec![
                HotwordDraft {
                    error: "误词甲".to_string(),
                    hotword: "热词甲".to_string(),
                    example: Some("说到了误词甲".to_string()),
                },
                HotwordDraft {
                    error: "误词乙".to_string(),
                    hotword: "热词乙".to_string(),
                    example: None,
                },
                HotwordDraft {
                    error: "误词戊".to_string(),
                    hotword: "热词戊".to_string(),
                    example: None,
                },
            ]
        );
        assert_eq!(
            fixture.session_ids(),
            vec![hotword_extraction_session_id()]
        );
        // system prompt＝热词提取任务书正文＋自己的输出契约（非 ASSIST 契约）。
        let context = &fixture.contexts()[0];
        assert!(context
            .polish
            .style_system_prompt
            .starts_with(TaskBriefId::HotwordExtraction.default_body()));
        assert!(context
            .polish
            .style_system_prompt
            .ends_with(HOTWORD_EXTRACTION_CONTRACT));
        assert_eq!(
            fixture.inputs(),
            vec!["【来源 1】\n说到了误词甲\n".to_string()]
        );
    }

    #[tokio::test]
    async fn extract_invalid_json_yields_empty() {
        // LLM 回普通文本 → Ok(空 Vec)，不报错。
        let fixture = FixtureTextPolisher::successful("unused")
            .with_hotword_json("这不是 JSON，只是普通文本");
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let drafts = extract_hotwords(
            &polisher,
            &store(),
            "test-llm",
            hotword_extraction_session_id(),
            &["随便说点什么".to_string()],
            TaskBriefId::HotwordExtraction.default_body(),
        )
        .await
        .expect("extraction should not error on unparseable output");

        assert!(drafts.is_empty());
    }

    #[tokio::test]
    async fn extract_parses_fenced_output() {
        // 契约要求裸 JSON 数组，模型偶尔仍包 ``` 围栏：剥掉后照常解析。
        let fenced = format!("```json\n[{{\"error\":\"甲\",\"hotword\":\"乙\",\"example\":\"丙\"}}]\n```");
        let fixture = FixtureTextPolisher::successful("unused").with_hotword_json(fenced);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let drafts = extract_hotwords(
            &polisher,
            &store(),
            "test-llm",
            hotword_extraction_session_id(),
            &["随便说点什么".to_string()],
            TaskBriefId::HotwordExtraction.default_body(),
        )
        .await
        .expect("extraction should succeed");

        assert_eq!(
            drafts,
            vec![HotwordDraft {
                error: "甲".to_string(),
                hotword: "乙".to_string(),
                example: Some("丙".to_string()),
            }]
        );
    }

    #[tokio::test]
    async fn empty_input_short_circuits() {
        // 全空输入不发 LLM 调用（fixture 会因无预置输出而 panic，走到这里说明短路）。
        let fixture = FixtureTextPolisher::successful("unused").with_hotword_json("[]");
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let drafts = extract_hotwords(
            &polisher,
            &store(),
            "test-llm",
            hotword_extraction_session_id(),
            &["  ".to_string()],
            TaskBriefId::HotwordExtraction.default_body(),
        )
        .await
        .expect("extraction should succeed");

        assert!(drafts.is_empty());
        assert!(fixture.session_ids().is_empty());
    }
}
