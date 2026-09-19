//! 常用语管理页的按需批量提取：把用户选中的历史语音转写交给 LLM，
//! 提取值得存成常用语的候选说法（`SnippetDraft`），由用户编辑勾选后逐条入库。
//!
//! LLM 调用模式照抄 [`crate::ghostwriter::assist::run_assist`]：
//! provider 照既有解析路径解析 LLM 通道，context 用 [`DictationContext::capture`]
//! 现场捕获后逐项覆写（mode=Light、style_system_prompt=提取任务书正文＋
//! 输出契约、清空热词/前文轮次/光标上下文、关翻译）。system prompt＝提取
//! 常用语任务书正文（任务书存储取默认或覆写）＋自己的小型 JSON 输出契约
//! （不复用 ASSIST_OUTPUT_CONTRACT）；user 输入＝选中的历史转写（多条拼接，
//! 每条标注来源序号，总长截断 [`MAX_TRANSCRIPT_CHARS`] 保留最近的）。
//!
//! fixture 路由（控制器裁决）：SessionId 是 UUID 新型别、无法携带字符串前缀，
//! 用 uuid5 确定性会话 id（[`extraction_session_id`]），fixture 按「精确等于
//! 此 id」路由 canned 输出；dispatcher 的 [`super::dispatcher::GhostwriterPolishDispatcher::extract_snippet_candidates`]
//! 用同一 helper 传参，生产与测试共享一个 id。

use std::sync::Arc;

use serde::Deserialize;

use crate::credentials::{CredentialStore, ProviderSlot};
use crate::dictation_context::{
    DictationContext, DictationProviderInvocations, DictationStartOptions, ProviderInvocation,
};
use crate::errors::BackendError;
use crate::ghostwriter::types::SnippetDraft;
use crate::ports::{TextPolisher, TextStreamChunk, TextStreamSink};
use crate::types::{PolishMode, SessionId};

/// 提取调用的固定会话 id（uuid5 确定性）：fixture 按「精确等于此 id」路由
/// canned 输出，dispatcher 调 [`extract_snippets`] 时也传它。
pub fn extraction_session_id() -> SessionId {
    SessionId::from_uuid(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        b"openless.ghostwriter.extract",
    ))
}

/// 输出契约（固定，拼在 system 末尾，逐字使用勿改）。
const EXTRACTION_OUTPUT_CONTRACT: &str =
    "只输出 JSON 数组：[{\"phrase\":\"整理好的说法\",\"suggestedTrigger\":\"短触发词\",\"example\":\"原话例句\"}]；没有就输出 []。";

/// 转写总长截断：多条拼接后超过该字符数就丢更旧的（调用方按最近在前传）。
const MAX_TRANSCRIPT_CHARS: usize = 6000;

struct DiscardTextStream;

impl TextStreamSink for DiscardTextStream {
    fn publish(&self, _chunk: TextStreamChunk) -> Result<(), BackendError> {
        Ok(())
    }
}

/// 跑一次按需提取，返回候选草稿（suggestedTrigger 缺省回落 phrase）。
/// 全空输入直接返回空（不发 LLM 调用）。
pub async fn extract_snippets(
    polisher: &Arc<dyn TextPolisher>,
    credential_store: &Arc<dyn CredentialStore>,
    active_llm_provider: &str,
    session_id: SessionId,
    transcripts: &[String],
    instruction_body: &str,
) -> Result<Vec<SnippetDraft>, BackendError> {
    let composed = compose_transcripts(transcripts);
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
            composed,
            Arc::new(DiscardTextStream),
        )
        .await?;
    Ok(parse_drafts(&output.text))
}

/// 多条转写拼接：每条 `【来源 N】`＋正文；总长超过 [`MAX_TRANSCRIPT_CHARS`]
/// 从更旧的开始丢（调用方按最近在前传）；单条超长时保留其尾部（最近的
/// 说话在记录尾部）。常用语与热词两个提取器共用同一拼接规则。
pub(crate) fn compose_transcripts(transcripts: &[String]) -> String {
    let mut sections: Vec<String> = Vec::new();
    let mut used = 0usize;
    for (index, transcript) in transcripts.iter().enumerate() {
        let budget = MAX_TRANSCRIPT_CHARS.saturating_sub(used);
        if budget == 0 {
            break;
        }
        let text = transcript.trim();
        if text.is_empty() {
            continue;
        }
        let total = text.chars().count();
        let text = if total > budget {
            text.chars().skip(total - budget).collect::<String>()
        } else {
            text.to_string()
        };
        used += text.chars().count();
        sections.push(format!("【来源 {}】\n{}\n", index + 1, text));
    }
    sections.join("\n")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftJson {
    phrase: String,
    #[serde(default)]
    suggested_trigger: Option<String>,
    #[serde(default)]
    example: Option<String>,
}

/// 解析 LLM 输出为草稿列表：`serde_json::from_str`；失败/空 → 空 Vec（合法
/// 返回，log::warn）。空说法丢弃；suggestedTrigger 缺省回落 phrase。
fn parse_drafts(text: &str) -> Vec<SnippetDraft> {
    let parsed: Vec<DraftJson> =
        match serde_json::from_str(crate::ghostwriter::assist::strip_json_fence(text)) {
            Ok(parsed) => parsed,
            Err(error) => {
                log::warn!(
                    "[ghostwriter] snippet extraction output is not valid JSON, treating as empty: {error}"
                );
                return Vec::new();
            }
        };
    parsed
        .into_iter()
        .filter(|draft| !draft.phrase.trim().is_empty())
        .map(|draft| {
            let suggested_trigger = draft
                .suggested_trigger
                .filter(|trigger| !trigger.trim().is_empty())
                .unwrap_or_else(|| draft.phrase.clone());
            SnippetDraft {
                phrase: draft.phrase,
                suggested_trigger,
                example: draft
                    .example
                    .filter(|example| !example.trim().is_empty()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::InMemoryCredentialStore;
    use crate::ghostwriter::prompts::TaskBriefId;
    use crate::testing::FixtureTextPolisher;

    const CANNED_ARRAY: &str = r#"[{"phrase":"把日志清一下","suggestedTrigger":"清日志","example":"把那个日志清一下就是那种缓存"},{"phrase":"用测试环境跑","example":"以后都用测试环境跑"},{"phrase":"发版前看灰度","suggestedTrigger":"看灰度","example":"发版前先看看灰度数据"}]"#;

    fn store() -> Arc<dyn CredentialStore> {
        Arc::new(InMemoryCredentialStore::default())
    }

    #[tokio::test]
    async fn extract_parses_canned_array_with_trigger_fallback() {
        // suggestedTrigger 给了用给的；没给的回落 phrase；空触发词也回落。
        let json = r#"[{"phrase":"说法甲","suggestedTrigger":"触发甲","example":"例句甲"},{"phrase":"说法乙","example":"例句乙"},{"phrase":"说法丙","suggestedTrigger":"  ","example":"例句丙"},{"phrase":"  ","suggestedTrigger":"空说法"}]"#;
        let fixture = FixtureTextPolisher::successful("unused").with_extraction_json(json);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let drafts = extract_snippets(
            &polisher,
            &store(),
            "test-llm",
            extraction_session_id(),
            &["第一段转写".to_string()],
            TaskBriefId::SedimentExtraction.default_body(),
        )
        .await
        .expect("extraction should succeed");

        assert_eq!(
            drafts,
            vec![
                SnippetDraft {
                    phrase: "说法甲".to_string(),
                    suggested_trigger: "触发甲".to_string(),
                    example: Some("例句甲".to_string()),
                },
                SnippetDraft {
                    phrase: "说法乙".to_string(),
                    suggested_trigger: "说法乙".to_string(),
                    example: Some("例句乙".to_string()),
                },
                SnippetDraft {
                    phrase: "说法丙".to_string(),
                    suggested_trigger: "说法丙".to_string(),
                    example: Some("例句丙".to_string()),
                },
            ]
        );
        assert_eq!(fixture.session_ids(), vec![extraction_session_id()]);
        let context = &fixture.contexts()[0];
        // system prompt＝提取任务书正文＋自己的输出契约（非 ASSIST_OUTPUT_CONTRACT）。
        assert!(context
            .polish
            .style_system_prompt
            .starts_with(TaskBriefId::SedimentExtraction.default_body()));
        assert!(context
            .polish
            .style_system_prompt
            .ends_with(EXTRACTION_OUTPUT_CONTRACT));
        assert_eq!(fixture.inputs(), vec!["【来源 1】\n第一段转写\n".to_string()]);
    }

    #[tokio::test]
    async fn extract_invalid_json_yields_empty() {
        // LLM 回普通文本 → Ok(空 Vec)，不报错。
        let fixture = FixtureTextPolisher::successful("unused")
            .with_extraction_json("这不是 JSON，只是普通文本");
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let drafts = extract_snippets(
            &polisher,
            &store(),
            "test-llm",
            extraction_session_id(),
            &["随便说点什么".to_string()],
            TaskBriefId::SedimentExtraction.default_body(),
        )
        .await
        .expect("extraction should not error on unparseable output");

        assert!(drafts.is_empty());
    }

    #[tokio::test]
    async fn extract_parses_fenced_canned_array() {
        // 契约要求裸 JSON 数组，模型偶尔仍包 ``` 围栏：剥掉后照常解析。
        let fenced = format!("```json\n{CANNED_ARRAY}\n```");
        let fixture = FixtureTextPolisher::successful("unused").with_extraction_json(fenced);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let drafts = extract_snippets(
            &polisher,
            &store(),
            "test-llm",
            extraction_session_id(),
            &["随便说点什么".to_string()],
            TaskBriefId::SedimentExtraction.default_body(),
        )
        .await
        .expect("extraction should succeed");

        assert_eq!(drafts.len(), 3);
        assert_eq!(drafts[0].suggested_trigger, "清日志");
    }

    #[tokio::test]
    async fn extract_empty_input_short_circuits() {
        // 全空输入不发 LLM 调用（fixture 会因无预置输出而 panic，走到这里说明短路）。
        let fixture = FixtureTextPolisher::successful("unused").with_extraction_json("[]");
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let drafts = extract_snippets(
            &polisher,
            &store(),
            "test-llm",
            extraction_session_id(),
            &["  ".to_string()],
            TaskBriefId::SedimentExtraction.default_body(),
        )
        .await
        .expect("extraction should succeed");

        assert!(drafts.is_empty());
        assert!(fixture.session_ids().is_empty());
    }

    #[tokio::test]
    async fn transcripts_are_annotated_and_truncated_keeping_recent() {
        // 多条拼接每条标注来源序号；总长截断保留最近的：最新一条超长吃满预算，
        // 更旧的整条丢弃；单条超长保留尾部（最近的说话在记录尾部）。
        let long = format!(
            "x{}{}",
            "b".repeat(MAX_TRANSCRIPT_CHARS + 100),
            "y".repeat(10)
        );
        let short = "a".repeat(10);
        let fixture = FixtureTextPolisher::successful("unused").with_extraction_json("[]");
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        extract_snippets(
            &polisher,
            &store(),
            "test-llm",
            extraction_session_id(),
            &[long.clone(), short.clone(), "第三段".to_string()],
            TaskBriefId::SedimentExtraction.default_body(),
        )
        .await
        .expect("extraction should succeed");

        let raw = &fixture.inputs()[0];
        assert!(raw.starts_with("【来源 1】\n"));
        // 超长单条：尾部保留、头部截掉。
        assert!(raw.contains("yyyyyyyyyy"));
        assert!(!raw.contains("xxxxxxxxxx"));
        // 预算被最新一条吃满：更旧的不再出现。
        assert!(!raw.contains(&short));
        assert!(!raw.contains("第三段"));
        assert!(!raw.contains("【来源 2】"));
        // 正文总长不超过预算（标注行另计）。
        let body_chars: usize = raw
            .lines()
            .filter(|line| !line.starts_with("【来源") && !line.is_empty())
            .map(|line| line.chars().count())
            .sum();
        assert!(body_chars <= MAX_TRANSCRIPT_CHARS);
    }
}
