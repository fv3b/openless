// FluidSnippets.tsx — 「常用语」管理页。
// 触发词/别名 → 表述文本的库：说话中说到触发词即按贴位生效（inline 进正文 / footnote 附在文末）。
// 骨架照 Style.tsx 简化：PageHeader + Card 列表 + 右侧编辑抽屉 + dirty 确认保护；
// 文案暂硬编码中文，Task 11 收编为 fluid.* i18n key。

import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import {
  createFluidSnippet,
  deleteFluidSnippet,
  listFluidSnippets,
  saveFluidSnippet,
  setFluidSnippetEnabled,
} from '../lib/ipc';
import type { Snippet, SnippetMode } from '../lib/types';
import { Btn, Card, PageHeader, Pill } from './_atoms';
import { Icon } from '../components/Icon';
import { SavedToast, type SaveToastState } from '../components/SavedToast';
import { Toggle } from './settings/shared';
import { useMobileLayout } from '../lib/useMobileLayout';

type BusyAction = 'loading' | 'saving' | 'deleting' | null;

const BLANK_SNIPPET: Snippet = {
  id: '',
  trigger: '',
  aliases: [],
  text: '',
  mode: 'inline',
  enabled: true,
};

const MODE_LABELS: Record<SnippetMode, string> = {
  inline: '贴进正文',
  footnote: '附在文末',
};

function cloneSnippet(snippet: Snippet): Snippet {
  return { ...snippet, aliases: [...snippet.aliases] };
}

/** dirty 判定指纹：五个可编辑字段全部参与。 */
function fingerprint(snippet: Snippet | null): string {
  if (!snippet) return '';
  return JSON.stringify([
    snippet.trigger,
    snippet.aliases,
    snippet.text,
    snippet.mode,
    snippet.enabled,
  ]);
}

function parseAliases(raw: string): string[] {
  return raw
    .split(',')
    .map((value) => value.trim())
    .filter(Boolean);
}

export function FluidSnippets() {
  const mobile = useMobileLayout();
  const [snippets, setSnippets] = useState<Snippet[]>([]);
  const [busy, setBusy] = useState<BusyAction>('loading');
  // 编辑抽屉：draft 非空即打开；draftIsNew = 新建（保存时走 create，id 留空给后端生成）。
  const [draft, setDraft] = useState<Snippet | null>(null);
  const [baseline, setBaseline] = useState<Snippet | null>(null);
  const [draftIsNew, setDraftIsNew] = useState(false);
  const [saveState, setSaveState] = useState<SaveToastState>('idle');
  const [saveMessage, setSaveMessage] = useState('');
  const statusTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (statusTimer.current !== null) window.clearTimeout(statusTimer.current);
    },
    [],
  );

  const showSaveStatus = (state: SaveToastState, message: string, temporary = false) => {
    if (statusTimer.current !== null) {
      window.clearTimeout(statusTimer.current);
      statusTimer.current = null;
    }
    setSaveState(state);
    setSaveMessage(message);
    if (temporary || state === 'failed') {
      const delay = state === 'failed' ? 6000 : 1600;
      statusTimer.current = window.setTimeout(() => {
        setSaveState('idle');
        setSaveMessage('');
        statusTimer.current = null;
      }, delay);
    }
  };

  const loadSnippets = async () => {
    setBusy('loading');
    try {
      const list = await listFluidSnippets();
      setSnippets(list);
    } catch (loadError) {
      showSaveStatus('failed', `加载失败：${String(loadError)}`);
    } finally {
      setBusy(null);
    }
  };

  useEffect(() => {
    void loadSnippets();
  }, []);

  const dirty = fingerprint(draft) !== fingerprint(baseline);

  const startCreate = () => {
    setDraft(cloneSnippet(BLANK_SNIPPET));
    setBaseline(cloneSnippet(BLANK_SNIPPET));
    setDraftIsNew(true);
  };

  const openEditor = (snippet: Snippet) => {
    setDraft(cloneSnippet(snippet));
    setBaseline(cloneSnippet(snippet));
    setDraftIsNew(false);
  };

  const discardDraftChanges = () => {
    if (baseline) setDraft(cloneSnippet(baseline));
  };

  // dirty 保护照 Style.tsx：关闭抽屉前若未保存，弹确认丢弃。
  const closeEditor = () => {
    if (dirty && !window.confirm('改动还没保存，确定丢弃吗？')) return;
    setDraft(null);
    setBaseline(null);
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
  }, [draft, baseline, dirty]);

  const patchDraft = (patch: Partial<Snippet>) => {
    setDraft((current) => (current ? { ...current, ...patch } : current));
  };

  const handleSave = async () => {
    if (!draft || busy === 'saving') return;
    const trigger = draft.trigger.trim();
    if (!trigger) return;
    setBusy('saving');
    showSaveStatus('saving', '保存中…');
    try {
      const saved = draftIsNew
        ? await createFluidSnippet({ ...draft, id: '', trigger })
        : await saveFluidSnippet({ ...draft, trigger });
      const list = await listFluidSnippets();
      setSnippets(list);
      // 保存期间用户可能已关掉抽屉（或切到别的条目）：只对齐仍指向同一条的草稿。
      setDraft((current) => (current && current.id === draft.id ? cloneSnippet(saved) : current));
      setBaseline((current) =>
        current && current.id === draft.id ? cloneSnippet(saved) : current,
      );
      setDraftIsNew(false);
      showSaveStatus('saved', '已保存', true);
    } catch (saveError) {
      showSaveStatus('failed', `保存失败：${String(saveError)}`);
    } finally {
      setBusy(null);
    }
  };

  // 启用开关乐观更新；后端失败回滚并报错，避免 UI 显示已停用但命中仍生效（issue #60 同款）。
  const handleToggle = async (snippet: Snippet) => {
    const next = !snippet.enabled;
    setSnippets((prev) =>
      prev.map((item) => (item.id === snippet.id ? { ...item, enabled: next } : item)),
    );
    try {
      await setFluidSnippetEnabled(snippet.id, next);
    } catch (toggleError) {
      setSnippets((prev) =>
        prev.map((item) => (item.id === snippet.id ? { ...item, enabled: snippet.enabled } : item)),
      );
      showSaveStatus('failed', `更新失败：${String(toggleError)}`);
    }
  };

  const handleDelete = async (snippet: Snippet) => {
    if (!window.confirm(`删除「${snippet.trigger}」？删除后不可恢复。`)) return;
    setBusy('deleting');
    try {
      await deleteFluidSnippet(snippet.id);
      setSnippets((prev) => prev.filter((item) => item.id !== snippet.id));
      if (draft && draft.id === snippet.id) {
        setDraft(null);
        setBaseline(null);
      }
      showSaveStatus('saved', '已删除', true);
    } catch (deleteError) {
      showSaveStatus('failed', `删除失败：${String(deleteError)}`);
    } finally {
      setBusy(null);
    }
  };

  const triggerMissing = Boolean(draft && !draft.trigger.trim());

  return (
    <div style={{ display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
      <PageHeader
        kicker="指令台"
        title="常用语"
        desc="存下你调优过的说法：说话中说到触发词，对应表述就按贴位融进贴给 AI 的文本。"
        right={
          <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
            <Btn
              variant="ghost"
              icon="refresh"
              onClick={() => void loadSnippets()}
              disabled={busy === 'loading'}
            >
              刷新
            </Btn>
            <Btn variant="primary" icon="plus" onClick={startCreate} disabled={busy === 'loading'}>
              新建常用语
            </Btn>
          </div>
        }
      />

      <SavedToast saveState={saveState} message={saveMessage} />

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
            alignItems: 'center',
            justifyContent: 'space-between',
            gap: 12,
          }}
        >
          <div style={{ fontSize: 15, fontWeight: 600, color: 'var(--ol-ink)' }}>全部常用语</div>
          <Pill tone="outline">{snippets.length} 条</Pill>
        </div>

        <div className="ol-thinscroll" style={{ overflow: 'auto', flex: '1 1 0', minHeight: 0 }}>
          {busy === 'loading' && snippets.length === 0 ? (
            <div style={{ padding: 24, fontSize: 12, color: 'var(--ol-ink-4)' }}>加载中…</div>
          ) : snippets.length === 0 ? (
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'center',
                justifyContent: 'center',
                gap: 12,
                padding: '64px 24px',
              }}
            >
              <div
                style={{
                  width: 52,
                  height: 52,
                  borderRadius: 999,
                  display: 'inline-flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  background: 'var(--ol-surface-2)',
                  color: 'var(--ol-ink-3)',
                }}
              >
                <Icon name="tag" size={24} />
              </div>
              <div style={{ fontSize: 13, color: 'var(--ol-ink-3)', lineHeight: 1.6 }}>
                存下你调优过的说法，用触发词随叫随到
              </div>
              <Btn variant="primary" icon="plus" onClick={startCreate}>
                新建常用语
              </Btn>
            </div>
          ) : (
            snippets.map((snippet) => (
              <div
                key={snippet.id}
                role="button"
                tabIndex={0}
                onClick={() => openEditor(snippet)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    openEditor(snippet);
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
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 8,
                      flexWrap: 'wrap',
                    }}
                  >
                    <span style={{ fontSize: 14, fontWeight: 600, color: 'var(--ol-ink)' }}>
                      {snippet.trigger}
                    </span>
                    <Pill tone={snippet.mode === 'inline' ? 'blue' : 'default'} size="sm">
                      {MODE_LABELS[snippet.mode]}
                    </Pill>
                    {!snippet.enabled && (
                      <Pill tone="outline" size="sm">
                        已停用
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
                    {snippet.text || '—'}
                  </div>
                </div>
                <Toggle on={snippet.enabled} onToggle={() => void handleToggle(snippet)} />
                <button
                  type="button"
                  onClick={(event) => {
                    event.stopPropagation();
                    openEditor(snippet);
                  }}
                  aria-label="编辑"
                  title="编辑"
                  style={{
                    width: 30,
                    height: 30,
                    flexShrink: 0,
                    border: 0,
                    borderRadius: 8,
                    background: 'transparent',
                    color: 'var(--ol-ink-3)',
                    display: 'inline-flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    cursor: 'default',
                  }}
                >
                  <Icon name="pencil" size={14} />
                </button>
                <button
                  type="button"
                  onClick={(event) => {
                    event.stopPropagation();
                    void handleDelete(snippet);
                  }}
                  disabled={busy === 'deleting'}
                  aria-label="删除"
                  title="删除"
                  style={{
                    width: 30,
                    height: 30,
                    flexShrink: 0,
                    border: 0,
                    borderRadius: 8,
                    background: 'transparent',
                    color: 'var(--ol-ink-3)',
                    display: 'inline-flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    cursor: 'default',
                    opacity: busy === 'deleting' ? 0.55 : 1,
                  }}
                >
                  <Icon name="trash" size={14} />
                </button>
              </div>
            ))
          )}
        </div>
      </Card>

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
              aria-label={draftIsNew ? '新建常用语' : '编辑常用语'}
              initial={{ x: '100%', opacity: 0 }}
              animate={{ x: 0, opacity: 1 }}
              exit={{ x: '100%', opacity: 0 }}
              transition={{ type: 'spring', damping: 26, stiffness: 280 }}
              style={
                mobile
                  ? {
                      position: 'fixed',
                      inset: 0,
                      width: '100%',
                      zIndex: 71,
                    }
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
                        {draftIsNew ? '新建常用语' : '编辑常用语'}
                      </div>
                      {dirty && <Pill tone="outline">未保存</Pill>}
                    </div>
                    <button
                      type="button"
                      onClick={closeEditor}
                      aria-label="关闭"
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
                  <label style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                    <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--ol-ink)' }}>
                      触发词
                    </span>
                    <input
                      value={draft.trigger}
                      onChange={(event) => patchDraft({ trigger: event.target.value })}
                      style={inputStyle}
                      placeholder="说话中说到就生效，例如：翻译"
                    />
                    <span style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}>
                      必填；同一触发词不可重复。
                    </span>
                  </label>

                  <label style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                    <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--ol-ink)' }}>
                      别名
                    </span>
                    <input
                      value={draft.aliases.join(', ')}
                      onChange={(event) =>
                        patchDraft({ aliases: parseAliases(event.target.value) })
                      }
                      style={inputStyle}
                      placeholder="多个别名用逗号分隔"
                    />
                    <span style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}>
                      别名和触发词一样都能命中。
                    </span>
                  </label>

                  <label style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                    <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--ol-ink)' }}>
                      表述文本
                    </span>
                    <textarea
                      value={draft.text}
                      onChange={(event) => patchDraft({ text: event.target.value })}
                      style={{ ...textareaStyle, minHeight: 110 }}
                      placeholder="命中后按下方贴位融进贴给 AI 的文本"
                    />
                  </label>

                  <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                    <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--ol-ink)' }}>
                      贴位
                    </span>
                    <div
                      style={{
                        display: 'inline-flex',
                        alignSelf: 'flex-start',
                        padding: 3,
                        borderRadius: 8,
                        background: 'var(--ol-surface-2)',
                        border: '0.5px solid var(--ol-line)',
                      }}
                    >
                      {(
                        [
                          ['inline', MODE_LABELS.inline],
                          ['footnote', MODE_LABELS.footnote],
                        ] as const
                      ).map(([value, label]) => {
                        const active = draft.mode === value;
                        return (
                          <button
                            key={value}
                            type="button"
                            aria-pressed={active}
                            onClick={() => patchDraft({ mode: value })}
                            style={{
                              padding: '6px 12px',
                              borderRadius: 6,
                              border: 0,
                              background: active ? 'var(--ol-surface)' : 'transparent',
                              color: active ? 'var(--ol-ink)' : 'var(--ol-ink-3)',
                              boxShadow: active ? 'var(--ol-shadow-sm)' : 'none',
                              fontSize: 12,
                              fontWeight: active ? 600 : 500,
                              fontFamily: 'inherit',
                            }}
                          >
                            {label}
                          </button>
                        );
                      })}
                    </div>
                    <span style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}>
                      贴进正文＝融进正文原位；附在文末＝文末附注块。
                    </span>
                  </div>

                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      gap: 12,
                    }}
                  >
                    <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--ol-ink)' }}>
                      启用
                    </span>
                    <Toggle on={draft.enabled} onToggle={(next) => patchDraft({ enabled: next })} />
                  </div>

                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      gap: 12,
                      flexWrap: 'wrap',
                    }}
                  >
                    <Btn
                      variant="ghost"
                      icon="refresh"
                      onClick={discardDraftChanges}
                      disabled={!dirty}
                    >
                      撤销改动
                    </Btn>
                    <Btn
                      variant="blue"
                      icon="check"
                      onClick={() => void handleSave()}
                      disabled={!dirty || triggerMissing || busy === 'saving'}
                    >
                      {busy === 'saving' ? '保存中…' : '保存'}
                    </Btn>
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

const inputStyle: CSSProperties = {
  width: '100%',
  boxSizing: 'border-box',
  minHeight: 38,
  padding: '9px 11px',
  borderRadius: 10,
  border: '0.5px solid var(--ol-line-strong)',
  background: 'var(--ol-style-input-bg)',
  color: 'var(--ol-ink)',
  font: 'inherit',
  fontSize: 12.5,
};

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
