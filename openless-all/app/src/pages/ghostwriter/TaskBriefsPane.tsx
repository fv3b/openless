// TaskBriefsPane.tsx — 「任务书」页签：五份任务书列表 + 右侧编辑抽屉。
// 正文可改、保存即生效（Core 下一次调用即取用）；折叠区展示系统固定的注入与输出契约。
// 列表与抽屉布局照 GhostwriterSnippets。

import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { AnimatePresence, motion } from 'framer-motion';
import {
  listGhostwriterTaskBriefs,
  resetGhostwriterTaskBrief,
  saveGhostwriterTaskBrief,
  type GhostwriterTaskBrief,
} from '../../lib/ipc';
import { Btn, Card, Collapsible, Pill } from '../_atoms';
import { Icon } from '../../components/Icon';
import { SavedToast, type SaveToastState } from '../../components/SavedToast';
import { useMobileLayout } from '../../lib/useMobileLayout';

type BusyAction = 'loading' | 'saving' | 'resetting' | null;

export function TaskBriefsPane() {
  const { t } = useTranslation();
  const mobile = useMobileLayout();
  const [briefs, setBriefs] = useState<GhostwriterTaskBrief[]>([]);
  const [busy, setBusy] = useState<BusyAction>('loading');
  const [draft, setDraft] = useState<GhostwriterTaskBrief | null>(null);
  const [baselineBody, setBaselineBody] = useState('');
  const [saveState, setSaveState] = useState<SaveToastState>('idle');
  const [saveMessage, setSaveMessage] = useState('');
  const statusTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (statusTimer.current !== null) window.clearTimeout(statusTimer.current);
    },
    [],
  );

  const showStatus = (state: SaveToastState, message: string, temporary = false) => {
    if (statusTimer.current !== null) {
      window.clearTimeout(statusTimer.current);
      statusTimer.current = null;
    }
    setSaveState(state);
    setSaveMessage(message);
    if (temporary || state === 'failed') {
      statusTimer.current = window.setTimeout(
        () => {
          setSaveState('idle');
          setSaveMessage('');
          statusTimer.current = null;
        },
        state === 'failed' ? 6000 : 1600,
      );
    }
  };

  const loadBriefs = async () => {
    setBusy('loading');
    try {
      setBriefs(await listGhostwriterTaskBriefs());
    } catch (loadError) {
      console.error('[ghostwriter] task briefs load failed', loadError);
      showStatus('failed', t('common.operationFailed'));
    } finally {
      setBusy(null);
    }
  };

  useEffect(() => {
    void loadBriefs();
  }, []);

  const dirty = Boolean(draft && draft.body !== baselineBody);

  const openEditor = (brief: GhostwriterTaskBrief) => {
    setDraft(brief);
    setBaselineBody(brief.body);
  };

  // 取消 / 关闭 / Esc：直接关抽屉，未保存的改动即丢弃（恢复默认才有不可逆后果、需确认）。
  const closeEditor = () => {
    setDraft(null);
    setBaselineBody('');
  };

  useEffect(() => {
    if (!draft) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        closeEditor();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, [draft]);

  const handleSave = async () => {
    if (!draft || busy === 'saving' || !draft.body.trim()) return;
    setBusy('saving');
    showStatus('saving', t('ghostwriter.briefs.saving'));
    try {
      const saved = await saveGhostwriterTaskBrief(draft.id, draft.body);
      setBriefs((prev) => prev.map((item) => (item.id === saved.id ? saved : item)));
      setDraft((current) => (current && current.id === saved.id ? saved : current));
      setBaselineBody(saved.body);
      showStatus('saved', t('ghostwriter.briefs.saved'), true);
    } catch (saveError) {
      console.error('[ghostwriter] task brief save failed', saveError);
      showStatus('failed', t('common.operationFailed'));
    } finally {
      setBusy(null);
    }
  };

  const handleReset = async () => {
    if (!draft || busy === 'resetting') return;
    if (!window.confirm(t('ghostwriter.briefs.resetConfirm'))) return;
    setBusy('resetting');
    try {
      const reset = await resetGhostwriterTaskBrief(draft.id);
      setBriefs((prev) => prev.map((item) => (item.id === reset.id ? reset : item)));
      setDraft((current) => (current && current.id === reset.id ? reset : current));
      setBaselineBody(reset.body);
      showStatus('saved', t('ghostwriter.briefs.saved'), true);
    } catch (resetError) {
      console.error('[ghostwriter] task brief reset failed', resetError);
      showStatus('failed', t('common.operationFailed'));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
      <Card
        padding={0}
        style={{
          overflow: 'hidden',
          flex: '1 1 0',
          minHeight: 0,
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <div
          style={{
            padding: '14px 18px',
            borderBottom: '0.5px solid var(--ol-line)',
            flexShrink: 0,
            display: 'flex',
            alignItems: 'flex-start',
            justifyContent: 'space-between',
            gap: 12,
          }}
        >
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 15, fontWeight: 600, color: 'var(--ol-ink)' }}>
              {t('ghostwriter.briefs.listTitle')}
            </div>
            <div style={{ marginTop: 4, fontSize: 12, color: 'var(--ol-ink-3)', lineHeight: 1.55 }}>
              {t('ghostwriter.briefs.desc')}
            </div>
          </div>
          <Pill tone="outline">{t('ghostwriter.briefs.listCount', { count: briefs.length })}</Pill>
        </div>

        <div className="ol-thinscroll" style={{ overflow: 'auto', flex: '1 1 0', minHeight: 0 }}>
          {busy === 'loading' && briefs.length === 0 ? (
            <div style={{ padding: 24, fontSize: 12, color: 'var(--ol-ink-4)' }}>
              {t('common.loading')}
            </div>
          ) : (
            briefs.map((brief) => (
              <div
                key={brief.id}
                role="button"
                tabIndex={0}
                onClick={() => openEditor(brief)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    openEditor(brief);
                  }
                }}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 12,
                  padding: '12px 18px',
                  borderBottom: '0.5px solid var(--ol-line)',
                  cursor: 'pointer',
                }}
              >
                <div style={{ minWidth: 0, flex: 1 }}>
                  <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
                    <span style={{ fontSize: 14, fontWeight: 600, color: 'var(--ol-ink)' }}>
                      {brief.title}
                    </span>
                    {brief.modified && (
                      <Pill tone="blue" size="sm">
                        {t('ghostwriter.briefs.modified')}
                      </Pill>
                    )}
                  </div>
                  <div
                    style={{
                      marginTop: 3,
                      fontSize: 12.5,
                      color: 'var(--ol-ink-3)',
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {brief.description}
                  </div>
                </div>
                <Icon name="chevRight" size={14} style={{ color: 'var(--ol-ink-4)' }} />
              </div>
            ))
          )}
        </div>
      </Card>

      <SavedToast saveState={saveState} message={saveMessage} />

      <AnimatePresence>
        {draft && (
          <>
            <motion.div
              aria-hidden="true"
              className="ol-modal-backdrop"
              onClick={closeEditor}
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.2, ease: 'easeOut' }}
              style={{
                position: 'fixed',
                inset: 0,
                background: 'var(--ol-overlay-bg)',
                ...(mobile
                  ? {}
                  : {
                      backdropFilter: 'blur(8px) saturate(140%)',
                      WebkitBackdropFilter: 'blur(8px) saturate(140%)',
                    }),
                zIndex: mobile ? 70 : 40,
              }}
            />
            <motion.div
              role="dialog"
              aria-modal="true"
              aria-label={draft.title}
              initial={{ x: '100%', opacity: 0 }}
              animate={{ x: 0, opacity: 1 }}
              exit={{ x: '100%', opacity: 0 }}
              transition={{ type: 'spring', damping: 26, stiffness: 280 }}
              style={
                mobile
                  ? { position: 'fixed', inset: 0, width: '100%', zIndex: 71 }
                  : {
                      position: 'fixed',
                      top: 16,
                      right: 16,
                      bottom: 16,
                      width: 'min(560px, calc(100vw - 32px))',
                      zIndex: 41,
                    }
              }
            >
              <Card
                padding={0}
                style={{
                  height: '100%',
                  display: 'grid',
                  gridTemplateRows: 'auto minmax(0, 1fr)',
                  overflow: 'hidden',
                  boxShadow: mobile ? 'none' : 'var(--ol-shadow-xl)',
                  borderRadius: mobile ? 0 : undefined,
                }}
              >
                <div style={{ padding: 18, borderBottom: '0.5px solid var(--ol-line)' }}>
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'flex-start',
                      justifyContent: 'space-between',
                      gap: 12,
                    }}
                  >
                    <div
                      style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}
                    >
                      <div style={{ fontSize: 15, fontWeight: 600, color: 'var(--ol-ink)' }}>
                        {draft.title}
                      </div>
                      {draft.modified && (
                        <Pill tone="blue" size="sm">
                          {t('ghostwriter.briefs.modified')}
                        </Pill>
                      )}
                    </div>
                    <button
                      type="button"
                      onClick={closeEditor}
                      aria-label={t('common.close')}
                      style={{
                        width: 28,
                        height: 28,
                        borderRadius: 999,
                        border: 0,
                        background: 'transparent',
                        color: 'var(--ol-ink-3)',
                        display: 'inline-flex',
                        alignItems: 'center',
                        justifyContent: 'center',
                        flexShrink: 0,
                      }}
                    >
                      <Icon name="close" size={14} />
                    </button>
                  </div>
                </div>

                <div
                  className="ol-thinscroll"
                  style={{
                    overflow: 'auto',
                    padding: 18,
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 16,
                  }}
                >
                  <div style={{ fontSize: 12.5, color: 'var(--ol-ink-3)', lineHeight: 1.6 }}>
                    {draft.description}
                  </div>

                  <label style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                    <textarea
                      value={draft.body}
                      onChange={(event) =>
                        setDraft((current) =>
                          current ? { ...current, body: event.target.value } : current,
                        )
                      }
                      style={{ ...textareaStyle, minHeight: 240 }}
                    />
                    <span
                      style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', alignSelf: 'flex-end' }}
                    >
                      {t('ghostwriter.briefs.charCount', { count: draft.body.length })}
                    </span>
                  </label>

                  <Collapsible title={t('ghostwriter.briefs.fixedPart')}>
                    <div style={{ fontSize: 12, color: 'var(--ol-ink-3)', lineHeight: 1.6 }}>
                      {t('ghostwriter.briefs.fixedPartHint')}
                    </div>
                  </Collapsible>

                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      gap: 12,
                      flexWrap: 'wrap',
                    }}
                  >
                    <Btn variant="ghost" icon="close" onClick={closeEditor}>
                      {t('ghostwriter.briefs.cancel')}
                    </Btn>
                    <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                      <Btn
                        variant="ghost"
                        icon="refresh"
                        onClick={() => void handleReset()}
                        disabled={!draft.modified || busy === 'resetting'}
                      >
                        {t('ghostwriter.briefs.reset')}
                      </Btn>
                      <Btn
                        variant="blue"
                        icon="check"
                        onClick={() => void handleSave()}
                        disabled={!dirty || !draft.body.trim() || busy === 'saving'}
                      >
                        {busy === 'saving'
                          ? t('ghostwriter.briefs.saving')
                          : t('ghostwriter.briefs.save')}
                      </Btn>
                    </div>
                  </div>
                </div>
              </Card>
            </motion.div>
          </>
        )}
      </AnimatePresence>
    </div>
  );
}

const textareaStyle: CSSProperties = {
  width: '100%',
  boxSizing: 'border-box',
  padding: '11px 12px',
  borderRadius: 12,
  border: '0.5px solid var(--ol-line-strong)',
  background: 'var(--ol-style-input-bg)',
  color: 'var(--ol-ink)',
  font: 'inherit',
  fontSize: 12.5,
  lineHeight: 1.65,
  resize: 'vertical',
};
