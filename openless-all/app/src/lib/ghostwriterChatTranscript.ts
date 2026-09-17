// 聊天记录（ghostwriterChat）的行解析：后端 chat_transcript 的行语法由代码固定——
// 每行以【我】或【助手】开头，按发生顺序混排（多行回话已由后端归一为空格）。
// 历史明细块只管把行拆成 {role, text} 交给渲染；这里不感知 UI。

export type ChatTranscriptRole = 'user' | 'assistant' | 'plain';

export interface ChatTranscriptLine {
  role: ChatTranscriptRole;
  /** 剥掉【我】/【助手】前缀后的内容；plain 行是整行原文（不丢内容）。 */
  text: string;
}

const USER_PREFIX = '【我】';
const ASSISTANT_PREFIX = '【助手】';

/** 聊天记录 → 逐行解析。空/纯空白输入返回 []；前缀行剥前缀取内容；
 *  未知前缀的非空行按 plain 行保留（后端行语法演进/旧数据坏行时内容不丢）；
 *  纯空白行不携带内容，丢弃。 */
export function parseChatTranscript(raw: string | null | undefined): ChatTranscriptLine[] {
  if (!raw || !raw.trim()) return [];
  const lines: ChatTranscriptLine[] = [];
  for (const line of raw.split('\n')) {
    if (!line.trim()) continue;
    if (line.startsWith(USER_PREFIX)) {
      lines.push({ role: 'user', text: line.slice(USER_PREFIX.length) });
    } else if (line.startsWith(ASSISTANT_PREFIX)) {
      lines.push({ role: 'assistant', text: line.slice(ASSISTANT_PREFIX.length) });
    } else {
      lines.push({ role: 'plain', text: line });
    }
  }
  return lines;
}
