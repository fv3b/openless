//! Fluid 段润色调度器：把会话产出的润色段派给 LLM，并把结果合回会话。
//!
//! 每个完成的段独立一个 tokio 任务（[`crate::fluid::segment_polisher::polish_segment`]）：
//! 成功后在状态锁内把结果合回对应会话（[`crate::fluid::session::FluidSession::apply_polished`]）
//! 并按同一把锁内的最新拼装发布 [`FluidPreviewChanged`]（保证预览事件按修订号升序发布）；
//! 失败时告警并发布 [`FluidNotice`]。会话可能已被取消/重置移除：找不到会话＝静默丢弃。
//! 尾段补润在 stop 路径同步 await：贴出前必须完成，失败回落尾巴原文（兜底追加仍在）。

use std::sync::{Arc, RwLock};

use crate::api::MutableState;
use crate::credentials::CredentialStore;
use crate::events::{BackendEventKind, EventBus};
use crate::ports::TextPolisher;
use crate::types::SessionId;

use super::segment_polisher::{SegmentPolishRequest, polish_segment};
use super::session::PolishableSegment;
use super::types::{FluidNotice, FluidPreviewChanged};

#[derive(Clone)]
pub struct FluidPolishDispatcher {
    state: Arc<RwLock<MutableState>>,
    events: Arc<EventBus>,
    polisher: Arc<dyn TextPolisher>,
    credential_store: Arc<dyn CredentialStore>,
    preferences: Arc<crate::PreferencesStore>,
}

impl FluidPolishDispatcher {
    pub(crate) fn new(
        state: Arc<RwLock<MutableState>>,
        events: Arc<EventBus>,
        polisher: Arc<dyn TextPolisher>,
        credential_store: Arc<dyn CredentialStore>,
        preferences: Arc<crate::PreferencesStore>,
    ) -> Self {
        Self {
            state,
            events,
            polisher,
            credential_store,
            preferences,
        }
    }

    /// 由段材料组装一次段润色请求；段会话 id 由 dispatcher 生成唯一新值。
    pub fn segment_request(&self, segment: &PolishableSegment) -> SegmentPolishRequest {
        SegmentPolishRequest {
            session_id: SessionId::new(),
            segment_index: segment.index,
            prior: segment.prior.clone(),
            segment: segment.text.clone(),
            materials: segment.materials.clone(),
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
                        log::warn!("[fluid] segment polish failed: {error}");
                        this.events.publish(
                            Some(session_id),
                            BackendEventKind::FluidNotice(FluidNotice {
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
                if let Some(session) = state.fluid_sessions.get_mut(&session_id) {
                    if session.apply_tail_polished(text) {
                        self.events.publish(
                            Some(session_id),
                            BackendEventKind::FluidPreviewChanged(FluidPreviewChanged {
                                text: session.assembled_text(),
                                revision: session.revision(),
                            }),
                        );
                    }
                }
            }
            Err(error) => log::warn!("[fluid] tail polish failed: {error}"),
        }
    }

    fn apply_segment(&self, session_id: SessionId, index: usize, text: String) {
        let mut state = self.state.write().expect("backend state lock poisoned");
        if let Some(session) = state.fluid_sessions.get_mut(&session_id) {
            if session.apply_polished(index, text) {
                self.events.publish(
                    Some(session_id),
                    BackendEventKind::FluidPreviewChanged(FluidPreviewChanged {
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
