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
