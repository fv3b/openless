import { useEffect, useRef, useState } from 'react';
import { isTauri } from '../lib/ipc';
import {
  applyTranscriptEvent,
  type BackendEvent,
  type TranscriptViewState,
} from '../lib/backendEvent';
import { fluidPanelActionFor, shouldUseFluidCapsule } from '../lib/fluidCapsule';
import { getSettings } from '../lib/ipc';

/**
 * fluid 浮框（M1）：说话时底部浮条实时转写，停止即收起、静默落字。
 *
 * 收放规则见 fluidCapsule.ts 的 fluidPanelActionFor：starting/recording 显示，
 * 其余一律立即隐藏——字落进光标本身就是回执；只有剪贴板兜底/粘贴确认这类
 * 需要用户动手的收尾，才以最小 toast 提示 2.5 秒。
 *
 * 布局为 M2-M4 预留：卡片是内容自适应的纵向 flex，后续动作 chips（M2）、
 * 注入徽标（M3）、主题建议（M4）作为新 slot 插在转写区下方即可，无需改窗口。
 * 定位沿用当前显示器底部居中；跟随光标所在屏的 Rust 编排在 M2 接入。
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
  const [visible, setVisible] = useState(false);
  const [recording, setRecording] = useState(false);
  const [level, setLevel] = useState(0);
  const [text, setText] = useState('');
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

        if (e.kind.type === 'dictation_state_changed') {
          const payload = e.kind.payload as
            | { phase?: string; level?: number }
            | undefined;
          const phase = payload?.phase;
          if (phase === 'starting') {
            setRecording(false);
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
            setNotice({ text: completionText(payload?.inserted, payload?.polishedText) });
            later(() => setNotice(null), FALLBACK_TOAST_MS);
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

  // 新文本到达时贴底滚动；用户上翻查看时停在原位。
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
              {recording ? '语音输入中' : '正在准备'}
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
          <div
            ref={scrollRef}
            style={{
              flex: 1,
              minHeight: 0,
              overflowY: 'auto',
              padding: '2px 16px 14px',
            }}
          >
            <p
              style={{
                margin: 0,
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-word',
                fontSize: 15,
                lineHeight: 1.7,
                color: '#fafafa',
              }}
            >
              {text}
              {recording ? (
                <span
                  style={{
                    display: 'inline-block',
                    width: 2,
                    height: 16,
                    marginLeft: 2,
                    verticalAlign: '-2px',
                    background: '#60a5fa',
                    animation: 'fluidCaret 1s step-end infinite',
                  }}
                />
              ) : null}
            </p>
            {recording && !text ? (
              <p style={{ margin: 0, fontSize: 13, color: 'rgba(250,250,250,0.35)' }}>
                正在聆听…
              </p>
            ) : null}
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
      `}</style>
    </div>
  );
}

function completionText(inserted: string | undefined, polishedText: string | undefined): string {
  switch (inserted) {
    case 'pasteSent':
      return '已发送粘贴，请确认落点';
    case 'copiedFallback':
      return '已复制到剪贴板，请手动粘贴';
    default:
      return `已输入 ${(polishedText ?? '').length} 字`;
  }
}
