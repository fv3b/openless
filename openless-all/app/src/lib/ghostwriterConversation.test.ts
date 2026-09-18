import {
  hasConversationHotkey,
  isConversationModifierPrimaryAllowed,
  isModifierOnlyPrimary,
  parseConversationHotkey,
  serializeConversationHotkey,
} from './ghostwriterConversation';

function assert(condition: boolean, message: string) {
  if (!condition) throw new Error(message);
}

function mockNavigator(platform: string, userAgent = '') {
  Object.defineProperty(globalThis, 'navigator', {
    value: { platform, userAgent },
    configurable: true,
  });
}

// serialize：修饰键按固定顺序、全小写（与 Tauri 宿主 parse_conversation_hotkey 同契约）
assert(
  serializeConversationHotkey({ primary: 'D', modifiers: ['shift', 'alt'] }) === 'alt+shift+d',
  'serialize 应规范化修饰键顺序并小写',
);
assert(
  serializeConversationHotkey({ primary: ';', modifiers: ['cmd', 'shift'] }) === 'cmd+shift+;',
  'serialize 应保留符号主键',
);
assert(
  serializeConversationHotkey({ primary: 'F2', modifiers: [] }) === 'f2',
  'serialize 无修饰键时只留主键',
);

// parse：空/坏输入 → null（视为未配置）
assert(parseConversationHotkey(null) === null, 'null 应解析为未配置');
assert(parseConversationHotkey('') === null, '空串应解析为未配置');
assert(parseConversationHotkey('alt+shift') === null, '全修饰键应视为未配置');
assert(parseConversationHotkey('alt++d') === null, '空 token 应视为未配置');
assert(parseConversationHotkey('d+alt') === null, '主键后还有 token 应视为未配置');

// parse：合法串还原 ShortcutBinding（录制器回显用）
{
  const binding = parseConversationHotkey('alt+shift+d');
  assert(
    binding?.primary === 'D' && binding.modifiers.join(',') === 'alt,shift',
    'parse 应还原主键（单字母回大写）与修饰键',
  );
  const named = parseConversationHotkey('cmd+f2');
  assert(
    named?.primary === 'f2' && named.modifiers.join(',') === 'cmd',
    'parse 应保留命名主键小写原样',
  );
}

// has：配置过可注册组合键才算有
assert(hasConversationHotkey('alt+d') === true, '合法串应有热键');
assert(hasConversationHotkey('shift') === false, 'modifier-only 不算有');
assert(hasConversationHotkey(undefined) === false, '未设置不算有');

// modifier-only 主键兜底检测
assert(isModifierOnlyPrimary('LeftShift') === true, '修饰键主键应被拦');
assert(isModifierOnlyPrimary('D') === false, '普通主键应放行');

// 对话热键单修饰键的平台旁路：仅 macOS 放行侧别修饰键
mockNavigator('MacIntel');
assert(
  isConversationModifierPrimaryAllowed('RightOption') === true,
  'macOS 上右 Option 应可作为对话热键',
);
assert(
  isConversationModifierPrimaryAllowed('LeftOption') === true,
  'macOS 上左 Option 应可作为对话热键',
);
assert(
  isConversationModifierPrimaryAllowed('RightAlt') === true,
  '右 Alt 别名与右 Option 同槽，应放行',
);
assert(
  isConversationModifierPrimaryAllowed('LeftCommand') === true,
  'macOS 上侧别 Command 应放行',
);
assert(
  isConversationModifierPrimaryAllowed('Shift') === false,
  '裸 Shift 无侧别，不应放行',
);
assert(isConversationModifierPrimaryAllowed('Fn') === false, 'Fn 与系统功能冲突，不应放行');
assert(isConversationModifierPrimaryAllowed('CapsLock') === false, '大小写锁定不应放行');
mockNavigator('Win32');
assert(
  isConversationModifierPrimaryAllowed('RightOption') === false,
  '非 macOS 上单修饰键仍应拒绝',
);
mockNavigator('');

// 单修饰键主键的序列化往返：录制器输出 RightOption → 存储串 rightoption → 解析回显
assert(
  serializeConversationHotkey({ primary: 'RightOption', modifiers: [] }) === 'rightoption',
  '右 Option 应序列化为小写主键串',
);
assert(
  parseConversationHotkey('rightoption')?.primary === 'rightoption',
  'rightoption 应解析回显（命名主键小写，formatPrimary 负责显示为 右 ⌥）',
);
assert(hasConversationHotkey('rightoption') === true, '单修饰键配置后应算已有热键');

console.log('ghostwriterConversation.test.ts: all assertions passed');
