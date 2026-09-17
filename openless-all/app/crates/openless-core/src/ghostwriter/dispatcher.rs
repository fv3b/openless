//! Ghostwriter 段润色调度器：把会话产出的润色段派给 LLM，并把结果合回会话；
//! 同时驱动实时助手（候选/推荐）。
//!
//! 每个完成的段独立一个 tokio 任务（[`crate::ghostwriter::segment_polisher::polish_segment`]）：
//! 成功后在状态锁内把结果合回对应会话（[`crate::ghostwriter::session::GhostwriterSession::apply_polished`]）
//! 并按同一把锁内的最新拼装发布 [`GhostwriterPreviewChanged`]（保证预览事件按修订号升序发布）；
//! 失败时告警并发布 [`GhostwriterNotice`]。会话可能已被取消/重置移除：找不到会话＝静默丢弃。
//! 尾段补润在 stop 路径同步 await：贴出前必须完成，失败回落尾巴原文（兜底追加仍在）。
//!
//! 实时助手（[`Self::maybe_trigger_assist`]）：对话会话优先分流——自动回话
//! 路径按会话回话门控决定是否抑制（冷却/封顶时代码注入抑制行，reply 由
//! assist 侧强制剥除），显式交话入口 [`Self::trigger_reply`] 绕过冷却；
//! 两者共用节流/在飞单飞机制，回话以 [`GhostwriterReplyChanged`] 落事件。
//! 普通会话照旧：prefs 门（候选/推荐全关直接返回）→ 候选节流（两次 assist
//! 间隔 ≥ candidate_throttle_ms）→ 在飞防叠（AtomicBool）→ spawn 组装输入
//! （缓冲尾部 [`ASSIST_CONTEXT_CHARS`]、启用常用语、任务书正文）并调
//! [`crate::ghostwriter::assist::run_assist`]。推荐节流独立
//! （recommendation_throttle_ms）且受推荐开关门控：开关关着不现取也不写锚点，
//! 未到点复用上次推荐填批次（推荐行不闪失），到点则现取并更新缓存（单槽即可
//! ——assist 天然单飞）。批次随 [`GhostwriterAssistChanged`] 发布，失败发
//! [`GhostwriterNotice`]（error）轻提示。节流计时一律走注入的 [`Clock`]（不读
//! 墙钟）；候选节流锚点在调用收尾写入，推荐节流锚点在现取时写入。常用语提醒
//! 与说话中自动提取已退役（2026-09-17 裁决）；按需批量提取见
//! [`Self::extract_snippet_candidates`]。
//!

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use crate::api::MutableState;
use crate::config::Clock;
use crate::credentials::CredentialStore;
use crate::events::{BackendEventKind, EventBus};
use crate::ports::TextPolisher;
use crate::shared_types::ConversationReplyTiming;
use crate::types::SessionId;

use super::assist::{AssistInput, ConversationAssistContext, run_assist};
use super::prompts::TaskBriefId;
use super::segment_polisher::{SegmentPolishRequest, polish_segment};
use super::session::PolishableSegment;
use super::snippet_store::Snippet;
use super::task_brief_store::TaskBriefStore;
use super::types::{
    AssistSnapshot, GhostwriterAssistChanged, GhostwriterCandidateGroup, GhostwriterCandidateItem,
    GhostwriterNotice, GhostwriterPreviewChanged, GhostwriterRecommendationItem,
    GhostwriterReplyChanged, LiveRecommendation, ReplyGate, SnippetDraft,
};

/// 静默触发阈值毫秒数：说话纯静默（无新增量）达到该值视为一次停顿
/// （api feed 点 spawn 的定时任务消费；锚点被更新即作废）。
pub const ASSIST_PAUSE_MS: u64 = 1500;

/// 助手上下文截取：说话缓冲尾部进 AssistInput 的最大字符数（dispatcher 侧截取）。
pub const ASSIST_CONTEXT_CHARS: usize = 800;

/// 助手触发原因（调用方判定，dispatcher 只管门/节流/在飞）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistTrigger {
    /// 说话静默 ≥ [`ASSIST_PAUSE_MS`]。
    Pause,
    /// 新完成段（句毕）。
    SegmentEnd,
}

#[derive(Clone)]
pub struct GhostwriterPolishDispatcher {
    state: Arc<RwLock<MutableState>>,
    events: Arc<EventBus>,
    polisher: Arc<dyn TextPolisher>,
    credential_store: Arc<dyn CredentialStore>,
    preferences: Arc<crate::PreferencesStore>,
    task_briefs: Arc<TaskBriefStore>,
    clock: Arc<dyn Clock>,
    /// 共享可变状态全部挂在 Arc 后：dispatcher 按 clone 派发进 tokio 任务，
    /// 各 clone 必须共享同一份节流锚点/在飞标记/缓存与建议（单飞语义）。
    assist_in_flight: Arc<AtomicBool>,
    last_assist: Arc<Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
    last_rec: Arc<Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
    /// 上一次现取的推荐批次（单槽）：推荐节流未到点时复用，避免推荐行闪失。
    last_recommendations: Arc<Mutex<Option<Vec<LiveRecommendation>>>>,
}

impl GhostwriterPolishDispatcher {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        state: Arc<RwLock<MutableState>>,
        events: Arc<EventBus>,
        polisher: Arc<dyn TextPolisher>,
        credential_store: Arc<dyn CredentialStore>,
        preferences: Arc<crate::PreferencesStore>,
        task_briefs: Arc<TaskBriefStore>,
            clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            state,
            events,
            polisher,
            credential_store,
            preferences,
            task_briefs,
            clock,
            assist_in_flight: Arc::new(AtomicBool::new(false)),
            last_assist: Arc::new(Mutex::new(None)),
            last_rec: Arc::new(Mutex::new(None)),
            last_recommendations: Arc::new(Mutex::new(None)),
        }
    }

    /// 由段材料组装一次段润色请求；段会话 id 由 dispatcher 生成唯一新值，
    /// 指令化任务书正文现取自任务书存储（保存即生效）。
    pub fn segment_request(&self, segment: &PolishableSegment) -> SegmentPolishRequest {
        SegmentPolishRequest {
            session_id: SessionId::new(),
            segment_index: segment.index,
            prior: segment.prior.clone(),
            segment: segment.text.clone(),
            materials: segment.materials.clone(),
            instruction: self.task_briefs.body(TaskBriefId::InstructionPolish),
        }
    }

    /// 派润一批说话中完成的段：每段一个 tokio 任务，结果合回会话。
    pub fn dispatch_segments(&self, session_id: SessionId, segments: Vec<PolishableSegment>) {
        for segment in segments {
            let request = self.segment_request(&segment);
            let index = segment.index;
            let this = self.clone();
            tokio::spawn(async move {
                match polish_segment(
                    &this.polisher,
                    &this.credential_store,
                    &this.active_llm_provider(),
                    &request,
                )
                .await
                {
                    Ok(text) => this.apply_segment(session_id, index, text),
                    Err(error) => {
                        log::warn!("[ghostwriter] segment polish failed: {error}");
                        this.events.publish(
                            Some(session_id),
                            BackendEventKind::GhostwriterNotice(GhostwriterNotice {
                                message: format!("段润色失败：{error}"),
                                level: "error".into(),
                            }),
                        );
                    }
                }
            });
        }
    }

    /// 尾段补润（stop 路径同步调用）：润色成功合回会话并发布预览；
    /// 失败仅告警，拼装回落尾巴原文、材料走兜底追加。
    pub async fn dispatch_tail(&self, session_id: SessionId, request: SegmentPolishRequest) {
        match polish_segment(
            &self.polisher,
            &self.credential_store,
            &self.active_llm_provider(),
            &request,
        )
        .await
        {
            Ok(text) => {
                let mut state = self.state.write().expect("backend state lock poisoned");
                if let Some(session) = state.ghostwriter_sessions.get_mut(&session_id) {
                    if session.apply_tail_polished(text) {
                        self.events.publish(
                            Some(session_id),
                            BackendEventKind::GhostwriterPreviewChanged(GhostwriterPreviewChanged {
                                text: session.assembled_text(),
                                revision: session.revision(),
                            }),
                        );
                    }
                }
            }
            Err(error) => {
                // 尾段失败不发布 GhostwriterNotice：此刻 stop 正在收尾，浮框已收起，
                // 没有可承接提示的面板；回落尾巴原文＋材料兜底追加即最终贴出。
                log::warn!("[ghostwriter] tail polish failed: {error}")
            }
        }
    }

    /// 触发一次实时助手（停顿/句毕调用）：对话会话走对话分支（自动回话路径，
    /// 回话门控决定是否抑制）；普通会话走 prefs 门（候选/推荐全关直接返回）→
    /// 候选节流 → 在飞防叠 → spawn 跑 [`run_assist`] 并合回批次。收尾（成败
    /// 都走）更新节流锚点、复位在飞标记并发布 [`GhostwriterAssistChanged`]。
    pub fn maybe_trigger_assist(&self, session_id: &SessionId, reason: AssistTrigger) {
        let prefs = self.preferences.get().ghostwriter;
        let conversational = {
            let state = self.state.read().expect("backend state lock poisoned");
            state
                .ghostwriter_sessions
                .get(session_id)
                .map(|session| session.conversational())
                .unwrap_or(false)
        };
        if conversational {
            // 对话会话：候选/推荐流开关只管代笔会话，不在此门控；
            // 停顿/句毕触发＝自动回话路径（显式交话时机下仅刷推荐）。
            self.spawn_conversation_assist(session_id, prefs, true);
            return;
        }
        if !prefs.candidates_enabled && !prefs.recommendations_enabled {
            return;
        }
        if !self.assist_throttle_passed(prefs.candidate_throttle_ms) {
            log::debug!("[ghostwriter] assist throttled ({reason:?})");
            return;
        }
        if self.assist_in_flight.swap(true, Ordering::AcqRel) {
            return;
        }
        let this = self.clone();
        let session_id = *session_id;
        tokio::spawn(async move {
            let result = this.run_assist_for_session(&session_id, &prefs).await;
            *this
                .last_assist
                .lock()
                .expect("assist throttle lock poisoned") = Some(this.clock.now_utc());
            this.assist_in_flight.store(false, Ordering::Release);
            match result {
                Ok(true) => this.refresh_assist_event(&session_id),
                Ok(false) => {}
                Err(error) => {
                    log::warn!("[ghostwriter] assist failed: {error}");
                    this.events.publish(
                        Some(session_id),
                        BackendEventKind::GhostwriterNotice(GhostwriterNotice {
                            message: "实时助手暂时不可用".into(),
                            level: "error".into(),
                        }),
                    );
                }
            }
        });
    }

    /// 显式交话入口（api 命令层调用）：会话须存在且为对话会话；auto=false
    /// 绕过冷却（封顶也只数自动回话）。同样发布回话事件/推荐事件。
    pub fn trigger_reply(&self, session_id: &SessionId) {
        let prefs = self.preferences.get().ghostwriter;
        let conversational = {
            let state = self.state.read().expect("backend state lock poisoned");
            state
                .ghostwriter_sessions
                .get(session_id)
                .map(|session| session.conversational())
                .unwrap_or(false)
        };
        if !conversational {
            log::debug!("[ghostwriter] trigger_reply ignored: session missing or not conversational");
            return;
        }
        self.spawn_conversation_assist(session_id, prefs, false);
    }

    /// 对话 assist 的 spawn 外壳（自动与显式共用）：节流/在飞防叠与代笔路径
    /// 共用同一套单飞机制；成功后照常刷新 assist 事件，失败发轻提示。
    fn spawn_conversation_assist(
        &self,
        session_id: &SessionId,
        prefs: crate::shared_types::GhostwriterPreferences,
        auto: bool,
    ) {
        // 候选节流只拦自动触发：显式交话（auto=false）不受 candidate_throttle_ms
        // 节流——热键是显式交话模式下 AI 开口的唯一途径，节流吞掉＝永远无回话；
        // 在飞防叠保留（防自动/显式叠飞）。
        if auto && !self.assist_throttle_passed(prefs.candidate_throttle_ms) {
            log::debug!("[ghostwriter] conversation assist throttled (auto={auto})");
            return;
        }
        if self.assist_in_flight.swap(true, Ordering::AcqRel) {
            return;
        }
        let this = self.clone();
        let session_id = *session_id;
        tokio::spawn(async move {
            let result = this
                .run_conversation_assist_for_session(&session_id, &prefs, auto)
                .await;
            *this
                .last_assist
                .lock()
                .expect("assist throttle lock poisoned") = Some(this.clock.now_utc());
            this.assist_in_flight.store(false, Ordering::Release);
            match result {
                Ok(true) => this.refresh_assist_event(&session_id),
                Ok(false) => {}
                Err(error) => {
                    log::warn!("[ghostwriter] conversation assist failed: {error}");
                    this.events.publish(
                        Some(session_id),
                        BackendEventKind::GhostwriterNotice(GhostwriterNotice {
                            message: "实时助手暂时不可用".into(),
                            level: "error".into(),
                        }),
                    );
                }
            }
        });
    }

    /// 跑一次对话 assist（回话＋推荐双产出）并合回会话。auto=true＝自动触发
    /// （回话门控：显式交话时机不自动回话、冷却/封顶时抑制——抑制由代码注入
    /// 行＋强制剥 reply 双保险落地）；auto=false＝显式交话（绕过冷却）。
    /// 回话 Some 且非空 → record_reply（计 auto 计数）＋发布回话事件；
    /// 推荐照旧合批次走 assist 事件。返回 Ok(false)＝会话已不存在（静默）。
    async fn run_conversation_assist_for_session(
        &self,
        session_id: &SessionId,
        prefs: &crate::shared_types::GhostwriterPreferences,
        auto: bool,
    ) -> Result<bool, crate::errors::BackendError> {
        // 状态锁内只读聊天记录与门控判定＋启用常用语。
        let (chat, suppress_reply, snippets) = {
            let state = self.state.read().expect("backend state lock poisoned");
            let Some(session) = state.ghostwriter_sessions.get(session_id) else {
                return Ok(false);
            };
            let suppress_reply = if auto {
                // 自动触发：显式交话时机不自动回话（机器契约），冷却/封顶时抑制。
                session.reply_timing() != ConversationReplyTiming::Pause
                    || session.reply_gate(true) != ReplyGate::Allow
            } else {
                // 显式交话绕过冷却，恒放行。
                false
            };
            (
                session.chat_transcript(),
                suppress_reply,
                state.ghostwriter_snippets.enabled(),
            )
        };
        let include_recommendations = prefs.conversation_recommendations
            && self.recommendation_recompute_due(prefs.recommendation_throttle_ms);
        let input = AssistInput {
            session_id: super::assist::assist_session_id(),
            context_text: String::new(),
            snippets: snippets.clone(),
            include_candidates: false,
            include_recommendations,
            instruction_candidates: String::new(),
            instruction_recommendations: self.task_briefs.body(TaskBriefId::Recommendations),
            instruction_conversation_reply: self.task_briefs.body(TaskBriefId::ConversationReply),
            conversation: Some(ConversationAssistContext {
                chat: chat.clone(),
                suppress_reply,
            }),
        };
        let outcome =
            run_assist(&self.polisher, &self.credential_store, &self.active_llm_provider(), &input)
                .await?;
        // 回话处理：Some 且非空白才动作（AI 的话不进待融、无撤销，只进聊天
        // 记录）；纯空白视为闭嘴，不入 chat、不发事件、不置冷却。
        if let Some(reply) = outcome
            .reply
            .clone()
            .filter(|text| !text.trim().is_empty())
        {
            {
                let mut state = self.state.write().expect("backend state lock poisoned");
                let Some(session) = state.ghostwriter_sessions.get_mut(session_id) else {
                    return Ok(false);
                };
                session.record_reply(reply.clone(), auto);
            }
            self.events.publish(
                Some(*session_id),
                BackendEventKind::GhostwriterReplyChanged(GhostwriterReplyChanged { text: reply }),
            );
        }
        let batch_recommendations = self.resolve_batch_recommendations(
            prefs.conversation_recommendations,
            include_recommendations,
            &outcome.recommendation_ids,
            &snippets,
        );
        {
            let mut state = self.state.write().expect("backend state lock poisoned");
            let Some(session) = state.ghostwriter_sessions.get_mut(session_id) else {
                return Ok(false);
            };
            session.set_live_batch(Vec::new(), batch_recommendations);
        }
        Ok(true)
    }

    /// 按 session 当前快照重发 [`GhostwriterAssistChanged`]（开关切换后调用）；
    /// 无批次 → 空数组。
    pub fn refresh_assist_event(&self, session_id: &SessionId) {
        let snapshot = {
            let state = self.state.read().expect("backend state lock poisoned");
            state
                .ghostwriter_sessions
                .get(session_id)
                .and_then(|session| session.assist_snapshot())
        };
        self.events.publish(
            Some(*session_id),
            BackendEventKind::GhostwriterAssistChanged(assist_changed_payload(
                snapshot.as_ref(),
            )),
        );
    }

    /// 按需批量提取候选常用语（常用语管理页触发）：把选中的历史转写交给
    /// [`super::snippet_extractor::extract_snippets`]（任务书正文现取自任务书
    /// 存储，会话 id 用固定提取 id，fixture 路由契约），返回可编辑草稿。
    pub async fn extract_snippet_candidates(
        &self,
        transcripts: Vec<String>,
    ) -> Result<Vec<SnippetDraft>, crate::errors::BackendError> {
        let instruction = self.task_briefs.body(TaskBriefId::SedimentExtraction);
        super::snippet_extractor::extract_snippets(
            &self.polisher,
            &self.credential_store,
            &self.active_llm_provider(),
            super::snippet_extractor::extraction_session_id(),
            &transcripts,
            &instruction,
        )
        .await
    }

    /// 任务书存储句柄（命令层读取/保存/恢复默认用；Arc 克隆共享同一份状态）。
    pub fn task_brief_store(&self) -> Arc<TaskBriefStore> {
        Arc::clone(&self.task_briefs)
    }

    /// 候选节流：从未跑过（None）直接放行；否则距上次 assist 收尾须 ≥ throttle_ms。
    fn assist_throttle_passed(&self, throttle_ms: u64) -> bool {
        let previous = *self
            .last_assist
            .lock()
            .expect("assist throttle lock poisoned");
        match previous {
            None => true,
            Some(previous) => {
                (self.clock.now_utc() - previous).num_milliseconds() >= throttle_ms as i64
            }
        }
    }

    /// 跑一次实时助手并合回会话。返回 Ok(true)＝成功（调用方发布刷新）、
    /// Ok(false)＝会话已不存在（静默）、Err＝调用失败（调用方发提示）。
    async fn run_assist_for_session(
        &self,
        session_id: &SessionId,
        prefs: &crate::shared_types::GhostwriterPreferences,
    ) -> Result<bool, crate::errors::BackendError> {
        // 组装输入：状态锁内只读会话缓冲与启用常用语，截取在 dispatcher 做。
        let (context_text, snippets) = {
            let state = self.state.read().expect("backend state lock poisoned");
            let Some(session) = state.ghostwriter_sessions.get(session_id) else {
                return Ok(false);
            };
            let text = session.debug_text();
            let skip = text.chars().count().saturating_sub(ASSIST_CONTEXT_CHARS);
            (
                text.chars().skip(skip).collect::<String>(),
                state.ghostwriter_snippets.enabled(),
            )
        };
        // 推荐节流独立计时＋推荐开关门（控制器裁决）：推荐开关开着且（首次
        // 或距上次现取 ≥ recommendation_throttle_ms）才现取；开关关闭时不现取
        // 也不写锚点（重新打开后按窗口语义自然现取）。
        let include_recommendations = prefs.recommendations_enabled
            && self.recommendation_recompute_due(prefs.recommendation_throttle_ms);
        let input = AssistInput {
            session_id: super::assist::assist_session_id(),
            context_text,
            snippets: snippets.clone(),
            include_candidates: prefs.candidates_enabled,
            include_recommendations,
            instruction_candidates: self.task_briefs.body(TaskBriefId::Candidates),
            instruction_recommendations: self.task_briefs.body(TaskBriefId::Recommendations),
            instruction_conversation_reply: String::new(),
            conversation: None,
        };
        let outcome =
            run_assist(&self.polisher, &self.credential_store, &self.active_llm_provider(), &input)
                .await?;
        let batch_recommendations = self.resolve_batch_recommendations(
            prefs.recommendations_enabled,
            include_recommendations,
            &outcome.recommendation_ids,
            &snippets,
        );
        // 批次合回：新批次即清上一批的选中（会话内语义）；会话可能已被移除。
        {
            let mut state = self.state.write().expect("backend state lock poisoned");
            let Some(session) = state.ghostwriter_sessions.get_mut(session_id) else {
                return Ok(false);
            };
            session.set_live_batch(outcome.candidate_groups, batch_recommendations);
        }
        Ok(true)
    }

    /// 推荐节流：首次（None）或距上次现取 ≥ throttle_ms 才现取；现取即写锚点
    /// （窗口自本次重算起计）。
    fn recommendation_recompute_due(&self, throttle_ms: u64) -> bool {
        let mut previous = self
            .last_rec
            .lock()
            .expect("recommendation throttle lock poisoned");
        let recompute = match *previous {
            None => true,
            Some(previous) => {
                (self.clock.now_utc() - previous).num_milliseconds() >= throttle_ms as i64
            }
        };
        if recompute {
            *previous = Some(self.clock.now_utc());
        }
        recompute
    }

    /// 推荐 id → 库内条目映射＋缓存决策 → 本次批次推荐（「推荐行不闪失」
    /// 语义集中在此）：开关关闭 → 空且不回填缓存；节流未到点或现取为空 →
    /// 沿用缓存；现取非空 → 更新缓存并采用。
    fn resolve_batch_recommendations(
        &self,
        recommendations_enabled: bool,
        include_recommendations: bool,
        recommendation_ids: &[String],
        snippets: &[Snippet],
    ) -> Vec<LiveRecommendation> {
        let recommendations: Vec<LiveRecommendation> = recommendation_ids
            .iter()
            .filter_map(|id| {
                snippets
                    .iter()
                    .find(|snippet| &snippet.id == id)
                    .map(|snippet| LiveRecommendation {
                        snippet_id: snippet.id.clone(),
                        title: snippet.trigger.clone(),
                        text: snippet.text.clone(),
                    })
            })
            .collect();
        if !recommendations_enabled {
            // 推荐开关关闭：批次不带推荐（缓存也不回填——关了就展示为空）。
            Vec::new()
        } else if !include_recommendations {
            // 推荐节流未到点：用缓存的上次推荐填批次（推荐行不闪失）。
            self.last_recommendations
                .lock()
                .expect("recommendation cache lock poisoned")
                .clone()
                .unwrap_or_default()
        } else if recommendations.is_empty() {
            // 现取结果为空（LLM 返回空推荐或 id 不在库内）：空结果不覆盖缓存、
            // 本次批次也沿用缓存——否则推荐行会闪失一次再凭旧缓存复活。
            self.last_recommendations
                .lock()
                .expect("recommendation cache lock poisoned")
                .clone()
                .unwrap_or_default()
        } else {
            *self
                .last_recommendations
                .lock()
                .expect("recommendation cache lock poisoned") = Some(recommendations.clone());
            recommendations
        }
    }

    fn apply_segment(&self, session_id: SessionId, index: usize, text: String) {
        let mut state = self.state.write().expect("backend state lock poisoned");
        if let Some(session) = state.ghostwriter_sessions.get_mut(&session_id) {
            if session.apply_polished(index, text) {
                self.events.publish(
                    Some(session_id),
                    BackendEventKind::GhostwriterPreviewChanged(GhostwriterPreviewChanged {
                        text: session.assembled_text(),
                        revision: session.revision(),
                    }),
                );
            }
        }
    }

    fn active_llm_provider(&self) -> String {
        self.preferences.get().active_llm_provider
    }
}

/// 会话批次视图＋建议 → 事件载荷；无批次 → 空数组＋建议（推荐行不闪失由
/// 批次侧缓存保证，这里只做视图到载荷的映射）。
fn assist_changed_payload(snapshot: Option<&AssistSnapshot>) -> GhostwriterAssistChanged {
    let Some(snapshot) = snapshot else {
        return GhostwriterAssistChanged {
            candidate_groups: Vec::new(),
            recommendations: Vec::new(),
        };
    };
    GhostwriterAssistChanged {
        candidate_groups: snapshot
            .candidate_groups
            .iter()
            .map(|group| GhostwriterCandidateGroup {
                kind: group.kind.clone(),
                items: group
                    .items
                    .iter()
                    .map(|item| GhostwriterCandidateItem {
                        index: item.index as u32,
                        text: item.text.clone(),
                        note: item.note.clone(),
                    })
                    .collect(),
            })
            .collect(),
        recommendations: snapshot
            .recommendations
            .iter()
            .map(|recommendation| GhostwriterRecommendationItem {
                snippet_id: recommendation.snippet_id.clone(),
                title: recommendation.title.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use crate::credentials::InMemoryCredentialStore;
    use crate::dictation_context::DictationContext;
    use crate::events::EventSubscription;
    use crate::ghostwriter::session::GhostwriterSession;
    use crate::ghostwriter::snippet_store::{Snippet, SnippetKind};
    use crate::ghostwriter::types::ReplyGate;
    use crate::ports::{PolishOutput, TextStreamSink};
    use crate::shared_types::{
        ConversationProbeDepth, ConversationReplyTiming, GhostwriterPreferences,
    };
    use crate::types::TranscriptDelta;

    /// 测试时钟：now 可推进，供节流窗口断言（不读墙钟，全走注入 clock）。
    struct MutableClock {
        now: Mutex<chrono::DateTime<chrono::Utc>>,
    }

    impl MutableClock {
        fn new() -> Self {
            Self {
                now: Mutex::new(chrono::Utc::now()),
            }
        }

        fn advance_ms(&self, ms: u64) {
            *self.now.lock().expect("clock lock poisoned") +=
                chrono::Duration::milliseconds(ms as i64);
        }
    }

    impl crate::config::Clock for MutableClock {
        fn now_utc(&self) -> chrono::DateTime<chrono::Utc> {
            *self.now.lock().expect("clock lock poisoned")
        }

        fn today_local(&self) -> chrono::NaiveDate {
            self.now_utc().date_naive()
        }
    }

    /// 脚本化润色器：按调用序吐出预置响应，记录每次调用的 (session_id, context)。
    struct ScriptedPolisher {
        responses: Mutex<VecDeque<PolishOutput>>,
        calls: Arc<Mutex<Vec<(SessionId, Arc<DictationContext>)>>>,
    }

    impl ScriptedPolisher {
        fn scripted(responses: Vec<PolishOutput>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<(SessionId, Arc<DictationContext>)> {
            self.calls.lock().expect("calls lock poisoned").clone()
        }
    }

    impl TextPolisher for ScriptedPolisher {
        fn polish(
            &self,
            session_id: SessionId,
            context: Arc<DictationContext>,
            _raw_text: String,
            _partials: Arc<dyn TextStreamSink>,
        ) -> futures_util::future::BoxFuture<'static, Result<PolishOutput, crate::errors::BackendError>>
        {
            self.calls
                .lock()
                .expect("calls lock poisoned")
                .push((session_id, Arc::clone(&context)));
            let response = self
                .responses
                .lock()
                .expect("responses lock poisoned")
                .pop_front()
                .expect("unexpected polish call");
            Box::pin(async move { Ok(response) })
        }

        fn cancel(
            &self,
            _session_id: SessionId,
        ) -> futures_util::future::BoxFuture<'static, Result<(), crate::errors::BackendError>>
        {
            Box::pin(async { Ok(()) })
        }
    }

    struct Harness {
        dispatcher: GhostwriterPolishDispatcher,
        events: Arc<EventBus>,
        clock: Arc<MutableClock>,
        polisher: Arc<ScriptedPolisher>,
        #[allow(dead_code)]
        state: Arc<RwLock<MutableState>>,
        session_id: SessionId,
        dir: std::path::PathBuf,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn harness(throttles: (u64, u64), responses: Vec<&str>) -> Harness {
        harness_with(throttles, responses, None)
    }

    /// 对话会话 harness：会话带对话标志，且已喂出一段完成段
    /// （聊天记录有【我】行）。
    fn conversational_harness(
        throttles: (u64, u64),
        responses: Vec<&str>,
        depth: ConversationProbeDepth,
    ) -> Harness {
        harness_with(throttles, responses, Some(depth))
    }

    fn harness_with(
        throttles: (u64, u64),
        responses: Vec<&str>,
        conversation_depth: Option<ConversationProbeDepth>,
    ) -> Harness {
        let dir = std::env::temp_dir().join(format!(
            "openless-ghostwriter-dispatcher-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let preferences = Arc::new(
            crate::PreferencesStore::open(dir.join("preferences.json")).unwrap(),
        );
        let mut user_prefs = crate::shared_types::UserPreferences::default();
        user_prefs.ghostwriter = GhostwriterPreferences {
            candidates_enabled: true,
            recommendations_enabled: true,
            candidate_throttle_ms: throttles.0,
            recommendation_throttle_ms: throttles.1,
            ..GhostwriterPreferences::default()
        };
        preferences.set(user_prefs).unwrap();
        let snippets = crate::ghostwriter::snippet_store::SnippetStore::in_memory();
        snippets
            .create(Snippet {
                id: "s-rec".into(),
                trigger: "推荐触发词".into(),
                aliases: Vec::new(),
                text: "推荐常用语的完整表述文本".into(),
                kind: SnippetKind::Phrasing,
                attachments: Vec::new(),
                enabled: true,
            })
            .unwrap();
        let state = Arc::new(RwLock::new(MutableState::for_ghostwriter_tests(snippets)));
        let session_id = SessionId::new();
        {
            let mut guard = state.write().expect("state lock poisoned");
            let mut session = GhostwriterSession::new();
            match conversation_depth {
                Some(depth) => {
                    session = session.with_conversation(ConversationReplyTiming::Pause, depth);
                    session
                        .feed(
                            &TranscriptDelta {
                                text: "把日志清一下。后面".into(),
                                offset: 0,
                                is_final: false,
                            },
                            &[],
                        )
                        .unwrap();
                }
                None => {
                    session
                        .feed(
                            &TranscriptDelta {
                                text: "把日志清一下就是那种缓存".into(),
                                offset: 0,
                                is_final: true,
                            },
                            &[],
                        )
                        .unwrap();
                }
            }
            guard.ghostwriter_sessions.insert(session_id, session);
        }
        let events = Arc::new(EventBus::new(64));
        let clock = Arc::new(MutableClock::new());
        let polisher = Arc::new(ScriptedPolisher::scripted(
            responses
                .into_iter()
                .map(PolishOutput::text)
                .collect::<Vec<_>>(),
        ));
        let dispatcher = GhostwriterPolishDispatcher::new(
            Arc::clone(&state),
            Arc::clone(&events),
            Arc::clone(&polisher) as Arc<dyn TextPolisher>,
            Arc::new(InMemoryCredentialStore::default()),
            preferences,
            Arc::new(TaskBriefStore::in_memory()),
            Arc::clone(&clock) as Arc<dyn Clock>,
        );
        Harness {
            dispatcher,
            events,
            clock,
            polisher,
            state,
            session_id,
            dir,
        }
    }

    async fn next_assist_changed(events: &mut EventSubscription) -> GhostwriterAssistChanged {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let event = tokio::time::timeout_at(deadline, events.recv())
                .await
                .expect("assist event did not arrive in time")
                .expect("event stream closed");
            if let BackendEventKind::GhostwriterAssistChanged(payload) = event.kind {
                return payload;
            }
        }
    }

    async fn next_reply_changed(events: &mut EventSubscription) -> GhostwriterReplyChanged {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let event = tokio::time::timeout_at(deadline, events.recv())
                .await
                .expect("reply event did not arrive in time")
                .expect("event stream closed");
            if let BackendEventKind::GhostwriterReplyChanged(payload) = event.kind {
                return payload;
            }
        }
    }

    async fn expect_no_more_events(events: &mut EventSubscription) {
        let probe = tokio::time::timeout(std::time::Duration::from_millis(150), events.recv()).await;
        assert!(probe.is_err(), "不应再有事件到达");
    }

    fn chat_transcript_of(harness: &Harness) -> String {
        harness
            .state
            .read()
            .expect("state lock poisoned")
            .ghostwriter_sessions
            .get(&harness.session_id)
            .expect("session exists")
            .chat_transcript()
    }

    // ===== 对话会话触发分支与显式交话（Task 5）=====

    #[tokio::test]
    async fn conversation_pause_trigger_records_reply_and_publishes_event() {
        let harness = conversational_harness(
            (0, 0),
            vec![r#"{"reply":"你说的是哪个日志？","recommendations":["s-rec"]}"#],
            ConversationProbeDepth::Single,
        );
        let mut events = harness.events.subscribe();

        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::Pause);
        let reply = next_reply_changed(&mut events).await;
        assert_eq!(reply.text, "你说的是哪个日志？");
        let assist = next_assist_changed(&mut events).await;
        assert_eq!(assist.recommendations.len(), 1);

        // 调用走 assistant 固定 uuid5 会话 id（fixture 路由契约）。
        assert_eq!(
            harness.polisher.calls()[0].0,
            super::super::assist::assist_session_id()
        );
        // 会话已记【助手】行；一点一问冷却已生效。
        assert!(chat_transcript_of(&harness).contains("【助手】你说的是哪个日志？"));
        let state = harness.state.read().expect("state lock poisoned");
        let session = state.ghostwriter_sessions.get(&harness.session_id).unwrap();
        assert_eq!(session.reply_gate(true), ReplyGate::Cooldown);
    }

    #[tokio::test]
    async fn conversation_cooldown_suppresses_reply_but_still_recommends() {
        let harness = conversational_harness(
            (0, 0),
            vec![
                r#"{"reply":"第一问","recommendations":["s-rec"]}"#,
                r#"{"reply":"模型不听话的第二问","recommendations":["s-rec"]}"#,
            ],
            ConversationProbeDepth::Single,
        );
        let mut events = harness.events.subscribe();

        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::Pause);
        let _ = next_reply_changed(&mut events).await;
        let _ = next_assist_changed(&mut events).await;

        harness.clock.advance_ms(1);
        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::Pause);
        // 冷却期照常调用出推荐。
        let second = next_assist_changed(&mut events).await;
        assert_eq!(second.recommendations.len(), 1);
        // canned 明明带 reply：回话事件不发、会话未记。
        expect_no_more_events(&mut events).await;
        assert_eq!(chat_transcript_of(&harness).matches("【助手】").count(), 1);
        // 机制级：第二次调用的 prompt 带抑制注入行。
        assert!(harness.polisher.calls()[1]
            .1
            .polish
            .style_system_prompt
            .contains("（系统提示：本次不要回话，用户还没有回应你上一句——reply 给 null。）"));
    }

    #[tokio::test]
    async fn conversation_cap_suppresses_auto_reply_but_still_recommends() {
        let harness = conversational_harness(
            (0, 0),
            vec![r#"{"reply":"不该出现的第六问","recommendations":["s-rec"]}"#],
            ConversationProbeDepth::UntilClear,
        );
        {
            let mut guard = harness.state.write().expect("state lock poisoned");
            let session = guard.ghostwriter_sessions.get_mut(&harness.session_id).unwrap();
            for i in 0..5 {
                session.record_reply(format!("历史第{}问", i + 1), true);
            }
        }
        let mut events = harness.events.subscribe();

        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::Pause);
        // 封顶照常出推荐；canned 带 reply 也不发回话事件。
        let assist = next_assist_changed(&mut events).await;
        assert_eq!(assist.recommendations.len(), 1);
        expect_no_more_events(&mut events).await;
        assert!(chat_transcript_of(&harness).contains("【助手】历史第5问"));
        assert!(!chat_transcript_of(&harness).contains("不该出现的第六问"));
        assert!(harness.polisher.calls()[0]
            .1
            .polish
            .style_system_prompt
            .contains("（系统提示：本次不要回话，用户还没有回应你上一句——reply 给 null。）"));
    }

    #[tokio::test]
    async fn explicit_trigger_reply_bypasses_cooldown_and_skips_auto_count() {
        // 追问到清：直接记 4 次自动回话，再显式交话——显式放行且不计入封顶，
        // 因此下一次自动触发仍在封顶内放行。
        let harness = conversational_harness(
            (0, 0),
            vec![
                r#"{"reply":"显式交的问","recommendations":["s-rec"]}"#,
                r#"{"reply":"第5次自动问","recommendations":["s-rec"]}"#,
            ],
            ConversationProbeDepth::UntilClear,
        );
        {
            let mut guard = harness.state.write().expect("state lock poisoned");
            let session = guard.ghostwriter_sessions.get_mut(&harness.session_id).unwrap();
            for i in 0..4 {
                session.record_reply(format!("历史第{}问", i + 1), true);
            }
        }
        let mut events = harness.events.subscribe();

        harness.dispatcher.trigger_reply(&harness.session_id);
        let reply = next_reply_changed(&mut events).await;
        assert_eq!(reply.text, "显式交的问");
        let _ = next_assist_changed(&mut events).await;

        harness.clock.advance_ms(1);
        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::Pause);
        let reply = next_reply_changed(&mut events).await;
        assert_eq!(reply.text, "第5次自动问");
        assert_eq!(chat_transcript_of(&harness).matches("【助手】").count(), 6);
    }

    #[tokio::test]
    async fn trigger_reply_ignores_non_conversational_session() {
        let harness = harness((0, 0), vec![]);
        let mut events = harness.events.subscribe();

        harness.dispatcher.trigger_reply(&harness.session_id);
        expect_no_more_events(&mut events).await;
        assert!(harness.polisher.calls().is_empty());
        assert_eq!(chat_transcript_of(&harness), "");
    }

    #[tokio::test]
    async fn explicit_trigger_reply_bypasses_candidate_throttle() {
        // 推荐调用刚收尾（节流窗口 2000ms 未过、时钟不推进）立即显式交话：
        // 显式触发不受 candidate_throttle_ms 节流——热键是显式交话模式下 AI
        // 开口的唯一途径，吞掉＝永远无回话。回话事件必须发出。
        let harness = conversational_harness(
            (2000, 2000),
            vec![
                r#"{"reply":"第一问","recommendations":["s-rec"]}"#,
                r#"{"reply":"显式交的问","recommendations":["s-rec"]}"#,
            ],
            ConversationProbeDepth::Single,
        );
        let mut events = harness.events.subscribe();

        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::Pause);
        let _ = next_reply_changed(&mut events).await;
        let _ = next_assist_changed(&mut events).await;

        // 时钟不推进：距上次 assist 收尾 0ms < 2000ms 节流窗口。
        harness.dispatcher.trigger_reply(&harness.session_id);
        let reply = next_reply_changed(&mut events).await;
        assert_eq!(reply.text, "显式交的问");
        assert_eq!(chat_transcript_of(&harness).matches("【助手】").count(), 2);
    }

    #[tokio::test]
    async fn blank_reply_is_treated_as_silence() {
        // 纯空白 reply＝闭嘴：不入聊天记录、不发回话事件、不置冷却。
        let harness = conversational_harness(
            (0, 0),
            vec![r#"{"reply":"   ","recommendations":["s-rec"]}"#],
            ConversationProbeDepth::Single,
        );
        let mut events = harness.events.subscribe();

        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::Pause);
        let assist = next_assist_changed(&mut events).await;
        assert_eq!(assist.recommendations.len(), 1);
        expect_no_more_events(&mut events).await;
        assert!(!chat_transcript_of(&harness).contains("【助手】"));
        let state = harness.state.read().expect("state lock poisoned");
        let session = state.ghostwriter_sessions.get(&harness.session_id).unwrap();
        assert_eq!(session.reply_gate(true), ReplyGate::Allow);
    }

    #[tokio::test]
    async fn dictation_session_never_publishes_reply() {
        // 普通会话回归：canned 带 reply 键也不发回话事件、聊天记录恒空。
        let harness = harness(
            (0, 0),
            vec![r#"{"reply":"普通会话不该有回话","recommendations":["s-rec"]}"#],
        );
        let mut events = harness.events.subscribe();

        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::SegmentEnd);
        let assist = next_assist_changed(&mut events).await;
        assert_eq!(assist.recommendations.len(), 1);
        expect_no_more_events(&mut events).await;
        assert_eq!(chat_transcript_of(&harness), "");
    }

    #[tokio::test]
    async fn recommendation_throttle_gates_recompute_and_reuses_cache() {
        // 推荐节流（1000ms）内第二次触发：include_recommendations=false——
        // system prompt 不含推荐任务书，推荐行仍非空（来自缓存）。
        let harness = harness(
            (0, 1000),
            vec![
                r#"{"candidateGroups":[],"recommendations":["s-rec"]}"#,
                r#"{"candidateGroups":[],"recommendations":["s-rec"]}"#,
            ],
        );
        let mut events = harness.events.subscribe();

        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::SegmentEnd);
        let first = next_assist_changed(&mut events).await;
        assert_eq!(first.recommendations.len(), 1);
        assert_eq!(first.recommendations[0].snippet_id, "s-rec");
        let (session_id, context) = &harness.polisher.calls()[0];
        assert_eq!(*session_id, super::super::assist::assist_session_id());
        assert!(context
            .polish
            .style_system_prompt
            .contains(TaskBriefId::Recommendations.default_body()));

        harness.clock.advance_ms(400);
        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::SegmentEnd);
        let second = next_assist_changed(&mut events).await;
        // 节流生效：第二次现取被拦，推荐来自缓存且行不闪失。
        assert_eq!(second.recommendations.len(), 1);
        assert_eq!(second.recommendations[0].snippet_id, "s-rec");
        assert_eq!(harness.polisher.calls().len(), 2);
        let (_, context) = &harness.polisher.calls()[1];
        assert!(!context
            .polish
            .style_system_prompt
            .contains(TaskBriefId::Recommendations.default_body()));
    }

    #[tokio::test]
    async fn empty_recompute_does_not_clobber_cached_recommendations() {
        // 推荐节流（0ms）放行第二次现取：LLM 返回库外 id → 映射为空 →
        // 空结果不覆盖缓存、本次批次沿用缓存（推荐行不闪失）。
        let harness = harness(
            (0, 0),
            vec![
                r#"{"candidateGroups":[],"recommendations":["s-rec"]}"#,
                r#"{"candidateGroups":[],"recommendations":["ghost-unknown-id"]}"#,
            ],
        );
        let mut events = harness.events.subscribe();

        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::SegmentEnd);
        let first = next_assist_changed(&mut events).await;
        assert_eq!(first.recommendations.len(), 1);
        assert!(harness.polisher.calls()[0]
            .1
            .polish
            .style_system_prompt
            .contains(TaskBriefId::Recommendations.default_body()));

        harness.clock.advance_ms(1);
        harness
            .dispatcher
            .maybe_trigger_assist(&harness.session_id, AssistTrigger::SegmentEnd);
        let second = next_assist_changed(&mut events).await;
        // 第二次确实走了现取（prompt 含推荐任务书），但空结果没有清空推荐行。
        assert!(harness.polisher.calls()[1]
            .1
            .polish
            .style_system_prompt
            .contains(TaskBriefId::Recommendations.default_body()));
        assert_eq!(second.recommendations.len(), 1);
        assert_eq!(second.recommendations[0].snippet_id, "s-rec");
        assert_eq!(second.recommendations[0].title, "推荐触发词");
    }
}
