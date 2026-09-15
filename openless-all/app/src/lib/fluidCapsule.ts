/**
 * Fluid 浮框（capsule 流体样式）的纯逻辑：把 dictation_completed 的 inserted 字段
 * 映射成用户可见的收尾文案。独立成 lib 以便单测（前端测试栈是 node+tsx，无 DOM）。
 */

const DONE_MESSAGES: Record<string, string> = {
  pasteSent: '已发送粘贴，请确认',
  copiedFallback: '已复制，请手动粘贴',
  notRequested: '处理完成',
};

export function completionMessage(inserted?: string | null, chars?: number | null): string {
  if (inserted && DONE_MESSAGES[inserted]) {
    return DONE_MESSAGES[inserted];
  }
  return `已输入 ${chars ?? 0} 字`;
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
 * - fluid_cancel_done：后端 cancel 不推新预览事件也不增 revision，所以成功撤销时
 *   本地换上后端拼装文本并前进一档修订（挡掉撤销前在途的旧预览），同时移除
 *   最近一枚徽标；无可撤销（cancelled=false / payload 缺失）原样返回；
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
    const payload = event.payload as { cancelled?: unknown; assembled?: unknown } | undefined;
    if (!payload || payload.cancelled !== true) return state;
    const hits = state.hits.slice(0, -1);
    if (typeof payload.assembled !== 'string') return { ...state, hits };
    return { ...state, text: payload.assembled, revision: state.revision + 1, hits };
  }
  return state;
}