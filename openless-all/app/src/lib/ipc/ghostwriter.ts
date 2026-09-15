import type { Snippet } from '../types';
import { invokeOrMock } from './shared';

/** ghostwriter_cancel_last 的返回：是否撤销了命中＋撤销后的指令预览（拼装文本＋
 * 后端权威修订号，润色结果迟迟未应用时撤销是预览前进的唯一推手）。 */
export interface GhostwriterCancelLastResult {
  cancelled: boolean;
  assembled: string | null;
  revision: number;
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
    assembled: null,
    revision: 0,
  }));
}
