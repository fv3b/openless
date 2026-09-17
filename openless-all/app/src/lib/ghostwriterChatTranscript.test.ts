import { parseChatTranscript } from './ghostwriterChatTranscript';

// 聊天记录行解析测试。与 src/lib 其它测试同风格：自定义 assert + 顶层执行
//（前端测试栈是 node+tsx，无 DOM；不用 node:test，避免 @types/node 依赖）。

function assert(condition: boolean, message: string) {
  if (!condition) throw new Error(message);
}

function assertLines(
  actual: ReturnType<typeof parseChatTranscript>,
  expected: Array<{ role: string; text: string }>,
  message: string,
) {
  assert(
    actual.length === expected.length &&
      actual.every((line, i) => line.role === expected[i].role && line.text === expected[i].text),
    `${message}（实际 ${JSON.stringify(actual)}）`,
  );
}

// --- 空/坏输入：一律回落空数组（明细块不渲染） ---

assert(parseChatTranscript(null).length === 0, 'null 应得空数组');
assert(parseChatTranscript(undefined).length === 0, 'undefined 应得空数组');
assert(parseChatTranscript('').length === 0, '空串应得空数组');
assert(parseChatTranscript('   \n \n').length === 0, '纯空白输入应得空数组');

// --- 正常行语法：【我】/【助手】按发生顺序混排，前缀剥掉、内容逐字保留 ---

assertLines(
  parseChatTranscript('【我】把日志清一下。\n【助手】哪个日志？\n【我】系统日志。'),
  [
    { role: 'user', text: '把日志清一下。' },
    { role: 'assistant', text: '哪个日志？' },
    { role: 'user', text: '系统日志。' },
  ],
  '混排记录应按行拆出角色与内容',
);

// 连续同角色行（用户逐条段）同样逐行拆。

assertLines(parseChatTranscript('【我】第一句。\n【我】继续说第二句。'), [
  { role: 'user', text: '第一句。' },
  { role: 'user', text: '继续说第二句。' },
], '连续用户行应逐行拆出');

// 前缀后为空：行仍保留角色，内容空串。

assertLines(parseChatTranscript('【我】'), [{ role: 'user', text: '' }], '只有前缀的行应得空内容');

// --- 未知前缀：按普通行渲染，内容不丢 ---

assertLines(
  parseChatTranscript('【我】开头一行。\n没有前缀的坏行\n【助手】结尾。'),
  [
    { role: 'user', text: '开头一行。' },
    { role: 'plain', text: '没有前缀的坏行' },
    { role: 'assistant', text: '结尾。' },
  ],
  '未知前缀行应按 plain 保留原文',
);

// 部分前缀（【助】/【我】缺右括号等）不算已知前缀 → plain 整行保留。

assertLines(parseChatTranscript('【助】不是助手行'), [
  { role: 'plain', text: '【助】不是助手行' },
], '前缀部分匹配应按 plain 整行保留');

// 中间的纯空白行丢弃（不携带内容），前后行仍解析。

assertLines(
  parseChatTranscript('【我】一行。\n\n   \n【助手】二行。'),
  [
    { role: 'user', text: '一行。' },
    { role: 'assistant', text: '二行。' },
  ],
  '中间空白行应丢弃且不破坏前后行',
);

console.log('ghostwriterChatTranscript.test.ts: all assertions passed');
