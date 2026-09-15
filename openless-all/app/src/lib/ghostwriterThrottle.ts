// ghostwriterThrottle.ts — Ghostwriter 节流间隔输入的取值范围与解析（设置页签用）。
// 单独成模块是为了让边界规则（500–10000 整数）能被测试直接覆盖。

export const THROTTLE_MIN_MS = 500;
export const THROTTLE_MAX_MS = 10000;

/** 越界判定：必须是 500–10000 的整数（后端字段为 u64）；非法一律 null。 */
export function parseThrottleMs(raw: string): number | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  const value = Number(trimmed);
  if (!Number.isInteger(value) || value < THROTTLE_MIN_MS || value > THROTTLE_MAX_MS) return null;
  return value;
}
