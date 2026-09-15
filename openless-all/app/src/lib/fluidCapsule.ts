/**
 * Fluid 浮框（capsule 流体样式）的纯逻辑：dictation_completed 的 inserted →
 * 收尾文案的 i18n key 映射与浮框收放规则。独立成 lib 以便单测（前端测试栈是
 * node+tsx，无 DOM）；具体译文在 i18n 的 fluid.panel.notice* key。
 */

const DONE_NOTICE_KEYS: Record<string, string> = {
  pasteSent: 'fluid.panel.noticePastedConfirm',
  copiedFallback: 'fluid.panel.noticeCopiedFallback',
  notRequested: 'fluid.panel.noticeNotRequested',
};

export function completionNotice(inserted?: string | null, chars?: number | null): {
  key: string;
  count?: number;
} {
  if (inserted && DONE_NOTICE_KEYS[inserted]) {
    return { key: DONE_NOTICE_KEYS[inserted] };
  }
  return { key: 'fluid.panel.noticeInserted', count: chars ?? 0 };
}

/** 是否启用 Fluid 浮框胶囊样式（prefs.capsuleStyle === 'fluid'）。 */
export function shouldUseFluidCapsule(
  prefs: { capsuleStyle?: string | null } | null | undefined,
): boolean {
  return prefs?.capsuleStyle === 'fluid';
}

export type FluidPanelAction = 'show' | 'hide' | 'show-fallback-toast';

/**
 * 浮框收放规则（事件驱动，纯函数）：
 * starting/recording → show，实时转写流；
 * transcribing/polishing/inserting → hide，停止键已按下，落字期间不打扰；
 * completed → hide，字已落进光标就是最好的回执；仅剪贴板兜底（copiedFallback）
 *   与粘贴确认（pasteSent）需要用户动手，用最小 toast 提示；
 * cancelled/failed/idle/未知 → hide，异常路径一律静默收起。
 */
export function fluidPanelActionFor(
  phase: string | null | undefined,
  inserted?: string | null,
): FluidPanelAction {
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

/** 一枚已生效常用语的徽标（fluid_snippets_hit payload）。 */
export interface FluidHitBadge {
  snippetId: string;
  title: string;
  mode: string;
}

/** 浮框顶区（命中徽标）＋中区（指令预览）的共享状态。 */
export interface FluidPreviewState {
  text: string;
  revision: number;
  hits: FluidHitBadge[];
}

/** 后端事件（kind 同结构：snake_case type + camelCase payload）＋撤销回流。 */
export interface FluidPreviewEvent {
  type: string;
  payload?: unknown;
}

export function emptyFluidPreviewState(): FluidPreviewState {
  return { text: '', revision: 0, hits: [] };
}

/**
 * 指令预览纯状态机：
 * - fluid_preview_changed：revision 低于本地即丢弃（乱序/重复事件不留痕）；
 * - fluid_snippets_hit：按 snippetId 去重入列；
 * - fluid_cancel_done：后端撤销成功即推进修订号并发布撤销后的预览事件，
 *   响应里的修订号是后端权威值——本地换上响应的拼装文本（在时）与修订号，
 *   撤销前在途的旧预览（修订号更低）由严格排序丢弃；响应缺修订号
 *   （旧响应/mock）时保持本地修订号。同时移除最近一枚徽标；无可撤销
 *   （cancelled=false / payload 缺失）原样返回；
 * - 未知事件原样返回。
 */
export function fluidPreviewReducer(
  state: FluidPreviewState,
  event: FluidPreviewEvent,
): FluidPreviewState {
  if (event.type === 'fluid_preview_changed') {
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
  if (event.type === 'fluid_snippets_hit') {
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
  if (event.type === 'fluid_cancel_done') {
    const payload = event.payload as
      | { cancelled?: unknown; assembled?: unknown; revision?: unknown }
      | undefined;
    if (!payload || payload.cancelled !== true) return state;
    const hits = state.hits.slice(0, -1);
    const revision =
      typeof payload.revision === 'number' && Number.isFinite(payload.revision)
        ? payload.revision
        : state.revision;
    const base = { ...state, revision, hits };
    return typeof payload.assembled === 'string' ? { ...base, text: payload.assembled } : base;
  }
  return state;
}
