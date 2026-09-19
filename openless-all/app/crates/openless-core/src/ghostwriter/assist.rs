//! 实时助手（assist）：说话期间按停顿/句毕触发的一次 LLM 调用，一次协同产出
//! 候选（把说话里说不清的点校准成叫法/命名，与卡词无关，见 ADR 0003）与
//! 常用语推荐。
//!
//! LLM 调用模式照抄 [`crate::ghostwriter::segment_polisher::polish_segment`]：
//! provider 照既有解析路径解析 LLM 通道，context 用 [`DictationContext::capture`]
//! 现场捕获后逐项覆写（mode=Light、style_system_prompt=任务书拼接结果、
//! 清空热词/前文轮次/光标上下文、关翻译）。代笔路径（conversation=None）的
//! system prompt＝候选任务书（按开关）＋推荐任务书（按开关）＋输出契约
//! （逐字）；user 输入＝当前内容＋常用语库。对话路径（conversation=Some，
//! 对话会话复用同一触发点）的 system prompt＝回话任务书＋（抑制注入行）＋
//! 推荐任务书＋对话输出契约，user 输入＝聊天记录＋常用语库。
//! 输出按契约解析为 JSON：失败/空 → 空 outcome（合法返回，表示「没什么可给」，
//! 不报错）；解析侧强制裁剪候选 ≤2 组、每组 ≤5 条、总 ≤8 条、推荐 ≤3 个，
//! 候选组 kind 白名单外的归一为 "term"（保内容不丢组）；解析后按输入开关
//! 机制级清零候选/推荐产出，对话抑制时代码强制剥掉 reply（不信任模型）。
//!
//! 常用语提醒与说话中自动提取已退役（2026-09-17 裁决）：批量提取改由
//! 常用语管理页按需触发（[`crate::ghostwriter::snippet_extractor`]）。

use std::sync::Arc;

use serde::Deserialize;

use crate::credentials::{CredentialStore, ProviderSlot};
use crate::dictation_context::{
    DictationContext, DictationProviderInvocations, DictationStartOptions, ProviderInvocation,
};
use crate::errors::BackendError;
use crate::ghostwriter::prompts::{ASSIST_OUTPUT_CONTRACT, CONVERSATION_OUTPUT_CONTRACT};
use crate::ghostwriter::types::CandidateItem;
use crate::ports::{TextPolisher, TextStreamChunk, TextStreamSink};
use crate::shared_types::ConversationProbeDepth;
use crate::types::{PolishMode, SessionId};

use super::snippet_store::Snippet;

/// 助手调用的固定会话 id（uuid5 确定性）：SessionId 是 UUID 新型别、无法携带
/// 字符串前缀，fixture 按「精确等于此 id」路由 canned 输出；dispatcher 调
/// [`run_assist`] 时也传它（生产与测试共享一个 id，照
/// [`crate::ghostwriter::snippet_extractor::extraction_session_id`] 的裁决机制）。
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

/// 候选组 kind 白名单：输出契约约定的两类；白名单外的 kind 归一为
/// "term"（保内容，不丢组）。
const CANDIDATE_KINDS: [&str; 2] = ["term", "naming"];

/// 对话会话抑制回话时的代码注入行（逐字，跟在回话正文之后）：门控判定的
/// 机制级落地的一部分——即便如此，reply 剥除仍由代码强制（不信任模型）。
const SUPPRESS_REPLY_NOTICE: &str =
    "（系统提示：本次不要回话，用户还没有回应你上一句——reply 给 null。）";

/// 追问到清档的深度口径注入行（逐字，跟在 suppress 行之后）：
/// 解除「一点一问」默认口径，但一次一句的边界保留。
const UNTIL_CLEAR_DEPTH_NOTICE: &str =
    "（系统提示：追问到清模式——同一个点没问清可以继续追问，但每次仍只说一句。）";

/// 回声确认档的深度口径注入行（逐字，跟在 suppress 行之后）：
/// 本档独有口径——先重述意图等确认，再展开。
const ECHO_DEPTH_NOTICE: &str =
    "（系统提示：回声确认模式——每次回话先用一句话重述你理解的他的意图，等他确认或纠正后再展开。）";

/// 对话会话的 assist 上下文：聊天记录（行语法【我】/【助手】）＋是否抑制回话
/// （门控判定由 dispatcher 传入）＋冻结的追问深度（口径注入行的依据）。
/// 内部结构，不外序列化。
#[derive(Debug, Clone)]
pub struct ConversationAssistContext {
    pub chat: String,
    pub suppress_reply: bool,
    /// 冻结的追问深度；Single（及 None＝未指定）不注入——回话任务书默认
    /// 口径即一点一问。
    pub probe_depth: Option<ConversationProbeDepth>,
}

/// 一次实时助手调用的输入（由调用方组装；缓冲截取与材料筛选都在调用方做）。
#[derive(Debug, Clone)]
pub struct AssistInput {
    /// 本次润色调用的会话 id。
    pub session_id: SessionId,
    /// 说话缓冲尾部 ≤800 chars（截取由调用方做，这里不截）；对话路径不用。
    pub context_text: String,
    /// 已启用的常用语。
    pub snippets: Vec<Snippet>,
    /// 是否产出候选（对话路径恒 false）。
    pub include_candidates: bool,
    /// 是否产出推荐。
    pub include_recommendations: bool,
    /// 候选任务书正文（[`crate::ghostwriter::prompts::TaskBriefId::Candidates`]；
    /// 对话路径不用）。
    pub instruction_candidates: String,
    /// 推荐任务书正文（[`crate::ghostwriter::prompts::TaskBriefId::Recommendations`]）。
    pub instruction_recommendations: String,
    /// 对话回话任务书正文（[`crate::ghostwriter::prompts::TaskBriefId::ConversationReply`]；
    /// 对话路径用，正文来自任务书存储，可编辑）。
    pub instruction_conversation_reply: String,
    /// 最近语音背景块（2026-09-18 批次 A；带「次要参考」块头，调用方从会话
    /// 冻结快照现算）；None＝无历史，user 输入零变化。命名校准/推荐受益。
    pub recent_voice_block: Option<String>,
    /// Some＝对话会话路径（回话＋推荐双产出）；None＝普通代笔路径，一切照旧。
    pub conversation: Option<ConversationAssistContext>,
}

/// 一次实时助手调用的产出；空 outcome 合法（LLM 判定「没什么可给」）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssistOutcome {
    /// 候选组：(kind: "term"|"naming", items)。
    pub candidate_groups: Vec<(String, Vec<CandidateItem>)>,
    /// 推荐的常用语 id。
    pub recommendation_ids: Vec<String>,
    /// AI 回话（对话路径；普通代笔路径恒 None；suppress 时代码强制 None）。
    pub reply: Option<String>,
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
    // 机制级强制：开关关闭时清零对应产出（不依赖模型自觉遵守输出契约）。
    if !input.include_candidates {
        outcome.candidate_groups.clear();
    }
    if !input.include_recommendations {
        outcome.recommendation_ids.clear();
    }
    // 对话抑制的机制级落地：模型不听话回了 reply 也强制剥掉（同
    // include_candidates 门控模式，不信任模型遵守注入行）。
    if input
        .conversation
        .as_ref()
        .is_some_and(|conversation| conversation.suppress_reply)
    {
        outcome.reply = None;
    }
    Ok(outcome)
}

/// system prompt 拼装：对话路径＝回话正文＋（抑制注入行）＋（深度口径注入行，
/// 按冻结档位）＋推荐正文（按开关）＋对话输出契约（逐字，永远在末尾）——
/// 不含候选/提醒任务书、不含代笔契约；代笔路径＝候选任务书（include_candidates
/// 时）＋推荐任务书（include_recommendations 时）＋输出契约（逐字，永远在末尾）。
fn compose_system_prompt(input: &AssistInput) -> String {
    if let Some(conversation) = &input.conversation {
        let mut parts: Vec<&str> = vec![input.instruction_conversation_reply.as_str()];
        if conversation.suppress_reply {
            parts.push(SUPPRESS_REPLY_NOTICE);
        }
        match conversation.probe_depth {
            Some(ConversationProbeDepth::Echo) => parts.push(ECHO_DEPTH_NOTICE),
            Some(ConversationProbeDepth::UntilClear) => parts.push(UNTIL_CLEAR_DEPTH_NOTICE),
            Some(ConversationProbeDepth::Single) | None => {}
        }
        if input.include_recommendations {
            parts.push(input.instruction_recommendations.as_str());
        }
        parts.push(CONVERSATION_OUTPUT_CONTRACT);
        return parts.join("\n");
    }
    let mut parts: Vec<&str> = Vec::new();
    if input.include_candidates {
        parts.push(input.instruction_candidates.as_str());
    }
    if input.include_recommendations {
        parts.push(input.instruction_recommendations.as_str());
    }
    parts.push(ASSIST_OUTPUT_CONTRACT);
    parts.join("\n")
}

/// user 输入拼装：对话路径＝聊天记录＋常用语库（suppress 只作用于 system
/// prompt，不改变 user 输入）；代笔路径＝当前内容＋常用语库（有料时，每条
/// `id|触发词|文本`）。最近语音背景块（若有）垫底——只是理解背景，供命名
/// 校准/推荐参考，不是本次要处理的指令。
fn compose_user_input(input: &AssistInput) -> String {
    let mut parts = match &input.conversation {
        Some(conversation) => vec![format!("聊天记录：\n{}", conversation.chat)],
        None => vec![format!("当前内容：{}", input.context_text)],
    };
    if !input.snippets.is_empty() {
        let lines: Vec<String> = input
            .snippets
            .iter()
            .map(|snippet| format!("{}|{}|{}", snippet.id, snippet.trigger, snippet.text))
            .collect();
        parts.push(format!("常用语库（id|触发词|文本）：\n{}", lines.join("\n")));
    }
    if let Some(block) = input
        .recent_voice_block
        .as_deref()
        .filter(|block| !block.trim().is_empty())
    {
        parts.push(block.to_string());
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
    reply: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CandidateGroupJson {
    kind: String,
    #[serde(default)]
    items: Vec<CandidateItemJson>,
}

/// 契约里一条候选是 `{"name","note"}` 对象；兼容裸字符串（只当名字，无注释）。
#[derive(Deserialize)]
#[serde(untagged)]
enum CandidateItemJson {
    Named {
        name: String,
        #[serde(default)]
        note: Option<String>,
    },
    Plain(String),
}

impl CandidateItemJson {
    fn name(&self) -> &str {
        match self {
            Self::Named { name, .. } => name,
            Self::Plain(name) => name,
        }
    }
}

impl From<CandidateItemJson> for CandidateItem {
    fn from(item: CandidateItemJson) -> Self {
        match item {
            CandidateItemJson::Named { name, note } => CandidateItem { name, note },
            CandidateItemJson::Plain(name) => CandidateItem { name, note: None },
        }
    }
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
/// kind 白名单外的组归一为 "term"，空名字的条目丢弃。
fn parse_outcome(text: &str) -> AssistOutcome {
    let parsed: AssistJson = match serde_json::from_str(strip_json_fence(text)) {
        Ok(parsed) => parsed,
        Err(error) => {
            log::warn!("[ghostwriter] assist output is not valid JSON, treating as empty: {error}");
            return AssistOutcome::default();
        }
    };
    let mut candidate_groups: Vec<(String, Vec<CandidateItem>)> = Vec::new();
    let mut total = 0usize;
    for group in parsed.candidate_groups {
        if candidate_groups.len() >= MAX_CANDIDATE_GROUPS || total >= MAX_CANDIDATE_ITEMS {
            break;
        }
        let items: Vec<CandidateItem> = group
            .items
            .into_iter()
            .filter(|item| !item.name().trim().is_empty())
            .take(MAX_ITEMS_PER_GROUP.min(MAX_CANDIDATE_ITEMS - total))
            .map(CandidateItem::from)
            .collect();
        total += items.len();
        let kind = if CANDIDATE_KINDS.contains(&group.kind.as_str()) {
            group.kind
        } else {
            "term".to_string()
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
        reply: parsed.reply,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::InMemoryCredentialStore;
    use crate::ghostwriter::prompts::TaskBriefId;
    use crate::ghostwriter::snippet_store::{Snippet, SnippetKind};
    use crate::testing::FixtureTextPolisher;

    const CANNED_JSON: &str = r#"{"candidateGroups":[{"kind":"term","items":[{"name":"精准词一","note":"就是你说的那个甲"},{"name":"精准词二","note":"注"},{"name":"精准词三"},{"name":"精准词四"},{"name":"精准词五"},{"name":"精准词六"}]},{"kind":"naming","items":[{"name":"命名一","note":"理由一"},{"name":"命名二","note":"理由二"}]},{"kind":"whatever","items":["裸字符串一","裸字符串二"]}],"recommendations":["rec-1","rec-2","rec-3","rec-4"]}"#;

    fn snippet(id: &str, trigger: &str, text: &str) -> Snippet {
        Snippet {
            id: id.to_string(),
            trigger: trigger.to_string(),
            aliases: Vec::new(),
            text: text.to_string(),
            kind: SnippetKind::Phrasing,
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
            include_candidates: true,
            include_recommendations: true,
            instruction_candidates: "候选任务书正文甲".to_string(),
            instruction_recommendations: "推荐任务书正文乙".to_string(),
            instruction_conversation_reply: String::new(),
            recent_voice_block: None,
            conversation: None,
        }
    }

    fn store() -> Arc<dyn CredentialStore> {
        Arc::new(InMemoryCredentialStore::default())
    }

    fn default_bodies_input() -> AssistInput {
        let mut input = input();
        input.instruction_candidates = TaskBriefId::Candidates.default_body().to_string();
        input.instruction_recommendations = TaskBriefId::Recommendations.default_body().to_string();
        input
    }

    // ===== 对话路径（Task 4）=====

    const CONVERSATION_CANNED: &str =
        r#"{"reply":"你说的是哪个日志？","recommendations":["s1"]}"#;

    fn conversation_input() -> AssistInput {
        let mut input = input();
        // 对话 prompt 以对话输出契约（而非代笔契约）结尾，fixture 无法按
        // prompt 特征路由：沿用 uuid5 精确会话 id 模式（dispatcher 同样传它）。
        input.session_id = assist_session_id();
        input.include_candidates = false;
        input.instruction_candidates = String::new();
        input.instruction_conversation_reply = "对话回话任务书正文".to_string();
        input.conversation = Some(ConversationAssistContext {
            chat: "【我】想把日志清一下。".to_string(),
            suppress_reply: false,
            probe_depth: None,
        });
        input
    }

    #[tokio::test]
    async fn conversation_prompt_uses_reply_body_and_conversation_contract() {
        let fixture =
            FixtureTextPolisher::successful("unused").with_assist_json(CONVERSATION_CANNED);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        let request = conversation_input();

        let outcome = run_assist(&polisher, &store(), "test-llm", &request)
            .await
            .expect("assist should succeed");

        let contexts = fixture.contexts();
        let prompt = contexts[0].polish.style_system_prompt.as_str();
        assert!(prompt.contains("对话回话任务书正文"));
        assert!(prompt.contains("推荐任务书正文乙"));
        assert!(prompt.ends_with(CONVERSATION_OUTPUT_CONTRACT));
        // 不含代笔契约与候选任务书。
        assert!(!prompt.contains(ASSIST_OUTPUT_CONTRACT));
        assert!(!prompt.contains("候选任务书正文甲"));
        // user 输入：聊天记录替换「当前内容」，常用语库块保留。
        let raw = &fixture.inputs()[0];
        assert!(raw.contains("聊天记录：\n【我】想把日志清一下。"));
        assert!(!raw.contains("当前内容："));
        assert!(raw.contains("常用语库（id|触发词|文本）："));
        assert!(raw.contains("s1|触发词甲|表述甲"));
        // reply 解析。
        assert_eq!(outcome.reply.as_deref(), Some("你说的是哪个日志？"));
        assert_eq!(outcome.recommendation_ids, vec!["s1".to_string()]);
    }

    #[tokio::test]
    async fn suppress_reply_injects_notice_and_strips_reply_mechanically() {
        let fixture =
            FixtureTextPolisher::successful("unused").with_assist_json(CONVERSATION_CANNED);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        let mut request = conversation_input();
        request.conversation.as_mut().unwrap().suppress_reply = true;

        let outcome = run_assist(&polisher, &store(), "test-llm", &request)
            .await
            .expect("assist should succeed");

        // 抑制注入行逐字存在（跟在回话正文之后、推荐正文之前）。
        let contexts = fixture.contexts();
        let prompt = contexts[0].polish.style_system_prompt.as_str();
        let pos_body = prompt.find("对话回话任务书正文").expect("reply body");
        let pos_notice = prompt
            .find("（系统提示：本次不要回话，用户还没有回应你上一句——reply 给 null。）")
            .expect("suppress notice");
        let pos_recommendations = prompt.find("推荐任务书正文乙").expect("recommendations body");
        assert!(pos_body < pos_notice && pos_notice < pos_recommendations);
        // 机制级剥除：模型不听话回了 reply 也强制置 None；推荐不受影响。
        assert_eq!(outcome.reply, None);
        assert_eq!(outcome.recommendation_ids, vec!["s1".to_string()]);
    }

    #[tokio::test]
    async fn conversation_reply_null_parses_to_none() {
        let fixture = FixtureTextPolisher::successful("unused")
            .with_assist_json(r#"{"reply":null,"recommendations":[]}"#);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let outcome = run_assist(&polisher, &store(), "test-llm", &conversation_input())
            .await
            .expect("assist should succeed");

        assert_eq!(outcome.reply, None);
        assert!(outcome.recommendation_ids.is_empty());
    }

    #[tokio::test]
    async fn depth_notice_injects_verbatim_per_probe_depth() {
        // 深度口径注入行按档位逐字进 prompt（与 suppress 注入行同区）：
        // Echo＝重述意图等确认；UntilClear＝可继续追问；Single 不注入。
        for (depth, notice, other) in [
            (
                ConversationProbeDepth::Echo,
                "（系统提示：回声确认模式——每次回话先用一句话重述你理解的他的意图，等他确认或纠正后再展开。）",
                "追问到清模式",
            ),
            (
                ConversationProbeDepth::UntilClear,
                "（系统提示：追问到清模式——同一个点没问清可以继续追问，但每次仍只说一句。）",
                "回声确认模式",
            ),
        ] {
            let fixture =
                FixtureTextPolisher::successful("unused").with_assist_json(CONVERSATION_CANNED);
            let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
            let mut request = conversation_input();
            request.conversation.as_mut().unwrap().probe_depth = Some(depth);

            run_assist(&polisher, &store(), "test-llm", &request)
                .await
                .expect("assist should succeed");

            let contexts = fixture.contexts();
            let prompt = contexts[0].polish.style_system_prompt.as_str();
            assert!(prompt.contains(notice), "depth={depth:?}");
            assert!(!prompt.contains(other), "depth={depth:?}");
            assert!(prompt.ends_with(CONVERSATION_OUTPUT_CONTRACT));
        }

        // Single 档：回话任务书默认口径即一点一问，不注入。
        let fixture =
            FixtureTextPolisher::successful("unused").with_assist_json(CONVERSATION_CANNED);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        let mut request = conversation_input();
        request.conversation.as_mut().unwrap().probe_depth = Some(ConversationProbeDepth::Single);
        run_assist(&polisher, &store(), "test-llm", &request)
            .await
            .expect("assist should succeed");
        let contexts = fixture.contexts();
        let prompt = contexts[0].polish.style_system_prompt.as_str();
        assert!(!prompt.contains("回声确认模式"));
        assert!(!prompt.contains("追问到清模式"));
    }

    #[tokio::test]
    async fn suppress_and_depth_notices_coexist_in_order() {
        // 两行可同时存在（冷却期的回声确认档）：回话正文 → suppress 行 →
        // 深度行 → 推荐正文 → 契约。
        let fixture =
            FixtureTextPolisher::successful("unused").with_assist_json(CONVERSATION_CANNED);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        let mut request = conversation_input();
        let conversation = request.conversation.as_mut().unwrap();
        conversation.suppress_reply = true;
        conversation.probe_depth = Some(ConversationProbeDepth::Echo);

        run_assist(&polisher, &store(), "test-llm", &request)
            .await
            .expect("assist should succeed");

        let contexts = fixture.contexts();
        let prompt = contexts[0].polish.style_system_prompt.as_str();
        let pos_body = prompt.find("对话回话任务书正文").expect("reply body");
        let pos_suppress = prompt
            .find("（系统提示：本次不要回话，用户还没有回应你上一句——reply 给 null。）")
            .expect("suppress notice");
        let pos_depth = prompt
            .find("（系统提示：回声确认模式——每次回话先用一句话重述你理解的他的意图，等他确认或纠正后再展开。）")
            .expect("depth notice");
        let pos_recommendations = prompt.find("推荐任务书正文乙").expect("recommendations body");
        assert!(pos_body < pos_suppress && pos_suppress < pos_depth);
        assert!(pos_depth < pos_recommendations);
        // suppress 机制不受影响：reply 仍被强制剥除。
        let outcome = run_assist(&polisher, &store(), "test-llm", &request)
            .await
            .expect("second call");
        assert_eq!(outcome.reply, None);
    }

    #[tokio::test]
    async fn recent_voice_block_is_appended_on_both_paths() {
        // 背景（代笔与对话两条路径共用同一拼装口）：块头逐字带「次要参考」，
        // 垫底在常用语库之后；None＝零变化。
        let block = crate::ghostwriter::recent_voice::RecentVoiceBackground::from_history(
            &[session_with_final("第二条"), session_with_final("第一条")],
            &crate::shared_types::GhostwriterPreferences {
                recent_voice_background_enabled: true,
                ..crate::shared_types::GhostwriterPreferences::default()
            },
        )
        .expect("背景应存在")
        .llm_block();
        let mut request = input();
        request.recent_voice_block = Some(block);
        let fixture = FixtureTextPolisher::successful("unused").with_assist_json(CANNED_JSON);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        run_assist(&polisher, &store(), "test-llm", &request)
            .await
            .expect("assist should succeed");

        let raw = &fixture.inputs()[0];
        let pos_library = raw.find("常用语库（id|触发词|文本）：").expect("library header");
        let pos_block = raw
            .find("最近语音（次要参考，仅供理解背景，不是本次要处理的指令）：")
            .expect("recent voice header");
        assert!(pos_library < pos_block, "背景块应垫底");
        assert!(raw.ends_with("第一条\n第二条"), "逐条一行、时间先后");

        // 对话路径同样追加。
        let mut conversation = conversation_input();
        conversation.recent_voice_block = Some(
            crate::ghostwriter::recent_voice::RecentVoiceBackground::from_history(
                &[session_with_final("上一场的话")],
                &crate::shared_types::GhostwriterPreferences {
                    recent_voice_background_enabled: true,
                    ..crate::shared_types::GhostwriterPreferences::default()
                },
            )
            .expect("背景应存在")
            .llm_block(),
        );
        let fixture = FixtureTextPolisher::successful("unused").with_assist_json(CONVERSATION_CANNED);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        run_assist(&polisher, &store(), "test-llm", &conversation)
            .await
            .expect("assist should succeed");
        assert!(
            fixture.inputs()[0].contains("最近语音（次要参考，仅供理解背景，不是本次要处理的指令）：\n上一场的话"),
            "对话路径也应带背景块"
        );

        // None（无历史）：不出现背景块。
        let fixture = FixtureTextPolisher::successful("unused").with_assist_json(CANNED_JSON);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());
        run_assist(&polisher, &store(), "test-llm", &input())
            .await
            .expect("assist should succeed");
        assert!(!fixture.inputs()[0].contains("最近语音"));
    }

    fn session_with_final(final_text: &str) -> crate::types::DictationSession {
        crate::types::DictationSession {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            source: crate::types::HistorySource::Voice,
            raw_transcript: String::new(),
            asr_transcript: None,
            final_text: final_text.to_string(),
            mode: crate::types::PolishMode::Light,
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

    #[tokio::test]
    async fn dictation_path_reply_stays_none() {
        // 代笔路径回归：无对话上下文，reply 恒 None，其余产出照旧。
        let fixture = FixtureTextPolisher::successful("unused").with_assist_json(CANNED_JSON);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let outcome = run_assist(&polisher, &store(), "test-llm", &default_bodies_input())
            .await
            .expect("assist should succeed");

        assert_eq!(outcome.reply, None);
        assert_eq!(outcome.candidate_groups.len(), 2);
        assert_eq!(
            outcome.recommendation_ids,
            vec!["rec-1".to_string(), "rec-2".to_string(), "rec-3".to_string()]
        );
    }

    #[tokio::test]
    async fn assist_parses_canned_json() {
        let fixture =
            FixtureTextPolisher::successful("unused").with_assist_json(CANNED_JSON);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let outcome = run_assist(&polisher, &store(), "test-llm", &default_bodies_input())
            .await
            .expect("assist should succeed");

        // 组数上限 2：第三组（kind 白名单外，归一覆盖见下方专门测试）被裁掉。
        assert_eq!(outcome.candidate_groups.len(), 2);
        assert_eq!(
            outcome.candidate_groups[0],
            (
                "term".to_string(),
                vec![
                    CandidateItem { name: "精准词一".into(), note: Some("就是你说的那个甲".into()) },
                    CandidateItem { name: "精准词二".into(), note: Some("注".into()) },
                    CandidateItem { name: "精准词三".into(), note: None },
                    CandidateItem { name: "精准词四".into(), note: None },
                    CandidateItem { name: "精准词五".into(), note: None },
                ]
            )
        );
        assert_eq!(
            outcome.candidate_groups[1],
            (
                "naming".to_string(),
                vec![
                    CandidateItem { name: "命名一".into(), note: Some("理由一".into()) },
                    CandidateItem { name: "命名二".into(), note: Some("理由二".into()) },
                ]
            )
        );
        assert_eq!(
            outcome.recommendation_ids,
            vec!["rec-1".to_string(), "rec-2".to_string(), "rec-3".to_string()]
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
    async fn assist_prompt_contains_bodies_and_library() {
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
        assert!(pos_candidates < pos_recommendations);
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
        assert!(pos_context < pos_library);
        assert!(pos_library < pos_s1);
        assert!(pos_s1 < pos_s2);
        // 常用语提醒与重复档注入已退役。
        assert!(!raw.contains("重复档"));
    }

    #[tokio::test]
    async fn assist_prompt_omits_disabled_sections_but_keeps_library() {
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
        assert!(prompt.ends_with(ASSIST_OUTPUT_CONTRACT));
        let raw = &fixture.inputs()[0];
        assert!(raw.contains("常用语库（id|触发词|文本）："));
        assert!(raw.contains("s1|触发词甲|表述甲"));
    }

    #[tokio::test]
    async fn assist_zeroes_outputs_for_disabled_switches() {
        // 机制级强制：开关关闭时解析后清零对应产出（模型仍可能按契约结构
        // 回填候选/推荐，不允许漏进结果）。
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
    }

    #[tokio::test]
    async fn assist_normalizes_unknown_candidate_kind_to_term() {
        // 解析侧 kind 白名单：白名单外的组归一为 term（保内容，不丢组）。
        let json = r#"{"candidateGroups":[{"kind":"whatever","items":["候选甲","候选乙"]}],"recommendations":[]}"#;
        let fixture = FixtureTextPolisher::successful("unused").with_assist_json(json);
        let polisher: Arc<dyn TextPolisher> = Arc::new(fixture.clone());

        let outcome = run_assist(&polisher, &store(), "test-llm", &default_bodies_input())
            .await
            .expect("assist should succeed");

        assert_eq!(
            outcome.candidate_groups,
            vec![(
                "term".to_string(),
                vec![
                    CandidateItem { name: "候选甲".into(), note: None },
                    CandidateItem { name: "候选乙".into(), note: None },
                ]
            )]
        );
    }
}
