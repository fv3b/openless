import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isTauri } from '../lib/ipc';
import {
  applyTranscriptEvent,
  type BackendEvent,
  type TranscriptViewState,
} from '../lib/backendEvent';
import {
  completionNotice,
  emptyFluidPreviewState,
  fluidPanelActionFor,
  fluidPreviewReducer,
  shouldUseFluidCapsule,
  type FluidPreviewState,
} from '../lib/fluidCapsule';
import { fluidCancelLast, getSettings } from '../lib/ipc';

/**
 * fluid 浮框：说话时底部浮框实时转写，停止即收起、静默落字。
 *
 * 收放规则见 fluidCapsule.ts 的 fluidPanelActionFor：starting/recording 显示，
 * 其余一律立即隐藏——字落进光标本身就是回执；只有剪贴板兜底/粘贴确认这类
 * 需要用户动手的收尾，才以最小 toast 提示 2.5 秒。
 *
 * 卡片内部三区纵向：顶区命中徽标行（✓ pills＋✕ 撤销最近命中）、中区指令预览
 * （fluid_preview_changed，若此刻停下将贴给 AI 的完整结果）、底区转写流（小字
 * 上下文参照）。命中/预览状态走 fluidCapsule.fluidPreviewReducer 纯状态机，
 * 撤销结果经同一 reducer 回流，revision 单调。
 * 定位固定当前显示器底部居中（Rust 侧未感知光标所在屏；跟随光标屏未实现）。
 */

const WINDOW_WIDTH = 560;
const CARD_WIDTH = 520;
const CARD_GAP = 18;
const FALLBACK_TOAST_MS = 2500;
const LEVEL_BARS = [0.45, 0.7, 1, 0.7, 0.45];

interface FallbackNotice {
  text: string;
}

export function FluidPanel() {
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
  const [preview, setPreview] = useState<FluidPreviewState>(emptyFluidPreviewState());
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

  const hideNow = async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().hide();
    } catch (error) {
      console.warn('[fluid] hide failed', error);
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
      console.warn('[fluid] show failed', error);
    }
  };

  const cancelLastHit = () => {
    const sessionId = transcriptRef.current.sessionId;
    if (!sessionId) return;
    void (async () => {
      try {
        const result = await fluidCancelLast(sessionId);
        // 后端撤销即推进修订号并发布撤销后的预览事件：响应里的修订号是
        // 后端权威值，凭它挡掉撤销前在途的旧预览，同时移除最近一枚徽标。
        setPreview(state =>
          fluidPreviewReducer(state, {
            type: 'fluid_cancel_done',
            payload: {
              cancelled: result.cancelled,
              assembled: result.assembled,
              revision: result.revision,
            },
          }),
        );
      } catch (error) {
        console.warn('[fluid] cancel last hit failed', error);
      }
    })();
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

        if (e.kind.type === 'fluid_preview_changed' || e.kind.type === 'fluid_snippets_hit') {
          setPreview(state => fluidPreviewReducer(state, e.kind));
        } else if (e.kind.type === 'fluid_notice') {
          const payload = e.kind.payload as { message?: string; level?: string } | undefined;
          if (payload?.level === 'error' && typeof payload.message === 'string' && payload.message) {
            clearTimers();
            setNotice({ text: payload.message });
            later(() => setNotice(null), FALLBACK_TOAST_MS);
          }
        } else if (e.kind.type === 'dictation_state_changed') {
          const payload = e.kind.payload as
            | { phase?: string; level?: number }
            | undefined;
          const phase = payload?.phase;
          if (phase === 'starting') {
            setRecording(false);
            setPreview(emptyFluidPreviewState());
          } else if (phase === 'recording') {
            setRecording(true);
            const raw = payload?.level;
            setLevel(typeof raw === 'number' && Number.isFinite(raw) ? Math.min(1, Math.max(0, raw)) : 0);
          }
          if (!visibleRef.current && (phase === 'starting' || phase === 'recording')) {
            void (async () => {
              try {
                if (!shouldUseFluidCapsule(await getSettings())) return;
              } catch {
                return;
              }
              clearTimers();
              setNotice(null);
              setPanelVisible(true);
              void showPanel();
            })();
          }
          const action = fluidPanelActionFor(phase);
          if (visibleRef.current && action === 'hide') {
            setRecording(false);
            clearTimers();
            setNotice(null);
            void hideNow();
          }
        } else if (e.kind.type === 'dictation_completed') {
          const payload = e.kind.payload as { inserted?: string; polishedText?: string } | undefined;
          const action = fluidPanelActionFor('completed', payload?.inserted);
          if (action === 'show-fallback-toast') {
            setRecording(false);
            clearTimers();
            const notice = completionNotice(payload?.inserted, (payload?.polishedText ?? '').length);
            setNotice({ text: tRef.current(notice.key, { count: notice.count ?? 0 }) });
            later(() => setNotice(null), FALLBACK_TOAST_MS);
            // 停止阶段窗口可能已被 hideNow 真隐藏；兜底提示是修订版决策 1 里唯一
            // 保留的展示通道，必须先把窗口重新唤起，否则用户对丢字毫无感知。
            void (async () => {
              try {
                if (!shouldUseFluidCapsule(await getSettings())) return;
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

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'flex-end',
        padding: CARD_GAP,
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
            maxHeight: 340,
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
                animation: recording ? 'fluidPulse 1.4s ease-in-out infinite' : undefined,
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
              {recording ? t('fluid.panel.recording') : t('fluid.panel.preparing')}
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
          {preview.hits.length > 0 ? (
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                flexWrap: 'wrap',
                gap: 6,
                padding: '4px 16px 0',
              }}
            >
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
                aria-label={t('fluid.panel.cancelLast')}
                title={t('fluid.panel.cancelLast')}
                onClick={cancelLastHit}
                className="fluid-cancel-btn"
                style={{
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
                }}
              >
                ✕
              </button>
            </div>
          ) : null}
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
              {preview.text || t('fluid.panel.previewPlaceholder')}
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
                    animation: 'fluidCaret 1s step-end infinite',
                  }}
                />
              ) : null}
              {recording && !text ? t('fluid.panel.listening') : null}
            </p>
          </div>
        </div>
      ) : null}
      <style>{`
        @keyframes fluidPulse {
          0%, 100% { opacity: 1; }
          50% { opacity: 0.35; }
        }
        @keyframes fluidCaret {
          0%, 100% { opacity: 1; }
          50% { opacity: 0; }
        }
        .fluid-cancel-btn:hover {
          background: rgba(255, 255, 255, 0.16) !important;
          color: #e4e4e7 !important;
        }
      `}</style>
    </div>
  );
}
