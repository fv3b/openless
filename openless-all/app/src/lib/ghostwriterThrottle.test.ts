// 节流间隔输入校验的边界行为：500–10000 的整数才合法。
import { THROTTLE_MAX_MS, THROTTLE_MIN_MS, parseThrottleMs } from './ghostwriterThrottle';

const assert = {
  equal(actual: unknown, expected: unknown, message: string) {
    if (actual !== expected) {
      throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`);
    }
  },
};

assert.equal(parseThrottleMs('500'), THROTTLE_MIN_MS, 'lower bound is accepted');
assert.equal(parseThrottleMs('10000'), THROTTLE_MAX_MS, 'upper bound is accepted');
assert.equal(parseThrottleMs(' 2000 '), 2000, 'surrounding whitespace is trimmed');
assert.equal(parseThrottleMs('499'), null, 'below the lower bound is rejected');
assert.equal(parseThrottleMs('10001'), null, 'above the upper bound is rejected');
assert.equal(parseThrottleMs('500.5'), null, 'fractions are rejected (backend field is u64)');
assert.equal(parseThrottleMs(''), null, 'empty input is rejected');
assert.equal(parseThrottleMs('   '), null, 'blank input is rejected');
assert.equal(parseThrottleMs('abc'), null, 'non-numeric input is rejected');
assert.equal(parseThrottleMs('NaN'), null, 'NaN is rejected');
console.log('ghostwriter throttle validation tests passed');
