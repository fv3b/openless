// ghostwriterTabs.ts — Ghostwriter 视图三页签的身份与跨页导航。
//
// 设置页入口藏在设置弹窗深处（RecordingInputSection），拿不到 Shell 的导航状态，
// 用 DOM CustomEvent 解耦（同 savedEvent / ol-channels-changed 先例）：发射方只广播
// 目标页签，FloatingShell 负责切页、关弹窗，并把页签交给 GhostwriterView。

export type GhostwriterTab = 'snippets' | 'briefs' | 'settings';

export const GHOSTWRITER_TAB_EVENT = 'ghostwriter:open-tab';

export function isGhostwriterTab(value: unknown): value is GhostwriterTab {
  return value === 'snippets' || value === 'briefs' || value === 'settings';
}

/** 请求打开 Ghostwriter 视图并切到指定页签；FloatingShell 是唯一接收方。 */
export function requestGhostwriterTab(tab: GhostwriterTab): void {
  window.dispatchEvent(new CustomEvent<GhostwriterTab>(GHOSTWRITER_TAB_EVENT, { detail: tab }));
}
