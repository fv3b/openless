// GhostwriterSnippets.tsx — 「常用语」管理页。
// 触发词/别名 → 文本的库：说话中说到触发词即按种类生效
// （表述融进正文、其附件进背景块；背景整条进背景块，落点由全局设置统一决定）。
// 骨架照 Style.tsx 简化：PageHeader + 工具行（搜索＋种类筛选）+ 单行列表 + 右侧编辑抽屉
// ＋ dirty 确认保护；点行任意处（Toggle 除外）直接开抽屉，删除挪进抽屉底部危险区。

import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion';
import {
  createGhostwriterSnippet,
  deleteGhostwriterSnippet,
  listGhostwriterSnippets,
  saveGhostwriterSnippet,
  setGhostwriterSnippetEnabled,
} from '../lib/ipc';
import type { Snippet, SnippetAttachment, SnippetKind } from '../lib/types';
import { Btn, Card, PageHeader, Pill } from './_atoms';
import { Icon } from '../components/Icon';
import { SavedToast, type SaveToastState } from '../components/SavedToast';
import { Toggle } from './settings/shared';
import { useMobileLayout } from '../lib/useMobileLayout';

type BusyAction = 'loading' | 'saving' | 'deleting' | null;
/** 种类筛选分段控件的取值。 */
type KindFilter = 'all' | SnippetKind;

const BLANK_SNIPPET: Snippet = {
  id: '',
  trigger: '',
  aliases: [],
  text: '',
  kind: 'phrasing',
  attachments: [],
  enabled: true,
};

function cloneSnippet(snippet: Snippet): Snippet {
  return {
    ...snippet,
    aliases: [...snippet.aliases],
    attachments: snippet.attachments.map((attachment) => ({ ...attachment })),
  };
}

/** dirty 判定指纹：全部可编辑字段参与。 */
function fingerprint(snippet: Snippet | null): string {
  if (!snippet) return '';
  return JSON.stringify([
    snippet.trigger,
    snippet.aliases,
    snippet.text,
    snippet.kind,
    snippet.attachments,
    snippet.enabled,
  ]);
}

/** 搜索匹配：触发词＋别名＋表述文本，大小写不敏感。 */
function matchesQuery(snippet: Snippet, query: string): boolean {
  const folded = query.trim().toLowerCase();
  if (!folded) return true;
  return [snippet.trigger, ...snippet.aliases, snippet.text].some((field) =>
    field.toLowerCase().includes(folded),
  );
}

function parseAliases(raw: string): string[] {
  return raw
    .split(',')
    .map((value) => value.trim())
    .filter(Boolean);
}

export function GhostwriterSnippets({ embedded = false }: { embedded?: boolean }) {
  const { t } = useTranslation();
  const mobile = useMobileLayout();
  const reducedMotion = useReducedMotion();
  const [snippets, setSnippets] = useState<Snippet[]>([]);
  const [busy, setBusy] = useState<BusyAction>('loading');
  // 工具行：搜索＋种类筛选；无结果时一键清除两者。
  const [query, setQuery] = useState('');
  const [kindFilter, setKindFilter] = useState<KindFilter>('all');
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
      const list = await listGhostwriterSnippets();
      setSnippets(list);
    } catch (loadError) {
      showSaveStatus('failed', t('ghostwriter.snippets.loadFailed', { error: String(loadError) }));
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
    if (dirty && !window.confirm(t('ghostwriter.snippets.discardConfirm'))) return;
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

  const patchAttachments = (
    update: (attachments: SnippetAttachment[]) => SnippetAttachment[],
  ) => {
    setDraft((current) =>
      current ? { ...current, attachments: update(current.attachments) } : current,
    );
  };

  // 提交前归一化：背景类不携带附件；空文本附件与未选择的引用剔除。
  const cleanDraft = (snippet: Snippet): Snippet => ({
    ...snippet,
    attachments:
      snippet.kind === 'background'
        ? []
        : snippet.attachments.filter((attachment) =>
            attachment.type === 'reference'
              ? attachment.snippetId.trim().length > 0
              : attachment.text.trim().length > 0,
          ),
  });

  const handleSave = async () => {
    if (!draft || busy === 'saving') return;
    const trigger = draft.trigger.trim();
    if (!trigger) return;
    setBusy('saving');
    showSaveStatus('saving', t('ghostwriter.snippets.saving'));
    try {
      const clean = cleanDraft(draft);
      const saved = draftIsNew
        ? await createGhostwriterSnippet({ ...clean, id: '', trigger })
        : await saveGhostwriterSnippet({ ...clean, trigger });
      const list = await listGhostwriterSnippets();
      setSnippets(list);
      // 保存期间用户可能已关掉抽屉（或切到别的条目）：只对齐仍指向同一条的草稿。
      setDraft((current) => (current && current.id === draft.id ? cloneSnippet(saved) : current));
      setBaseline((current) =>
        current && current.id === draft.id ? cloneSnippet(saved) : current,
      );
      setDraftIsNew(false);
      showSaveStatus('saved', t('ghostwriter.snippets.saved'), true);
    } catch (saveError) {
      showSaveStatus('failed', t('ghostwriter.snippets.saveFailed', { error: String(saveError) }));
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
      await setGhostwriterSnippetEnabled(snippet.id, next);
    } catch (toggleError) {
      setSnippets((prev) =>
        prev.map((item) => (item.id === snippet.id ? { ...item, enabled: snippet.enabled } : item)),
      );
      showSaveStatus('failed', t('ghostwriter.snippets.updateFailed', { error: String(toggleError) }));
    }
  };

  const handleDelete = async (snippet: Snippet) => {
    if (!window.confirm(t('ghostwriter.snippets.deleteConfirm', { name: snippet.trigger }))) return;
    setBusy('deleting');
    try {
      await deleteGhostwriterSnippet(snippet.id);
      setSnippets((prev) => prev.filter((item) => item.id !== snippet.id));
      if (draft && draft.id === snippet.id) {
        setDraft(null);
        setBaseline(null);
      }
      showSaveStatus('saved', t('ghostwriter.snippets.deleted'), true);
    } catch (deleteError) {
      showSaveStatus('failed', t('ghostwriter.snippets.deleteFailed', { error: String(deleteError) }));
    } finally {
      setBusy(null);
    }
  };

  const triggerMissing = Boolean(draft && !draft.trigger.trim());

  const clearFilters = () => {
    setQuery('');
    setKindFilter('all');
  };

  const filtered = useMemo(
    () =>
      snippets.filter(
        (snippet) =>
          (kindFilter === 'all' || snippet.kind === kindFilter) && matchesQuery(snippet, query),
      ),
    [snippets, kindFilter, query],
  );

  // 「选已有背景」下拉：启用中的背景类常用语（排除已挂参考行，避免重复行）。
  const backgroundOptions = draft
    ? snippets.filter(
        (item) =>
          item.kind === 'background' &&
          item.enabled &&
          !draft.attachments.some(
            (attachment) =>
              attachment.type === 'reference' && attachment.snippetId === item.id,
          ),
      )
    : [];

  const actions = (
    <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
      <Btn
        variant="ghost"
        icon="refresh"
        size={embedded ? 'sm' : 'md'}
        onClick={() => void loadSnippets()}
        disabled={busy === 'loading'}
      >
        {t('ghostwriter.snippets.refresh')}
      </Btn>
      <Btn
        variant="primary"
        icon="plus"
        size={embedded ? 'sm' : 'md'}
        onClick={startCreate}
        disabled={busy === 'loading'}
      >
        {t('ghostwriter.snippets.create')}
      </Btn>
    </div>
  );

  return (
    <div style={{ display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
      {!embedded && (
        <PageHeader
          kicker={t('ghostwriter.snippets.kicker')}
          title={t('ghostwriter.snippets.title')}
          desc={t('ghostwriter.snippets.desc')}
          right={actions}
        />
      )}

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
          <div style={{ fontSize: 15, fontWeight: 600, color: 'var(--ol-ink)' }}>
            {t('ghostwriter.snippets.listTitle')}
          </div>
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              flexShrink: 0,
              flexWrap: 'wrap',
            }}
          >
            <Pill tone="outline">
              {t('ghostwriter.snippets.listCount', { count: snippets.length })}
            </Pill>
            {/* 嵌入 Ghostwriter 视图时页头让给页签，页头按钮落到列表头行。 */}
            {embedded && actions}
          </div>
        </div>

        {/* 工具行：搜索框＋种类筛选分段控件 */}
        <div
          style={{
            padding: '10px 18px',
            borderBottom: '0.5px solid var(--ol-line)',
            flexShrink: 0,
            display: 'flex',
            alignItems: 'center',
            gap: 10,
            flexWrap: 'wrap',
          }}
        >
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t('ghostwriter.snippets.searchPlaceholder')}
            style={{ ...toolbarInputStyle, flex: '1 1 180px', minWidth: 0 }}
          />
          <div
            style={{
              display: 'inline-flex',
              padding: 3,
              borderRadius: 8,
              background: 'var(--ol-surface-2)',
              border: '0.5px solid var(--ol-line)',
              flexShrink: 0,
            }}
          >
            {(
              [
                ['all', 'ghostwriter.snippets.filterAll'],
                ['phrasing', 'ghostwriter.snippets.filterPhrasing'],
                ['background', 'ghostwriter.snippets.filterBackground'],
              ] as const
            ).map(([value, labelKey]) => {
              const active = kindFilter === value;
              return (
                <button
                  key={value}
                  type="button"
                  aria-pressed={active}
                  onClick={() => setKindFilter(value)}
                  style={{
                    padding: '5px 12px',
                    borderRadius: 6,
                    border: 0,
                    background: active ? 'var(--ol-surface)' : 'transparent',
                    color: active ? 'var(--ol-ink)' : 'var(--ol-ink-3)',
                    boxShadow: active ? 'var(--ol-shadow-sm)' : 'none',
                    fontSize: 12,
                    fontWeight: active ? 600 : 500,
                    fontFamily: 'inherit',
                    cursor: 'default',
                  }}
                >
                  {t(labelKey)}
                </button>
              );
            })}
          </div>
        </div>

        <div className="ol-thinscroll" style={{ overflow: 'auto', flex: '1 1 0', minHeight: 0 }}>
          {busy === 'loading' && snippets.length === 0 ? (
            <div style={{ padding: 24, fontSize: 12, color: 'var(--ol-ink-4)' }}>
              {t('ghostwriter.snippets.loading')}
            </div>
          ) : snippets.length === 0 ? (
            // 空库：直通新建
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
                {t('ghostwriter.snippets.emptyTitle')}
              </div>
              <Btn variant="primary" icon="plus" onClick={startCreate}>
                {t('ghostwriter.snippets.create')}
              </Btn>
            </div>
          ) : filtered.length === 0 ? (
            // 搜索/筛选无结果：一键清除搜索与筛选
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
              <div style={{ fontSize: 13, color: 'var(--ol-ink-3)' }}>
                {t('ghostwriter.snippets.emptyFiltered')}
              </div>
              <Btn variant="ghost" icon="close" onClick={clearFilters}>
                {t('ghostwriter.snippets.clearFilters')}
              </Btn>
            </div>
          ) : (
            filtered.map((snippet) => (
              // 单行网格：触发词＋种类标签（固定宽左列）｜表述文本预览（1fr）｜启用 Toggle；
              // 三列同一网格模板 → 表述列跨行左对齐在同一 x 起点。
              <div
                key={snippet.id}
                role="button"
                tabIndex={0}
                className="ol-ring"
                onClick={() => openEditor(snippet)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    openEditor(snippet);
                  }
                }}
                style={{
                  display: 'grid',
                  gridTemplateColumns: '180px minmax(0, 1fr) auto',
                  alignItems: 'center',
                  gap: 12,
                  padding: '10px 18px',
                  borderBottom: '0.5px solid var(--ol-line)',
                  cursor: 'pointer',
                }}
              >
                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    minWidth: 0,
                  }}
                >
                  <span
                    style={{
                      fontSize: 14,
                      fontWeight: 600,
                      color: 'var(--ol-ink)',
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {snippet.trigger}
                  </span>
                  {/* 种类标签只在「全部」筛选下渲染，单一种类视图整行更干净 */}
                  {kindFilter === 'all' && (
                    <Pill tone={snippet.kind === 'phrasing' ? 'blue' : 'default'} size="sm">
                      {t(
                        snippet.kind === 'phrasing'
                          ? 'ghostwriter.snippets.kindPhrasing'
                          : 'ghostwriter.snippets.kindBackground',
                      )}
                    </Pill>
                  )}
                </div>
                <div
                  style={{
                    fontSize: 12.5,
                    color: 'var(--ol-ink-3)',
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {snippet.text || '—'}
                </div>
                <div
                  style={{ flexShrink: 0 }}
                  onClick={(event) => event.stopPropagation()}
                  onKeyDown={(event) => event.stopPropagation()}
                >
                  <Toggle on={snippet.enabled} onToggle={() => void handleToggle(snippet)} />
                </div>
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
              transition={{ duration: reducedMotion ? 0 : 0.2, ease: 'easeOut' }}
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
              aria-label={draftIsNew ? t('ghostwriter.snippets.createTitle') : t('ghostwriter.snippets.editTitle')}
              initial={reducedMotion ? { opacity: 0 } : { x: '100%', opacity: 0 }}
              animate={{ x: 0, opacity: 1 }}
              exit={reducedMotion ? { opacity: 0 } : { x: '100%', opacity: 0 }}
              transition={
                reducedMotion
                  ? { duration: 0 }
                  : { type: 'spring', damping: 26, stiffness: 280 }
              }
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
                        {draftIsNew ? t('ghostwriter.snippets.createTitle') : t('ghostwriter.snippets.editTitle')}
                      </div>
                      {dirty && <Pill tone="outline">{t('ghostwriter.snippets.unsavedBadge')}</Pill>}
                    </div>
                    <button
                      type="button"
                      onClick={closeEditor}
                      aria-label={t('ghostwriter.snippets.close')}
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
                      {t('ghostwriter.snippets.trigger')}
                    </span>
                    <input
                      value={draft.trigger}
                      onChange={(event) => patchDraft({ trigger: event.target.value })}
                      style={inputStyle}
                      placeholder={t('ghostwriter.snippets.triggerPlaceholder')}
                    />
                    <span style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}>
                      {t('ghostwriter.snippets.triggerHint')}
                    </span>
                  </label>

                  <label style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                    <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--ol-ink)' }}>
                      {t('ghostwriter.snippets.aliases')}
                    </span>
                    <input
                      value={draft.aliases.join(', ')}
                      onChange={(event) =>
                        patchDraft({ aliases: parseAliases(event.target.value) })
                      }
                      style={inputStyle}
                      placeholder={t('ghostwriter.snippets.aliasesPlaceholder')}
                    />
                    <span style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}>
                      {t('ghostwriter.snippets.aliasesHint')}
                    </span>
                  </label>

                  <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                    <span style={fieldLabelStyle}>{t('ghostwriter.snippets.kindLabel')}</span>
                    {(
                      [
                        ['phrasing', 'kindPhrasing', 'kindPhrasingHint'],
                        ['background', 'kindBackground', 'kindBackgroundHint'],
                      ] as const
                    ).map(([value, labelKey, hintKey]) => (
                      <label
                        key={value}
                        style={{
                          display: 'flex',
                          alignItems: 'flex-start',
                          gap: 8,
                          cursor: 'default',
                        }}
                      >
                        <input
                          type="radio"
                          name="snippet-kind"
                          checked={draft.kind === value}
                          onChange={() => patchDraft({ kind: value })}
                          style={{ marginTop: 2 }}
                        />
                        <span style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                          <span style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink)' }}>
                            {t(`ghostwriter.snippets.${labelKey}`)}
                          </span>
                          <span
                            style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}
                          >
                            {t(`ghostwriter.snippets.${hintKey}`)}
                          </span>
                        </span>
                      </label>
                    ))}
                  </div>

                  <label style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                    <span style={fieldLabelStyle}>
                      {draft.kind === 'background'
                        ? t('ghostwriter.snippets.textBackground')
                        : t('ghostwriter.snippets.textPhrasing')}
                    </span>
                    <textarea
                      value={draft.text}
                      onChange={(event) => patchDraft({ text: event.target.value })}
                      style={{ ...textareaStyle, minHeight: 110 }}
                      placeholder={
                        draft.kind === 'background'
                          ? t('ghostwriter.snippets.textBackgroundPlaceholder')
                          : t('ghostwriter.snippets.textPhrasingPlaceholder')
                      }
                    />
                  </label>

                  {draft.kind === 'phrasing' && (
                    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                      <span style={fieldLabelStyle}>
                        {t('ghostwriter.snippets.attachmentsTitle')}
                      </span>
                      <span style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}>
                        {t('ghostwriter.snippets.attachmentsHint')}
                      </span>
                      {draft.attachments.map((attachment, index) => {
                        const referenced =
                          attachment.type === 'reference'
                            ? snippets.find((item) => item.id === attachment.snippetId)
                            : undefined;
                        return (
                          <div
                            key={index}
                            style={{
                              display: 'flex',
                              alignItems: 'center',
                              gap: 8,
                              padding: '6px 8px',
                              borderRadius: 10,
                              border: '0.5px solid var(--ol-line)',
                              background: 'var(--ol-surface-2)',
                            }}
                          >
                            {attachment.type === 'reference' ? (
                              <span
                                style={{
                                  flex: 1,
                                  minWidth: 0,
                                  fontSize: 12.5,
                                  color: referenced ? 'var(--ol-ink)' : 'var(--ol-ink-3)',
                                  overflow: 'hidden',
                                  textOverflow: 'ellipsis',
                                  whiteSpace: 'nowrap',
                                }}
                              >
                                {referenced
                                  ? referenced.trigger
                                  : t('ghostwriter.snippets.attachmentMissingReference')}
                              </span>
                            ) : (
                              <input
                                value={attachment.text}
                                onChange={(event) =>
                                  patchAttachments((list) =>
                                    list.map((item, i) =>
                                      i === index
                                        ? { type: 'text', text: event.target.value }
                                        : item,
                                    ),
                                  )
                                }
                                style={{ ...inputStyle, minHeight: 32, flex: 1 }}
                              />
                            )}
                            <button
                              type="button"
                              onClick={() =>
                                patchAttachments((list) =>
                                  list.filter((_, i) => i !== index),
                                )
                              }
                              aria-label={t('ghostwriter.snippets.delete')}
                              title={t('ghostwriter.snippets.delete')}
                              style={{
                                width: 26,
                                height: 26,
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
                              <Icon name="close" size={12} />
                            </button>
                          </div>
                        );
                      })}
                      <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                        <select
                          value=""
                          disabled={backgroundOptions.length === 0}
                          onChange={(event) => {
                            const snippetId = event.target.value;
                            if (!snippetId) return;
                            patchAttachments((list) => [
                              ...list,
                              { type: 'reference', snippetId },
                            ]);
                          }}
                          style={{ ...inputStyle, minHeight: 34, flex: '1 1 160px' }}
                        >
                          <option value="">
                            {t('ghostwriter.snippets.attachmentAddReference')}
                          </option>
                          {backgroundOptions.map((item) => (
                            <option key={item.id} value={item.id}>
                              {item.trigger}
                            </option>
                          ))}
                        </select>
                        <Btn
                          variant="ghost"
                          icon="plus"
                          onClick={() =>
                            patchAttachments((list) => [...list, { type: 'text', text: '' }])
                          }
                        >
                          {t('ghostwriter.snippets.attachmentAddText')}
                        </Btn>
                      </div>
                      {backgroundOptions.length === 0 && (
                        <span
                          style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}
                        >
                          {t('ghostwriter.snippets.attachmentNoOptions')}
                        </span>
                      )}
                    </div>
                  )}

                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      gap: 12,
                    }}
                  >
                    <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--ol-ink)' }}>
                      {t('ghostwriter.snippets.enabled')}
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
                      {t('ghostwriter.snippets.cancelChanges')}
                    </Btn>
                    <Btn
                      variant="blue"
                      icon="check"
                      onClick={() => void handleSave()}
                      disabled={!dirty || triggerMissing || busy === 'saving'}
                    >
                      {busy === 'saving' ? t('ghostwriter.snippets.saving') : t('ghostwriter.snippets.save')}
                    </Btn>
                  </div>

                  {/* 底部危险区：删除已存条目（confirm 照旧）；新建草稿无可删 */}
                  {!draftIsNew && (
                    <div
                      style={{
                        paddingTop: 14,
                        borderTop: '0.5px solid var(--ol-line)',
                        display: 'flex',
                        justifyContent: 'flex-start',
                      }}
                    >
                      <Btn
                        variant="ghost"
                        icon="trash"
                        onClick={() => void handleDelete(draft)}
                        disabled={busy === 'deleting'}
                        style={{ color: 'var(--ol-err)', borderColor: 'var(--ol-err)' }}
                      >
                        {busy === 'deleting'
                          ? t('ghostwriter.snippets.deleting')
                          : t('ghostwriter.snippets.delete')}
                      </Btn>
                    </div>
                  )}
                </div>
              </Card>
            </motion.div>
          </>
        )}
      </AnimatePresence>
    </div>
  );
}

const fieldLabelStyle: CSSProperties = {
  fontSize: 12,
  fontWeight: 600,
  color: 'var(--ol-ink)',
};

const toolbarInputStyle: CSSProperties = {
  boxSizing: 'border-box',
  minHeight: 34,
  padding: '8px 11px',
  borderRadius: 10,
  border: '0.5px solid var(--ol-line-strong)',
  background: 'var(--ol-style-input-bg)',
  color: 'var(--ol-ink)',
  font: 'inherit',
  fontSize: 12.5,
};

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
