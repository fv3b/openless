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
//! M3：live assist 批次（候选＋推荐）可口头选中——「用候选N/用常用语N」
//! 在新话增量里识别（浮着才认、失败不剔），剔除先于断句与段产出；
//! 命中与选中混入统一动作序，[`GhostwriterSession::cancel_last_action`]
//! 取最新撤销。

use std::collections::HashSet;

use crate::errors::BackendError;
use crate::types::TranscriptDelta;

use super::segmenter::{Segment, Segmenter};
use super::snippet_store::{Snippet, SnippetMode};
use super::types::{
    CandidateGroupView, CandidateItemView, GhostwriterSnippetHit, LiveRecommendation,
    RecommendationView,
};

pub use super::types::{FeedOutcome, PolishableSegment};
pub use super::types::{AssistSnapshot, LastAction, Selection, SelectionKind};

/// 无标点尾巴的硬切阈值（字符数）：ASR 长时间不出标点时按长度断段。
const DEFAULT_MAX_FORCE_CHARS: usize = 120;

/// prior 截断长度：已润前文只带尾部 200 字符进润色 prompt。
const PRIOR_MAX_CHARS: usize = 200;

/// 一条已生效且未撤销的命中：附注块、待融材料与会话内去重的共同依据。
#[derive(Debug, Clone)]
struct ActiveHit {
    hit: GhostwriterSnippetHit,
    /// 命中时刻的常用语全量文本（附注行与 inline 材料共用）。
    text: String,
    /// inline 命中（材料进待融队列）；false＝footnote（全量文本进附注块）。
    inline: bool,
    /// inline 材料是否仍在待融队列（随段/尾巴润色转移后置 false，撤销不再出队）。
    material_pending: bool,
}

/// 一条已选中且未取消的选中：待融材料与批次视图选中态的共同依据。
#[derive(Debug, Clone)]
struct ActiveSelection {
    selection: Selection,
    /// 材料是否仍在待融队列（随段/尾巴润色转移后置 false，撤销不再出队）。
    material_pending: bool,
}

/// 统一动作序的条目：命中与选中混排，按生效先后记录，撤销取最新。
#[derive(Debug, Clone)]
enum ActiveAction {
    Hit(ActiveHit),
    Selection(ActiveSelection),
}

/// 当前 live assist 批次：现场候选（按组）与推荐常用语。
#[derive(Debug, Clone)]
struct LiveBatch {
    candidates: Vec<Vec<String>>,
    recommendations: Vec<LiveRecommendation>,
}

/// 批次视图的组别标签（按组序固定映射，与 assist 解析侧组序对齐）。
const CANDIDATE_GROUP_KINDS: [&str; 3] = ["term", "phrase", "naming"];

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
    /// 统一动作序：已生效未撤销的命中与选中按生效顺序混排；
    /// 命中同时是 snippet_id 去重依据。
    active_actions: Vec<ActiveAction>,
    /// 当前 live assist 批次；None＝此刻没有可口头选择的批次。
    live_batch: Option<LiveBatch>,
    /// inline 材料待融队列（先进先出，随下一段或尾巴补润转移）。
    inline_pending: Vec<String>,
    /// 缓冲中已做过命中扫描的字符数（只扫增量，改写旧文的替换重扫）。
    scanned_chars: usize,
    /// 撤销过且尚未被新话再触发的 snippet：旧文领地（重扫）保持死亡，
    /// 新话增量仍可再触发（触发即出集，规则 6 的撤销-再生效交互）。
    cancelled_once: HashSet<String>,
    /// 最近一次尾巴补润成功时覆盖的尾巴字符长度；尾巴长度一变即失配，
    /// 旧润色结果对不上新尾巴，作废回落原文。
    tail_polished_covered: usize,
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
            scanned_chars: 0,
            cancelled_once: HashSet::new(),
            tail_polished_covered: 0,
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
        // 口头命令扫描走在命中扫描与断句之前：命令短语连同紧邻的一个标点
        // 从缓冲剔除，段文本与尾巴从此不含命令（段文本剔除因此天然发生在
        // PolishableSegment 产出之前）；无可解析批次/序号越界 → 不剔除、
        // 当普通话。命令只在新话增量里找（与命中扫描同领地），跨增量的
        // 断裂短语不认——与命中匹配同一局限。
        let cursor = self.scanned_chars.min(self.buffer.chars().count());
        let tail_increment: String = self.buffer.chars().skip(cursor).collect();
        let (stripped_increment, new_selections) = self.scan_commands(&tail_increment);
        if stripped_increment != tail_increment {
            let prefix: String = self.buffer.chars().take(cursor).collect();
            self.buffer = format!("{prefix}{stripped_increment}");
        }
        self.scanned_chars = cursor + stripped_increment.chars().count();
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
        // 吃命令剔除后的增量：命令短语不再参与命中匹配。
        new_hits.extend(self.scan_hits(&stripped_increment, snippets, !rewritten));
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
            new_selections,
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
    /// 命中且未生效过 → footnote 进附注、inline 材料进待融队列；
    /// 返回本次新生效的命中。`allow_cancelled`=false 的扫描属旧文领地
    /// （段文本重扫、改写后的全量重扫）：撤销过且未被新话再触发的
    /// snippet 不复活；=true 的扫描是真正的新话增量，可再触发——
    /// 触发即从撤销集出集（规则 6：撤销后同 snippet 再说到可再生效）。
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
            let Some(text) = match_snippet(text, snippet) else {
                continue;
            };
            if allow_cancelled {
                self.cancelled_once.remove(&snippet.id);
            }
            let mode = match snippet.mode {
                SnippetMode::Inline => "inline",
                SnippetMode::Footnote => "footnote",
            };
            let hit = GhostwriterSnippetHit {
                snippet_id: snippet.id.clone(),
                title: snippet.trigger.clone(),
                mode: mode.to_string(),
            };
            match snippet.mode {
                SnippetMode::Inline => {
                    self.inline_pending.push(text.clone());
                    self.active_actions.push(ActiveAction::Hit(ActiveHit {
                        hit: hit.clone(),
                        text,
                        inline: true,
                        material_pending: true,
                    }));
                }
                SnippetMode::Footnote => {
                    self.active_actions.push(ActiveAction::Hit(ActiveHit {
                        hit: hit.clone(),
                        text,
                        inline: false,
                        material_pending: false,
                    }));
                }
            }
            hits.push(hit);
        }
        hits
    }

    fn is_active(&self, snippet_id: &str) -> bool {
        self.active_actions.iter().any(|action| {
            matches!(action, ActiveAction::Hit(active) if active.hit.snippet_id == snippet_id)
        })
    }

    /// 待融材料随段（或尾巴补润）转移后，把仍在排队的命中与选中材料
    /// 标记为已转移。
    fn mark_pending_materials_moved(&mut self) {
        for action in &mut self.active_actions {
            match action {
                ActiveAction::Hit(active) if active.inline => {
                    active.material_pending = false;
                }
                ActiveAction::Selection(active) => {
                    active.material_pending = false;
                }
                _ => {}
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

    /// 撤销最近一次生效的动作（命中与选中混排按生效时间取最新）：
    /// 命中按原撤销语义处理（inline 材料出待融、footnote 移除附注、
    /// snippet 恢复「未生效」可再触发）；选中从待融出队其材料并标记
    /// 未选中（同批次可再选中）。成功撤销推进修订号（后端权威），
    /// 调用方据此发布撤销后的预览并让前端丢弃在途旧预览。
    /// 返回被撤销者供事件确认。
    pub fn cancel_last_action(&mut self) -> Option<LastAction> {
        match self.active_actions.pop()? {
            ActiveAction::Hit(active) => {
                self.cancelled_once.insert(active.hit.snippet_id.clone());
                if active.inline && active.material_pending {
                    if let Some(position) =
                        self.inline_pending.iter().rposition(|m| *m == active.text)
                    {
                        self.inline_pending.remove(position);
                    }
                }
                self.revision += 1;
                Some(LastAction::Hit(active.hit))
            }
            ActiveAction::Selection(active) => {
                if active.material_pending {
                    if let Some(position) = self
                        .inline_pending
                        .iter()
                        .rposition(|m| *m == active.selection.text)
                    {
                        self.inline_pending.remove(position);
                    }
                }
                self.revision += 1;
                Some(LastAction::Selection(active.selection))
            }
        }
    }

    /// 换上新的 live assist 批次（替换式）：新批次到达即清 active
    /// selections（已入待融的材料不受影响，随段/尾巴润色照常转移）。
    pub fn set_live_batch(
        &mut self,
        candidates: Vec<Vec<String>>,
        recommendations: Vec<LiveRecommendation>,
    ) {
        self.live_batch = Some(LiveBatch {
            candidates,
            recommendations,
        });
        self.active_actions
            .retain(|action| matches!(action, ActiveAction::Hit(_)));
    }

    /// 当前批次视图（浮框候选区渲染依据）；无批次 → None。
    /// 候选序号为跨组连续的 1-based 全局序号，与口头命令「用候选N」对齐。
    pub fn assist_snapshot(&self) -> Option<AssistSnapshot> {
        let batch = self.live_batch.as_ref()?;
        let mut next_index = 1usize;
        let candidate_groups = batch
            .candidates
            .iter()
            .enumerate()
            .map(|(group_index, items)| CandidateGroupView {
                kind: CANDIDATE_GROUP_KINDS
                    .get(group_index)
                    .copied()
                    .unwrap_or("phrase")
                    .to_string(),
                items: items
                    .iter()
                    .map(|text| {
                        let index = next_index;
                        next_index += 1;
                        CandidateItemView {
                            index,
                            text: text.clone(),
                            selected: self.is_selection_active(SelectionKind::Candidate, index),
                        }
                    })
                    .collect(),
            })
            .collect();
        let recommendations = batch
            .recommendations
            .iter()
            .enumerate()
            .map(|(i, recommendation)| RecommendationView {
                snippet_id: recommendation.snippet_id.clone(),
                title: recommendation.title.clone(),
                selected: self.is_selection_active(SelectionKind::Recommendation, i + 1),
            })
            .collect();
        Some(AssistSnapshot {
            candidate_groups,
            recommendations,
        })
    }

    /// 切换一条批次内选择的生效态（1-based 全局序号）：
    /// 未选中→选中：材料进待融队列（同 inline 命中路径）、记入动作序；
    /// 已选中→取消：材料从待融按文本 rposition 移除（已转移则不动）
    /// 并标记未选中；无批次/序号越界 → None。两个方向都推进修订号。
    pub fn toggle_selection(&mut self, kind: SelectionKind, index: usize) -> Option<Selection> {
        let text = self.batch_text(kind, index)?;
        if let Some(position) = self.active_actions.iter().position(|action| {
            matches!(action, ActiveAction::Selection(active)
                if active.selection.kind == kind && active.selection.index == index)
        }) {
            let ActiveAction::Selection(active) = self.active_actions.remove(position) else {
                unreachable!("position matched a Selection action");
            };
            if active.material_pending {
                if let Some(pending) = self
                    .inline_pending
                    .iter()
                    .rposition(|m| *m == active.selection.text)
                {
                    self.inline_pending.remove(pending);
                }
            }
            self.revision += 1;
            return Some(active.selection);
        }
        let snippet_id = match kind {
            SelectionKind::Candidate => None,
            SelectionKind::Recommendation => self
                .live_batch
                .as_ref()
                .and_then(|batch| batch.recommendations.get(index - 1))
                .map(|recommendation| recommendation.snippet_id.clone()),
        };
        let selection = Selection {
            kind,
            index,
            snippet_id,
            text,
        };
        self.inline_pending.push(selection.text.clone());
        self.active_actions.push(ActiveAction::Selection(
            ActiveSelection {
                selection: selection.clone(),
                material_pending: true,
            },
        ));
        self.revision += 1;
        Some(selection)
    }

    /// live 批次中某类选择在 1-based 序号处的材料文本；
    /// 无批次或序号越界 → None。
    fn batch_text(&self, kind: SelectionKind, index: usize) -> Option<String> {
        let batch = self.live_batch.as_ref()?;
        let item = match kind {
            SelectionKind::Candidate => batch.candidates.iter().flatten().nth(index.checked_sub(1)?),
            SelectionKind::Recommendation => batch
                .recommendations
                .get(index.checked_sub(1)?)
                .map(|recommendation| &recommendation.text),
        };
        item.cloned()
    }

    /// 某条批次选择当前是否处于选中态（批次视图 selected 标记依据）。
    fn is_selection_active(&self, kind: SelectionKind, index: usize) -> bool {
        self.active_actions.iter().any(|action| {
            matches!(action, ActiveAction::Selection(active)
                if active.selection.kind == kind && active.selection.index == index)
        })
    }

    /// 扫一段新话增量里的口头命令「用候选N」「用常用语N」：有 live 批次且
    /// 序号可解析且未越界 → 调 toggle_selection 生效并返回选中，同时把
    /// 命令短语连同紧邻的一个标点（，。、）从文本剔除；已选中的同一条
    /// 只剔除不重复生效（ASR 快照重发的幂等）；其余情形原样保留（当普通话）。
    /// 返回剔除后的文本与本次新生效的选中。
    fn scan_commands(&mut self, increment: &str) -> (String, Vec<Selection>) {
        let mut selections = Vec::new();
        if self.live_batch.is_none() || increment.is_empty() {
            return (increment.to_string(), selections);
        }
        let chars: Vec<char> = increment.chars().collect();
        let mut kept: Vec<char> = Vec::with_capacity(chars.len());
        let mut i = 0;
        while i < chars.len() {
            let Some((kind, head_len)) = match_command_head(&chars[i..]) else {
                kept.push(chars[i]);
                i += 1;
                continue;
            };
            let number = chars
                .get(i + head_len)
                .copied()
                .and_then(parse_command_number);
            let Some(number) = number else {
                // 序号不可解析：当普通话，头部照抄后从下一字符继续
                kept.extend_from_slice(&chars[i..i + head_len]);
                i += head_len;
                continue;
            };
            if self.batch_text(kind, number).is_none() {
                // 序号越界：不剔除、当普通话
                kept.extend_from_slice(&chars[i..i + head_len + 1]);
                i += head_len + 1;
                continue;
            }
            // 命中：先剔除（短语＋紧邻的一个后续标点），已选中则不再生效
            if !self.is_selection_active(kind, number) {
                if let Some(selection) = self.toggle_selection(kind, number) {
                    selections.push(selection);
                }
            }
            i += head_len + 1;
            if matches!(chars.get(i), Some('，') | Some('。') | Some('、')) {
                i += 1;
            }
        }
        (kept.into_iter().collect(), selections)
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

    /// 指令预览＝最终贴出：主文本（润色回落段原文）＋尾巴（补润回落原文），
    /// 待融剩余材料按材料文本追加在主文本后（润色失败兜底），
    /// 附注块在命中非空时按「- 标题：文本」逐行拼在最后。
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
        let lines: Vec<String> = self
            .active_actions
            .iter()
            .filter_map(|action| match action {
                ActiveAction::Hit(active) if !active.inline => {
                    Some(format!("- {}：{}", active.hit.title, active.text))
                }
                _ => None,
            })
            .collect();
        if !lines.is_empty() {
            assembled.push_str("\n\n[附注]\n");
            assembled.push_str(&lines.join("\n"));
        }
        assembled
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

/// 口头命令头匹配：返回（选择种类, 头长度）。
fn match_command_head(chars: &[char]) -> Option<(SelectionKind, usize)> {
    if chars.starts_with(&['用', '常', '用', '语']) {
        return Some((SelectionKind::Recommendation, 4));
    }
    if chars.starts_with(&['用', '候', '选']) {
        return Some((SelectionKind::Candidate, 3));
    }
    None
}

/// 口头命令序号解析：中文数字一二两三四五六七八与 ASCII 1-8 写死映射，
/// 其余（九/十/0/9+/多字数字）不解析 → 调用方按普通话保留。
fn parse_command_number(c: char) -> Option<usize> {
    match c {
        '一' => Some(1),
        '二' | '两' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '1'..='8' => c.to_digit(10).map(|d| d as usize),
        _ => None,
    }
}

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

    fn snip(id: &str, trigger: &str, mode: SnippetMode) -> Snippet {
        Snippet {
            id: id.to_string(),
            trigger: trigger.to_string(),
            aliases: Vec::new(),
            text: format!("【{id}】{trigger}的完整表述文本"),
            mode,
            enabled: true,
        }
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
        // inline 常用语「翻译」随段命中：材料随 PolishableSegment 转移，待融队列清空
        let mut s = GhostwriterSession::new();
        let snippets = vec![snip("s1", "翻译", SnippetMode::Inline)];
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
            vec![snip("s1", "翻译", SnippetMode::Inline).text]
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
    fn foot_note_hit_records_once_and_dedupes() {
        // 同 snippet 说到两次 → new_hits 只第一次；附注块只拼一条
        let mut s = GhostwriterSession::new();
        let snippet = snip("f1", "附注", SnippetMode::Footnote);
        let snippets = vec![snippet.clone()];
        let outcome = s
            .feed(&delta("帮我加个附注说明。继续", 0, false), &snippets)
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(outcome.new_hits[0].mode, "footnote");
        assert_eq!(outcome.new_hits[0].title, "附注");
        let expected = format!("\n\n[附注]\n- 附注：{}", snippet.text);
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
        assert_eq!(assembled.matches("[附注]").count(), 1);
        assert!(assembled.ends_with(&expected));
    }

    #[test]
    fn alias_match_fires_hit_case_folded() {
        // 别名分支：trigger 未出现，别名 "fy" 以大写形式出现在文本里 → 大小写折叠命中；
        // 无句末标点不成段：inline 材料留在待融队列，追加在转写后
        let mut s = GhostwriterSession::new();
        let mut snippet = snip("s-fy", "翻译", SnippetMode::Inline);
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
        let mut snippet = snip("s-alias", "翻译", SnippetMode::Inline);
        snippet.aliases = vec![" a ".to_string(), String::new(), "  b  ".to_string()];
        let snippets = vec![snippet];
        let outcome = s
            .feed(&delta("请B一下。", 0, false), &snippets)
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
    }

    #[test]
    fn assembled_joins_polished_with_fallback_and_footnote_block() {
        // 两段：第 1 段已润、第 2 段未润回落原文；附注块格式「- 标题：文本」
        let mut s = GhostwriterSession::new();
        let snippet = snip("f1", "标记", SnippetMode::Footnote);
        let snippets = vec![snippet.clone()];
        s.feed(&delta("第一句原文。标记一下然后", 0, false), &snippets)
            .unwrap();
        s.feed(&delta("第二句原文。尾", 0, false), &snippets).unwrap();
        assert_eq!(s.segments().len(), 2);
        assert!(s.apply_polished(0, "第一句润好。".to_string()));
        assert_eq!(
            s.assembled_text(),
            format!(
                "第一句润好。标记一下然后第二句原文。尾\n\n[附注]\n- 标记：{}",
                snippet.text
            )
        );
    }

    #[test]
    fn cancel_last_hit_removes_latest_and_allows_re_hit() {
        // footnote：撤销后附注块消失，同 snippet 再说到 → 再次生效
        let mut s = GhostwriterSession::new();
        let snippet = snip("f1", "附注", SnippetMode::Footnote);
        let snippets = vec![snippet.clone()];
        s.feed(&delta("第一句。", 0, false), &snippets).unwrap();
        s.feed(&delta("加个附注", 0, false), &snippets).unwrap();
        let cancelled = cancelled_hit(s.cancel_last_action());
        assert_eq!(cancelled.snippet_id, "f1");
        assert_eq!(cancelled.mode, "footnote");
        assert!(!s.assembled_text().contains("[附注]"));
        assert!(s.cancel_last_action().is_none());
        let outcome = s.feed(&delta("再说附注。完", 0, false), &snippets).unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert!(s.assembled_text().contains(&format!("- 附注：{}", snippet.text)));

        // inline：撤销把材料从待融队列出队，预览不再追加；可再次生效。
        // 「请翻译一下」后无句末标点不产段：材料留在待融队列等尾巴补润。
        let mut s = GhostwriterSession::new();
        let inline = snip("s1", "翻译", SnippetMode::Inline);
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
        // 重生效的 inline 材料：或留在待融队列（未断句）或已随段转移，不散落
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
        let inline = snip("s1", "翻译", SnippetMode::Inline);
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
        // footnote：命中→撤销→final 前缀延伸含触发词 → 不复活
        // （重扫旧文不算「再说到」，规则 6 只认新话）
        let mut s = GhostwriterSession::new();
        let snippet = snip("f1", "附注", SnippetMode::Footnote);
        let snippets = vec![snippet.clone()];
        s.feed(&delta("第一句。", 0, false), &snippets).unwrap();
        s.feed(&delta("加个附注", 0, false), &snippets).unwrap();
        assert!(s.cancel_last_action().is_some());
        let outcome = s.feed(&delta("第一句。加个附注。", 0, true), &snippets).unwrap();
        assert!(outcome.new_hits.is_empty());
        assert!(!s.assembled_text().contains("[附注]"));

        // 全量替换型 final（非前缀延伸）：全量重扫但被撤销者保持死亡
        let outcome = s.feed(&delta("换个说法提到附注。", 0, true), &snippets).unwrap();
        assert!(outcome.new_hits.is_empty());
        assert!(!s.assembled_text().contains("[附注]"));

        // 规则 6 仍成立：之后真正的新话再说到 → 再次生效
        let outcome = s.feed(&delta("再提附注一下", 0, false), &snippets).unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert!(s.assembled_text().contains(&format!("- 附注：{}", snippet.text)));

        // inline：撤销后 final 不把材料送回待融队列
        let mut s = GhostwriterSession::new();
        let inline = snip("s1", "翻译", SnippetMode::Inline);
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
        let snippet = snip("f1", "附注", SnippetMode::Footnote);
        let snippets = vec![snippet.clone()];
        let outcome = s
            .feed(&delta("加个附注。完毕", 0, false), &snippets)
            .unwrap();
        assert_eq!(outcome.new_hits.len(), 1);
        assert_eq!(s.revision(), 0);
        assert!(cancelled_hit(s.cancel_last_action()).snippet_id == "f1");
        assert_eq!(s.revision(), 1);
        assert!(!s.assembled_text().contains("[附注]"));
        // 撤销后再喂新内容：预览反映新内容，修订号保持（feed 路径不增）
        s.feed(&delta("，新话来了。", 0, false), &snippets).unwrap();
        assert_eq!(s.revision(), 1);
        assert!(s.assembled_text().contains("新话来了"));
        assert!(!s.assembled_text().contains("[附注]"));
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

    #[test]
    fn voice_command_strips_from_tail_and_selects() {
        let mut s = GhostwriterSession::new();
        assert!(s.assist_snapshot().is_none());
        s.set_live_batch(
            vec![vec![
                "第一个候选的完整文本".into(),
                "第二个候选的完整文本".into(),
            ]],
            vec![],
        );
        let outcome = s.feed(&delta("帮我看看用候选2", 0, false), &[]).unwrap();
        assert_eq!(s.debug_buffer(), "帮我看看");
        assert_eq!(outcome.new_selections.len(), 1);
        assert_eq!(outcome.new_selections[0].index, 2);
        assert_eq!(outcome.new_selections[0].text, "第二个候选的完整文本");
        assert!(outcome.new_selections[0].snippet_id.is_none());
        // 材料进待融队列：尾巴补润输入携带，预览兜底追加可见
        let input = s.tail_polish_input().unwrap();
        assert!(input.materials.contains(&"第二个候选的完整文本".to_string()));
        assert!(s.assembled_text().ends_with("第二个候选的完整文本"));
        // 已选中再命令：仅剔除不重复入队（不取消、不重复生效）
        let outcome = s.feed(&delta("，再用候选2", 0, false), &[]).unwrap();
        assert!(outcome.new_selections.is_empty());
        assert_eq!(s.debug_buffer(), "帮我看看，再");
        assert_eq!(s.assembled_text().matches("第二个候选的完整文本").count(), 1);
    }

    #[test]
    fn voice_command_in_segment_strips_before_polish() {
        let mut s = GhostwriterSession::new();
        s.set_live_batch(
            vec![vec![
                "甲候选".into(),
                "乙候选".into(),
                "丙候选".into(),
            ]],
            vec![],
        );
        let outcome = s
            .feed(&delta("第一句。用候选三，然后第二句", 0, false), &[])
            .unwrap();
        assert_eq!(outcome.new_segments.len(), 1);
        assert_eq!(outcome.new_segments[0].text, "第一句。");
        assert_eq!(outcome.new_selections.len(), 1);
        assert_eq!(outcome.new_selections[0].text, "丙候选");
        assert_eq!(s.debug_buffer(), "第一句。然后第二句");
    }

    #[test]
    fn unresolvable_command_keeps_text() {
        let mut s = GhostwriterSession::new();
        let outcome = s.feed(&delta("用候选二看看", 0, false), &[]).unwrap();
        assert_eq!(s.debug_buffer(), "用候选二看看");
        assert!(outcome.new_selections.is_empty());
    }

    #[test]
    fn out_of_range_command_keeps_text() {
        let mut s = GhostwriterSession::new();
        s.set_live_batch(vec![vec!["甲".into(), "乙".into()]], vec![]);
        let outcome = s.feed(&delta("用候选五看看", 0, false), &[]).unwrap();
        assert_eq!(s.debug_buffer(), "用候选五看看");
        assert!(outcome.new_selections.is_empty());
        // 推荐列表为空：用常用语N 同样越界不剔除
        let outcome = s.feed(&delta("再试试用常用语一", 0, false), &[]).unwrap();
        assert_eq!(s.debug_buffer(), "用候选五看看再试试用常用语一");
        assert!(outcome.new_selections.is_empty());
    }

    #[test]
    fn toggle_selection_adds_and_removes_material() {
        let mut s = GhostwriterSession::new();
        assert!(s.toggle_selection(SelectionKind::Candidate, 1).is_none());
        s.set_live_batch(
            vec![vec!["甲候选文本".into()]],
            vec![LiveRecommendation {
                snippet_id: "r1".into(),
                title: "推荐标题".into(),
                text: "推荐一的全量文本".into(),
            }],
        );
        assert!(s.toggle_selection(SelectionKind::Candidate, 2).is_none());
        assert!(s.toggle_selection(SelectionKind::Recommendation, 2).is_none());
        let selection = s.toggle_selection(SelectionKind::Candidate, 1).unwrap();
        assert_eq!(selection.text, "甲候选文本");
        assert!(selection.snippet_id.is_none());
        assert!(s.assembled_text().ends_with("甲候选文本"));
        // 再 toggle 同一条：取消选中，材料出待融
        let deselected = s.toggle_selection(SelectionKind::Candidate, 1).unwrap();
        assert_eq!(deselected.text, "甲候选文本");
        assert!(!s.assembled_text().contains("甲候选文本"));
        // 推荐选中：材料带 snippet_id 进待融
        let rec = s.toggle_selection(SelectionKind::Recommendation, 1).unwrap();
        assert_eq!(rec.snippet_id.as_deref(), Some("r1"));
        assert!(s.assembled_text().ends_with("推荐一的全量文本"));
    }

    #[test]
    fn new_batch_clears_selections_but_keeps_materials() {
        let mut s = GhostwriterSession::new();
        s.set_live_batch(vec![vec!["甲候选文本".into(), "乙候选文本".into()]], vec![]);
        s.toggle_selection(SelectionKind::Candidate, 1).unwrap();
        let snapshot = s.assist_snapshot().unwrap();
        assert!(snapshot.candidate_groups[0].items[0].selected);
        assert_eq!(snapshot.candidate_groups[0].items[0].index, 1);
        // 新批次替换：选中态清空，已入待融的材料不受影响
        s.set_live_batch(vec![vec!["丙候选文本".into()]], vec![]);
        let snapshot = s.assist_snapshot().unwrap();
        assert_eq!(snapshot.candidate_groups[0].kind, "term");
        assert_eq!(snapshot.candidate_groups[0].items[0].text, "丙候选文本");
        assert!(!snapshot.candidate_groups[0].items[0].selected);
        assert!(s.assembled_text().ends_with("甲候选文本"));
        // 清空后同序号按新批次文本重新选中
        let selection = s.toggle_selection(SelectionKind::Candidate, 1).unwrap();
        assert_eq!(selection.text, "丙候选文本");
        // 组别标签固定序＋候选全局 1-based 连续计数
        s.set_live_batch(
            vec![vec!["甲".into()], vec!["乙".into()], vec!["丙".into()]],
            vec![],
        );
        let snapshot = s.assist_snapshot().unwrap();
        let kinds: Vec<&str> = snapshot
            .candidate_groups
            .iter()
            .map(|group| group.kind.as_str())
            .collect();
        assert_eq!(kinds, vec!["term", "phrase", "naming"]);
        let indices: Vec<usize> = snapshot
            .candidate_groups
            .iter()
            .flat_map(|group| group.items.iter().map(|item| item.index))
            .collect();
        assert_eq!(indices, vec![1, 2, 3]);
    }

    #[test]
    fn cancel_last_action_mixed_order() {
        let mut s = GhostwriterSession::new();
        let snippet = snip("f1", "附注", SnippetMode::Footnote);
        let snippets = vec![snippet.clone()];
        s.feed(&delta("加个附注", 0, false), &snippets).unwrap();
        s.set_live_batch(vec![vec!["甲候选文本".into()]], vec![]);
        s.toggle_selection(SelectionKind::Candidate, 1).unwrap();
        // 最新生效的是选中 → 先撤销选中，材料出待融
        match s.cancel_last_action().unwrap() {
            LastAction::Selection(selection) => assert_eq!(selection.text, "甲候选文本"),
            LastAction::Hit(_) => panic!("期望先撤销的是选中"),
        }
        assert!(!s.assembled_text().contains("甲候选文本"));
        // 再撤销 → 命中
        match s.cancel_last_action().unwrap() {
            LastAction::Hit(hit) => assert_eq!(hit.snippet_id, "f1"),
            LastAction::Selection(_) => panic!("期望再撤销的是命中"),
        }
        assert!(!s.assembled_text().contains("[附注]"));
        assert!(s.cancel_last_action().is_none());
    }
}
