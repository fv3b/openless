import { useEffect, useRef, useState } from 'react';
import { isTauri } from '../lib/ipc';
import {
  applyTranscriptEvent,
  type BackendEvent,
  type TranscriptViewState,
} from '../lib/backendEvent';

/**
 * fluid 浮框：听写期间的实时转写面板（M1）。
 *
 * 数据源是广播的 `backend:event`（与 Capsule 同一条通道，见 tauri_events.rs 的
 * `app.emit("backend:event")`），窗口显隐由本组件自管：
 * starting/recording → 显示；dictation_completed → 残留 3 秒后渐隐收起；
 * cancelled/failed → 立即收起。M1 只展示转写原文，最终润色文本仍走上游插入
 * 管线（不改 core 的 stop 路径）；M2 起替换为分段润色流。
 *
 * 定位：当前显示器底部居中、压在胶囊光条上方。多显示器「跟随正在输入的那块屏」
 * 需要胶囊同款的 Rust 侧鼠标定位（capsule_target_monitor），留到 M2 接 Rust 编排。
 */

type PanelState = 'hidden' | 'live' | 'done' | 'fading';

const WINDOW_WIDTH = 560;
const WINDOW_HEIGHT = 420;
/** 逻辑像素：浮框底边距屏幕底部的距离（胶囊光条占 ~80-140，再留间隙）。 */
const BOTTOM_OFFSET = 220;
const DONE_LINGER_MS = 3000;
const FADE_MS = 300;

interface DictationCompletedPayload {
  polishedText?: string;
  inserted?: string;
}

function completionMessage(payload: DictationCompletedPayload | undefined): string {
  const chars = payload?.polishedText?.length ?? 0;
  switch (payload?.inserted) {
    case 'pasteSent':
      return '已发送粘贴，请确认';
    case 'copiedFallback':
      return '已复制，请手动粘贴';
    case 'notRequested':
      return '处理完成';
    case 'inserted':
    default:
      return `已输入 ${chars} 字`;
  }
}

export function FluidPanel() {
  const [state, setState] = useState<PanelState>('hidden');
  const [text, setText] = useState('');
  const [doneMessage, setDoneMessage] = useState('');
  const stateRef = useRef<PanelState>('hidden');
  const transcriptRef = useRef<TranscriptViewState>({ sessionId: null, sequence: 0, text: '' });
  const timersRef = useRef<number[]>([]);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const setPanelState = (next: PanelState) => {
    stateRef.current = next;
    setState(next);
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
    setPanelState('hidden');
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
        const y =
          monitor.position.y +
          monitor.size.height -
          WINDOW_HEIGHT * scale -
          BOTTOM_OFFSET * scale;
        await win.setPosition(new PhysicalPosition(Math.round(x), Math.round(y)));
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
        // 转写缓冲：内部按会话过滤，starting 自动重置（见 backendEvent.ts）。
        const next = applyTranscriptEvent(transcriptRef.current, e);
        transcriptRef.current = next;
        setText(next.text);

        if (e.kind.type === 'dictation_state_changed') {
          const payload = e.kind.payload as { phase?: string } | undefined;
          const phase = payload?.phase;
          if (phase === 'starting' || phase === 'recording') {
            clearTimers();
            setDoneMessage('');
            setPanelState('live');
            void showPanel();
          } else if (phase === 'cancelled' || phase === 'failed') {
            clearTimers();
            void hideNow();
          } else if (phase === 'idle') {
            // 守卫：没走到 done 就回 idle 的异常路径，短暂延迟后收起。
            if (stateRef.current === 'live') {
              later(() => void hideNow(), 600);
            }
          }
        } else if (e.kind.type === 'dictation_completed') {
          const payload = e.kind.payload as DictationCompletedPayload | undefined;
          clearTimers();
          setDoneMessage(completionMessage(payload));
          setPanelState('done');
          later(() => setPanelState('fading'), DONE_LINGER_MS);
          later(() => void hideNow(), DONE_LINGER_MS + FADE_MS);
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

  // 新文本到达时贴底滚动（M1 全量跟随；用户上翻查看时再考虑停在用户位置）。
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [text]);

  if (state === 'hidden') return null;

  return (
    <div
      className="h-screen w-screen p-3"
      style={{ opacity: state === 'fading' ? 0 : 1, transition: `opacity ${FADE_MS}ms ease` }}
    >
      <div className="flex h-full w-full flex-col overflow-hidden rounded-2xl border border-white/10 bg-black/75 shadow-2xl backdrop-blur-xl">
        <div className="flex items-center gap-2 px-4 py-2.5">
          <span
            className={`h-2 w-2 rounded-full ${state === 'live' ? 'animate-pulse bg-red-500' : 'bg-emerald-400'}`}
          />
          <span className="text-xs font-medium text-white/70">
            {state === 'live' ? '语音输入中' : doneMessage}
          </span>
        </div>
        <div ref={scrollRef} className="flex-1 overflow-y-auto px-4 pb-4">
          <p className="whitespace-pre-wrap text-[15px] leading-relaxed text-white/90">
            {text}
            {state === 'live' && text ? <span className="animate-pulse">▌</span> : null}
          </p>
          {state === 'live' && !text ? (
            <p className="text-sm text-white/40">正在聆听…</p>
          ) : null}
        </div>
      </div>
    </div>
  );
}
