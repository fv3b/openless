// ghostwriterWindowFit.ts — 浮框窗口贴合卡片（2026-09-18 裁决，替代轮询穿透）
// 的纯判定：内容尺寸变化达到阈值才下发 ghostwriter_fit_window，过滤转写光标、
// 滚动条微调等亚阈值噪声，避免窗口微抖。几何换算与锚定在 Rust 侧
//（commands/ghostwriter.rs 的 fit_window_geometry），这里只管「要不要贴合」。

/** 尺寸变化阈值（逻辑 px）：两维变化都低于它时不贴合。 */
export const GHOSTWRITER_FIT_THRESHOLD_PX = 4;

/** 任一维度变化 ≥ threshold 即贴合；非法测量值（NaN/±∞）一律不贴合。 */
export function shouldRefitWindow(
  lastWidth: number,
  lastHeight: number,
  nextWidth: number,
  nextHeight: number,
  threshold: number = GHOSTWRITER_FIT_THRESHOLD_PX,
): boolean {
  if (!Number.isFinite(nextWidth) || !Number.isFinite(nextHeight)) return false;
  return (
    Math.abs(nextWidth - lastWidth) >= threshold ||
    Math.abs(nextHeight - lastHeight) >= threshold
  );
}
