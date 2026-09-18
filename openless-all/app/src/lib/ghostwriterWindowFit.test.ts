// 浮框窗口贴合的尺寸阈值判定：内容变化 <4px（两个维度同时）不下发 fit，
// 过滤光标行高、滚动条微调等噪声，防止窗口微抖；任一维度达阈值即贴合。
import { GHOSTWRITER_FIT_THRESHOLD_PX, shouldRefitWindow } from './ghostwriterWindowFit';

const assert = {
  equal(actual: unknown, expected: unknown, message: string) {
    if (actual !== expected) {
      throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`);
    }
  },
};

assert.equal(GHOSTWRITER_FIT_THRESHOLD_PX, 4, '阈值常量应为 4px');

// 同尺寸：不贴合
assert.equal(shouldRefitWindow(520, 200, 520, 200), false, '尺寸未变不贴合');

// 双维度都低于阈值：不贴合
assert.equal(shouldRefitWindow(520, 200, 522, 202), false, '双维度变化 <4px 不贴合');

// 高度达阈值：贴合（候选区浮出/转写流长高）
assert.equal(shouldRefitWindow(520, 200, 520, 205), true, '高度变化 ≥4px 贴合');

// 宽度达阈值：贴合（兜底提示变宽）
assert.equal(shouldRefitWindow(520, 200, 525, 201), true, '宽度变化 ≥4px 贴合');

// 边界：恰好等于阈值算贴合
assert.equal(shouldRefitWindow(520, 200, 524, 200), true, '变化恰为 4px 贴合');

// 负向变化（收起）同样贴合
assert.equal(shouldRefitWindow(520, 396, 520, 150), true, '高度收起 ≥4px 贴合');

// 非法测量值：不贴合
assert.equal(shouldRefitWindow(520, 200, Number.NaN, 300), false, 'NaN 不贴合');
assert.equal(shouldRefitWindow(520, 200, 520, Number.POSITIVE_INFINITY), false, 'Infinity 不贴合');

// 自定义阈值
assert.equal(shouldRefitWindow(520, 200, 521, 200, 1), true, '阈值 1px 时 1px 变化贴合');

console.log('ghostwriter window fit threshold tests passed');
