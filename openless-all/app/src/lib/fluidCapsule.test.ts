import {
  completionNotice,
  emptyFluidPreviewState,
  fluidPanelActionFor,
  fluidPreviewReducer,
  shouldUseFluidCapsule,
} from './fluidCapsule';

// Fluid 浮框纯逻辑测试。与 src/lib 其它测试同风格：自定义 assert + 顶层执行
//（前端测试栈是 node+tsx，无 DOM；不用 node:test，避免 @types/node 依赖）。

function assert(condition: boolean, message: string) {
  if (!condition) throw new Error(message);
}

// --- completionNotice：dictation_completed 的 inserted → 收尾文案 i18n key ---

assert(completionNotice('pasteSent', 12).key === 'fluid.panel.noticePastedConfirm', 'pasteSent 应映射为粘贴确认 key');
assert(completionNotice('copiedFallback', 3).key === 'fluid.panel.noticeCopiedFallback', 'copiedFallback 应映射为复制兜底 key');
assert(completionNotice('notRequested', 8).key === 'fluid.panel.noticeNotRequested', 'notRequested 应映射为处理完成 key');
assert(completionNotice('inserted', 42).count === 42, 'inserted 应携带字数');
assert(completionNotice('some-future-status', 7).count === 7, '未知状态回退到字数');
assert(completionNotice('inserted', undefined).count === 0, '无字数时以 0 兜底');

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

// --- fluidPreviewReducer：浮框三区（命中徽标/指令预览）的纯状态机 ---

const EMPTY = emptyFluidPreviewState();
assert(EMPTY.text === '' && EMPTY.revision === 0 && EMPTY.hits.length === 0, '空状态应为零值');

// preview revision 丢弃旧值（乱序事件按修订号取舍）
{
  const s1 = fluidPreviewReducer(EMPTY, {
    type: 'fluid_preview_changed',
    payload: { text: 'A', revision: 2 },
  });
  assert(s1.text === 'A', '新修订应被接受');
  assert(s1.revision === 2, '接受后修订号同步');
  const s2 = fluidPreviewReducer(s1, {
    type: 'fluid_preview_changed',
    payload: { text: 'B', revision: 1 },
  });
  assert(s2.text === 'A', '旧修订应被丢弃');
  assert(s2.revision === 2, '丢弃时不改修订号');
  const s3 = fluidPreviewReducer(s2, {
    type: 'fluid_preview_changed',
    payload: { text: 'C', revision: 2 },
  });
  assert(s3.text === 'C', '同修订按后到覆盖（后端事件同会话严格递增）');
  const s4 = fluidPreviewReducer(s2, { type: 'fluid_preview_changed' });
  assert(s4 === s2, 'payload 缺失时原样返回');
}

// hit 徽标去重按 snippetId：同 id 两次 → 徽标数组长度 1
{
  const hit = { type: 'fluid_snippets_hit', payload: { snippetId: 's1', title: '翻译', mode: 'footnote' } };
  const s1 = fluidPreviewReducer(EMPTY, hit);
  assert(s1.hits.length === 1 && s1.hits[0].title === '翻译', '首次命中应入列');
  const s2 = fluidPreviewReducer(s1, hit);
  assert(s2.hits.length === 1, '同 snippetId 重复命中不重复入列');
  const s3 = fluidPreviewReducer(s2, {
    type: 'fluid_snippets_hit',
    payload: { snippetId: 's2', title: '术语', mode: 'inline' },
  });
  assert(s3.hits.length === 2, '不同 snippetId 各占一枚');
  const s4 = fluidPreviewReducer(s3, { type: 'fluid_snippets_hit', payload: { snippetId: 's3' } });
  assert(s4.hits.length === 2, 'payload 缺字段时原样返回');
}

// 撤销命中（fluidCancelLast 结果回流）：文本换响应拼装 + 修订号取响应权威值 + 移除最后一枚徽标
{
  const s1 = fluidPreviewReducer(EMPTY, {
    type: 'fluid_preview_changed',
    payload: { text: '第一句。请翻译', revision: 2 },
  });
  const s2 = fluidPreviewReducer(s1, {
    type: 'fluid_snippets_hit',
    payload: { snippetId: 's1', title: '翻译', mode: 'footnote' },
  });
  const s3 = fluidPreviewReducer(s2, {
    type: 'fluid_snippets_hit',
    payload: { snippetId: 's2', title: '术语', mode: 'inline' },
  });
  const s4 = fluidPreviewReducer(s3, {
    type: 'fluid_cancel_done',
    payload: { cancelled: true, assembled: '第一句。', revision: 3 },
  });
  assert(s4.text === '第一句。', '撤销后应显示后端拼装文本');
  assert(s4.revision === 3, '修订号取响应权威值（撤销推进后的 N+1）');
  assert(s4.hits.length === 1 && s4.hits[0].snippetId === 's1', '撤销移除最后一枚徽标');

  // 撤销前在途的旧 preview（revision N）被严格排序丢弃，预览不被旧文本打回
  const s5 = fluidPreviewReducer(s4, {
    type: 'fluid_preview_changed',
    payload: { text: '撤销前的旧预览', revision: 2 },
  });
  assert(s5.text === '第一句。', '撤销后的旧预览事件应被丢弃');
  assert(s5.revision === 3, '丢弃时不改修订号');
  // 后端 feed 路径不再增修订号：撤销后的新预览与响应同修订，按等号规则接受
  const s6 = fluidPreviewReducer(s5, {
    type: 'fluid_preview_changed',
    payload: { text: '第一句。新话', revision: 3 },
  });
  assert(s6.text === '第一句。新话', '撤销后的新预览应按等号规则被接受');
}

// 修订号以后端响应为唯一权威（非本地 +1）：响应跳档时本地跟着跳，
// 在途的低修订预览一律丢弃
{
  const s1 = fluidPreviewReducer(EMPTY, {
    type: 'fluid_preview_changed',
    payload: { text: '第一句。', revision: 2 },
  });
  const s2 = fluidPreviewReducer(s1, {
    type: 'fluid_cancel_done',
    payload: { cancelled: true, revision: 9 },
  });
  assert(s2.revision === 9, '修订号取响应值而非本地 +1');
  assert(s2.text === '第一句。', '响应无拼装文本时保留现有预览文本');
  const s3 = fluidPreviewReducer(s2, {
    type: 'fluid_preview_changed',
    payload: { text: '在途旧预览', revision: 4 },
  });
  assert(s3.text === '第一句。' && s3.revision === 9, '低于响应修订的在途预览应被丢弃');
}

// 无可撤销：cancelled=false 或无 payload → 状态原样；响应缺修订号 → 保持本地档位
{
  const s1 = fluidPreviewReducer(EMPTY, {
    type: 'fluid_cancel_done',
    payload: { cancelled: false, assembled: 'x', revision: 5 },
  });
  assert(s1 === EMPTY, '无可撤销时原样返回');
  const s2 = fluidPreviewReducer(EMPTY, { type: 'fluid_cancel_done' });
  assert(s2 === EMPTY, 'payload 缺失时原样返回');
  const base = fluidPreviewReducer(EMPTY, {
    type: 'fluid_snippets_hit',
    payload: { snippetId: 's1', title: '翻译', mode: 'footnote' },
  });
  const s3 = fluidPreviewReducer(base, {
    type: 'fluid_cancel_done',
    payload: { cancelled: true },
  });
  assert(s3.hits.length === 0 && s3.revision === base.revision, '响应缺修订号只移除徽标');
}

// 未知事件原样返回
{
  const s1 = fluidPreviewReducer(EMPTY, { type: 'something_else', payload: {} });
  assert(s1 === EMPTY, '未知事件原样返回');
}

console.log('fluidCapsule.test.ts: all assertions passed');