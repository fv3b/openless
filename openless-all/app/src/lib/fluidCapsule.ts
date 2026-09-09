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