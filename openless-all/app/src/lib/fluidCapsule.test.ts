import { completionMessage, fluidPanelActionFor, shouldUseFluidCapsule } from './fluidCapsule';

// Fluid 浮框纯逻辑测试。与 src/lib 其它测试同风格：自定义 assert + 顶层执行
//（前端测试栈是 node+tsx，无 DOM；不用 node:test，避免 @types/node 依赖）。

function assert(condition: boolean, message: string) {
  if (!condition) throw new Error(message);
}

// --- completionMessage：dictation_completed 的 inserted → 收尾文案 ---

assert(completionMessage('pasteSent', 12) === '已发送粘贴，请确认', 'pasteSent 应映射为已发送粘贴');
assert(completionMessage('copiedFallback', 3) === '已复制，请手动粘贴', 'copiedFallback 应映射为已复制手动粘贴');
assert(completionMessage('notRequested', 8) === '处理完成', 'notRequested 应映射为处理完成');
assert(completionMessage('inserted', 42) === '已输入 42 字', 'inserted 应显示字数');
assert(completionMessage('some-future-status', 7) === '已输入 7 字', '未知状态回退到字数');
assert(completionMessage('inserted', undefined) === '已输入 0 字', '无字数时以 0 兜底');

// --- shouldUseFluidCapsule：只有 capsuleStyle === fluid 才接管浮框 ---

assert(shouldUseFluidCapsule({ capsuleStyle: 'fluid' }) === true, 'fluid 样式应启用浮框');
assert(shouldUseFluidCapsule({ capsuleStyle: 'siri' }) === false, 'siri 不应启用');
assert(shouldUseFluidCapsule({ capsuleStyle: 'classic' }) === false, 'classic 不应启用');
assert(shouldUseFluidCapsule({}) === false, '缺省样式不应启用');
assert(shouldUseFluidCapsule(null) === false, 'null prefs 不应启用');

// --- fluidPanelActionFor：dictation_state_changed/completed → 浮框动作 ---
// 约定（2026-09 用户拍板）：说话中显示实时转写；按下停止键后浮框立即收起、
// 静默落字进光标；只有剪贴板兜底/粘贴确认这类需要用户动手的收尾才允许显示。

assert(fluidPanelActionFor('starting') === 'show', 'starting 应显示浮框');
assert(fluidPanelActionFor('recording') === 'show', 'recording 应显示浮框');
assert(fluidPanelActionFor('transcribing') === 'hide', '停止后 transcribing 应立即隐藏');
assert(fluidPanelActionFor('polishing') === 'hide', 'polishing 应立即隐藏');
assert(fluidPanelActionFor('inserting') === 'hide', 'inserting 应立即隐藏');
assert(fluidPanelActionFor('cancelled') === 'hide', 'cancelled 应隐藏');
assert(fluidPanelActionFor('failed') === 'hide', 'failed 应隐藏');
assert(fluidPanelActionFor('idle') === 'hide', 'idle 应隐藏');
assert(fluidPanelActionFor(undefined) === 'hide', '未知 phase 应保守隐藏');
assert(
  fluidPanelActionFor('completed', 'inserted') === 'hide',
  '已完成且字已入光标：不留任何展示',
);
assert(fluidPanelActionFor('completed', 'notRequested') === 'hide', '无需插入时也不展示');
assert(
  fluidPanelActionFor('completed', 'copiedFallback') === 'show-fallback-toast',
  '剪贴板兜底需要用户手动粘贴：必须提示',
);
assert(
  fluidPanelActionFor('completed', 'pasteSent') === 'show-fallback-toast',
  '粘贴已发送需要用户确认落点：必须提示',
);
assert(
  fluidPanelActionFor('completed', undefined) === 'hide',
  'completed 无 inserted 字段按已入光标处理',
);

console.log('fluidCapsule.test.ts: all assertions passed');