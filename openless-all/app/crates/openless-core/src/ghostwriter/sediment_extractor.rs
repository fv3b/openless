//! 会话后提取常用语：会话结束后把终稿交给 LLM，提取 0-3 条值得复用的说法。
//!
//! LLM 调用模式照抄 [`crate::ghostwriter::assist::run_assist`]：
//! provider 照既有解析路径解析 LLM 通道，context 用 [`DictationContext::capture`]
//! 现场捕获后逐项覆写（mode=Light、style_system_prompt=任务书正文＋输出契约、
//! 清空热词/前文轮次/光标上下文、关翻译）。system prompt＝提取常用语任务书
//! ＋输出契约（固定，逐字拼在末尾）；user 输入＝终稿原文。输出按契约解析为
//! JSON 数组：失败 → 空 Vec（合法返回，不报错）；解析侧强制至多 3 项。
//!
//! fixture 路由（控制器裁决）：计划里的「session_id 字符串前缀」类型上不可
//! 承载（SessionId 是 UUID 新型别），改为 uuid5 确定性会话 id
//! （[`extraction_session_id`]），fixture 按精确相等路由；Task 6 dispatcher
//! 用同一 helper 传参，生产与测试共享一个 id。

use std::sync::Arc;

use serde::Deserialize;

use crate::credentials::{CredentialStore, ProviderSlot};
use crate::dictation_context::{
    DictationContext, DictationProviderInvocations, DictationStartOptions, ProviderInvocation,
};
use crate::errors::BackendError;
use crate::ports::{TextPolisher, TextStreamChunk, TextStreamSink};
use crate::types::{PolishMode, SessionId};

/// 抽取调用的固定会话 id（uuid5 确定性）：fixture 按「精确等于此 id」路由
/// canned 输出，Task 6 dispatcher 调 extract_phrases 时也传它。
pub fn extraction_session_id() -> SessionId {
    SessionId::from_uuid(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        b"openless.ghostwriter.extract",
    ))
}

/// 输出契约（固定，拼在 system 末尾，逐字使用勿改）。
const EXTRACTION_OUTPUT_CONTRACT: &str = "只输出 JSON 数组，至多 3 项：[{\"phrase\":\"可复用说法\",\"example\":\"原话例句\"}]；没有就输出 []。挑长期可能重复的背景/偏好/约束类说法，忽略一次性内容。";

/// 解析侧强制上限：至多 3 条（先到先得）。
const MAX_PHRASES: usize = 3;

struct DiscardTextStream;

impl TextStreamSink for DiscardTextStream {
    fn publish(&self, _chunk: TextStreamChunk) -> Result<(), BackendError> {
        Ok(())
    }
}

/// 跑一次会话后提取常用语，返回 (说法, 例句) 列表（至多 3 条）。
/// 空终稿直接返回空（不发 LLM 调用）。
pub async fn extract_phrases(
    polisher: &Arc<dyn TextPolisher>,
    credential_store: &Arc<dyn CredentialStore>,
    active_llm_provider: &str,
    session_id: SessionId,
    final_text: &str,
    instruction_body: &str,
) -> Result<Vec<(String, String)>, BackendError> {
    if final_text.trim().is_empty() {
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
            ProviderInvocation::for_provider("ghostwriter-extract-unused-asr"),
            llm,
            ProviderInvocation::for_provider("ghostwriter-extract-unused-omni"),
        ),
        Vec::new(),
        Vec::new(),
        &DictationStartOptions::default(),
    );
    context.asr.prompt = None;
    context.polish.mode = PolishMode::Light;
    context.polish.style_system_prompt =
        format!("{instruction_body}\n{EXTRACTION_OUTPUT_CONTRACT}");
    context.polish.hotwords.clear();
    context.polish.translation_active = false;
    context.polish.cursor_context = None;
    context.polish.prior_turns.clear();
    let output = polisher
        .polish(
            session_id,
            Arc::new(context),
            final_text.to_string(),
            Arc::new(DiscardTextStream),
        )
        .await?;
    Ok(parse_phrases(&output.text))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtractionItem {
    phrase: String,
    #[serde(default)]
    example: String,
}

/// 解析 LLM 输出为 (说法, 例句) 列表：`serde_json::from_str`；失败/空 → 空
/// Vec（合法返回，log::warn）。解析侧强制至多 3 条（先到先得）。
fn parse_phrases(text: &str) -> Vec<(String, String)> {
    let parsed: Vec<ExtractionItem> =
        match serde_json::from_str(super::assist::strip_json_fence(text)) {
            Ok(parsed) => parsed,
            Err(error) => {
                log::warn!(
                    "[ghostwriter] extraction output is not valid JSON, treating as empty: {error}"
                );
                return Vec::new();
            }
        };
    parsed
        .into_iter()
        .take(MAX_PHRASES)
        .map(|item| (item.phrase, item.example))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::InMemoryCredentialStore;
    use crate::ghostwriter::prompts::TaskBriefId;
    use crate::testing::FixtureTextPolisher;

    const CANNED_ARRAY: &str = r#"[{"phrase":"把日志清一下","example":"把那个日志清一下就是那种缓存"},{"phrase":"用测试环境跑","example":"以后都用测试环境跑"},{"phrase":"发版前看灰度","example":"发版前先看看灰度数据"},{"phrase":"第四条应被裁掉","example":"多余的一条"}]"#;

    fn store() -> Arc<dyn CredentialStore> {
        Arc::new(InMemoryCredentialStore::default())
    }

    #[tokio::test]
    async fn extract_parses_canned_array() {
        // 契约样例数组故意给 4 项 → 解析侧强制裁到前 3 条；session_id 用的
        // 是固定抽取 id，system prompt＝任务书正文＋契约收尾，user 输入＝终稿。
        let fixture = FixtureTextPolisher::successful("unused").with_extraction_json(CANNED_ARRAY);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        let final_text = "把那个日志清一下就是那种缓存。以后都用测试环境跑。发版前先看看灰度数据。多余的内容。";

        let phrases = extract_phrases(
            &polisher,
            &store(),
            "test-llm",
            extraction_session_id(),
            final_text,
            TaskBriefId::SedimentExtraction.default_body(),
        )
        .await
        .expect("extraction should succeed");

        assert_eq!(
            phrases,
            vec![
                (
                    "把日志清一下".to_string(),
                    "把那个日志清一下就是那种缓存".to_string()
                ),
                ("用测试环境跑".to_string(), "以后都用测试环境跑".to_string()),
                ("发版前看灰度".to_string(), "发版前先看看灰度数据".to_string()),
            ]
        );
        assert_eq!(fixture.session_ids(), vec![extraction_session_id()]);
        let context = &fixture.contexts()[0];
        assert!(context
            .polish
            .style_system_prompt
            .starts_with(TaskBriefId::SedimentExtraction.default_body()));
        assert!(context
            .polish
            .style_system_prompt
            .ends_with(EXTRACTION_OUTPUT_CONTRACT));
        assert_eq!(fixture.inputs(), vec![final_text.to_string()]);
    }

    #[tokio::test]
    async fn extract_invalid_json_yields_empty() {
        // LLM 回普通文本 → Ok(空 Vec)，不报错。
        let fixture = FixtureTextPolisher::successful("unused")
            .with_extraction_json("这不是 JSON，只是普通文本");
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let phrases = extract_phrases(
            &polisher,
            &store(),
            "test-llm",
            extraction_session_id(),
            "随便说点什么",
            TaskBriefId::SedimentExtraction.default_body(),
        )
        .await
        .expect("extraction should not error on unparseable output");

        assert!(phrases.is_empty());
    }

    #[tokio::test]
    async fn extract_parses_fenced_canned_array() {
        // 契约要求裸 JSON 数组，模型偶尔仍包 ``` 围栏：剥掉后照常解析。
        let fenced = format!("```json\n{CANNED_ARRAY}\n```");
        let fixture = FixtureTextPolisher::successful("unused").with_extraction_json(fenced);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let phrases = extract_phrases(
            &polisher,
            &store(),
            "test-llm",
            extraction_session_id(),
            "随便说点什么",
            TaskBriefId::SedimentExtraction.default_body(),
        )
        .await
        .expect("extraction should succeed");

        assert_eq!(phrases.len(), 3);
        assert_eq!(phrases[0].0, "把日志清一下");
    }
}
