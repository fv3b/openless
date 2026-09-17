//! GhostwriterSession：一次听写会话的流式缓冲、段润色对齐与最终拼装。
//!
//! 自适应缓冲：不依赖 [`TranscriptAccumulator`]（绝对替换位语义，
//! 会把火山/讯飞的后缀增量 partial 拼碎），自持完整缓冲文本，
//! 按 delta 形态判型：
//! - final：全量替换（offset 忽略）；
//! - partial offset:0 且缓冲是 text 前缀：全量快照型 → 整体替换；
//! - partial offset:0 其余：后缀增量型（火山/讯飞实测形态）→ 追加；
//! - partial offset>0：替换式修订 → 保留前缀后写入。
//! 说话中缓冲始终是完整累计文本，segmenter 断句可用。
//! M2：新完成段产出 [`PolishableSegment`]（已润前文尾部＋待融材料随段转移），
//! 命中扫描 trigger/aliases（会话内去重，撤销后可再生效），
//! [`GhostwriterSession::assembled_text`] 是指令预览与最终贴出的唯一同源拼装。
//! M3：live assist 批次（候选＋推荐）进浮框候选区展示。候选与推荐均纯展示
//! （2026-09-17 裁决）：不可点选、无口头命令（「用候选N/用常用语N」等说法
//! 一律当普通话保留）；用户看到提示自己说出来，触发词命中/润色自然吸收。
//! 命中混入统一动作序，[`GhostwriterSession::cancel_last_action`] 取最新撤销。

use std::collections::HashSet;

use crate::errors::BackendError;
use crate::shared_types::{ConversationProbeDepth, ConversationReplyTiming};
use crate::types::TranscriptDelta;

use super::segmenter::{Segment, Segmenter};
use super::snippet_store::{Snippet, SnippetAttachment, SnippetKind, SnippetPlacement};
use super::types::{
    CandidateGroupView, CandidateItem, CandidateItemView, ChatRole, ChatTurn,
    GhostwriterSnippetHit, LiveRecommendation, RecommendationView, ReplyGate,
};

pub use super::types::{FeedOutcome, PolishableSegment};
pub use super::types::{AssistSnapshot, LastAction};

/// 无标点尾巴的硬切阈值（字符数）：ASR 长时间不出标点时按长度断段。
const DEFAULT_MAX_FORCE_CHARS: usize = 120;

/// prior 截断长度：已润前文只带尾部 200 字符进润色 prompt。
const PRIOR_MAX_CHARS: usize = 200;

/// 追问到清深度的自动回话总数封顶（机器契约：默认 5，只数自动回话）。
const AUTO_REPLY_CAP: usize = 5;

/// 一条已生效且未撤销的命中：背景块、待融材料与会话内去重的共同依据。
#[derive(Debug, Clone)]
struct ActiveHit {
    hit: GhostwriterSnippetHit,
    /// 待融材料（表述类命中的全量文本；背景类为 None）。
    material: Option<String>,
    /// 背景块行（标签, 文本）× 多行，按生效顺序；头/尾由会话的全局落点现算。
    background_lines: Vec<(String, String)>,
    /// 待融材料是否仍在待融队列（随段/尾巴润色转移后置 false，撤销不再出队）。
    material_pending: bool,
    /// 本次命中登记的背景去重键；撤销时释放，同 snippet 再说到可再贴。
    dedup_keys: Vec<String>,
}

/// 统一动作序的条目：按生效先后记录，撤销取最新。
/// 候选与推荐均纯展示（2026-09-17 裁决），选中子系统已整体移除。
#[derive(Debug, Clone)]
enum ActiveAction {
    Hit(ActiveHit),
}

/// 当前 live assist 批次：现场候选（按组，组别随批次携带）与推荐常用语。
#[derive(Debug, Clone)]
struct LiveBatch {
    candidates: Vec<(String, Vec<CandidateItem>)>,
    recommendations: Vec<LiveRecommendation>,
}

pub struct GhostwriterSession {
    /// 说话中的完整累计文本（自适应缓冲）。
    buffer: String,
    /// 最近一次 partial 应用后缓冲的字符长度。
    #[allow(dead_code)]
    last_partial_len: usize,
    segmenter: Segmenter,
    segments: Vec<Segment>,
    /// 与 segments 对齐的润色结果；None＝未润（拼装回落段原文）。
    polished: Vec<Option<String>>,
    /// 尾巴补润结果；None＝拼装回落尾巴原文。
    tail_polished: Option<String>,
    revision: u64,
    /// 统一动作序：已生效未撤销的命中按生效顺序排列；
    /// 命中同时是 snippet_id 去重依据。
    active_actions: Vec<ActiveAction>,
    /// 当前 live assist 批次；None＝此刻没有展示中的批次。
    live_batch: Option<LiveBatch>,
    /// inline 材料待融队列（先进先出，随下一段或尾巴补润转移）。
    inline_pending: Vec<String>,
    /// 已贴背景的去重键（背景类/引用项＝snippet id、手写项＝归一化文本）：
    /// 同一背景一次会话只贴一遍，触发命中、被引用、被多条表述引用互相生效。
    background_applied: HashSet<String>,
    /// 缓冲中已做过命中扫描的字符数（只扫增量，改写旧文的替换重扫）。
    scanned_chars: usize,
    /// 撤销过且尚未被新话再触发的 snippet：旧文领地（重扫）保持死亡，
    /// 新话增量仍可再触发（触发即出集，规则 6 的撤销-再生效交互）。
    cancelled_once: HashSet<String>,
    /// 最近一次尾巴补润成功时覆盖的尾巴字符长度；尾巴长度一变即失配，
    /// 旧润色结果对不上新尾巴，作废回落原文。
    tail_polished_covered: usize,
    /// 全局背景落点（源自 Ghostwriter 偏好 backgroundPlacement）：
    /// 所有背景块（背景类命中＋表述附件）拼装时按它现算头/尾位置；
    /// 设置变更经 [`Self::set_background_placement`] 即时生效。
    background_placement: SnippetPlacement,
    /// 对话模式标志：true＝对话会话（聊天记录与回话门控生效）；普通会话恒
    /// false，一切行为照旧（chat 恒空）。
    conversational: bool,
    /// 回话时机（创建时从偏好冻结）：自动触发与显式交话的行为分流依据。
    reply_timing: ConversationReplyTiming,
    /// 追问深度（创建时从偏好冻结）：回话门控的机制依据。
    probe_depth: ConversationProbeDepth,
    /// 聊天记录：【我】/【助手】按发生顺序混排（仅对话会话维护）。
    chat: Vec<ChatTurn>,
    /// 自动回话冷却：AI 回话后置位，用户新段完成即解除（一点一问/回声确认）。
    reply_cooldown: bool,
    /// 本次会话已发生的自动回话数（封顶只数自动）。
    auto_reply_count: usize,
}

impl Default for GhostwriterSession {
    fn default() -> Self {
        Self::new()
    }
}

impl GhostwriterSession {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            last_partial_len: 0,
            segmenter: Segmenter::new(DEFAULT_MAX_FORCE_CHARS),
            segments: Vec::new(),
            polished: Vec::new(),
            tail_polished: None,
            revision: 0,
            active_actions: Vec::new(),
            live_batch: None,
            inline_pending: Vec::new(),
            background_applied: HashSet::new(),
            scanned_chars: 0,
            cancelled_once: HashSet::new(),
            tail_polished_covered: 0,
            background_placement: SnippetPlacement::default(),
            conversational: false,
            reply_timing: ConversationReplyTiming::default(),
            probe_depth: ConversationProbeDepth::default(),
            chat: Vec::new(),
            reply_cooldown: false,
            auto_reply_count: 0,
        }
    }

    /// 创建时指定全局背景落点（api 从偏好取值传入）；不传按默认文末。
    pub fn with_background_placement(mut self, placement: SnippetPlacement) -> Self {
        self.background_placement = placement;
        self
    }

    /// 更新全局背景落点（偏好保存后对存活会话即时生效）：
    /// 拼装现算，已生效的背景块位置随之挪，无需其他处理。
    pub fn set_background_placement(&mut self, placement: SnippetPlacement) {
        self.background_placement = placement;
    }

    // ===== 对话会话：聊天记录、模式标志、回话门控 =====

    /// 创建对话会话：聊天记录与回话门控自此生效；回话时机与追问深度从偏好
    /// 取值冻结进会话。不调用即普通代笔会话，一切行为照旧。
    pub fn with_conversation(
        mut self,
        timing: ConversationReplyTiming,
        depth: ConversationProbeDepth,
    ) -> Self {
        self.conversational = true;
        self.reply_timing = timing;
        self.probe_depth = depth;
        self
    }

    /// 是否对话会话。
    pub fn conversational(&self) -> bool {
        self.conversational
    }

    /// 创建时冻结的回话时机（dispatcher 据此区分自动触发与显式交话）。
    pub fn reply_timing(&self) -> ConversationReplyTiming {
        self.reply_timing
    }

    /// 回话门控（机制级，不靠模型自觉）：显式交话（auto=false）恒放行——
    /// 绕过冷却、封顶只数自动回话；自动触发（auto=true）——一点一问/回声
    /// 确认下冷却激活即 [`ReplyGate::Cooldown`]（用户新段完成即解除），
    /// 追问到清下自动回话数达 [`AUTO_REPLY_CAP`] 即 [`ReplyGate::Cap`]；
    /// 其余放行。
    pub fn reply_gate(&self, auto: bool) -> ReplyGate {
        if !auto {
            return ReplyGate::Allow;
        }
        match self.probe_depth {
            ConversationProbeDepth::Single | ConversationProbeDepth::Echo => {
                if self.reply_cooldown {
                    ReplyGate::Cooldown
                } else {
                    ReplyGate::Allow
                }
            }
            ConversationProbeDepth::UntilClear => {
                if self.auto_reply_count >= AUTO_REPLY_CAP {
                    ReplyGate::Cap
                } else {
                    ReplyGate::Allow
                }
            }
        }
    }

    /// 记一条 AI 回话进聊天记录：置自动回话冷却、推进修订号；auto=true 时
    /// 计入自动回话数（封顶只数自动，显式交话不计）。回话内换行归一为空格
    /// ——聊天记录「每行一条」的行语法（【我】/【助手】）不被多行回话撑破。
    /// 普通会话不维护聊天记录，调用无效。
    pub fn record_reply(&mut self, text: String, auto: bool) {
        if !self.conversational {
            return;
        }
        self.chat
            .push(ChatTurn::assistant(text.replace(['\r', '\n'], " ")));
        self.reply_cooldown = true;
        if auto {
            self.auto_reply_count += 1;
        }
        self.revision += 1;
    }

    /// 聊天记录按行语法格式化（【我】/【助手】逐行，代码固定）；空记录返回
    /// 空串。对话 assist 与对话出稿的输入都出自这里。
    pub fn chat_transcript(&self) -> String {
        self.chat
            .iter()
            .map(|turn| match turn.role {
                ChatRole::User => format!("【我】{}", turn.text),
                ChatRole::Assistant => format!("【助手】{}", turn.text),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// stop 路径调用一次：未断段的尾巴作为用户话进聊天记录（api stop 时调）。
    /// 尾巴为空不产生行；普通会话不维护聊天记录。
    pub fn record_tail_into_chat(&mut self) {
        if !self.conversational {
            return;
        }
        let tail = self.segmenter.tail(&self.buffer).to_string();
        if !tail.is_empty() {
            self.chat.push(ChatTurn::user(tail));
        }
    }

    /// 喂入一条转写增量与当前启用的常用语。缓冲按 delta 形态自适应
    /// （见模块注释）；随后驱动断句器收集新完成的段并对段文本做命中扫描，
    /// 最后扫缓冲尾部增量（先段后尾：尾巴里说出的触发词，其材料留给
    /// 尾巴补润或下一段，不落进本 feed 刚完成的段）；扫描领地规则与
    /// 撤销-复活交互见方法体内的注释。每段产出 [`PolishableSegment`]
    /// （prior＝已润前文尾部 200 字符；materials＝此刻待融队列全部材料，
    /// 取走后队列清空）。
    pub fn feed(
        &mut self,
        delta: &TranscriptDelta,
        snippets: &[Snippet],
    ) -> Result<FeedOutcome, BackendError> {
        let old_buffer = self.buffer.clone();
        self.apply_delta(delta)?;
        // 替换类增量的扫描领地：新缓冲是旧缓冲的前缀延伸（final 后缀追加
        // 的常见形态）→ 游标原地保留，只扫新增后缀——被撤销的命中不会因
        // 重扫旧文复活；新缓冲改写了旧文 → 游标归零全量重扫，重扫属旧文
        // 领地，被撤销过的命中不复活（新话增量仍可再触发，见 scan_hits）。
        let rewritten = !self.buffer.starts_with(old_buffer.as_str());
        if rewritten {
            self.scanned_chars = 0;
        }
        // 扫描领地：只扫新话增量（命中同款规则），改写重扫属旧文领地。
        let cursor = self.scanned_chars.min(self.buffer.chars().count());
        let tail_increment: String = self.buffer.chars().skip(cursor).collect();
        self.scanned_chars = cursor + tail_increment.chars().count();
        let text = self.buffer.clone();
        let completed = self.segmenter.update(&text);
        // 尾巴在补润后又长出（或缩回）新内容：旧润色结果对不上新尾巴，
        // 作废回落原文，等下次 apply_tail_polished。
        if self.tail_polished.is_some()
            && self.segmenter.tail(&text).chars().count() != self.tail_polished_covered
        {
            self.tail_polished = None;
        }
        let mut new_hits = Vec::new();
        let mut new_segments = Vec::new();
        for segment in completed {
            let index = self.segments.len();
            let segment_text = segment.text.clone();
            self.segments.push(segment);
            self.polished.push(None);
            if self.conversational {
                // 对话会话：用户段进聊天记录；新段＝回应了上一问，
                // 自动回话冷却解除。
                self.chat.push(ChatTurn::user(segment_text.clone()));
                self.reply_cooldown = false;
            }
            // 先扫段文本再取材料：段内说出的触发词，其材料随本段转移。
            // 段文本可含已扫过的旧文，撤销过的命中不在此复活（新话触发
            // 由下方增量扫描负责）。
            new_hits.extend(self.scan_hits(&segment_text, snippets, false));
            let prior = self.polished_so_far(index);
            let materials = std::mem::take(&mut self.inline_pending);
            self.mark_pending_materials_moved();
            new_segments.push(PolishableSegment {
                index,
                prior,
                text: segment_text,
                materials,
            });
        }
        // 尾巴增量后扫：尾巴里说出的触发词，材料留给尾巴补润或下一段；
        // 非改写路径的增量是真正的新话，撤销过的 snippet 可在此再生效。
        new_hits.extend(self.scan_hits(&tail_increment, snippets, !rewritten));
        if !new_segments.is_empty() {
            log::debug!(
                "[ghostwriter] segmenter: {} segment(s) completed (buffer={} chars)",
                new_segments.len(),
                text.chars().count()
            );
        }
        Ok(FeedOutcome {
            new_segments,
            new_hits,
        })
    }

    /// 按 delta 形态应用一条转写增量（判型规则见模块注释）。
    fn apply_delta(&mut self, delta: &TranscriptDelta) -> Result<(), BackendError> {
        if delta.is_final {
            self.buffer = delta.text.clone(); // final 全量替换，offset 忽略
            return Ok(());
        }
        let cur_chars = self.buffer.chars().count();
        let offset = usize::try_from(delta.offset).map_err(|_| {
            crate::errors::BackendError::new(
                crate::errors::BackendErrorCode::InvalidArgument,
                "transcript offset exceeds this platform's address space",
            )
        })?;
        if offset > cur_chars {
            return Err(crate::errors::BackendError::new(
                crate::errors::BackendErrorCode::InvalidArgument,
                "transcript delta starts after the current text",
            ));
        }
        if offset == 0
            && cur_chars <= delta.text.chars().count()
            && self.buffer.chars().zip(delta.text.chars()).all(|(a, b)| a == b)
        {
            // 全量快照型：text 以当前缓冲为前缀 → 整体替换
            self.buffer = delta.text.clone();
        } else if offset == 0 {
            // 后缀增量型 → 追加
            self.buffer.push_str(&delta.text);
        } else {
            // 替换式修订（offset>0）→ 照旧语义
            let kept: String = self.buffer.chars().take(offset).collect();
            self.buffer = format!("{kept}{}", delta.text);
        }
        self.last_partial_len = self.buffer.chars().count();
        Ok(())
    }

    /// 扫一段文本的命中：trigger＋aliases contains 匹配（大小写折叠），
    /// 命中且未生效过 → 表述材料进待融队列、附件解析进背景行；
    /// 背景整条进背景块（按自身落点）。返回本次新生效的命中。
    /// `allow_cancelled`=false 的扫描属旧文领地（段文本重扫、改写后的
    /// 全量重扫）：撤销过且未被新话再触发的 snippet 不复活；=true 的扫描
    /// 是真正的新话增量，可再触发——触发即从撤销集出集
    /// （规则 6：撤销后同 snippet 再说到可再生效）。
    fn scan_hits(
        &mut self,
        text: &str,
        snippets: &[Snippet],
        allow_cancelled: bool,
    ) -> Vec<GhostwriterSnippetHit> {
        let mut hits = Vec::new();
        if text.is_empty() {
            return hits;
        }
        for snippet in snippets {
            if !snippet.enabled || self.is_active(&snippet.id) {
                continue;
            }
            if !allow_cancelled && self.cancelled_once.contains(&snippet.id) {
                continue;
            }
            if match_snippet(text, snippet).is_none() {
                continue;
            }
            if allow_cancelled {
                self.cancelled_once.remove(&snippet.id);
            }
            let hit = GhostwriterSnippetHit {
                snippet_id: snippet.id.clone(),
                title: snippet.trigger.clone(),
                // 背景类按命中当时会话的全局落点记录（历史快照语义，事后改
                // 设置不改旧记录）；表述恒 "inline"（即使带附件）。
                mode: hit_mode(snippet.kind, self.background_placement).to_string(),
            };
            match snippet.kind {
                SnippetKind::Phrasing => {
                    // 表述：融合材料照旧进待融队列；附件被去重过滤不影响
                    // 融合与命中登记（撤销单元含融合＋未被过滤的附件）。
                    self.inline_pending.push(snippet.text.clone());
                    let (lines, keys) = self.resolve_attachments(snippet, snippets);
                    self.active_actions.push(ActiveAction::Hit(ActiveHit {
                        hit: hit.clone(),
                        material: Some(snippet.text.clone()),
                        background_lines: lines,
                        material_pending: true,
                        dedup_keys: keys,
                    }));
                }
                SnippetKind::Background => {
                    // 背景：整条进背景块；去重后无内容可贴 → 不登记命中。
                    let key = background_key_for_id(&snippet.id);
                    if self.background_applied.contains(&key) {
                        continue;
                    }
                    self.background_applied.insert(key.clone());
                    self.active_actions.push(ActiveAction::Hit(ActiveHit {
                        hit: hit.clone(),
                        material: None,
                        background_lines: vec![(snippet.trigger.clone(), snippet.text.clone())],
                        material_pending: false,
                        dedup_keys: vec![key],
                    }));
                }
            }
            hits.push(hit);
        }
        hits
    }

    /// 表述附件逐条解析成背景行（标签＝引用背景的触发词／手写时表述的触发词），
    /// 同时登记去重键（引用项＝被引用 id、手写项＝归一化文本）；命中去重
    /// （同背景已贴过）的条目直接跳过。引用项在本次 feed 传入的启用常用语列表里
    /// 按 id 现取，缺失/已删 → 跳过该条并 log::debug。
    fn resolve_attachments(
        &mut self,
        snippet: &Snippet,
        snippets: &[Snippet],
    ) -> (Vec<(String, String)>, Vec<String>) {
        let mut lines = Vec::new();
        let mut keys = Vec::new();
        for attachment in &snippet.attachments {
            match attachment {
                SnippetAttachment::Reference { snippet_id } => {
                    let Some(referenced) = snippets.iter().find(|s| s.id == *snippet_id) else {
                        log::debug!(
                            "[ghostwriter] attachment reference {snippet_id} not in enabled snippets; skipped"
                        );
                        continue;
                    };
                    let key = background_key_for_id(&referenced.id);
                    if !self.background_applied.insert(key.clone()) {
                        continue;
                    }
                    keys.push(key);
                    lines.push((referenced.trigger.clone(), referenced.text.clone()));
                }
                SnippetAttachment::Text { text } => {
                    let key = background_key_for_text(text);
                    if !self.background_applied.insert(key.clone()) {
                        continue;
                    }
                    keys.push(key);
                    lines.push((snippet.trigger.clone(), text.clone()));
                }
            }
        }
        (lines, keys)
    }

    fn is_active(&self, snippet_id: &str) -> bool {
        self.active_actions.iter().any(|action| {
            matches!(action, ActiveAction::Hit(active) if active.hit.snippet_id == snippet_id)
        })
    }

    /// 待融材料随段（或尾巴补润）转移后，把仍在排队的命中材料标记为已转移。
    fn mark_pending_materials_moved(&mut self) {
        for action in &mut self.active_actions {
            if let ActiveAction::Hit(active) = action {
                if active.material.is_some() {
                    active.material_pending = false;
                }
            }
        }
    }

    /// 已润前文（polished 回落段原文）拼接后的尾部 200 字符。
    fn polished_so_far(&self, index: usize) -> String {
        let joined: String = (0..index)
            .map(|i| {
                self.polished[i]
                    .clone()
                    .unwrap_or_else(|| self.segments[i].text.clone())
            })
            .collect();
        let chars = joined.chars().count();
        joined
            .chars()
            .skip(chars.saturating_sub(PRIOR_MAX_CHARS))
            .collect()
    }

    /// 应用一段润色结果：越界返回 false（revision 不增）。
    /// 材料消耗在 feed 产出时已转移，这里不动材料。
    pub fn apply_polished(&mut self, index: usize, text: String) -> bool {
        if index >= self.polished.len() {
            return false;
        }
        self.polished[index] = Some(text);
        self.revision += 1;
        true
    }

    /// 应用尾巴补润结果：此刻待融材料视为已融进润色文本（出队）。
    /// 记录本次覆盖的尾巴长度，尾巴后续再变即作废本次润色结果。
    pub fn apply_tail_polished(&mut self, text: String) -> bool {
        self.tail_polished = Some(text);
        self.inline_pending.clear();
        self.mark_pending_materials_moved();
        self.tail_polished_covered = self.segmenter.tail(&self.buffer).chars().count();
        self.revision += 1;
        true
    }

    /// 撤销最近一次生效的动作（命中）：
    /// 表述材料出待融、背景行随动作弹出消失、去重键释放、snippet 恢复
    /// 「未生效」可再触发；引用型背景键随撤销把被引用者一并记入撤销集——
    /// 旧文重扫（改写式 final、offset 修订，allow_cancelled=false）不得让
    /// 刚被撤单元的背景行复活，新话路径照常出集。成功撤销推进修订号
    /// （后端权威），调用方据此发布撤销后的预览并让前端丢弃在途旧预览。
    /// 返回被撤销者供事件确认。
    pub fn cancel_last_action(&mut self) -> Option<LastAction> {
        match self.active_actions.pop()? {
            ActiveAction::Hit(active) => {
                self.cancelled_once.insert(active.hit.snippet_id.clone());
                if active.material_pending {
                    if let Some(material) = active.material.as_deref() {
                        if let Some(position) =
                            self.inline_pending.iter().rposition(|m| m == material)
                        {
                            self.inline_pending.remove(position);
                        }
                    }
                }
                for key in &active.dedup_keys {
                    self.background_applied.remove(key);
                    if let Some(referenced_id) = key.strip_prefix(BACKGROUND_KEY_ID_PREFIX) {
                        self.cancelled_once.insert(referenced_id.to_string());
                    }
                }
                self.revision += 1;
                Some(LastAction::Hit(active.hit))
            }
        }
    }

    /// 换上新的 live assist 批次（替换式）：候选组带各自组别（kind 随
    /// assist 产出直达视图，无按位置的固定映射）。候选与推荐均纯展示
    /// （2026-09-17 裁决），批次替换只换展示内容。
    pub fn set_live_batch(
        &mut self,
        candidates: Vec<(String, Vec<CandidateItem>)>,
        recommendations: Vec<LiveRecommendation>,
    ) {
        self.live_batch = Some(LiveBatch {
            candidates,
            recommendations,
        });
    }

    /// 当前批次视图（浮框候选区渲染依据）；无批次 → None。
    /// 候选序号为跨组连续的 1-based 全局序号（纯展示，仅作展示与事件对齐）。
    pub fn assist_snapshot(&self) -> Option<AssistSnapshot> {
        let batch = self.live_batch.as_ref()?;
        let mut next_index = 1usize;
        let candidate_groups = batch
            .candidates
            .iter()
            .map(|(kind, items)| CandidateGroupView {
                kind: kind.clone(),
                items: items
                    .iter()
                    .map(|item| {
                        let index = next_index;
                        next_index += 1;
                        CandidateItemView {
                            index,
                            text: item.name.clone(),
                            note: item.note.clone(),
                        }
                    })
                    .collect(),
            })
            .collect();
        let recommendations = batch
            .recommendations
            .iter()
            .map(|recommendation| RecommendationView {
                snippet_id: recommendation.snippet_id.clone(),
                title: recommendation.title.clone(),
            })
            .collect();
        Some(AssistSnapshot {
            candidate_groups,
            recommendations,
        })
    }

    /// 尾巴补润的输入：segmenter 尾巴非空时返回
    /// （尾巴原文, 已润前文尾部 200 字符, 此刻全部待融材料）。
    /// 材料此处只是随请求交付，待 [`Self::apply_tail_polished`] 成功才出队，
    /// 润色失败时兜底追加仍在 assembled 生效。
    pub fn tail_polish_input(&self) -> Option<PolishableSegment> {
        let tail = self.segmenter.tail(&self.buffer).to_string();
        if tail.is_empty() {
            return None;
        }
        let index = self.segments.len();
        Some(PolishableSegment {
            index,
            prior: self.polished_so_far(index),
            text: tail,
            materials: self.inline_pending.clone(),
        })
    }

    /// 指令预览＝最终贴出：主文本（润色回落段原文）＋尾巴（补润回落原文）
    /// ＋待融剩余材料（润色失败兜底）＋背景块。块格式＝节头 `[背景]`＋行式
    /// 「- 标签：文本」，多条同节按生效顺序（含表述附件行）；全部背景行
    /// 按会话当前全局落点归入同一块——头块拼在最前、尾块拼在最后。
    pub fn assembled_text(&self) -> String {
        let mut main: String = self
            .segments
            .iter()
            .enumerate()
            .map(|(i, segment)| {
                self.polished[i]
                    .clone()
                    .unwrap_or_else(|| segment.text.clone())
            })
            .collect();
        let tail = self
            .tail_polished
            .clone()
            .unwrap_or_else(|| self.segmenter.tail(&self.buffer).to_string());
        main.push_str(&tail);
        let mut assembled = main.trim().to_string();
        for material in &self.inline_pending {
            assembled.push_str(material);
        }
        let background_lines = self.background_lines();
        match self.background_placement {
            SnippetPlacement::Head => {
                if !background_lines.is_empty() {
                    assembled = format!("[背景]\n{}\n\n{}", background_lines.join("\n"), assembled);
                }
            }
            SnippetPlacement::Tail => {
                if !background_lines.is_empty() {
                    assembled.push_str("\n\n[背景]\n");
                    assembled.push_str(&background_lines.join("\n"));
                }
            }
        }
        assembled
    }

    /// 现算全部背景块行（「- 标签：文本」），按动作生效顺序（含表述附件行）；
    /// 头/尾归属由调用方按会话当前全局落点决定。
    fn background_lines(&self) -> Vec<String> {
        self.active_actions
            .iter()
            .filter_map(|action| match action {
                ActiveAction::Hit(active) => Some(active.background_lines.iter()),
                _ => None,
            })
            .flatten()
            .map(|(label, text)| format!("- {label}：{text}"))
            .collect()
    }

    /// 说话中的完整缓冲（原样，未 trim）：测试与 M1′ 验证探针。
    pub fn debug_text(&self) -> &str {
        &self.buffer
    }

    /// [`Self::debug_text`] 的别名：实时缓冲探针。
    pub fn debug_buffer(&self) -> &str {
        &self.buffer
    }

    pub fn is_empty(&self) -> bool {
        self.assembled_text().is_empty()
    }

    /// 预览修订号：润色结果应用成功或命中撤销成功后递增。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 已完成的段（与 polished 对齐，拼装回落段原文）。
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// 历史归档用：仍生效（未撤销）的命中快照，按生效顺序（标题＋贴位）。
    /// 候选与推荐纯展示（2026-09-17 裁决），新会话不再产生历史选中；
    /// 旧历史记录里的选中照常展示（ghostwriter_selections 字段只读保留）。
    pub fn history_hits(&self) -> Vec<GhostwriterSnippetHit> {
        self.active_actions
            .iter()
            .map(|action| match action {
                ActiveAction::Hit(active) => active.hit.clone(),
            })
            .collect()
    }

    /// 对话终稿出稿用的命中材料：全部仍生效（未撤销）命中的表述文本，按
    /// 生效顺序。含已被段润色取走的（出稿以聊天记录重新出稿，段润色结果
    /// 只进预览，材料必须在出稿输入里重新在场）；背景类命中无材料不入。
    pub fn active_materials(&self) -> Vec<String> {
        self.active_actions
            .iter()
            .filter_map(|action| match action {
                ActiveAction::Hit(active) => active.material.clone(),
            })
            .collect()
    }
}

/// contains 命中：trigger 与 aliases 逐个大小写折叠匹配，命中返回常用语全量文本。
fn match_snippet(text: &str, snippet: &Snippet) -> Option<String> {
    let folded = text.to_lowercase();
    std::iter::once(&snippet.trigger)
        .chain(snippet.aliases.iter())
        .map(|keyword| keyword.trim().to_lowercase())
        .find(|keyword| !keyword.is_empty() && folded.contains(keyword.as_str()))
        .map(|_| snippet.text.clone())
}

/// 命中记录里的贴位字符串：表述恒 "inline"（即使带附件）；
/// 背景按命中当时会话的全局落点记 head/tail（历史快照语义）。
fn hit_mode(kind: SnippetKind, placement: SnippetPlacement) -> &'static str {
    match kind {
        SnippetKind::Phrasing => "inline",
        SnippetKind::Background => match placement {
            SnippetPlacement::Head => "head",
            SnippetPlacement::Tail => "tail",
        },
    }
}

/// 背景去重键的 id 前缀：背景类命中与引用附件共用（撤销时按此前缀
/// 还原被引用的 snippet id）。
const BACKGROUND_KEY_ID_PREFIX: &str = "id:";

/// 背景去重键（按 snippet）：背景类命中、引用附件指向同一条时同键。
fn background_key_for_id(id: &str) -> String {
    format!("{BACKGROUND_KEY_ID_PREFIX}{id}")
}

/// 手写背景去重键：归一化文本（trim＋连续空白折叠）。
fn background_key_for_text(text: &str) -> String {
    format!(
        "text:{}",
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    )
}

/// 口头命令头匹配已移除：候选与推荐均纯展示（2026-09-17 裁决），
/// 「用候选N/用常用语N」不再是命令，一律当普通话保留。

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(text: &str, offset: u64, is_final: bool) -> TranscriptDelta {
        TranscriptDelta {
            text: text.to_string(),
            offset,
            is_final,
        }
    }

    fn snip(id: &str, trigger: &str, kind: SnippetKind) -> Snippet {
        Snippet {
            id: id.to_string(),
            trigger: trigger.to_string(),
            aliases: Vec::new(),
            text: format!("【{id}】{trigger}的完整表述文本"),
            kind,
            attachments: Vec::new(),
            enabled: true,
        }
    }

    /// 指定全局背景落点的会话（背景块头/尾位置测试用）。
    fn session_with_placement(placement: SnippetPlacement) -> GhostwriterSession {
        GhostwriterSession::new().with_background_placement(placement)
    }

    /// 给表述挂附件（引用/手写，按列表顺序）。
    fn with_attachments(mut snippet: Snippet, attachments: Vec<SnippetAttachment>) -> Snippet {
        snippet.attachments = attachments;
        snippet
    }

    fn cancelled_hit(action: Option<LastAction>) -> GhostwriterSnippetHit {
        match action {
            Some(LastAction::Hit(hit)) => hit,
            _ => panic!("期望撤销的是常用语命中"),
        }
    }

    #[test]
    fn suffix_partial_deltas_accumulate_and_final_replaces() {
        // 后缀增量型 provider（火山/讯飞实测形态）：partial 全部 offset:0
        let mut s = GhostwriterSession::new();
        s.feed(&delta("你好", 0, false), &[]).unwrap();
        s.feed(&delta("，", 0, false), &[]).unwrap();
        assert_eq!(s.debug_buffer(), "你好，");
        assert_eq!(s.last_partial_len, 3);
        // final 全量替换
        s.feed(&delta("你好，这是测试。", 0, true), &[]).unwrap();
        assert_eq!(s.debug_text(), "你好，这是测试。");
        assert_eq!(s.last_partial_len, 3);
    }

    #[test]
    fn full_snapshot_partial_deltas_replace_not_append() {
        // 全量快照型 partial：text 是累计全文
        let mut s = GhostwriterSession::new();
        s.feed(&delta("你好", 0, false), &[]).unwrap();
        s.feed(&delta("你好，", 0, false), &[]).unwrap();
        assert_eq!(s.debug_text(), "你好，");
    }

    #[test]
    fn offset_revision_still_replaces() {
        // 替换式修订（offset>0）语义保留
        let mut s = GhostwriterSession::new();
        s.feed(&delta("ABC", 0, false), &[]).unwrap();
        s.feed(&delta("X", 1, false), &[]).unwrap();
        assert_eq!(s.debug_text(), "AX");
        assert_eq!(s.last_partial_len, 2);
    }

    #[test]
    fn rejects_offset_beyond_the_buffer() {
        let mut session = GhostwriterSession::new();
        let error = session.feed(&delta("越界", 5, false), &[]).unwrap_err();
        assert_eq!(error.code, crate::errors::BackendErrorCode::InvalidArgument);
    }

    #[test]
    fn collects_completed_segments_from_the_buffer() {
        let mut session = GhostwriterSession::new();
        session.feed(&delta("第一句。第二句还没说完", 0, false), &[]).unwrap();
        assert_eq!(session.segments().len(), 1);
        assert_eq!(session.segments()[0].text, "第一句。");
        // 句末标点停在缓冲末尾时不触发断句（ASR 可能继续追加）；
        // 尾巴留给会话结束时的补润（Segmenter::tail 读取）。
        session.feed(&delta("第一句。第二句还没说完。", 0, true), &[]).unwrap();
        assert_eq!(session.segments().len(), 1);
        assert_eq!(session.assembled_text(), "第一句。第二句还没说完。");
    }

    #[test]
    fn empty_session_assembles_to_empty() {
        let session = GhostwriterSession::new();
        assert!(session.is_empty());
        assert_eq!(session.assembled_text(), "");
    }

    #[test]
    fn assembled_text_trims_surrounding_whitespace() {
        let mut session = GhostwriterSession::new();
        session.feed(&delta("  两边空白  ", 0, true), &[]).unwrap();
        assert_eq!(session.assembled_text(), "两边空白");
    }

    #[test]
    fn feed_emits_segments_with_prior_and_materials() {
        // 表述常用语「翻译」随段命中：材料随 PolishableSegment 转移，待融队列清空
        let mut s = GhostwriterSession::new();
        let snippets = vec![snip("s1", "翻译", SnippetKind::Phrasing)];
        let outcome = s
            .feed(&delta("你好，帮我翻译一下。请", 0, false), &snippets)
            .unwrap();
        assert_eq!(outcome.new_segments.len(), 1);
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(outcome.new_hits[0].mode, "inline");
        let segment = &outcome.new_segments[0];
        assert_eq!(segment.index, 0);
        assert_eq!(segment.text, "你好，帮我翻译一下。");
        assert_eq!(segment.prior, "");
        assert_eq!(
            segment.materials,
            vec![snip("s1", "翻译", SnippetKind::Phrasing).text]
        );
        // 材料已随段转移：预览不再追加材料文本
        assert_eq!(s.assembled_text(), "你好，帮我翻译一下。请");

        // 第 2 段的 prior＝第 1 段已润文本的尾部 200 字符
        assert!(s.apply_polished(0, format!("润{}", "好".repeat(250))));
        let outcome = s.feed(&delta("第二句。完", 0, false), &snippets).unwrap();
        assert_eq!(outcome.new_segments.len(), 1);
        let prior = &outcome.new_segments[0].prior;
        assert_eq!(prior.chars().count(), 200);
        assert!(prior.ends_with(&"好".repeat(50)));
    }

    #[test]
    fn background_hit_records_once_and_dedupes() {
        // 同 snippet 说到两次 → new_hits 只第一次；背景块只拼一条
        let mut s = GhostwriterSession::new();
        let snippet = snip("f1", "附注", SnippetKind::Background);
        let snippets = vec![snippet.clone()];
        let outcome = s
            .feed(&delta("帮我加个附注说明。继续", 0, false), &snippets)
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(outcome.new_hits[0].mode, "tail");
        assert_eq!(outcome.new_hits[0].title, "附注");
        let expected = format!("\n\n[背景]\n- 附注：{}", snippet.text);
        assert_eq!(
            s.assembled_text(),
            format!("帮我加个附注说明。继续{expected}")
        );
        // 拼装确定性：同一状态两次调用结果一致
        assert_eq!(s.assembled_text(), s.assembled_text());

        let outcome = s
            .feed(&delta("再提一次附注。完", 0, false), &snippets)
            .unwrap();
        assert!(outcome.new_hits.is_empty());
        let assembled = s.assembled_text();
        assert_eq!(assembled.matches("[背景]").count(), 1);
        assert!(assembled.ends_with(&expected));
    }

    #[test]
    fn alias_match_fires_hit_case_folded() {
        // 别名分支：trigger 未出现，别名 "fy" 以大写形式出现在文本里 → 大小写折叠命中；
        // 无句末标点不成段：融合材料留在待融队列，追加在转写后
        let mut s = GhostwriterSession::new();
        let mut snippet = snip("s-fy", "翻译", SnippetKind::Phrasing);
        snippet.aliases = vec!["fy".to_string()];
        let material = snippet.text.clone();
        let snippets = vec![snippet];
        let outcome = s
            .feed(&delta("帮我FY一下", 0, false), &snippets)
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(outcome.new_hits[0].snippet_id, "s-fy");
        assert!(s.assembled_text().contains(&material));
    }

    #[test]
    fn alias_match_skips_blank_and_tolerates_surrounding_space() {
        // 别名容错：空串不得通配命中；两侧空白经 trim 折叠后命中
        let mut s = GhostwriterSession::new();
        let mut snippet = snip("s-alias", "翻译", SnippetKind::Phrasing);
        snippet.aliases = vec![" a ".to_string(), String::new(), "  b  ".to_string()];
        let snippets = vec![snippet];
        let outcome = s
            .feed(&delta("请B一下。", 0, false), &snippets)
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
    }

    #[test]
    fn assembled_joins_polished_with_fallback_and_background_block() {
        // 两段：第 1 段已润、第 2 段未润回落原文；背景块格式「- 标签：文本」
        let mut s = GhostwriterSession::new();
        let snippet = snip("f1", "标记", SnippetKind::Background);
        let snippets = vec![snippet.clone()];
        s.feed(&delta("第一句原文。标记一下然后", 0, false), &snippets)
            .unwrap();
        s.feed(&delta("第二句原文。尾", 0, false), &snippets).unwrap();
        assert_eq!(s.segments().len(), 2);
        assert!(s.apply_polished(0, "第一句润好。".to_string()));
        assert_eq!(
            s.assembled_text(),
            format!(
                "第一句润好。标记一下然后第二句原文。尾\n\n[背景]\n- 标记：{}",
                snippet.text
            )
        );
    }

    #[test]
    fn cancel_last_hit_removes_latest_and_allows_re_hit() {
        // 背景：撤销后背景块消失，同 snippet 再说到 → 再次生效
        let mut s = GhostwriterSession::new();
        let snippet = snip("f1", "附注", SnippetKind::Background);
        let snippets = vec![snippet.clone()];
        s.feed(&delta("第一句。", 0, false), &snippets).unwrap();
        s.feed(&delta("加个附注", 0, false), &snippets).unwrap();
        let cancelled = cancelled_hit(s.cancel_last_action());
        assert_eq!(cancelled.snippet_id, "f1");
        assert_eq!(cancelled.mode, "tail");
        assert!(!s.assembled_text().contains("[背景]"));
        assert!(s.cancel_last_action().is_none());
        let outcome = s.feed(&delta("再说附注。完", 0, false), &snippets).unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert!(s.assembled_text().contains(&format!("- 附注：{}", snippet.text)));

        // 表述：撤销把材料从待融队列出队，预览不再追加；可再次生效。
        // 「请翻译一下」后无句末标点不产段：材料留在待融队列等尾巴补润。
        let mut s = GhostwriterSession::new();
        let inline = snip("s1", "翻译", SnippetKind::Phrasing);
        let snippets = vec![inline.clone()];
        s.feed(&delta("第一句。", 0, false), &snippets).unwrap();
        s.feed(&delta("请翻译一下", 0, false), &snippets).unwrap();
        assert!(s.assembled_text().ends_with(&inline.text));
        let cancelled = cancelled_hit(s.cancel_last_action());
        assert_eq!(cancelled.snippet_id, "s1");
        assert_eq!(cancelled.mode, "inline");
        assert_eq!(s.assembled_text(), "第一句。请翻译一下");
        let outcome = s.feed(&delta("再翻译一次。完", 0, false), &snippets).unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        // 重生效的融合材料：或留在待融队列（未断句）或已随段转移，不散落
        let rehit_materials = s.assembled_text();
        assert!(rehit_materials.ends_with(&inline.text) || !rehit_materials.contains(&inline.text));
    }

    #[test]
    fn revision_increments_on_apply_polished() {
        let mut s = GhostwriterSession::new();
        s.feed(&delta("第一句。续", 0, false), &[]).unwrap();
        assert_eq!(s.revision(), 0);
        assert!(s.apply_polished(0, "第一句润好。".to_string()));
        assert_eq!(s.revision(), 1);
        // 越界 index：false 且不增
        assert!(!s.apply_polished(9, "越界".to_string()));
        assert_eq!(s.revision(), 1);
        assert!(s.apply_tail_polished("尾巴润好".to_string()));
        assert_eq!(s.revision(), 2);
    }

    #[test]
    fn tail_polish_input_returns_tail_with_pending_materials() {
        // 尾巴非空 → Some(尾巴, prior, 待融材料)；材料 apply 成功才出队，
        // 失败时兜底追加仍在预览生效
        let mut s = GhostwriterSession::new();
        assert!(s.tail_polish_input().is_none());
        let inline = snip("s1", "翻译", SnippetKind::Phrasing);
        let snippets = vec![inline.clone()];
        s.feed(&delta("第一句。", 0, false), &snippets).unwrap();
        s.feed(&delta("第二句来了", 0, false), &snippets).unwrap();
        s.feed(&delta("翻译一下", 0, false), &snippets).unwrap();
        let input = s.tail_polish_input().unwrap();
        assert_eq!(input.index, 1);
        assert_eq!(input.prior, "第一句。");
        assert_eq!(input.text, "第二句来了翻译一下");
        assert_eq!(input.materials, vec![inline.text.clone()]);
        // 未 apply：材料仍在待融队列，预览兜底追加
        assert_eq!(
            s.assembled_text(),
            format!("第一句。第二句来了翻译一下{}", inline.text)
        );
        // apply 成功：材料视为已融，出队且不再追加
        assert!(s.apply_tail_polished("整理好的尾巴".to_string()));
        assert_eq!(s.assembled_text(), "第一句。整理好的尾巴");
    }

    #[test]
    fn final_rescan_does_not_resurrect_cancelled_hits() {
        // 背景：命中→撤销→final 前缀延伸含触发词 → 不复活
        // （重扫旧文不算「再说到」，规则 6 只认新话）
        let mut s = GhostwriterSession::new();
        let snippet = snip("f1", "附注", SnippetKind::Background);
        let snippets = vec![snippet.clone()];
        s.feed(&delta("第一句。", 0, false), &snippets).unwrap();
        s.feed(&delta("加个附注", 0, false), &snippets).unwrap();
        assert!(s.cancel_last_action().is_some());
        let outcome = s.feed(&delta("第一句。加个附注。", 0, true), &snippets).unwrap();
        assert!(outcome.new_hits.is_empty());
        assert!(!s.assembled_text().contains("[背景]"));

        // 全量替换型 final（非前缀延伸）：全量重扫但被撤销者保持死亡
        let outcome = s.feed(&delta("换个说法提到附注。", 0, true), &snippets).unwrap();
        assert!(outcome.new_hits.is_empty());
        assert!(!s.assembled_text().contains("[背景]"));

        // 规则 6 仍成立：之后真正的新话再说到 → 再次生效
        let outcome = s.feed(&delta("再提附注一下", 0, false), &snippets).unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert!(s.assembled_text().contains(&format!("- 附注：{}", snippet.text)));

        // 表述：撤销后 final 不把材料送回待融队列
        let mut s = GhostwriterSession::new();
        let inline = snip("s1", "翻译", SnippetKind::Phrasing);
        let snippets = vec![inline.clone()];
        s.feed(&delta("帮我", 0, false), &snippets).unwrap();
        s.feed(&delta("翻译一下", 0, false), &snippets).unwrap();
        assert!(s.cancel_last_action().is_some());
        let outcome = s.feed(&delta("帮我翻译一下。", 0, true), &snippets).unwrap();
        assert!(outcome.new_hits.is_empty());
        assert_eq!(s.assembled_text(), "帮我翻译一下。");
    }

    #[test]
    fn cancel_last_hit_bumps_revision_so_previews_flow_after_undo() {
        // 修订号只在 apply_*/撤销处递增，feed 路径不增：润色结果迟迟未应用时
        // （如 LLM 失败回落），撤销是预览前进的唯一推手。撤销须推进修订号，
        // 之后 feed 预览按等号规则继续被前端接受，不会冻结在撤销快照。
        let mut s = GhostwriterSession::new();
        let snippet = snip("f1", "附注", SnippetKind::Background);
        let snippets = vec![snippet.clone()];
        let outcome = s
            .feed(&delta("加个附注。完毕", 0, false), &snippets)
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(s.revision(), 0);
        assert!(cancelled_hit(s.cancel_last_action()).snippet_id == "f1");
        assert_eq!(s.revision(), 1);
        assert!(!s.assembled_text().contains("[背景]"));
        // 撤销后再喂新内容：预览反映新内容，修订号保持（feed 路径不增）
        s.feed(&delta("，新话来了。", 0, false), &snippets).unwrap();
        assert_eq!(s.revision(), 1);
        assert!(s.assembled_text().contains("新话来了"));
        assert!(!s.assembled_text().contains("[背景]"));
    }

    #[test]
    fn tail_growth_after_apply_invalidates_stale_tail_polish() {
        // 尾巴长出来 → 补润 → 尾巴又长 → 预览回落新尾巴原文（不显示旧润），
        // tail_polish_input 再次可用；再次 apply 后预览显示新润色尾巴
        let mut s = GhostwriterSession::new();
        s.feed(&delta("第一句。", 0, false), &[]).unwrap();
        s.feed(&delta("尾巴第一截", 0, false), &[]).unwrap();
        assert!(s.apply_tail_polished("尾巴一润".to_string()));
        assert_eq!(s.assembled_text(), "第一句。尾巴一润");
        s.feed(&delta("尾巴第二截", 0, false), &[]).unwrap();
        assert_eq!(s.assembled_text(), "第一句。尾巴第一截尾巴第二截");
        let input = s.tail_polish_input().unwrap();
        assert_eq!(input.text, "尾巴第一截尾巴第二截");
        assert!(s.apply_tail_polished("尾巴二润".to_string()));
        assert_eq!(s.assembled_text(), "第一句。尾巴二润");
    }

    // ===== 对话会话：聊天记录、模式标志、回话门控 =====

    fn conversational_session(depth: ConversationProbeDepth) -> GhostwriterSession {
        GhostwriterSession::new().with_conversation(ConversationReplyTiming::Pause, depth)
    }

    #[test]
    fn normal_session_never_maintains_chat() {
        // 普通会话（未调 with_conversation）：聊天记录恒空，回话登记、尾巴入记、
        // 门控都不产生任何对话行为。
        let mut s = GhostwriterSession::new();
        assert!(!s.conversational());
        s.feed(&delta("第一句。继续", 0, false), &[]).unwrap();
        s.record_reply("回话一句".into(), true);
        s.record_tail_into_chat();
        assert_eq!(s.chat_transcript(), "");
        assert_eq!(s.reply_gate(true), ReplyGate::Allow);
        assert_eq!(s.reply_gate(false), ReplyGate::Allow);
    }

    #[test]
    fn conversational_flag_follows_builder() {
        assert!(conversational_session(ConversationProbeDepth::Single).conversational());
    }

    #[test]
    fn conversational_segments_enter_chat_as_user_turns() {
        let mut s = conversational_session(ConversationProbeDepth::Single);
        s.feed(&delta("第一句。继续说", 0, false), &[]).unwrap();
        s.feed(&delta("第二句。还没完", 0, false), &[]).unwrap();
        assert_eq!(s.chat_transcript(), "【我】第一句。\n【我】继续说第二句。");
    }

    #[test]
    fn chat_transcript_is_empty_before_any_turn() {
        let s = conversational_session(ConversationProbeDepth::Single);
        assert_eq!(s.chat_transcript(), "");
    }

    #[test]
    fn record_reply_appends_assistant_turn_and_bumps_revision() {
        let mut s = conversational_session(ConversationProbeDepth::Single);
        s.feed(&delta("想把日志清一下。帮", 0, false), &[]).unwrap();
        let before = s.revision();
        s.record_reply("你说的是哪个日志？".into(), true);
        assert_eq!(s.revision(), before + 1);
        assert_eq!(
            s.chat_transcript(),
            "【我】想把日志清一下。\n【助手】你说的是哪个日志？"
        );
    }

    #[test]
    fn reply_gate_matrix_over_depths_and_auto() {
        // 一点一问：回话后冷却，用户新段解除；显式交话恒放行（绕过冷却）。
        let mut s = conversational_session(ConversationProbeDepth::Single);
        assert_eq!(s.reply_gate(true), ReplyGate::Allow);
        s.record_reply("第一问".into(), true);
        assert_eq!(s.reply_gate(true), ReplyGate::Cooldown);
        assert_eq!(s.reply_gate(false), ReplyGate::Allow);
        s.feed(&delta("新段。继续", 0, false), &[]).unwrap();
        assert_eq!(s.reply_gate(true), ReplyGate::Allow);

        // 回声确认：抑制机制同一点一问。
        let mut s = conversational_session(ConversationProbeDepth::Echo);
        assert_eq!(s.reply_gate(true), ReplyGate::Allow);
        s.record_reply("回声".into(), true);
        assert_eq!(s.reply_gate(true), ReplyGate::Cooldown);
        assert_eq!(s.reply_gate(false), ReplyGate::Allow);

        // 追问到清：无冷却；封顶只数自动回话，第 6 次自动 Cap；显式恒放行。
        let mut s = conversational_session(ConversationProbeDepth::UntilClear);
        for i in 0..5 {
            assert_eq!(s.reply_gate(true), ReplyGate::Allow, "第 {} 次应放行", i + 1);
            s.record_reply(format!("第{}问", i + 1), true);
        }
        assert_eq!(s.reply_gate(true), ReplyGate::Cap);
        assert_eq!(s.reply_gate(false), ReplyGate::Allow);
    }

    #[test]
    fn explicit_replies_do_not_count_toward_cap() {
        let mut s = conversational_session(ConversationProbeDepth::UntilClear);
        for _ in 0..6 {
            s.record_reply("显式回话".into(), false);
        }
        assert_eq!(s.reply_gate(true), ReplyGate::Allow);
    }

    #[test]
    fn record_reply_normalizes_newlines_to_keep_line_syntax() {
        // 多行回话归一成单行（换行→空格）：聊天记录「每行一条」的行语法
        // 不被多行回话撑破。
        let mut s = conversational_session(ConversationProbeDepth::Single);
        s.feed(&delta("想把日志清一下。帮", 0, false), &[]).unwrap();
        s.record_reply("第一行\n第二行".into(), true);
        assert_eq!(
            s.chat_transcript(),
            "【我】想把日志清一下。\n【助手】第一行 第二行"
        );
    }

    #[test]
    fn tail_is_recorded_into_chat_once_at_stop() {
        let mut s = conversational_session(ConversationProbeDepth::Single);
        s.feed(&delta("把日志清一下。顺便", 0, false), &[]).unwrap();
        s.record_tail_into_chat();
        assert_eq!(s.chat_transcript(), "【我】把日志清一下。\n【我】顺便");
        // 空尾巴不产生行。
        let mut s = conversational_session(ConversationProbeDepth::Single);
        s.record_tail_into_chat();
        assert_eq!(s.chat_transcript(), "");
    }

    #[test]
    fn selection_phrases_are_plain_speech_now() {
        // 候选与推荐纯展示（2026-09-17 裁决）：「用候选N」「用常用语N」都不再是
        // 命令，有批次在场也当普通话保留、原样进文本，不产生选中。
        let mut s = GhostwriterSession::new();
        s.set_live_batch(
            vec![("term".to_string(), vec!["甲".into(), "乙".into()])],
            vec![LiveRecommendation {
                snippet_id: "r1".into(),
                title: "推荐一".into(),
                text: "推荐一的完整文本".into(),
            }],
        );
        let outcome = s.feed(&delta("用候选二看看，用常用语一", 0, false), &[]).unwrap();
        assert_eq!(s.debug_buffer(), "用候选二看看，用常用语一");
        assert!(outcome.new_hits.is_empty());
        assert!(s.assembled_text().contains("用常用语一"));
        assert!(!s.assembled_text().contains("推荐一的完整文本"));
    }

    #[test]
    fn background_block_position_follows_session_placement() {
        // 全局落点驱动背景块位置：默认文末 → 尾块；改头 → 头块；
        // 背景类与表述附件两类都随会话落点走。
        let mut s = session_with_placement(SnippetPlacement::Tail);
        let bg = snip("b1", "文末", SnippetKind::Background);
        let phrasing = with_attachments(
            snip("p1", "方案", SnippetKind::Phrasing),
            vec![SnippetAttachment::Text {
                text: "手写的背景说明".to_string(),
            }],
        );
        let outcome = s
            .feed(
                &delta("文末一下。给个方案。继续", 0, true),
                &[bg.clone(), phrasing],
            )
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 2);
        assert_eq!(
            s.assembled_text(),
            format!(
                "文末一下。给个方案。继续\n\n[背景]\n- 文末：{}\n- 方案：手写的背景说明",
                bg.text
            )
        );

        // 同样两条命中，会话落点为头 → 头块在最前
        let mut s = session_with_placement(SnippetPlacement::Head);
        let phrasing = with_attachments(
            snip("p1", "方案", SnippetKind::Phrasing),
            vec![SnippetAttachment::Text {
                text: "手写的背景说明".to_string(),
            }],
        );
        s.feed(
            &delta("文末一下。给个方案。继续", 0, true),
            &[bg.clone(), phrasing],
        )
        .unwrap();
        assert_eq!(
            s.assembled_text(),
            format!(
                "[背景]\n- 文末：{}\n- 方案：手写的背景说明\n\n文末一下。给个方案。继续",
                bg.text
            )
        );
    }

    #[test]
    fn set_background_placement_moves_existing_blocks() {
        // 设置变更即时生效：命中生效后改会话落点，已贴的背景块位置随动
        // （拼装现算，改字段即可）；命中 mode 是命中当时的快照，不随改。
        let mut s = GhostwriterSession::new();
        let bg = snip("b1", "附注", SnippetKind::Background);
        let outcome = s.feed(&delta("加个附注。完毕", 0, true), &[bg.clone()]).unwrap();
        assert_eq!(outcome.new_hits[0].mode, "tail");
        let tail_assembled = s.assembled_text();
        assert!(tail_assembled.ends_with(&format!("\n\n[背景]\n- 附注：{}", bg.text)));

        s.set_background_placement(SnippetPlacement::Head);
        assert_eq!(
            s.assembled_text(),
            format!("[背景]\n- 附注：{}\n\n加个附注。完毕", bg.text)
        );
        // 撤销后重新命中：新命中 mode 按当时落点记 head
        assert!(s.cancel_last_action().is_some());
        let outcome = s.feed(&delta("再提附注", 0, false), &[bg.clone()]).unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(outcome.new_hits[0].mode, "head");
    }

    #[test]
    fn phrasing_attachments_lines_carry_kind_labels() {
        // 表述带附件（引用＋手写）按会话全局落点进块：引用行标签＝被引用背景的
        // 触发词、手写行标签＝表述的触发词；表述文本本身仍进待融队列，
        // 命中 mode 恒 "inline"（即使带附件）。
        let mut s = GhostwriterSession::new();
        let bg = snip("bg1", "素材", SnippetKind::Background);
        let phrasing = with_attachments(
            snip("p1", "方案", SnippetKind::Phrasing),
            vec![
                SnippetAttachment::Reference {
                    snippet_id: "bg1".to_string(),
                },
                SnippetAttachment::Text {
                    text: "手写的背景说明".to_string(),
                },
            ],
        );
        let outcome = s
            .feed(&delta("给个方案。继续", 0, false), &[bg.clone(), phrasing])
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(outcome.new_hits[0].snippet_id, "p1");
        assert_eq!(outcome.new_hits[0].mode, "inline");
        // 材料随段转移（本 feed 完成段），不散落在预览文本里
        assert_eq!(
            s.assembled_text(),
            format!(
                "给个方案。继续\n\n[背景]\n- 素材：{}\n- 方案：手写的背景说明",
                bg.text
            )
        );
    }

    #[test]
    fn missing_reference_is_skipped() {
        // 引用不在本次传入的启用列表里（被禁用/删除）→ 跳过该条并 log；
        // 表述照常融合与登记命中，其余附件不受影响。
        let mut s = GhostwriterSession::new();
        let phrasing = with_attachments(
            snip("p1", "方案", SnippetKind::Phrasing),
            vec![
                SnippetAttachment::Reference {
                    snippet_id: "ghost".to_string(),
                },
                SnippetAttachment::Text {
                    text: "仍在".to_string(),
                },
            ],
        );
        let outcome = s
            .feed(&delta("方案说一下。尾", 0, false), &[phrasing])
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        let assembled = s.assembled_text();
        assert!(!assembled.contains("ghost"));
        assert!(assembled.contains("- 方案：仍在"));
    }

    #[test]
    fn duplicate_background_applied_once() {
        // 背景先触发命中；随后表述引用同一条背景（id 去重）＋两条手写
        // 归一化后同文本（文本去重）→ 背景块只多一行，重复项被过滤；
        // 表述自身照常登记与融合。
        let mut s = GhostwriterSession::new();
        let bg = snip("bg1", "素材", SnippetKind::Background);
        let phrasing = with_attachments(
            snip("p1", "方案", SnippetKind::Phrasing),
            vec![
                SnippetAttachment::Reference {
                    snippet_id: "bg1".to_string(),
                },
                SnippetAttachment::Text {
                    text: "手写重复".to_string(),
                },
                SnippetAttachment::Text {
                    text: "  手写重复  ".to_string(),
                },
            ],
        );
        let outcome = s
            .feed(&delta("先说素材再说方案。尾", 0, false), &[bg.clone(), phrasing])
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 2);
        assert_eq!(outcome.new_hits[0].snippet_id, "bg1");
        assert_eq!(outcome.new_hits[1].snippet_id, "p1");
        let assembled = s.assembled_text();
        assert_eq!(assembled.matches("[背景]").count(), 1);
        assert_eq!(assembled.matches(&bg.text).count(), 1);
        assert_eq!(assembled.matches("手写重复").count(), 1);
        assert!(assembled.contains(&format!("- 素材：{}", bg.text)));
        assert!(assembled.contains("- 方案：手写重复"));
    }

    #[test]
    fn background_hit_with_no_remaining_content_registers_nothing() {
        // 表述先引用背景（背景行已贴并登记去重键）；背景自身触发词随后命中 →
        // 去重后无内容可贴 → 不登记命中（无徽标、无撤销单元）。
        let mut s = GhostwriterSession::new();
        let bg = snip("bg1", "素材", SnippetKind::Background);
        let phrasing = with_attachments(
            snip("p1", "方案", SnippetKind::Phrasing),
            vec![SnippetAttachment::Reference {
                snippet_id: "bg1".to_string(),
            }],
        );
        // 列表顺序：表述在前，先解析引用登记背景去重键
        let outcome = s
            .feed(&delta("方案和素材都说了。尾", 0, false), &[phrasing, bg.clone()])
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(outcome.new_hits[0].snippet_id, "p1");
        let assembled = s.assembled_text();
        assert_eq!(assembled.matches("[背景]").count(), 1);
        assert_eq!(assembled.matches(&bg.text).count(), 1);
        // 撤销表述后背景去重键释放：背景再说到可单独贴
        assert!(s.cancel_last_action().is_some());
        let outcome = s.feed(&delta("再提素材", 0, false), &[bg.clone()]).unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(outcome.new_hits[0].snippet_id, "bg1");
    }

    #[test]
    fn cancel_removes_material_and_all_attachment_lines() {
        // 一次命中＝一个撤销单元：融合材料与全部背景行一起撤；撤销后
        // 同 snippet 再说到可再贴（去重键随撤销释放）。
        let mut s = session_with_placement(SnippetPlacement::Head);
        let bg = snip("bg1", "素材", SnippetKind::Background);
        let phrasing = with_attachments(
            snip("p1", "方案", SnippetKind::Phrasing),
            vec![
                SnippetAttachment::Reference {
                    snippet_id: "bg1".to_string(),
                },
                SnippetAttachment::Text {
                    text: "手写".to_string(),
                },
            ],
        );
        let snippets = vec![bg.clone(), phrasing.clone()];
        s.feed(&delta("说个方案", 0, false), &snippets).unwrap();
        let assembled = s.assembled_text();
        assert!(assembled.starts_with("[背景]\n"));
        assert!(assembled.contains(&format!("- 素材：{}", bg.text)));
        assert!(assembled.contains("- 方案：手写"));
        assert!(assembled.ends_with(&phrasing.text));
        // 撤销：材料出待融、全部背景行消失（头块随之消失）
        let cancelled = cancelled_hit(s.cancel_last_action());
        assert_eq!(cancelled.snippet_id, "p1");
        assert_eq!(cancelled.mode, "inline");
        assert_eq!(s.assembled_text(), "说个方案");
        // 再说到可再贴：去重键已释放，引用行重新出现
        let outcome = s.feed(&delta("再说方案", 0, false), &snippets).unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        let assembled = s.assembled_text();
        assert!(assembled.contains("[背景]"));
        assert!(assembled.contains(&format!("- 素材：{}", bg.text)));
    }

    #[test]
    fn cancelled_attachment_background_not_resurrected_by_rewrite() {
        // 表述带引用（库序表述在前）→ 命中生效 → 撤销：被引用背景随撤销一并
        // 记入撤销集——改写式 final（allow_cancelled=false 全量重扫旧文）
        // 不得把刚撤单元的背景行复活；随后新话说到背景触发词仍可再命中。
        let mut s = GhostwriterSession::new();
        let bg = snip("bg1", "素材", SnippetKind::Background);
        let phrasing = with_attachments(
            snip("p1", "方案", SnippetKind::Phrasing),
            vec![SnippetAttachment::Reference {
                snippet_id: "bg1".to_string(),
            }],
        );
        // 库序：表述在前 → 引用先登记背景去重键，背景自身在本次扫描被去重跳过
        let snippets = vec![phrasing.clone(), bg.clone()];
        let outcome = s
            .feed(&delta("先说方案再提素材", 0, false), &snippets)
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(outcome.new_hits[0].snippet_id, "p1");
        assert!(s.assembled_text().contains(&format!("- 素材：{}", bg.text)));

        let cancelled = cancelled_hit(s.cancel_last_action());
        assert_eq!(cancelled.snippet_id, "p1");
        assert!(!s.assembled_text().contains("[背景]"));

        // 改写式 final（非前缀延伸）：游标归零、allow_cancelled=false 全量重扫旧文
        let outcome = s
            .feed(&delta("改成先提素材再说方案。", 0, true), &snippets)
            .unwrap();
        assert!(outcome.new_hits.is_empty());
        let assembled = s.assembled_text();
        assert!(!assembled.contains("[背景]"));
        assert!(!assembled.contains(&bg.text));

        // 新话增量说到背景触发词：撤销集出集，背景可再命中
        let outcome = s.feed(&delta("再聊聊素材", 0, false), &snippets).unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(outcome.new_hits[0].snippet_id, "bg1");
        assert_eq!(outcome.new_hits[0].mode, "tail");
        assert!(s.assembled_text().contains(&format!("- 素材：{}", bg.text)));
    }

}
