//! 实时助手（assist）：说话期间按停顿/句毕触发的一次 LLM 调用，一次协同产出
//! 候选（卡词时给精准词/候选表述/命名）、常用语推荐与常用语提醒。
//!
//! LLM 调用模式照抄 [`crate::ghostwriter::segment_polisher::polish_segment`]：
//! provider 照既有解析路径解析 LLM 通道，context 用 [`DictationContext::capture`]
//! 现场捕获后逐项覆写（mode=Light、style_system_prompt=任务书拼接结果、
//! 清空热词/前文轮次/光标上下文、关翻译）。system prompt＝候选任务书（按开关）
//! ＋推荐任务书（按开关）＋常用语提醒任务书＋输出契约（逐字）；user 输入＝当前
//! 内容＋常用语库＋重复档。输出按契约解析为 JSON：失败/空 → 空 outcome（合法
//! 返回，表示「没什么可给」，不报错）；解析侧强制裁剪候选 ≤2 组、每组 ≤5 条、
//! 总 ≤8 条、推荐 ≤3 个，候选组 kind 白名单外的归一为 "phrase"（保内容不丢组）；
//! 解析后按输入开关机制级清零候选/推荐产出（不依赖模型自觉遵守输出契约，
//! 沉淀不受这两个开关控制）。

use std::sync::Arc;

use serde::Deserialize;

use crate::credentials::{CredentialStore, ProviderSlot};
use crate::dictation_context::{
    DictationContext, DictationProviderInvocations, DictationStartOptions, ProviderInvocation,
};
use crate::errors::BackendError;
use crate::ghostwriter::prompts::ASSIST_OUTPUT_CONTRACT;
use crate::ports::{TextPolisher, TextStreamChunk, TextStreamSink};
use crate::types::{PolishMode, SessionId};

use super::snippet_store::Snippet;

/// 助手调用的固定会话 id（uuid5 确定性）：SessionId 是 UUID 新型别、无法携带
/// 字符串前缀，fixture 按「精确等于此 id」路由 canned 输出；dispatcher 调
/// [`run_assist`] 时也传它（生产与测试共享一个 id，照
/// [`crate::ghostwriter::sediment_extractor::extraction_session_id`] 的裁决机制）。
pub fn assist_session_id() -> SessionId {
    SessionId::from_uuid(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        b"openless.ghostwriter.assist",
    ))
}

/// 解析侧裁剪上限：候选组数。
const MAX_CANDIDATE_GROUPS: usize = 2;
/// 解析侧裁剪上限：每组候选条数。
const MAX_ITEMS_PER_GROUP: usize = 5;
/// 解析侧裁剪上限：候选总条数。
const MAX_CANDIDATE_ITEMS: usize = 8;
/// 解析侧裁剪上限：推荐条数。
const MAX_RECOMMENDATIONS: usize = 3;

/// 候选组 kind 白名单：输出契约约定的三类；白名单外的 kind 归一为
/// "phrase"（保内容，不丢组）。
const CANDIDATE_KINDS: [&str; 3] = ["term", "phrase", "naming"];

/// 重复档里一条未提示过的说法摘要（调用方从重复档 store 取，≤20 条，仅未 prompted）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceSummary {
    pub phrase: String,
    pub count: u32,
}

/// 沉淀命中：说话人在重复某个值得收进常用语的说法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SedimentMatch {
    pub phrase: String,
    pub count: u32,
    pub suggested_trigger: String,
}

/// 一次实时助手调用的输入（由调用方组装；缓冲截取与材料筛选都在调用方做）。
#[derive(Debug, Clone)]
pub struct AssistInput {
    /// 本次润色调用的会话 id。
    pub session_id: SessionId,
    /// 说话缓冲尾部 ≤800 chars（截取由调用方做，这里不截）。
    pub context_text: String,
    /// 已启用的常用语。
    pub snippets: Vec<Snippet>,
    /// 重复档摘要（≤20 条，仅未 prompted）。
    pub recurrence: Vec<RecurrenceSummary>,
    /// 是否产出候选。
    pub include_candidates: bool,
    /// 是否产出推荐。
    pub include_recommendations: bool,
    /// 候选任务书正文（[`crate::ghostwriter::prompts::TaskBriefId::Candidates`]）。
    pub instruction_candidates: String,
    /// 推荐任务书正文（[`crate::ghostwriter::prompts::TaskBriefId::Recommendations`]）。
    pub instruction_recommendations: String,
    /// 常用语提醒任务书正文（[`crate::ghostwriter::prompts::TaskBriefId::SedimentNotice`]）。
    pub instruction_sediment: String,
}

/// 一次实时助手调用的产出；空 outcome 合法（LLM 判定「没什么可给」）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssistOutcome {
    /// 候选组：(kind: "term"|"phrase"|"naming", texts)。
    pub candidate_groups: Vec<(String, Vec<String>)>,
    /// 推荐的常用语 id。
    pub recommendation_ids: Vec<String>,
    /// 沉淀命中。
    pub sediment: Option<SedimentMatch>,
}

struct DiscardTextStream;

impl TextStreamSink for DiscardTextStream {
    fn publish(&self, _chunk: TextStreamChunk) -> Result<(), BackendError> {
        Ok(())
    }
}

/// 跑一次实时助手调用，返回解析后的产出。
pub async fn run_assist(
    polisher: &Arc<dyn TextPolisher>,
    credential_store: &Arc<dyn CredentialStore>,
    active_llm_provider: &str,
    input: &AssistInput,
) -> Result<AssistOutcome, BackendError> {
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
            ProviderInvocation::for_provider("ghostwriter-assist-unused-asr"),
            llm,
            ProviderInvocation::for_provider("ghostwriter-assist-unused-omni"),
        ),
        Vec::new(),
        Vec::new(),
        &DictationStartOptions::default(),
    );
    context.asr.prompt = None;
    context.polish.mode = PolishMode::Light;
    context.polish.style_system_prompt = compose_system_prompt(input);
    context.polish.hotwords.clear();
    context.polish.translation_active = false;
    context.polish.cursor_context = None;
    context.polish.prior_turns.clear();
    let output = polisher
        .polish(
            input.session_id,
            Arc::new(context),
            compose_user_input(input),
            Arc::new(DiscardTextStream),
        )
        .await?;
    let mut outcome = parse_outcome(&output.text);
    // 机制级强制：开关关闭时清零对应产出（不依赖模型自觉遵守输出契约；
    // 沉淀不受这两个开关控制——无独立开关，保持现状）。
    if !input.include_candidates {
        outcome.candidate_groups.clear();
    }
    if !input.include_recommendations {
        outcome.recommendation_ids.clear();
    }
    Ok(outcome)
}

/// system prompt 拼装：候选任务书（include_candidates 时）＋推荐任务书
/// （include_recommendations 时）＋常用语提醒任务书＋输出契约（逐字，永远在末尾）。
fn compose_system_prompt(input: &AssistInput) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if input.include_candidates {
        parts.push(input.instruction_candidates.as_str());
    }
    if input.include_recommendations {
        parts.push(input.instruction_recommendations.as_str());
    }
    parts.push(input.instruction_sediment.as_str());
    parts.push(ASSIST_OUTPUT_CONTRACT);
    parts.join("\n")
}

/// user 输入拼装：当前内容＋常用语库（有料时，每条 `id|触发词|文本`）＋重复档
/// （有料时，每条 `说法×次数`）。
fn compose_user_input(input: &AssistInput) -> String {
    let mut parts = vec![format!("当前内容：{}", input.context_text)];
    if !input.snippets.is_empty() {
        let lines: Vec<String> = input
            .snippets
            .iter()
            .map(|snippet| format!("{}|{}|{}", snippet.id, snippet.trigger, snippet.text))
            .collect();
        parts.push(format!("常用语库（id|触发词|文本）：\n{}", lines.join("\n")));
    }
    if !input.recurrence.is_empty() {
        let lines: Vec<String> = input
            .recurrence
            .iter()
            .map(|entry| format!("{}×{}", entry.phrase, entry.count))
            .collect();
        parts.push(format!("重复档：\n{}", lines.join("\n")));
    }
    parts.join("\n")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssistJson {
    #[serde(default)]
    candidate_groups: Vec<CandidateGroupJson>,
    #[serde(default)]
    recommendations: Vec<String>,
    #[serde(default)]
    sediment: Option<SedimentJson>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CandidateGroupJson {
    kind: String,
    #[serde(default)]
    items: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SedimentJson {
    phrase: String,
    count: u32,
    suggested_trigger: String,
}

/// 剥掉 LLM 输出外的 ``` 代码围栏：契约要求裸 JSON，模型偶尔仍包 ```/```json
/// 围栏。trim 后以 ``` 开头就剥掉首行（含语言标注）与末行围栏，其余原样返回。
pub(crate) fn strip_json_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some((_, body)) = rest.split_once('\n') else {
        return trimmed;
    };
    let body = body.trim();
    body.strip_suffix("```").map(str::trim).unwrap_or(body)
}

/// 解析 LLM 输出为产出：`serde_json::from_str`；失败/空 → 空 outcome（合法返回，
/// log::warn）。解析侧强制裁剪：候选 ≤2 组、每组 ≤5 条、总 ≤8 条、推荐 ≤3 个；
/// kind 白名单外的组归一为 "phrase"。
fn parse_outcome(text: &str) -> AssistOutcome {
    let parsed: AssistJson = match serde_json::from_str(strip_json_fence(text)) {
        Ok(parsed) => parsed,
        Err(error) => {
            log::warn!("[ghostwriter] assist output is not valid JSON, treating as empty: {error}");
            return AssistOutcome::default();
        }
    };
    let mut candidate_groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut total = 0usize;
    for group in parsed.candidate_groups {
        if candidate_groups.len() >= MAX_CANDIDATE_GROUPS || total >= MAX_CANDIDATE_ITEMS {
            break;
        }
        let items: Vec<String> = group
            .items
            .into_iter()
            .take(MAX_ITEMS_PER_GROUP.min(MAX_CANDIDATE_ITEMS - total))
            .collect();
        total += items.len();
        let kind = if CANDIDATE_KINDS.contains(&group.kind.as_str()) {
            group.kind
        } else {
            "phrase".to_string()
        };
        candidate_groups.push((kind, items));
    }
    AssistOutcome {
        candidate_groups,
        recommendation_ids: parsed
            .recommendations
            .into_iter()
            .take(MAX_RECOMMENDATIONS)
            .collect(),
        sediment: parsed.sediment.map(|s| SedimentMatch {
            phrase: s.phrase,
            count: s.count,
            suggested_trigger: s.suggested_trigger,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::InMemoryCredentialStore;
    use crate::ghostwriter::prompts::TaskBriefId;
    use crate::ghostwriter::snippet_store::{Snippet, SnippetKind, SnippetPlacement};
    use crate::testing::FixtureTextPolisher;

    const CANNED_JSON: &str = r#"{"candidateGroups":[{"kind":"term","items":["精准词一","精准词二","精准词三","精准词四","精准词五","精准词六"]},{"kind":"phrase","items":["候选表述一","候选表述二","候选表述三"]},{"kind":"naming","items":["命名一","命名二"]}],"recommendations":["rec-1","rec-2","rec-3","rec-4"],"sediment":{"phrase":"把那个日志清一下","count":3,"suggestedTrigger":"清日志"}}"#;

    fn snippet(id: &str, trigger: &str, text: &str) -> Snippet {
        Snippet {
            id: id.to_string(),
            trigger: trigger.to_string(),
            aliases: Vec::new(),
            text: text.to_string(),
            kind: SnippetKind::Phrasing,
            placement: SnippetPlacement::Tail,
            attachments: Vec::new(),
            enabled: true,
        }
    }

    fn input() -> AssistInput {
        AssistInput {
            session_id: SessionId::new(),
            context_text: "把那个日志清一下就是那种缓存".to_string(),
            snippets: vec![
                snippet("s1", "触发词甲", "表述甲"),
                snippet("s2", "触发词乙", "表述乙"),
            ],
            recurrence: vec![RecurrenceSummary {
                phrase: "重复说法丁".to_string(),
                count: 4,
            }],
            include_candidates: true,
            include_recommendations: true,
            instruction_candidates: "候选任务书正文甲".to_string(),
            instruction_recommendations: "推荐任务书正文乙".to_string(),
            instruction_sediment: "常用语提醒任务书正文丙".to_string(),
        }
    }

    fn store() -> Arc<dyn CredentialStore> {
        Arc::new(InMemoryCredentialStore::default())
    }

    fn default_bodies_input() -> AssistInput {
        let mut input = input();
        input.instruction_candidates = TaskBriefId::Candidates.default_body().to_string();
        input.instruction_recommendations = TaskBriefId::Recommendations.default_body().to_string();
        input.instruction_sediment = TaskBriefId::SedimentNotice.default_body().to_string();
        input
    }

    #[tokio::test]
    async fn assist_parses_canned_json() {
        let fixture =
            FixtureTextPolisher::successful("unused").with_assist_json(CANNED_JSON);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let outcome = run_assist(&polisher, &store(), "test-llm", &default_bodies_input())
            .await
            .expect("assist should succeed");

        assert_eq!(outcome.candidate_groups.len(), 2);
        assert_eq!(
            outcome.candidate_groups[0],
            (
                "term".to_string(),
                vec![
                    "精准词一".to_string(),
                    "精准词二".to_string(),
                    "精准词三".to_string(),
                    "精准词四".to_string(),
                    "精准词五".to_string(),
                ]
            )
        );
        assert_eq!(
            outcome.candidate_groups[1],
            (
                "phrase".to_string(),
                vec![
                    "候选表述一".to_string(),
                    "候选表述二".to_string(),
                    "候选表述三".to_string(),
                ]
            )
        );
        assert_eq!(
            outcome.recommendation_ids,
            vec!["rec-1".to_string(), "rec-2".to_string(), "rec-3".to_string()]
        );
        assert_eq!(
            outcome.sediment,
            Some(SedimentMatch {
                phrase: "把那个日志清一下".to_string(),
                count: 3,
                suggested_trigger: "清日志".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn assist_invalid_json_yields_empty_outcome() {
        let fixture = FixtureTextPolisher::successful("unused")
            .with_assist_json("这不是 JSON，只是普通文本");
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let outcome = run_assist(&polisher, &store(), "test-llm", &default_bodies_input())
            .await
            .expect("assist should not error on unparseable output");

        assert_eq!(outcome, AssistOutcome::default());
    }

    #[tokio::test]
    async fn assist_parses_fenced_canned_json() {
        // 契约要求裸 JSON，模型偶尔仍包 ``` 围栏：剥掉后照常解析。
        let fenced = format!("```json\n{CANNED_JSON}\n```");
        let fixture = FixtureTextPolisher::successful("unused").with_assist_json(fenced);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let outcome = run_assist(&polisher, &store(), "test-llm", &default_bodies_input())
            .await
            .expect("assist should succeed");

        assert_eq!(outcome.candidate_groups.len(), 2);
        assert_eq!(
            outcome.recommendation_ids,
            vec!["rec-1".to_string(), "rec-2".to_string(), "rec-3".to_string()]
        );
    }

    #[tokio::test]
    async fn assist_prompt_contains_bodies_library_and_recurrence() {
        let fixture =
            FixtureTextPolisher::successful("unused").with_assist_json(CANNED_JSON);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        let request = input();

        run_assist(&polisher, &store(), "test-llm", &request)
            .await
            .expect("assist should succeed");

        assert_eq!(fixture.session_ids(), vec![request.session_id]);
        let context = fixture.contexts();
        assert_eq!(context.len(), 1);
        let prompt = context[0].polish.style_system_prompt.as_str();
        let pos_candidates = prompt.find("候选任务书正文甲").expect("candidates body");
        let pos_recommendations = prompt.find("推荐任务书正文乙").expect("recommendations body");
        let pos_sediment = prompt.find("常用语提醒任务书正文丙").expect("sediment body");
        assert!(pos_candidates < pos_recommendations);
        assert!(pos_recommendations < pos_sediment);
        assert!(prompt.ends_with(ASSIST_OUTPUT_CONTRACT));
        assert_eq!(context[0].llm.provider_id, "test-llm");
        assert_eq!(context[0].polish.mode, PolishMode::Light);
        assert!(!context[0].polish.translation_active);
        assert!(context[0].polish.hotwords.is_empty());
        assert!(context[0].polish.cursor_context.is_none());
        assert!(context[0].polish.prior_turns.is_empty());
        assert!(context[0].asr.prompt.is_none());

        let raw = &fixture.inputs()[0];
        let pos_context = raw.find("当前内容：把那个日志清一下就是那种缓存").expect("context text");
        let pos_library = raw.find("常用语库（id|触发词|文本）：").expect("library header");
        let pos_s1 = raw.find("s1|触发词甲|表述甲").expect("snippet 1 line");
        let pos_s2 = raw.find("s2|触发词乙|表述乙").expect("snippet 2 line");
        let pos_recurrence = raw.find("重复档：").expect("recurrence header");
        let pos_phrase = raw.find("重复说法丁×4").expect("recurrence line");
        assert!(pos_context < pos_library);
        assert!(pos_library < pos_s1);
        assert!(pos_s1 < pos_s2);
        assert!(pos_s2 < pos_recurrence);
        assert!(pos_recurrence < pos_phrase);
    }

    #[tokio::test]
    async fn assist_prompt_omits_disabled_sections_but_keeps_library_and_recurrence() {
        let fixture =
            FixtureTextPolisher::successful("unused").with_assist_json(CANNED_JSON);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        let mut request = input();
        request.include_candidates = false;
        request.include_recommendations = false;

        run_assist(&polisher, &store(), "test-llm", &request)
            .await
            .expect("assist should succeed");

        let contexts = fixture.contexts();
        let prompt = contexts[0].polish.style_system_prompt.as_str();
        assert!(!prompt.contains("候选任务书正文甲"));
        assert!(!prompt.contains("推荐任务书正文乙"));
        assert!(prompt.contains("常用语提醒任务书正文丙"));
        assert!(prompt.ends_with(ASSIST_OUTPUT_CONTRACT));
        let raw = &fixture.inputs()[0];
        assert!(raw.contains("常用语库（id|触发词|文本）："));
        assert!(raw.contains("s1|触发词甲|表述甲"));
        assert!(raw.contains("重复档："));
        assert!(raw.contains("重复说法丁×4"));
    }

    #[tokio::test]
    async fn assist_zeroes_outputs_for_disabled_switches() {
        // 机制级强制：开关关闭时解析后清零对应产出（模型仍可能按契约结构
        // 回填候选/推荐，不允许漏进结果）；沉淀不受这两个开关控制。
        let fixture =
            FixtureTextPolisher::successful("unused").with_assist_json(CANNED_JSON);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        let mut request = default_bodies_input();
        request.include_candidates = false;
        request.include_recommendations = false;

        let outcome = run_assist(&polisher, &store(), "test-llm", &request)
            .await
            .expect("assist should succeed");

        assert!(outcome.candidate_groups.is_empty());
        assert!(outcome.recommendation_ids.is_empty());
        assert!(outcome.sediment.is_some());
    }

    #[tokio::test]
    async fn assist_normalizes_unknown_candidate_kind_to_phrase() {
        // 解析侧 kind 白名单：白名单外的组归一为 phrase（保内容，不丢组）。
        let json = r#"{"candidateGroups":[{"kind":"whatever","items":["候选甲","候选乙"]}],"recommendations":[],"sediment":null}"#;
        let fixture = FixtureTextPolisher::successful("unused").with_assist_json(json);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let outcome = run_assist(&polisher, &store(), "test-llm", &default_bodies_input())
            .await
            .expect("assist should succeed");

        assert_eq!(
            outcome.candidate_groups,
            vec![(
                "phrase".to_string(),
                vec!["候选甲".to_string(), "候选乙".to_string()]
            )]
        );
    }
}
