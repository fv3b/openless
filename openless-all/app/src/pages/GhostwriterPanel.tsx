import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { isTauri } from '../lib/ipc';
import {
  applyTranscriptEvent,
  type BackendEvent,
  type TranscriptViewState,
} from '../lib/backendEvent';
import {
  completionNotice,
  emptyGhostwriterAssistState,
  emptyGhostwriterPreviewState,
  ghostwriterAssistReducer,
  ghostwriterHasUndoAction,
  ghostwriterPanelActionFor,
  ghostwriterPreviewReducer,
  shouldUseGhostwriterCapsule,
  type GhostwriterPreviewState,
} from '../lib/ghostwriterCapsule';
import {
  createGhostwriterSnippet,
  getSettings,
  ghostwriterCancelLast,
  ghostwriterDismissSuggestion,
  ghostwriterSaveSuggestion,
  ghostwriterToggleSelection,
  type GhostwriterSelectionKind,
} from '../lib/ipc';
import type { GhostwriterAssistState, GhostwriterCandidateKind } from '../lib/types';

/**
 * ghostwriter 浮框：说话时底部浮框实时转写，停止即收起、静默落字。
 *
 * 收放规则见 ghostwriterCapsule.ts 的 ghostwriterPanelActionFor：starting/recording 显示，
 * 其余一律立即隐藏——字落进光标本身就是回执；只有剪贴板兜底/粘贴确认这类
 * 需要用户动手的收尾，才以最小 toast 提示 2.5 秒。
 *
 * 卡片内部五区纵向：顶区命中徽标行（✓ pills＋✕ 撤销最近生效动作）、候选区
 * （沉淀提醒条／候选组 chips／推荐行，ghostwriter_assist_changed 整体替换）、
 * 中区指令预览（若此刻停下将贴给 AI 的完整结果）、底区转写流（小字上下文参照）。
 * 命中/预览走 ghostwriterPreviewReducer，候选区走 ghostwriterAssistReducer，
 * 选中态与撤销结果都由后端事件回流（前端只做渲染与判定）。
 * 定位固定当前显示器底部居中（Rust 侧未感知光标所在屏；跟随光标屏未实现）。
 */

const WINDOW_WIDTH = 560;
const CARD_WIDTH = 520;
const CARD_GAP = 18;
// 卡片竖向留白（420 − 2×12 = 396：候选区出现时向上长高的上限即卡片 maxHeight）。
const CARD_VERTICAL_PADDING = 12;
const CARD_MAX_HEIGHT = 420 - 2 * CARD_VERTICAL_PADDING;
// 候选区收起时保留最后一帧渲染到 max-height 过渡结束（.2s），否则高度过渡看不见。
const ASSIST_COLLAPSE_MS = 220;
const FALLBACK_TOAST_MS = 2500;
const LEVEL_BARS = [0.45, 0.7, 1, 0.7, 0.45];

const CANDIDATE_KIND_LABEL_KEYS: Record<GhostwriterCandidateKind, string> = {
  term: 'ghostwriter.panel.kindTerm',
  phrase: 'ghostwriter.panel.kindPhrase',
  naming: 'ghostwriter.panel.kindNaming',
};

const ASSIST_ROW_STYLE: CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  flexWrap: 'wrap',
  gap: 6,
  padding: '4px 16px 0',
};

const ASSIST_LABEL_STYLE: CSSProperties = {
  fontSize: 11,
  fontWeight: 600,
  color: '#a1a1aa',
  letterSpacing: '0.06em',
  flexShrink: 0,
};

const ICON_BUTTON_STYLE: CSSProperties = {
  width: 22,
  height: 22,
  padding: 0,
  borderRadius: 999,
  display: 'flex',
  alignItems: 'center',
  justifyContent: 'center',
  background: 'rgba(255, 255, 255, 0.08)',
  border: '1px solid rgba(255, 255, 255, 0.12)',
  color: '#a1a1aa',
  fontSize: 11,
  lineHeight: 1,
  cursor: 'pointer',
  flexShrink: 0,
  pointerEvents: 'auto',
};

const ACTION_BUTTON_STYLE: CSSProperties = {
  padding: '2px 10px',
  borderRadius: 999,
  background: 'rgba(255, 255, 255, 0.08)',
  border: '1px solid rgba(255, 255, 255, 0.12)',
  color: '#e4e4e7',
  fontSize: 12,
  fontWeight: 500,
  lineHeight: 1.5,
  cursor: 'pointer',
  flexShrink: 0,
  pointerEvents: 'auto',
};

const CHIP_SAVE_BUTTON_STYLE: CSSProperties = {
  padding: '0 6px',
  borderRadius: 999,
  background: 'rgba(52, 211, 153, 0.18)',
  border: '1px solid rgba(52, 211, 153, 0.4)',
  color: '#6ee7b7',
  fontSize: 11,
  lineHeight: '16px',
  cursor: 'pointer',
  flexShrink: 0,
  pointerEvents: 'auto',
};

function chipStyle(selected: boolean): CSSProperties {
  return {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    maxWidth: '100%',
    padding: '3px 10px',
    borderRadius: 999,
    background: selected ? 'rgba(52, 211, 153, 0.12)' : 'rgba(255, 255, 255, 0.06)',
    border: selected ? '1px solid rgba(52, 211, 153, 0.32)' : '1px solid rgba(255, 255, 255, 0.12)',
    color: selected ? '#6ee7b7' : '#e4e4e7',
    fontSize: 12,
    fontWeight: 500,
    letterSpacing: '0.02em',
    lineHeight: 1.5,
    cursor: 'pointer',
    pointerEvents: 'auto',
  };
}

const CHIP_LABEL_STYLE: CSSProperties = {
  whiteSpace: 'normal',
  wordBreak: 'break-word',
  minWidth: 0,
};

interface FallbackNotice {
  text: string;
}

export function GhostwriterPanel() {
  const { t } = useTranslation();
  // 事件监听只挂一次，经 ref 取最新 t：语言切换后兜底提示不再回退旧语言。
  const tRef = useRef(t);
  useEffect(() => {
    tRef.current = t;
  });
  const [visible, setVisible] = useState(false);
  const [recording, setRecording] = useState(false);
  const [level, setLevel] = useState(0);
  const [text, setText] = useState('');
  const [preview, setPreview] = useState<GhostwriterPreviewState>(emptyGhostwriterPreviewState());
  const [assist, setAssist] = useState<GhostwriterAssistState>(emptyGhostwriterAssistState());
  // 候选区渲染快照：内容清空时保留最后一帧到收起动画结束（见下 effect）。
  const [assistView, setAssistView] = useState<GhostwriterAssistState>(emptyGhostwriterAssistState());
  const [notice, setNotice] = useState<FallbackNotice | null>(null);
  const visibleRef = useRef(false);
  const transcriptRef = useRef<TranscriptViewState>({ sessionId: null, sequence: 0, text: '' });
  const timersRef = useRef<number[]>([]);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const setPanelVisible = (next: boolean) => {
    visibleRef.current = next;
    setVisible(next);
  };

  const clearTimers = () => {
    timersRef.current.forEach(t => clearTimeout(t));
    timersRef.current = [];
  };

  const later = (fn: () => void, ms: number) => {
    timersRef.current.push(window.setTimeout(fn, ms));
  };

  const showNotice = (text: string) => {
    clearTimers();
    setNotice({ text });
    later(() => setNotice(null), FALLBACK_TOAST_MS);
  };

  const hideNow = async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().hide();
    } catch (error) {
      console.warn('[ghostwriter] hide failed', error);
    }
    setPanelVisible(false);
  };

  const showPanel = async () => {
    try {
      const [{ getCurrentWindow, currentMonitor }, { PhysicalPosition }] = await Promise.all([
        import('@tauri-apps/api/window'),
        import('@tauri-apps/api/dpi'),
      ]);
      const win = getCurrentWindow();
      const monitor = await currentMonitor();
      if (monitor) {
        const scale = monitor.scaleFactor;
        const x = monitor.position.x + (monitor.size.width - WINDOW_WIDTH * scale) / 2;
        await win.setPosition(
          new PhysicalPosition(
            Math.round(x),
            Math.round(monitor.position.y + monitor.size.height - 440 * scale),
          ),
        );
      }
      await win.show();
    } catch (error) {
      console.warn('[ghostwriter] show failed', error);
    }
  };

  const cancelLastAction = () => {
    const sessionId = transcriptRef.current.sessionId;
    if (!sessionId) return;
    void (async () => {
      try {
        const result = await ghostwriterCancelLast(sessionId);
        // 后端撤销即推进修订号并发布撤销后的预览事件：响应里的修订号是
        // 后端权威值，凭它挡掉撤销前在途的旧预览；action 决定徽标是否落一枚
        // （选中撤销只换文本）；选中态由后端候选区事件回流。
        setPreview(state =>
          ghostwriterPreviewReducer(state, {
            type: 'ghostwriter_cancel_done',
            payload: {
              cancelled: result.cancelled,
              action: result.action,
              assembled: result.assembled,
              revision: result.revision,
            },
          }),
        );
      } catch (error) {
        console.warn('[ghostwriter] cancel last action failed', error);
      }
    })();
  };

  // 点选失败（会话已停等）静默：选中态以后端候选区事件为权威，刷新即归位。
  const toggleSelection = (kind: GhostwriterSelectionKind, index: number) => {
    const sessionId = transcriptRef.current.sessionId;
    if (!sessionId) return;
    void ghostwriterToggleSelection(sessionId, kind, index).catch(error => {
      console.warn('[ghostwriter] toggle selection failed', error);
    });
  };

  // 沉淀建议存为常用语：成功后后端刷新候选区事件（提醒条随消失）；
  // 失败（触发词重复等）走现有 notice 药丸。
  const saveSuggestion = () => {
    const sessionId = transcriptRef.current.sessionId;
    if (!sessionId) return;
    void ghostwriterSaveSuggestion(sessionId).catch(error => {
      console.warn('[ghostwriter] save suggestion failed', error);
      showNotice(tRef.current('ghostwriter.panel.saveFailedDuplicate'));
    });
  };

  const dismissSuggestion = () => {
    const sessionId = transcriptRef.current.sessionId;
    if (!sessionId) return;
    void ghostwriterDismissSuggestion(sessionId).catch(error => {
      console.warn('[ghostwriter] dismiss suggestion failed', error);
    });
  };

  // 选中候选 chip 的 [存]：把这条说法收成常用语（触发词＝候选文本、贴进正文、启用）。
  const saveCandidateSnippet = (text: string) => {
    void createGhostwriterSnippet({
      id: '',
      trigger: text,
      aliases: [],
      text,
      mode: 'inline',
      enabled: true,
    }).catch(error => {
      console.warn('[ghostwriter] save candidate snippet failed', error);
      showNotice(tRef.current('ghostwriter.panel.saveFailedDuplicate'));
    });
  };

  useEffect(() => {
    if (!isTauri) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    (async () => {
      const { listen } = await import('@tauri-apps/api/event');
      const handle = await listen<BackendEvent>('backend:event', event => {
        const e = event.payload;
        const next = applyTranscriptEvent(transcriptRef.current, e);
        transcriptRef.current = next;
        setText(next.text);

        if (e.kind.type === 'ghostwriter_preview_changed' || e.kind.type === 'ghostwriter_snippets_hit') {
          setPreview(state => ghostwriterPreviewReducer(state, e.kind));
        } else if (e.kind.type === 'ghostwriter_assist_changed') {
          setAssist(state => ghostwriterAssistReducer(state, e.kind));
        } else if (e.kind.type === 'ghostwriter_notice') {
          const payload = e.kind.payload as { message?: string; level?: string } | undefined;
          if (payload?.level === 'error' && typeof payload.message === 'string' && payload.message) {
            showNotice(payload.message);
          }
        } else if (e.kind.type === 'dictation_state_changed') {
          const payload = e.kind.payload as
            | { phase?: string; level?: number }
            | undefined;
          const phase = payload?.phase;
          if (phase === 'starting') {
            setRecording(false);
            setPreview(emptyGhostwriterPreviewState());
            setAssist(emptyGhostwriterAssistState());
          } else if (phase === 'recording') {
            setRecording(true);
            const raw = payload?.level;
            setLevel(typeof raw === 'number' && Number.isFinite(raw) ? Math.min(1, Math.max(0, raw)) : 0);
          }
          if (!visibleRef.current && (phase === 'starting' || phase === 'recording')) {
            void (async () => {
              try {
                if (!shouldUseGhostwriterCapsule(await getSettings())) return;
              } catch {
                return;
              }
              clearTimers();
              setNotice(null);
              setPanelVisible(true);
              void showPanel();
            })();
          }
          const action = ghostwriterPanelActionFor(phase);
          if (visibleRef.current && action === 'hide') {
            setRecording(false);
            clearTimers();
            setNotice(null);
            void hideNow();
          }
        } else if (e.kind.type === 'dictation_completed') {
          const payload = e.kind.payload as { inserted?: string; polishedText?: string } | undefined;
          const action = ghostwriterPanelActionFor('completed', payload?.inserted);
          if (action === 'show-fallback-toast') {
            setRecording(false);
            const notice = completionNotice(payload?.inserted, (payload?.polishedText ?? '').length);
            showNotice(tRef.current(notice.key, { count: notice.count ?? 0 }));
            // 停止阶段窗口可能已被 hideNow 真隐藏；兜底提示是修订版决策 1 里唯一
            // 保留的展示通道，必须先把窗口重新唤起，否则用户对丢字毫无感知。
            void (async () => {
              try {
                if (!shouldUseGhostwriterCapsule(await getSettings())) return;
              } catch {
                return;
              }
              void showPanel();
            })();
          } else if (visibleRef.current) {
            setRecording(false);
            clearTimers();
            setNotice(null);
            void hideNow();
          }
        }
      });
      if (cancelled) handle();
      else unlisten = handle;
    })();
    return () => {
      cancelled = true;
      clearTimers();
      if (unlisten) unlisten();
    };
  }, []);

  // 转写流新文本到达时贴底滚动；用户上翻查看时停在原位。
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    if (nearBottom) el.scrollTop = el.scrollHeight;
  }, [text]);

  // 候选区三行（沉淀条／候选组／推荐行）任一为空整行不渲染，全空整区收起。
  const assistHasContent =
    assist.sediment !== null ||
    assist.candidateGroups.some(group => group.items.length > 0) ||
    assist.recommendations.length > 0;

  // 收起时保留最后一帧到 max-height 过渡结束，否则内容先没了、高度过渡看不见。
  useEffect(() => {
    if (assistHasContent) {
      setAssistView(assist);
      return;
    }
    const timer = window.setTimeout(
      () => setAssistView(emptyGhostwriterAssistState()),
      ASSIST_COLLAPSE_MS,
    );
    return () => window.clearTimeout(timer);
  }, [assist, assistHasContent]);

  const undoable = ghostwriterHasUndoAction(preview, assist);
  const kindLabel = (kind: GhostwriterCandidateKind) => t(CANDIDATE_KIND_LABEL_KEYS[kind] ?? kind);

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'flex-end',
        padding: `${CARD_VERTICAL_PADDING}px ${CARD_GAP}px`,
        pointerEvents: 'none',
        fontFamily:
          '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Segoe UI", sans-serif',
      }}
    >
      {notice ? (
        <div
          style={{
            marginBottom: 10,
            padding: '8px 16px',
            borderRadius: 999,
            background: 'rgba(24, 26, 32, 0.92)',
            border: '1px solid rgba(255, 255, 255, 0.12)',
            boxShadow: '0 12px 32px rgba(0, 0, 0, 0.45)',
            color: '#f4f4f5',
            fontSize: 13,
            fontWeight: 500,
            letterSpacing: '0.01em',
          }}
        >
          {notice.text}
        </div>
      ) : null}
      {visible ? (
        <div
          style={{
            width: CARD_WIDTH,
            maxHeight: CARD_MAX_HEIGHT,
            display: 'flex',
            flexDirection: 'column',
            borderRadius: 20,
            background: 'rgba(19, 21, 26, 0.88)',
            border: '1px solid rgba(255, 255, 255, 0.09)',
            boxShadow: '0 18px 50px rgba(0, 0, 0, 0.45), 0 1px 0 rgba(255,255,255,0.06) inset',
            backdropFilter: 'blur(28px) saturate(1.4)',
            WebkitBackdropFilter: 'blur(28px) saturate(1.4)',
            overflow: 'hidden',
          }}
        >
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              padding: '12px 16px 8px',
            }}
          >
            <span
              style={{
                width: 8,
                height: 8,
                borderRadius: 999,
                background: recording ? '#60a5fa' : '#34d399',
                boxShadow: recording ? '0 0 10px rgba(96, 165, 250, 0.7)' : 'none',
                animation: recording ? 'ghostwriterPulse 1.4s ease-in-out infinite' : undefined,
                flexShrink: 0,
              }}
            />
            <span
              style={{
                fontSize: 12,
                fontWeight: 500,
                color: '#a1a1aa',
                letterSpacing: '0.04em',
              }}
            >
              {recording ? t('ghostwriter.panel.recording') : t('ghostwriter.panel.preparing')}
            </span>
            <div style={{ flex: 1 }} />
            <div
              style={{
                display: 'flex',
                alignItems: 'flex-end',
                gap: 3,
                height: 16,
              }}
            >
              {LEVEL_BARS.map((factor, i) => {
                const h = recording ? 3 + Math.round(level * 13 * factor) : 2;
                return (
                  <span
                    key={i}
                    style={{
                      width: 3,
                      height: h,
                      borderRadius: 2,
                      background: recording && level > 0.02 ? '#60a5fa' : 'rgba(255,255,255,0.18)',
                      transition: 'height 120ms ease, background 240ms ease',
                    }}
                  />
                );
              })}
            </div>
          </div>
          {preview.hits.length > 0 || undoable ? (
            <div style={ASSIST_ROW_STYLE}>
              {preview.hits.map(hit => (
                <span
                  key={hit.snippetId}
                  style={{
                    padding: '3px 10px',
                    borderRadius: 999,
                    background: 'rgba(52, 211, 153, 0.12)',
                    border: '1px solid rgba(52, 211, 153, 0.32)',
                    color: '#6ee7b7',
                    fontSize: 12,
                    fontWeight: 500,
                    letterSpacing: '0.02em',
                  }}
                >
                  ✓ {hit.title}
                </span>
              ))}
              <div style={{ flex: 1 }} />
              <button
                type="button"
                aria-label={t('ghostwriter.panel.cancelLast')}
                title={t('ghostwriter.panel.cancelLast')}
                onClick={cancelLastAction}
                className="ghostwriter-cancel-btn"
                style={ICON_BUTTON_STYLE}
              >
                ✕
              </button>
            </div>
          ) : null}
          <div
            className="ghostwriter-assist"
            aria-hidden={!assistHasContent}
            style={{
              maxHeight: assistHasContent ? CARD_MAX_HEIGHT : 0,
              opacity: assistHasContent ? 1 : 0,
            }}
          >
            {assistView.sediment ? (
              <div style={ASSIST_ROW_STYLE}>
                <span
                  style={{
                    fontSize: 12,
                    lineHeight: 1.5,
                    color: '#e4e4e7',
                    letterSpacing: '0.02em',
                  }}
                >
                  {t('ghostwriter.panel.sedimentSuggest', {
                    phrase: assistView.sediment.phrase,
                    count: assistView.sediment.count,
                  })}
                </span>
                <div style={{ flex: 1 }} />
                <button
                  type="button"
                  onClick={saveSuggestion}
                  className="ghostwriter-chip-action"
                  style={ACTION_BUTTON_STYLE}
                >
                  {t('ghostwriter.panel.saveSuggestion')}
                </button>
                <button
                  type="button"
                  aria-label={t('ghostwriter.panel.dismiss')}
                  title={t('ghostwriter.panel.dismiss')}
                  onClick={dismissSuggestion}
                  className="ghostwriter-cancel-btn"
                  style={ICON_BUTTON_STYLE}
                >
                  ✕
                </button>
              </div>
            ) : null}
            {assistView.candidateGroups.map((group, groupIndex) =>
              group.items.length > 0 ? (
                <div key={`${group.kind}-${groupIndex}`} style={ASSIST_ROW_STYLE}>
                  <span style={ASSIST_LABEL_STYLE}>{kindLabel(group.kind)}</span>
                  {group.items.map(item => (
                    <span
                      key={item.index}
                      role="button"
                      onClick={() => toggleSelection('candidate', item.index)}
                      style={chipStyle(item.selected)}
                    >
                      <span style={CHIP_LABEL_STYLE}>
                        {item.selected ? '✓ ' : ''}
                        {item.index}·{item.text}
                      </span>
                      {item.selected ? (
                        <button
                          type="button"
                          onClick={event => {
                            event.stopPropagation();
                            saveCandidateSnippet(item.text);
                          }}
                          className="ghostwriter-chip-action"
                          style={CHIP_SAVE_BUTTON_STYLE}
                        >
                          {t('ghostwriter.snippets.save')}
                        </button>
                      ) : null}
                    </span>
                  ))}
                </div>
              ) : null,
            )}
            {assistView.recommendations.length > 0 ? (
              <div style={ASSIST_ROW_STYLE}>
                <span style={ASSIST_LABEL_STYLE}>{t('ghostwriter.panel.recommendLabel')}</span>
                {assistView.recommendations.map((recommendation, index) => (
                  <span
                    key={recommendation.snippetId}
                    role="button"
                    onClick={() => toggleSelection('recommendation', index + 1)}
                    style={chipStyle(recommendation.selected)}
                  >
                    <span style={CHIP_LABEL_STYLE}>
                      {recommendation.selected ? '✓ ' : ''}
                      {index + 1}·{recommendation.title}
                    </span>
                  </span>
                ))}
              </div>
            ) : null}
          </div>
          <div
            style={{
              flex: 1,
              minHeight: 0,
              overflowY: 'auto',
              padding: '10px 16px 4px',
            }}
          >
            <p
              style={{
                margin: 0,
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-word',
                fontSize: 16,
                lineHeight: 1.65,
                color: preview.text ? '#fafafa' : 'rgba(250,250,250,0.35)',
              }}
            >
              {preview.text || t('ghostwriter.panel.previewPlaceholder')}
            </p>
          </div>
          <div
            ref={scrollRef}
            style={{
              flexShrink: 0,
              maxHeight: 64,
              overflowY: 'auto',
              padding: '2px 16px 12px',
            }}
          >
            <p
              style={{
                margin: 0,
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-word',
                fontSize: 13,
                lineHeight: 1.6,
                color: 'rgba(250,250,250,0.55)',
              }}
            >
              {text}
              {recording ? (
                <span
                  style={{
                    display: 'inline-block',
                    width: 2,
                    height: 14,
                    marginLeft: 2,
                    verticalAlign: '-2px',
                    background: '#60a5fa',
                    animation: 'ghostwriterCaret 1s step-end infinite',
                  }}
                />
              ) : null}
              {recording && !text ? t('ghostwriter.panel.listening') : null}
            </p>
          </div>
        </div>
      ) : null}
      <style>{`
        @keyframes ghostwriterPulse {
          0%, 100% { opacity: 1; }
          50% { opacity: 0.35; }
        }
        @keyframes ghostwriterCaret {
          0%, 100% { opacity: 1; }
          50% { opacity: 0; }
        }
        .ghostwriter-assist {
          overflow: hidden;
          transition: opacity .15s ease, max-height .2s ease;
        }
        @media (prefers-reduced-motion: reduce) {
          .ghostwriter-assist { transition: none; }
        }
        .ghostwriter-cancel-btn:hover {
          background: rgba(255, 255, 255, 0.16) !important;
          color: #e4e4e7 !important;
        }
        .ghostwriter-chip-action:hover {
          filter: brightness(1.3);
        }
      `}</style>
    </div>
  );
}
