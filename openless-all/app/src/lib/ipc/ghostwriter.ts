import type { Snippet } from '../types';
import { invokeOrMock } from './shared';

/** ghostwriter_cancel_last 的返回：是否撤销了命中/选中（action 由后端权威）＋
 * 撤销后的指令预览（拼装文本＋后端权威修订号，润色结果迟迟未应用时撤销是
 * 预览前进的唯一推手）。 */
export interface GhostwriterCancelLastResult {
  cancelled: boolean;
  /** 被撤销者："hit" | "selection" | "none"。 */
  action: string;
  assembled: string | null;
  revision: number;
}

/** 候选区点选的两种载体（ghostwriter_toggle_selection 的 kind 参数）。 */
export type GhostwriterSelectionKind = 'candidate' | 'recommendation';

/** 任务书快照：身份＋用途说明＋是否已被用户覆写＋当前生效正文。 */
export interface GhostwriterTaskBrief {
  id: string;
  title: string;
  description: string;
  modified: boolean;
  body: string;
}

// 非 Tauri 环境的内存 mock：五份任务书的覆写表（无持久化），仅供浏览器内开发。
const mockTaskBriefDefaults: Array<Omit<GhostwriterTaskBrief, 'modified'>> = [
  {
    id: 'instruction_polish',
    title: '指令化润色',
    description: '管段润色怎么把口语转写整理成指令，改了会影响贴给 AI 的指令。',
    body: '把用户的口语流水账整理成 AI 能一次听懂的指令。（浏览器 mock：真实正文由后端提供）',
  },
  {
    id: 'candidates',
    title: '候选生成',
    description: '管说话卡词时出不出候选（精准词/候选表述/命名建议），改了会影响候选区。',
    body: '判断说话人此刻是否在卡词，卡词时给出现成的候选。（浏览器 mock：真实正文由后端提供）',
  },
  {
    id: 'recommendations',
    title: '推荐挑选',
    description: '管从常用语库里挑哪些条目推荐，改了会影响推荐区。',
    body: '从常用语库里挑出与当前内容真正相关的条目。（浏览器 mock：真实正文由后端提供）',
  },
  {
    id: 'sediment_notice',
    title: '沉淀提醒',
    description: '管判断当前内容是否在重复未入库说法，改了会影响沉淀提醒。',
    body: '判断说话人是否又在说某条还没入库的说法。（浏览器 mock：真实正文由后端提供）',
  },
  {
    id: 'sediment_extraction',
    title: '沉淀抽取',
    description: '管从说话内容里抽取哪些说法去沉淀，改了会影响沉淀抽取结果。',
    body: '从转写里找出值得沉淀成常用语的说法。（浏览器 mock：真实正文由后端提供）',
  },
];

const mockTaskBriefOverrides = new Map<string, string>();

function mockTaskBriefInfo(id: string): GhostwriterTaskBrief {
  const base = mockTaskBriefDefaults.find((brief) => brief.id === id) ?? mockTaskBriefDefaults[0];
  const override = mockTaskBriefOverrides.get(base.id);
  return { ...base, modified: override !== undefined, body: override ?? base.body };
}

// 非 Tauri 环境的内存 mock 库：仅保证 CRUD 与撤销链路走通，无持久化。
const mockSnippets: Snippet[] = [];

function mockId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `mock-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export function listGhostwriterSnippets(): Promise<Snippet[]> {
  return invokeOrMock('list_ghostwriter_snippets', undefined, () =>
    mockSnippets.map((snippet) => ({ ...snippet, aliases: [...snippet.aliases] })),
  );
}

export function createGhostwriterSnippet(snippet: Snippet): Promise<Snippet> {
  return invokeOrMock('create_ghostwriter_snippet', { snippet }, () => {
    const created = { ...snippet, id: snippet.id || mockId() };
    mockSnippets.push(created);
    return { ...created, aliases: [...created.aliases] };
  });
}

export function saveGhostwriterSnippet(snippet: Snippet): Promise<Snippet> {
  return invokeOrMock('save_ghostwriter_snippet', { snippet }, () => {
    const index = mockSnippets.findIndex((existing) => existing.id === snippet.id);
    if (index < 0) throw new Error('Snippet not found');
    mockSnippets[index] = snippet;
    return { ...snippet, aliases: [...snippet.aliases] };
  });
}

export function deleteGhostwriterSnippet(id: string): Promise<void> {
  return invokeOrMock('delete_ghostwriter_snippet', { id }, () => {
    const index = mockSnippets.findIndex((existing) => existing.id === id);
    if (index < 0) throw new Error('Snippet not found');
    mockSnippets.splice(index, 1);
    return undefined;
  });
}

export function setGhostwriterSnippetEnabled(id: string, enabled: boolean): Promise<void> {
  return invokeOrMock('set_ghostwriter_snippet_enabled', { id, enabled }, () => {
    const snippet = mockSnippets.find((existing) => existing.id === id);
    if (!snippet) throw new Error('Snippet not found');
    snippet.enabled = enabled;
    return undefined;
  });
}

export function ghostwriterCancelLast(sessionId: string): Promise<GhostwriterCancelLastResult> {
  return invokeOrMock('ghostwriter_cancel_last', { sessionId }, () => ({
    cancelled: false,
    action: 'none',
    assembled: null,
    revision: 0,
  }));
}

/** 点选/取消候选区一条（index 为候选跨组全局 1-based 序号 / 推荐独立 1-based 序号）。 */
export function ghostwriterToggleSelection(
  sessionId: string,
  kind: GhostwriterSelectionKind,
  index: number,
): Promise<void> {
  return invokeOrMock('ghostwriter_toggle_selection', { sessionId, kind, index }, () => undefined);
}

/** 沉淀建议存为常用语；无建议时后端返回 null。触发词重复时 reject（调用方提示）。 */
export function ghostwriterSaveSuggestion(sessionId: string): Promise<Snippet | null> {
  return invokeOrMock('ghostwriter_save_suggestion', { sessionId }, () => null);
}

/** 忽略沉淀建议（本次会话不再提）。 */
export function ghostwriterDismissSuggestion(sessionId: string): Promise<void> {
  return invokeOrMock('ghostwriter_dismiss_suggestion', { sessionId }, () => undefined);
}

/** 五份任务书的列表（固定顺序由后端注册表决定）。 */
export function listGhostwriterTaskBriefs(): Promise<GhostwriterTaskBrief[]> {
  return invokeOrMock('list_ghostwriter_task_briefs', undefined, () =>
    mockTaskBriefDefaults.map((brief) => mockTaskBriefInfo(brief.id)),
  );
}

/** 保存任务书正文覆写（trim 后非空才收）；保存即被 Core 采用。 */
export function saveGhostwriterTaskBrief(id: string, body: string): Promise<GhostwriterTaskBrief> {
  return invokeOrMock('save_ghostwriter_task_brief', { id, body }, () => {
    const trimmed = body.trim();
    if (!trimmed) throw new Error('task brief body is empty');
    mockTaskBriefOverrides.set(id, trimmed);
    return mockTaskBriefInfo(id);
  });
}

/** 恢复任务书默认正文（删掉该份覆写）。 */
export function resetGhostwriterTaskBrief(id: string): Promise<GhostwriterTaskBrief> {
  return invokeOrMock('reset_ghostwriter_task_brief', { id }, () => {
    mockTaskBriefOverrides.delete(id);
    return mockTaskBriefInfo(id);
  });
}
