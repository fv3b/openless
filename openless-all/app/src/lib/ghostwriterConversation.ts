import type { ShortcutBinding } from './types';

/**
 * 对话热键的序列化契约：前端把 ShortcutBinding 存成全小写、`+` 分隔的字符串
 * （修饰键在前、主键最后，如 "alt+shift+d"）落进 ghostwriter.conversationHotkey；
 * Tauri 宿主按同一契约解析后注册全局键（见 src-tauri parse_conversation_hotkey）。
  */

function normalizeModifier(token: string): string | null {
  switch (token) {
    case 'cmd':
    case 'command':
    case 'super':
    case 'meta':
    case 'win':
      return 'cmd';
    case 'ctrl':
    case 'control':
      return 'ctrl';
    case 'alt':
    case 'option':
    case 'opt':
      return 'alt';
    case 'shift':
      return 'shift';
    default:
      return null;
  }
}

/** ShortcutBinding → 存储串（"alt+shift+d"）。修饰键按 cmd→ctrl→alt→shift 固定顺序。 */
export function serializeConversationHotkey(binding: ShortcutBinding): string {
  const parts: string[] = [];
  for (const tag of ['cmd', 'ctrl', 'alt', 'shift'] as const) {
    if (binding.modifiers.some((m) => m.toLowerCase() === tag)) parts.push(tag);
  }
  parts.push(binding.primary.trim().toLowerCase());
  return parts.join('+');
}

/** 存储串 → ShortcutBinding（供录制器回显）。空串/全修饰键/未知 token → null（未配置）。 */
export function parseConversationHotkey(
  raw: string | null | undefined,
): ShortcutBinding | null {
  const trimmed = (raw ?? '').trim().toLowerCase();
  if (!trimmed) return null;
  const modifiers: string[] = [];
  let primary: string | null = null;
  for (const token of trimmed.split('+')) {
    const modifier = normalizeModifier(token.trim());
    // 契约：修饰键在前、主键只能是最后一个 token；主键后再出现任何 token 都无效。
    if (primary || !token) return null;
    if (modifier) {
      modifiers.push(modifier);
      continue;
    }
    primary = token;
  }
  if (!primary) return null;
  // 单字母主键回大写，与录制器输出的 ShortcutBinding 一致。
  const restored = primary.length === 1 ? primary.toUpperCase() : primary;
  return { primary: restored, modifiers };
}

/** 存储串里是否已配置出可注册的组合键（主键非修饰键）。 */
export function hasConversationHotkey(raw: string | null | undefined): boolean {
  return parseConversationHotkey(raw) !== null;
}

/** 录制器 modifier-only 主键可能出现的名字（裸修饰键与侧别修饰键，见 ShortcutRecorder.modifierPrimaryFromCode）。 */
const MODIFIER_PRIMARIES = new Set([
  'shift',
  'control',
  'ctrl',
  'alt',
  'option',
  'opt',
  'meta',
  'command',
  'cmd',
  'win',
  'super',
  'fn',
  'fnlock',
  'capslock',
  'leftshift',
  'rightshift',
  'leftcontrol',
  'rightcontrol',
  'leftalt',
  'rightalt',
  'leftoption',
  'rightoption',
  'leftcommand',
  'rightcommand',
  'leftmeta',
  'rightmeta',
  'leftwin',
  'rightwin',
  'leftsuper',
  'rightsuper',
]);

/** 录制结果是否是单修饰键（全局热键无法注册；comboOnly 录制器已拦，此处兜底）。 */
export function isModifierOnlyPrimary(primary: string): boolean {
  return MODIFIER_PRIMARIES.has(primary.trim().toLowerCase());
}
