export interface TranscriptDelta {
  text: string;
  offset: number;
  isFinal: boolean;
}

/** 转写流里的一条 AI 回话行（ghostwriter_reply_changed）：seq 为事件序号
 * （事件保序即时间序），text 原样保留（可能含多行）。回话不产生生效动作。 */
export interface TranscriptReplyLine {
  seq: number;
  text: string;
}

export interface BackendEvent {
  sequence: number;
  sessionId: string | null;
  kind: { type: string; payload?: unknown };
}

export interface TranscriptViewState {
  sessionId: string | null;
  sequence: number;
  text: string;
  /** 对话会话的 AI 回话行，按到达顺序追加；会话结束（starting 重置/终态/
   * dictation_completed）清空。普通会话恒空。 */
  replyLines?: TranscriptReplyLine[];
}

/** 会话终态 phase：回话行随会话结束清空（浮框此时已收起或正收起）。 */
const SESSION_END_PHASES = new Set(['completed', 'cancelled', 'failed', 'idle']);

function withClearedReplyLines(state: TranscriptViewState): TranscriptViewState {
  if ((state.replyLines?.length ?? 0) === 0) return state;
  return { ...state, replyLines: [] };
}

export function applyTranscriptEvent(
  state: TranscriptViewState,
  event: BackendEvent,
): TranscriptViewState {
  if (event.sequence <= state.sequence) return state;
  if (event.kind.type === 'dictation_state_changed') {
    const payload = event.kind.payload as { phase?: string; sessionId?: string | null } | undefined;
    if (payload?.phase === 'starting' && payload.sessionId) {
      return { sessionId: payload.sessionId, sequence: event.sequence, text: '', replyLines: [] };
    }
    if (payload?.phase !== undefined && SESSION_END_PHASES.has(payload.phase)) {
      return { ...withClearedReplyLines(state), sequence: event.sequence };
    }
    return { ...state, sequence: event.sequence };
  }
  if (event.kind.type === 'ghostwriter_reply_changed') {
    if (state.sessionId !== null && event.sessionId !== state.sessionId) return state;
    const payload = event.kind.payload as { text?: unknown } | undefined;
    if (!payload || typeof payload.text !== 'string') return state;
    return {
      ...state,
      sequence: event.sequence,
      replyLines: [...(state.replyLines ?? []), { seq: event.sequence, text: payload.text }],
    };
  }
  if (event.kind.type === 'dictation_completed') {
    return { ...withClearedReplyLines(state), sequence: event.sequence };
  }
  if (event.kind.type !== 'transcript_delta') return { ...state, sequence: event.sequence };
  if (state.sessionId !== null && event.sessionId !== state.sessionId) return state;
  const delta = event.kind.payload as TranscriptDelta | undefined;
  if (!delta || !Number.isSafeInteger(delta.offset) || delta.offset < 0) return state;
  const current = Array.from(state.text);
  if (delta.offset > current.length) return state;
  return {
    sessionId: event.sessionId,
    sequence: event.sequence,
    text: current.slice(0, delta.offset).join('') + delta.text,
  };
}
