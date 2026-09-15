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