import {
  completionNotice,
  emptyGhostwriterAssistState,
  emptyGhostwriterPreviewState,
  ghostwriterAssistReducer,
  ghostwriterHasUndoAction,
  ghostwriterPanelActionFor,
  ghostwriterPreviewReducer,
  ghostwriterProcessingExpired,
  ghostwriterStageKey,
  ghostwriterStageOverrideFromEvent,
  GHOSTWRITER_PROCESSING_TIMEOUT_MS,
  shouldUseGhostwriterCapsule,
} from './ghostwriterCapsule';
import { applyTranscriptEvent, type TranscriptViewState } from './backendEvent';

// Ghostwriter 浮框纯逻辑测试。与 src/lib 其它测试同风格：自定义 assert + 顶层执行
//（前端测试栈是 node+tsx，无 DOM；不用 node:test，避免 @types/node 依赖）。

function assert(condition: boolean, message: string) {
  if (!condition) throw new Error(message);
}

// --- completionNotice：dictation_completed 的 inserted → 收尾文案 i18n key ---

assert(completionNotice('pasteSent', 12).key === 'ghostwriter.panel.noticePastedConfirm', 'pasteSent 应映射为粘贴确认 key');
assert(completionNotice('copiedFallback', 3).key === 'ghostwriter.panel.noticeCopiedFallback', 'copiedFallback 应映射为复制兜底 key');
assert(completionNotice('notRequested', 8).key === 'ghostwriter.panel.noticeNotRequested', 'notRequested 应映射为处理完成 key');
assert(completionNotice('inserted', 42).count === 42, 'inserted 应携带字数');
assert(completionNotice('some-future-status', 7).count === 7, '未知状态回退到字数');
assert(completionNotice('inserted', undefined).count === 0, '无字数时以 0 兜底');

// --- shouldUseGhostwriterCapsule：只有 capsuleStyle === ghostwriter 才接管浮框 ---

assert(shouldUseGhostwriterCapsule({ capsuleStyle: 'fluid' }) === true, "'fluid' 样式应启用浮框");
assert(shouldUseGhostwriterCapsule({ capsuleStyle: 'siri' }) === false, 'siri 不应启用');
assert(shouldUseGhostwriterCapsule({ capsuleStyle: 'classic' }) === false, 'classic 不应启用');
assert(shouldUseGhostwriterCapsule({}) === false, '缺省样式不应启用');
assert(shouldUseGhostwriterCapsule(null) === false, 'null prefs 不应启用');

// --- ghostwriterPanelActionFor：dictation_state_changed/completed → 浮框动作 ---
// 约定（2026-09-18 批次 B 用户裁决）：按下停止键浮框**不收起**，显示处理阶段
// （正在润色/正在写入…）；内容真正落进光标（dictation_completed inserted）才收
// ——消失＝粘贴完成＝可以继续下一个动作的信号。插入失败/处理失败浮框保留
// 错误信息等用户手动关；只有剪贴板兜底/粘贴确认以浮框内提示呈现。

assert(ghostwriterPanelActionFor('starting') === 'show', 'starting 应显示浮框');
assert(ghostwriterPanelActionFor('recording') === 'show', 'recording 应显示浮框');
assert(
  ghostwriterPanelActionFor('transcribing') === 'show',
  '停止后 transcribing 浮框保留（显示阶段行）',
);
assert(ghostwriterPanelActionFor('polishing') === 'show', 'polishing 浮框保留');
assert(ghostwriterPanelActionFor('inserting') === 'show', 'inserting 浮框保留（正在写入）');
assert(ghostwriterPanelActionFor('failed') === 'show', 'failed 浮框保留错误信息，等手动关');
assert(ghostwriterPanelActionFor('cancelled') === 'hide', 'cancelled 应隐藏');
assert(ghostwriterPanelActionFor('idle') === 'hide', 'idle 应隐藏');
assert(ghostwriterPanelActionFor(undefined) === 'hide', '未知 phase 应保守隐藏');
assert(
  ghostwriterPanelActionFor('completed', 'inserted') === 'hide',
  '已完成且字已入光标：浮框收起（粘贴完成信号）',
);
assert(ghostwriterPanelActionFor('completed', 'notRequested') === 'hide', '无需插入时也不展示');
assert(
  ghostwriterPanelActionFor('completed', 'copiedFallback') === 'show-fallback-toast',
  '剪贴板兜底需要用户手动粘贴：浮框内提示',
);
assert(
  ghostwriterPanelActionFor('completed', 'pasteSent') === 'show-fallback-toast',
  '粘贴已发送需要用户确认落点：浮框内提示',
);
assert(
  ghostwriterPanelActionFor('completed', undefined) === 'hide',
  'completed 无 inserted 字段按已入光标处理',
);

// --- ghostwriterStageOverrideFromEvent：ghostwriter_stage_changed 载荷校验 ---

assert(
  ghostwriterStageOverrideFromEvent('polishing') === 'polishing',
  'polishing 阶段应被接受',
);
assert(
  ghostwriterStageOverrideFromEvent('finalizing') === 'finalizing',
  'finalizing 阶段应被接受',
);
assert(ghostwriterStageOverrideFromEvent('unknown') === null, '未知阶段应被拒绝');
assert(ghostwriterStageOverrideFromEvent(undefined) === null, '载荷缺失应被拒绝');
assert(ghostwriterStageOverrideFromEvent(42) === null, '非字符串应被拒绝');

// --- ghostwriterStageKey：收尾阶段的阶段行文案 key ---
// ghostwriter 会话润色模式强制 Raw（引擎不进入 polishing 相），stop 后的 LLM
// 等待发生在 transcribing 相——阶段行以 ghostwriter_stage_changed 事件为准，
// 相位映射只做兜底。

assert(
  ghostwriterStageKey('transcribing', null, false) === 'ghostwriter.panel.stageTranscribing',
  'transcribing 无阶段事件兜底显示正在识别',
);
assert(
  ghostwriterStageKey('transcribing', 'polishing', false) === 'ghostwriter.panel.stagePolishing',
  '补润阶段事件覆盖 transcribing 相：正在润色',
);
assert(
  ghostwriterStageKey('transcribing', 'finalizing', false) === 'ghostwriter.panel.stageFinalizing',
  '出稿阶段事件覆盖 transcribing 相：正在出稿',
);
assert(
  ghostwriterStageKey('polishing', null, false) === 'ghostwriter.panel.stagePolishing',
  'polishing 相兜底显示正在润色',
);
assert(
  ghostwriterStageKey('polishing', null, true) === 'ghostwriter.panel.stageFinalizing',
  '对话会话 polishing 相兜底显示正在出稿',
);
assert(
  ghostwriterStageKey('inserting', 'polishing', false) === 'ghostwriter.panel.stageInserting',
  '写入相优先于阶段事件：正在写入',
);
assert(ghostwriterStageKey('recording', null, false) === null, '录音相无阶段行');
assert(ghostwriterStageKey('completed', null, false) === null, '完成无阶段行');
assert(ghostwriterStageKey('failed', null, false) === null, '失败无阶段行（错误态接管）');

// --- ghostwriterProcessingExpired：收尾护栏（处理卡死不永久占屏） ---

assert(GHOSTWRITER_PROCESSING_TIMEOUT_MS === 60_000, '护栏阈值应为 60 秒');
assert(ghostwriterProcessingExpired(null, Date.now()) === false, '未进入处理阶段不超时');
const startedAt = 1_000_000;
assert(
  ghostwriterProcessingExpired(startedAt, startedAt + GHOSTWRITER_PROCESSING_TIMEOUT_MS - 1) ===
    false,
  '阈值内不算超时',
);
assert(
  ghostwriterProcessingExpired(startedAt, startedAt + GHOSTWRITER_PROCESSING_TIMEOUT_MS) === true,
  '达到阈值即超时（转失败态显示错误，可手动关）',
);

// --- ghostwriterPreviewReducer：浮框三区（命中徽标/指令预览）的纯状态机 ---

const EMPTY = emptyGhostwriterPreviewState();
assert(EMPTY.text === '' && EMPTY.revision === 0 && EMPTY.hits.length === 0, '空状态应为零值');

// preview revision 丢弃旧值（乱序事件按修订号取舍）
{
  const s1 = ghostwriterPreviewReducer(EMPTY, {
    type: 'ghostwriter_preview_changed',
    payload: { text: 'A', revision: 2 },
  });
  assert(s1.text === 'A', '新修订应被接受');
  assert(s1.revision === 2, '接受后修订号同步');
  const s2 = ghostwriterPreviewReducer(s1, {
    type: 'ghostwriter_preview_changed',
    payload: { text: 'B', revision: 1 },
  });
  assert(s2.text === 'A', '旧修订应被丢弃');
  assert(s2.revision === 2, '丢弃时不改修订号');
  const s3 = ghostwriterPreviewReducer(s2, {
    type: 'ghostwriter_preview_changed',
    payload: { text: 'C', revision: 2 },
  });
  assert(s3.text === 'C', '同修订按后到覆盖（后端事件同会话严格递增）');
  const s4 = ghostwriterPreviewReducer(s2, { type: 'ghostwriter_preview_changed' });
  assert(s4 === s2, 'payload 缺失时原样返回');
}

// hit 徽标去重按 snippetId：同 id 两次 → 徽标数组长度 1
{
  const hit = { type: 'ghostwriter_snippets_hit', payload: { snippetId: 's1', title: '翻译', mode: 'footnote' } };
  const s1 = ghostwriterPreviewReducer(EMPTY, hit);
  assert(s1.hits.length === 1 && s1.hits[0].title === '翻译', '首次命中应入列');
  const s2 = ghostwriterPreviewReducer(s1, hit);
  assert(s2.hits.length === 1, '同 snippetId 重复命中不重复入列');
  const s3 = ghostwriterPreviewReducer(s2, {
    type: 'ghostwriter_snippets_hit',
    payload: { snippetId: 's2', title: '术语', mode: 'inline' },
  });
  assert(s3.hits.length === 2, '不同 snippetId 各占一枚');
  const s4 = ghostwriterPreviewReducer(s3, { type: 'ghostwriter_snippets_hit', payload: { snippetId: 's3' } });
  assert(s4.hits.length === 2, 'payload 缺字段时原样返回');
}

// 撤销命中（ghostwriterCancelLast 结果回流）：文本换响应拼装 + 修订号取响应权威值 + 移除最后一枚徽标
{
  const s1 = ghostwriterPreviewReducer(EMPTY, {
    type: 'ghostwriter_preview_changed',
    payload: { text: '第一句。请翻译', revision: 2 },
  });
  const s2 = ghostwriterPreviewReducer(s1, {
    type: 'ghostwriter_snippets_hit',
    payload: { snippetId: 's1', title: '翻译', mode: 'footnote' },
  });
  const s3 = ghostwriterPreviewReducer(s2, {
    type: 'ghostwriter_snippets_hit',
    payload: { snippetId: 's2', title: '术语', mode: 'inline' },
  });
  const s4 = ghostwriterPreviewReducer(s3, {
    type: 'ghostwriter_cancel_done',
    payload: { cancelled: true, assembled: '第一句。', revision: 3 },
  });
  assert(s4.text === '第一句。', '撤销后应显示后端拼装文本');
  assert(s4.revision === 3, '修订号取响应权威值（撤销推进后的 N+1）');
  assert(s4.hits.length === 1 && s4.hits[0].snippetId === 's1', '撤销移除最后一枚徽标');

  // 撤销前在途的旧 preview（revision N）被严格排序丢弃，预览不被旧文本打回
  const s5 = ghostwriterPreviewReducer(s4, {
    type: 'ghostwriter_preview_changed',
    payload: { text: '撤销前的旧预览', revision: 2 },
  });
  assert(s5.text === '第一句。', '撤销后的旧预览事件应被丢弃');
  assert(s5.revision === 3, '丢弃时不改修订号');
  // 后端 feed 路径不再增修订号：撤销后的新预览与响应同修订，按等号规则接受
  const s6 = ghostwriterPreviewReducer(s5, {
    type: 'ghostwriter_preview_changed',
    payload: { text: '第一句。新话', revision: 3 },
  });
  assert(s6.text === '第一句。新话', '撤销后的新预览应按等号规则被接受');
}

// 修订号以后端响应为唯一权威（非本地 +1）：响应跳档时本地跟着跳，
// 在途的低修订预览一律丢弃
{
  const s1 = ghostwriterPreviewReducer(EMPTY, {
    type: 'ghostwriter_preview_changed',
    payload: { text: '第一句。', revision: 2 },
  });
  const s2 = ghostwriterPreviewReducer(s1, {
    type: 'ghostwriter_cancel_done',
    payload: { cancelled: true, revision: 9 },
  });
  assert(s2.revision === 9, '修订号取响应值而非本地 +1');
  assert(s2.text === '第一句。', '响应无拼装文本时保留现有预览文本');
  const s3 = ghostwriterPreviewReducer(s2, {
    type: 'ghostwriter_preview_changed',
    payload: { text: '在途旧预览', revision: 4 },
  });
  assert(s3.text === '第一句。' && s3.revision === 9, '低于响应修订的在途预览应被丢弃');
}

// 无可撤销：cancelled=false 或无 payload → 状态原样；响应缺修订号 → 保持本地档位
{
  const s1 = ghostwriterPreviewReducer(EMPTY, {
    type: 'ghostwriter_cancel_done',
    payload: { cancelled: false, assembled: 'x', revision: 5 },
  });
  assert(s1 === EMPTY, '无可撤销时原样返回');
  const s2 = ghostwriterPreviewReducer(EMPTY, { type: 'ghostwriter_cancel_done' });
  assert(s2 === EMPTY, 'payload 缺失时原样返回');
  const base = ghostwriterPreviewReducer(EMPTY, {
    type: 'ghostwriter_snippets_hit',
    payload: { snippetId: 's1', title: '翻译', mode: 'footnote' },
  });
  const s3 = ghostwriterPreviewReducer(base, {
    type: 'ghostwriter_cancel_done',
    payload: { cancelled: true },
  });
  assert(s3.hits.length === 0 && s3.revision === base.revision, '响应缺修订号只移除徽标');
}

// 未知事件原样返回
{
  const s1 = ghostwriterPreviewReducer(EMPTY, { type: 'something_else', payload: {} });
  assert(s1 === EMPTY, '未知事件原样返回');
}

// 撤销语义分流：action 由后端权威（命中/选中统一撤销）——只有命中撤销落徽标
{
  const base = ghostwriterPreviewReducer(EMPTY, {
    type: 'ghostwriter_snippets_hit',
    payload: { snippetId: 's1', title: '翻译', mode: 'footnote' },
  });
  const s1 = ghostwriterPreviewReducer(base, {
    type: 'ghostwriter_cancel_done',
    payload: { cancelled: true, action: 'selection', assembled: '甲', revision: 5 },
  });
  assert(s1.hits.length === 1 && s1.text === '甲', '撤销选中：换拼装文本、命中徽标原样');
  const s2 = ghostwriterPreviewReducer(s1, {
    type: 'ghostwriter_cancel_done',
    payload: { cancelled: true, action: 'hit', assembled: '乙', revision: 6 },
  });
  assert(s2.hits.length === 0 && s2.text === '乙', '撤销命中：移除最后一枚徽标');
  const withHit = ghostwriterPreviewReducer(EMPTY, {
    type: 'ghostwriter_snippets_hit',
    payload: { snippetId: 's2', title: '术语', mode: 'inline' },
  });
  const s3 = ghostwriterPreviewReducer(withHit, {
    type: 'ghostwriter_cancel_done',
    payload: { cancelled: true, assembled: '丙', revision: 7 },
  });
  assert(s3.hits.length === 0, '旧响应缺 action 时按命中撤销兜底（与历史行为一致）');
}

// --- ghostwriterAssistReducer：候选区（候选组/推荐）的纯状态机 ---
// 候选组改为累积合并（2026-09-18 批次 A 裁决；同日复核改纯累积）：新批次并入
// 现有集合、按 text 去重、空批次不清空、**无上限不淘汰**（会话结束才清）；
// 推荐照旧整体替换。

const EMPTY_ASSIST = emptyGhostwriterAssistState();
assert(
  EMPTY_ASSIST.candidateGroups.length === 0 &&
    EMPTY_ASSIST.recommendations.length === 0,
  'assist 空状态应为零值',
);

// 候选并入：新批次条目并入现有集合，不同批次共存；推荐照旧整体替换
{
  const s1 = ghostwriterAssistReducer(EMPTY_ASSIST, {
    type: 'ghostwriter_assist_changed',
    payload: {
      candidateGroups: [
        { kind: 'term', items: [{ index: 1, text: '灰度发布' }] },
        { kind: 'naming', items: [{ index: 2, text: '先在小范围试运行' }] },
      ],
      recommendations: [{ snippetId: 's1', title: '项目背景' }],
    },
  });
  assert(s1.candidateGroups.length === 2, '首批候选应入列');
  assert(
    s1.recommendations.length === 1 && s1.recommendations[0].snippetId === 's1',
    '推荐照旧整体替换',
  );
  // 新批次含重复 text（灰度发布）与新条目：重复不重排、新条目追加，序号跨组重排连续
  const s2 = ghostwriterAssistReducer(s1, {
    type: 'ghostwriter_assist_changed',
    payload: {
      candidateGroups: [
        { kind: 'term', items: [{ index: 1, text: '灰度发布' }, { index: 2, text: '金丝雀发布' }] },
      ],
      recommendations: [],
    },
  });
  const flat = s2.candidateGroups.flatMap(group => group.items.map(item => item.text));
  // 组序按 kind 首现；组内既有条目原位不动、新条目追加在组尾。
  assert(
    JSON.stringify(flat) === JSON.stringify(['灰度发布', '金丝雀发布', '先在小范围试运行']),
    '并入应按 text 去重，新条目并入同 kind 组尾',
  );
  assert(
    s2.candidateGroups.map(group => group.kind).join(',') === 'term,naming',
    'kind 首现顺序即组序',
  );
  assert(
    s2.candidateGroups.flatMap(group => group.items.map(item => item.index)).join(',') === '1,2,3',
    '并入后 index 应跨组重排 1..N',
  );
  // 空批次不清空既有条目（用户实测痛点：整批替换会冲掉上一批非空提示）
  const s3 = ghostwriterAssistReducer(s2, {
    type: 'ghostwriter_assist_changed',
    payload: { candidateGroups: [], recommendations: [] },
  });
  assert(
    JSON.stringify(s3.candidateGroups.flatMap(group => group.items.map(item => item.text))) ===
      JSON.stringify(flat),
    '空 candidateGroups 不应清空既有条目',
  );
  // 已存在条目重复到达：note 不被改写
  const withNote = ghostwriterAssistReducer(EMPTY_ASSIST, {
    type: 'ghostwriter_assist_changed',
    payload: {
      candidateGroups: [{ kind: 'term', items: [{ index: 1, text: '灰度发布', note: '旧注释' }] }],
      recommendations: [],
    },
  });
  const reArrived = ghostwriterAssistReducer(withNote, {
    type: 'ghostwriter_assist_changed',
    payload: {
      candidateGroups: [{ kind: 'term', items: [{ index: 1, text: '灰度发布', note: '新注释' }] }],
      recommendations: [],
    },
  });
  assert(
    reArrived.candidateGroups[0].items[0].note === '旧注释',
    '已存在条目应原样不动（note 不改写）',
  );
}

// 纯累积无淘汰（用户复核裁决）：超过 8 条仍全保留、不淘汰最旧
{
  let state = EMPTY_ASSIST;
  for (let i = 1; i <= 10; i++) {
    state = ghostwriterAssistReducer(state, {
      type: 'ghostwriter_assist_changed',
      payload: {
        candidateGroups: [{ kind: 'term', items: [{ index: 1, text: `候选${i}` }] }],
        recommendations: [],
      },
    });
  }
  const texts = state.candidateGroups.flatMap(group => group.items.map(item => item.text));
  assert(
    JSON.stringify(texts) ===
      JSON.stringify(['候选1', '候选2', '候选3', '候选4', '候选5', '候选6', '候选7', '候选8', '候选9', '候选10']),
    '纯累积：超过 8 条应全部保留，不 FIFO 淘汰',
  );
  assert(
    state.candidateGroups[0].items[0].index === 1 &&
      state.candidateGroups[0].items[9].index === 10,
    '序号应重排 1..N',
  );
}

// 会话结束/新会话清空：调用方以 emptyGhostwriterAssistState() 复位（starting
// 阶段既有清空路径），复位后累积集合归零
{
  const merged = ghostwriterAssistReducer(EMPTY_ASSIST, {
    type: 'ghostwriter_assist_changed',
    payload: {
      candidateGroups: [{ kind: 'term', items: [{ index: 1, text: '灰度发布' }] }],
      recommendations: [],
    },
  });
  const reset = ghostwriterAssistReducer(emptyGhostwriterAssistState(), {
    type: 'ghostwriter_assist_changed',
    payload: { candidateGroups: [], recommendations: [] },
  });
  assert(
    reset.candidateGroups.length === 0 &&
      merged.candidateGroups.length === 1 &&
      JSON.stringify(reset) === JSON.stringify(EMPTY_ASSIST),
    '复位后新会话应从空集合开始',
  );
}

// assist_unknown_event_noop：未知事件／坏 payload 原样返回
{
  const base = ghostwriterAssistReducer(EMPTY_ASSIST, {
    type: 'ghostwriter_assist_changed',
    payload: { candidateGroups: [], recommendations: [] },
  });
  const s1 = ghostwriterAssistReducer(base, {
    type: 'ghostwriter_preview_changed',
    payload: { text: 'x', revision: 9 },
  });
  assert(s1 === base, 'assist_unknown_event_noop: 未知事件原样返回');
  const s2 = ghostwriterAssistReducer(base, { type: 'ghostwriter_assist_changed' });
  assert(s2 === base, 'payload 缺失时原样返回');
}

// ghostwriterHasUndoAction：✕ 显隐规则（只认命中；候选/推荐纯展示不可撤销）
{
  const preview = emptyGhostwriterPreviewState();
  assert(!ghostwriterHasUndoAction(preview, EMPTY_ASSIST), '全空时无可撤销');
  const assistWithChips = ghostwriterAssistReducer(EMPTY_ASSIST, {
    type: 'ghostwriter_assist_changed',
    payload: {
      candidateGroups: [{ kind: 'term', items: [{ index: 1, text: '灰度发布' }] }],
      recommendations: [{ snippetId: 's1', title: '项目背景' }],
    },
  });
  assert(!ghostwriterHasUndoAction(preview, assistWithChips), '候选/推荐纯展示，不构成可撤销');
  const hit = ghostwriterPreviewReducer(preview, {
    type: 'ghostwriter_snippets_hit',
    payload: { snippetId: 's1', title: '翻译', mode: 'footnote' },
  });
  assert(ghostwriterHasUndoAction(hit, assistWithChips), '有命中即可撤销');
}

// --- 对话回话行（ghostwriter_reply_changed）：转写视图状态追加/清空 ---

const EMPTY_TRANSCRIPT: TranscriptViewState = { sessionId: null, sequence: 0, text: '' };

function replyEvent(sequence: number, sessionId: string, text: string) {
  return {
    sequence,
    sessionId,
    kind: { type: 'ghostwriter_reply_changed', payload: { text } },
  };
}

// 回话行追加保序：事件保序即时间序，多行文本原样保留
{
  const started = applyTranscriptEvent(EMPTY_TRANSCRIPT, {
    sequence: 1,
    sessionId: 's1',
    kind: { type: 'dictation_state_changed', payload: { phase: 'starting', sessionId: 's1' } },
  });
  assert(started.replyLines?.length === 0, 'starting 重置后回话行应为空');
  const withDelta = applyTranscriptEvent(started, {
    sequence: 2,
    sessionId: 's1',
    kind: { type: 'transcript_delta', payload: { text: '把日志', offset: 0, isFinal: false } },
  });
  const r1 = applyTranscriptEvent(withDelta, replyEvent(3, 's1', '你说的是哪个日志？'));
  assert(
    r1.replyLines?.length === 1 && r1.replyLines[0].seq === 3 && r1.replyLines[0].text === '你说的是哪个日志？',
    '回话行应按到达顺序追加',
  );
  const r2 = applyTranscriptEvent(r1, replyEvent(4, 's1', '第一行\n第二行'));
  assert(
    r2.replyLines?.length === 2 && r2.replyLines[1].text === '第一行\n第二行',
    '第二条回话按序追加，多行文本原样保留',
  );
  assert(r2.text === '把日志', '回话不影响转写文本');
  // 旧会话的迟到回话不落行
  const late = applyTranscriptEvent(r2, replyEvent(5, 'old', '迟到的回话'));
  assert(late.replyLines?.length === 2, '其他会话的回话不追加');
  // 坏 payload 原样返回
  const bad = applyTranscriptEvent(r2, {
    sequence: 6,
    sessionId: 's1',
    kind: { type: 'ghostwriter_reply_changed' },
  });
  assert(bad.replyLines?.length === 2, '回话载荷缺 text 时原样返回');
}

// 会话结束清空：终态 phase 与 dictation_completed 都收走回话行
{
  const started = applyTranscriptEvent(EMPTY_TRANSCRIPT, {
    sequence: 1,
    sessionId: 's1',
    kind: { type: 'dictation_state_changed', payload: { phase: 'starting', sessionId: 's1' } },
  });
  const withReplies = applyTranscriptEvent(
    applyTranscriptEvent(started, replyEvent(2, 's1', '第一句')),
    replyEvent(3, 's1', '第二句'),
  );
  assert(withReplies.replyLines?.length === 2, '前置：已有两条回话行');
  const ended = applyTranscriptEvent(withReplies, {
    sequence: 4,
    sessionId: 's1',
    kind: { type: 'dictation_completed', payload: { inserted: 'inserted' } },
  });
  assert(ended.replyLines?.length === 0, 'dictation_completed 应清空回话行');
  const restarted = applyTranscriptEvent(
    applyTranscriptEvent(ended, {
      sequence: 5,
      sessionId: 's2',
      kind: { type: 'dictation_state_changed', payload: { phase: 'starting', sessionId: 's2' } },
    }),
    replyEvent(6, 's2', '新会话第一句'),
  );
  assert(
    restarted.replyLines?.length === 1 && restarted.replyLines[0].text === '新会话第一句',
    '新会话 starting 重置后只留新回话',
  );
  const cancelled = applyTranscriptEvent(restarted, {
    sequence: 7,
    sessionId: 's2',
    kind: { type: 'dictation_state_changed', payload: { phase: 'cancelled' } },
  });
  assert(cancelled.replyLines?.length === 0, 'cancelled 终态应清空回话行');
}

// --- 对话会话推荐行 sticky：推荐为空的批次不塌行，保留最后非空内容 ---

{
  const withRecs = ghostwriterAssistReducer(EMPTY_ASSIST, {
    type: 'ghostwriter_assist_changed',
    payload: {
      candidateGroups: [],
      recommendations: [{ snippetId: 's1', title: '项目背景' }],
    },
  });
  // 普通会话：空批次照旧整行塌掉
  const normal = ghostwriterAssistReducer(withRecs, {
    type: 'ghostwriter_assist_changed',
    payload: { candidateGroups: [], recommendations: [] },
  });
  assert(normal.recommendations.length === 0, '普通会话空推荐应整体替换（可塌）');
  // 对话会话：空批次保留最后非空推荐
  const sticky = ghostwriterAssistReducer(withRecs, {
    type: 'ghostwriter_assist_changed',
    payload: { candidateGroups: [], recommendations: [] },
  }, true);
  assert(
    sticky.recommendations.length === 1 && sticky.recommendations[0].snippetId === 's1',
    '对话会话空推荐应保留上一批非空内容',
  );
  // 非空批次照旧刷新
  const refreshed = ghostwriterAssistReducer(sticky, {
    type: 'ghostwriter_assist_changed',
    payload: { candidateGroups: [], recommendations: [{ snippetId: 's2', title: '测试环境' }] },
  }, true);
  assert(
    refreshed.recommendations.length === 1 && refreshed.recommendations[0].snippetId === 's2',
    '对话会话非空批次应照常刷新',
  );
  // 坏 payload 在对话会话下也原样返回（sticky 不救坏事件）
  assert(
    ghostwriterAssistReducer(refreshed, { type: 'ghostwriter_assist_changed' }, true) === refreshed,
    '对话会话下 payload 缺失仍原样返回',
  );
}

console.log('ghostwriterCapsule.test.ts: all assertions passed');