/**
 * Ghostwriter 浮框（capsule 流体样式）的纯逻辑：dictation_completed 的 inserted →
 * 收尾文案的 i18n key 映射、浮框收放规则、指令预览状态机与候选区状态机。
 * 独立成 lib 以便单测（前端测试栈是 node+tsx，无 DOM）；
 * 具体译文在 i18n 的 ghostwriter.panel.* key。
 */

import type { GhostwriterAssistState } from './types';

const DONE_NOTICE_KEYS: Record<string, string> = {
  pasteSent: 'ghostwriter.panel.noticePastedConfirm',
  copiedFallback: 'ghostwriter.panel.noticeCopiedFallback',
  notRequested: 'ghostwriter.panel.noticeNotRequested',
};

export function completionNotice(inserted?: string | null, chars?: number | null): {
  key: string;
  count?: number;
} {
  if (inserted && DONE_NOTICE_KEYS[inserted]) {
    return { key: DONE_NOTICE_KEYS[inserted] };
  }
  return { key: 'ghostwriter.panel.noticeInserted', count: chars ?? 0 };
}

/** 是否启用 Ghostwriter 浮框胶囊样式（prefs.capsuleStyle === 'fluid'）。 */
export function shouldUseGhostwriterCapsule(
  prefs: { capsuleStyle?: string | null } | null | undefined,
): boolean {
  return prefs?.capsuleStyle === 'fluid';
}

export type GhostwriterPanelAction = 'show' | 'hide' | 'show-fallback-toast';

/**
 * 浮框收放规则（事件驱动，纯函数）：
 * starting/recording → show，实时转写流；
 * transcribing/polishing/inserting → hide，停止键已按下，落字期间不打扰；
 * completed → hide，字已落进光标就是最好的回执；仅剪贴板兜底（copiedFallback）
 *   与粘贴确认（pasteSent）需要用户动手，用最小 toast 提示；
 * cancelled/failed/idle/未知 → hide，异常路径一律静默收起。
 */
export function ghostwriterPanelActionFor(
  phase: string | null | undefined,
  inserted?: string | null,
): GhostwriterPanelAction {
  switch (phase) {
    case 'starting':
    case 'recording':
      return 'show';
    case 'completed':
      return inserted === 'copiedFallback' || inserted === 'pasteSent'
        ? 'show-fallback-toast'
        : 'hide';
    default:
      return 'hide';
  }
}

// --- 指令预览状态机（纯函数，node 可测） ---

/** 一枚已生效常用语的徽标（ghostwriter_snippets_hit payload）。 */
export interface GhostwriterHitBadge {
  snippetId: string;
  title: string;
  mode: string;
}

/** 浮框顶区（命中徽标）＋中区（指令预览）的共享状态。 */
export interface GhostwriterPreviewState {
  text: string;
  revision: number;
  hits: GhostwriterHitBadge[];
}

/** 后端事件（kind 同结构：snake_case type + camelCase payload）＋撤销回流。 */
export interface GhostwriterEvent {
  type: string;
  payload?: unknown;
}

export function emptyGhostwriterPreviewState(): GhostwriterPreviewState {
  return { text: '', revision: 0, hits: [] };
}

/**
 * 指令预览纯状态机：
 * - ghostwriter_preview_changed：revision 低于本地即丢弃（乱序/重复事件不留痕）；
 * - ghostwriter_snippets_hit：按 snippetId 去重入列；
 * - ghostwriter_cancel_done：后端撤销成功即推进修订号并发布撤销后的预览事件，
 *   响应里的修订号是后端权威值——本地换上响应的拼装文本（在时）与修订号，
 *   撤销前在途的旧预览（修订号更低）由严格排序丢弃；响应缺修订号
 *   （旧响应/mock）时保持本地修订号。撤销语义由响应 action 分流：只有命中
 *   撤销移除最近一枚徽标（选中撤销交给候选区事件刷新）；action 缺失（旧响应）
 *   按命中撤销兜底。无可撤销（cancelled=false / payload 缺失）原样返回；
 * - 未知事件原样返回。
 */
export function ghostwriterPreviewReducer(
  state: GhostwriterPreviewState,
  event: GhostwriterEvent,
): GhostwriterPreviewState {
  if (event.type === 'ghostwriter_preview_changed') {
    const payload = event.payload as { text?: unknown; revision?: unknown } | undefined;
    if (
      !payload ||
      typeof payload.text !== 'string' ||
      typeof payload.revision !== 'number' ||
      !Number.isFinite(payload.revision)
    ) {
      return state;
    }
    if (payload.revision < state.revision) return state;
    return { ...state, text: payload.text, revision: payload.revision };
  }
  if (event.type === 'ghostwriter_snippets_hit') {
    const payload = event.payload as
      | { snippetId?: unknown; title?: unknown; mode?: unknown }
      | undefined;
    if (
      !payload ||
      typeof payload.snippetId !== 'string' ||
      typeof payload.title !== 'string' ||
      typeof payload.mode !== 'string'
    ) {
      return state;
    }
    if (state.hits.some((hit) => hit.snippetId === payload.snippetId)) return state;
    return {
      ...state,
      hits: [...state.hits, { snippetId: payload.snippetId, title: payload.title, mode: payload.mode }],
    };
  }
  if (event.type === 'ghostwriter_cancel_done') {
    const payload = event.payload as
      | { cancelled?: unknown; action?: unknown; assembled?: unknown; revision?: unknown }
      | undefined;
    if (!payload || payload.cancelled !== true) return state;
    // action 由后端权威（"hit"|"selection"|"none"）：选中撤销不动命中徽标；
    // 缺失（旧响应/mock）按命中撤销处理，与历史行为一致。
    const removesHit = payload.action === undefined || payload.action === 'hit';
    const hits = removesHit ? state.hits.slice(0, -1) : state.hits;
    const revision =
      typeof payload.revision === 'number' && Number.isFinite(payload.revision)
        ? payload.revision
        : state.revision;
    const base = { ...state, revision, hits };
    return typeof payload.assembled === 'string' ? { ...base, text: payload.assembled } : base;
  }
  return state;
}

// --- 候选区状态机（纯函数，node 可测） ---

/** 浮框候选区的空状态（无批次＝三行全空）。 */
export function emptyGhostwriterAssistState(): GhostwriterAssistState {
  return { candidateGroups: [], recommendations: [] };
}

/**
 * 候选区纯状态机：只认 ghostwriter_assist_changed，payload 两块整体替换
 * （批次无修订号，事件总线保序，无乱序丢弃逻辑）；payload 缺失/形状不对
 * 与未知事件一律原样返回。
 *
 * conversational=true（对话会话）时推荐行常驻：某次批次推荐为空则保留上一批
 * 的非空推荐（整场只此一份、可刷新不消失），候选组照旧整体替换；会话结束由
 * 调用方重置整个 assist 状态（复位 sticky）。普通会话（false，缺省）行为不变。
 */
export function ghostwriterAssistReducer(
  state: GhostwriterAssistState,
  event: GhostwriterEvent,
  conversational = false,
): GhostwriterAssistState {
  if (event.type !== 'ghostwriter_assist_changed') return state;
  const payload = event.payload as Partial<GhostwriterAssistState> | undefined;
  if (
    !payload ||
    !Array.isArray(payload.candidateGroups) ||
    !Array.isArray(payload.recommendations)
  ) {
    return state;
  }
  const keepStaleRecommendations =
    conversational && payload.recommendations.length === 0 && state.recommendations.length > 0;
  return {
    candidateGroups: payload.candidateGroups,
    recommendations: keepStaleRecommendations ? state.recommendations : payload.recommendations,
  };
}

/**
 * 浮框 ✕（撤销最近一次生效动作）的显隐规则：有命中即显示。
 * 候选与推荐纯展示（2026-09-17 裁决），选中撤销已随选择子系统移除。
 */
export function ghostwriterHasUndoAction(
  preview: GhostwriterPreviewState,
  _assist: GhostwriterAssistState,
): boolean {
  return preview.hits.length > 0;
}
