import type { Snippet } from '../types';
import { invokeOrMock } from './shared';

/** fluid_cancel_last 的返回：是否撤销了命中＋撤销后的指令预览拼装文本。 */
export interface FluidCancelLastResult {
  cancelled: boolean;
  assembled: string | null;
}

// 非 Tauri 环境的内存 mock 库：仅保证 CRUD 与撤销链路走通，无持久化。
const mockSnippets: Snippet[] = [];

function mockId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `mock-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export function listFluidSnippets(): Promise<Snippet[]> {
  return invokeOrMock('list_fluid_snippets', undefined, () =>
    mockSnippets.map((snippet) => ({ ...snippet, aliases: [...snippet.aliases] })),
  );
}

export function createFluidSnippet(snippet: Snippet): Promise<Snippet> {
  return invokeOrMock('create_fluid_snippet', { snippet }, () => {
    const created = { ...snippet, id: snippet.id || mockId() };
    mockSnippets.push(created);
    return { ...created, aliases: [...created.aliases] };
  });
}

export function saveFluidSnippet(snippet: Snippet): Promise<Snippet> {
  return invokeOrMock('save_fluid_snippet', { snippet }, () => {
    const index = mockSnippets.findIndex((existing) => existing.id === snippet.id);
    if (index < 0) throw new Error('Snippet not found');
    mockSnippets[index] = snippet;
    return { ...snippet, aliases: [...snippet.aliases] };
  });
}

export function deleteFluidSnippet(id: string): Promise<void> {
  return invokeOrMock('delete_fluid_snippet', { id }, () => {
    const index = mockSnippets.findIndex((existing) => existing.id === id);
    if (index < 0) throw new Error('Snippet not found');
    mockSnippets.splice(index, 1);
    return undefined;
  });
}

export function setFluidSnippetEnabled(id: string, enabled: boolean): Promise<void> {
  return invokeOrMock('set_fluid_snippet_enabled', { id, enabled }, () => {
    const snippet = mockSnippets.find((existing) => existing.id === id);
    if (!snippet) throw new Error('Snippet not found');
    snippet.enabled = enabled;
    return undefined;
  });
}

export function fluidCancelLast(sessionId: string): Promise<FluidCancelLastResult> {
  return invokeOrMock('fluid_cancel_last', { sessionId }, () => ({
    cancelled: false,
    assembled: null,
  }));
}
