// Vocab.tsx — 「词典」页。
// 结构：
//   - 顶部：标题 + 右上「新词」「从语音记录提取热词」入口（弹窗：直接输入 或 从预设模板批量导入）
//   - 工具行：所有 / 自动添加 / 手动添加 分段筛选 + 右侧圆形搜索（点击向左展开）
//   - 词条网格：卡片默认只显文字，hover 变灰并浮现「编辑 / 删除」操作
//   - 编辑走弹窗（update_vocab 保 id/hits）；场景预设保持卡片区块
//   - 纠正规则已迁往「工具 → 纠正规则」页（Corrections.tsx）
//   - 热词提取向导（2026-09-18 批 3）：全历史多选 → LLM 从 raw 原文找识别
//     混乱的词 → 编辑勾选批量进词典（只进词不加备注）；已提取记录仅淡化
// 数据落地到 ~/Library/Application Support/OpenLess/dictionary.json（与 Swift 同名）。

import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Icon } from '../components/Icon';
import { Tooltip } from '../components/Tooltip';
import { SavedToast, type SaveToastState } from '../components/SavedToast';
import {
  addVocab,
  extractHotwordCandidates,
  isTauri,
  listVocab,
  listHistory,
  markHistoryExtracted,
  removeVocab,
  setVocabEnabled,
  updateVocab,
} from '../lib/ipc';
import type { DictationSession, DictionaryEntry, GhostwriterHotwordDraft, VocabPreset } from '../lib/types';
import { DEFAULT_VOCAB_PRESETS, loadVocabPresets, persistVocabPresets } from '../lib/vocabPresets';
import { useExitMount } from '../lib/useExitMount';
import { useMobileLayout } from '../lib/useMobileLayout';
import { Btn, Card, Collapsible, PageHeader, Pill } from './_atoms';

const NEW_PRESET_DRAFT_ID = '__new__';

/** 自动收集词条靠 note 认（后端 accept_pending_correction 打的就是这个标记）。 */
const LEARNED_NOTE = '从手改中自动收集';

type SourceFilter = 'all' | 'auto' | 'manual';

export function Vocab() {
  const { t } = useTranslation();
  const mobile = useMobileLayout();
  const [entries, setEntries] = useState<DictionaryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const inputRef = useRef<HTMLInputElement>(null);

  const [error, setError] = useState<string | null>(null);
  const [presets, setPresets] = useState<VocabPreset[]>(DEFAULT_VOCAB_PRESETS);
  const [selectedPresetIds, setSelectedPresetIds] = useState<string[]>([]);
  const [editingPresetId, setEditingPresetId] = useState<string | null>(null);
  const [presetNameDraft, setPresetNameDraft] = useState('');
  const [presetPhrasesDraft, setPresetPhrasesDraft] = useState('');

  // 词典改版新增状态
  const [filter, setFilter] = useState<SourceFilter>('all');
  const [query, setQuery] = useState('');
  const [searchOpen, setSearchOpen] = useState(false);
  const [editingEntry, setEditingEntry] = useState<DictionaryEntry | null>(null);
  const [editDraft, setEditDraft] = useState('');
  const [editError, setEditError] = useState<string | null>(null);
  const [newWordOpen, setNewWordOpen] = useState(false);
  const [newWordDraft, setNewWordDraft] = useState('');
  const [newWordTemplateIds, setNewWordTemplateIds] = useState<string[]>([]);
  const [saveState, setSaveState] = useState<'idle' | 'saved'>('idle');
  const editMount = useExitMount(editingEntry !== null);
  const newWordMount = useExitMount(newWordOpen);

  // 词条网格 FLIP：增删/筛选让行位移时，从旧位置滑到新位置；
  // 新卡片入场走 .ol-word-card 的 CSS 动画，删除走 onRemove 里的退场动画。
  const cardRefs = useRef(new Map<string, HTMLDivElement>());
  const prevCardTops = useRef(new Map<string, number>());
  const [removingIds, setRemovingIds] = useState<Set<string>>(new Set());
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [batchBusy, setBatchBusy] = useState(false);

  // 热词提取向导（照 GhostwriterSnippets 两步向导模式）：
  // pick＝选历史语音记录（全部历史、rawTranscript 非空、已提取淡化），
  // review＝编辑/勾选候选热词后逐条 addVocab 入库（只进词不加备注）。
  const [extractView, setExtractView] = useState<'closed' | 'pick' | 'review'>('closed');
  const [voiceRecords, setVoiceRecords] = useState<DictationSession[]>([]);
  const [pickedIds, setPickedIds] = useState<Set<string>>(new Set());
  const [hotwordDrafts, setHotwordDrafts] = useState<GhostwriterHotwordDraft[]>([]);
  const [hotwordChecks, setHotwordChecks] = useState<boolean[]>([]);
  const [failedHotwordIdx, setFailedHotwordIdx] = useState<Set<number>>(new Set());
  const [extractBusy, setExtractBusy] = useState(false);
  const [savingExtract, setSavingExtract] = useState(false);
  const [extractSaveState, setExtractSaveState] = useState<SaveToastState>('idle');
  const [extractMessage, setExtractMessage] = useState('');
  const extractTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (extractTimer.current !== null) window.clearTimeout(extractTimer.current);
    },
    [],
  );

  const showExtractStatus = (state: SaveToastState, message: string) => {
    if (extractTimer.current !== null) window.clearTimeout(extractTimer.current);
    setExtractSaveState(state);
    setExtractMessage(message);
    if (state !== 'idle') {
      const delay = state === 'failed' ? 6000 : 1600;
      extractTimer.current = window.setTimeout(() => setExtractSaveState('idle'), delay);
    }
  };

  const refresh = async () => {
    try {
      setError(null);
      const data = await listVocab();
      setEntries(data);
      const ids = new Set(data.map((entry) => entry.id));
      setSelectedIds((current) => new Set([...current].filter((id) => ids.has(id))));
    } catch (e) {
      // 之前没 try/catch,后端 decode 失败时 spinner 永久卡死。
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
    void loadVocabPresets()
      .then(setPresets)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
    // 订阅后端 vocab:updated：每段口述结束、record_hits 触发后由 coordinator 推送。
    // Vocab 页面打开期间能即时看到命中数累加，无需切到其他 tab 再切回。
    if (!isTauri) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    (async () => {
      const { listen } = await import('@tauri-apps/api/event');
      const handle = await listen('vocab:updated', () => {
        void refresh();
      });
      if (cancelled) handle();
      else unlisten = handle;
    })();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const flashSaved = () => {
    setSaveState('saved');
    window.setTimeout(() => setSaveState('idle'), 1600);
  };

  // ── 热词提取向导 ────────────────────────────────────────────────

  /** 提取向导的相对时间标签：<24h → x 小时前；24–48h → 昨天；之后 → x 天前。 */
  const extractRelativeAgeLabel = (createdAt: string): string => {
    const time = new Date(createdAt).getTime();
    if (!Number.isFinite(time)) return '';
    const hours = Math.floor((Date.now() - time) / 3_600_000);
    if (hours < 24) return t('vocab.extract.hoursAgo', { n: Math.max(1, hours) });
    if (hours < 48) return t('vocab.extract.yesterday');
    return t('vocab.extract.daysAgo', { n: Math.floor(hours / 24) });
  };

  const openHotwordExtract = async () => {
    try {
      const all = await listHistory();
      // 全部历史多选（与常用语提取同范围），只要求有转写原文；新到旧排。
      const records = all
        .filter((record) => record.rawTranscript.trim().length > 0)
        .sort((a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime());
      setVoiceRecords(records);
      setPickedIds(new Set(records.map((record) => record.id)));
      setExtractView('pick');
    } catch (loadError) {
      showExtractStatus('failed', t('vocab.extract.loadFailed', { error: String(loadError) }));
    }
  };

  const closeHotwordExtract = () => {
    setExtractView('closed');
    setVoiceRecords([]);
    setPickedIds(new Set());
    setHotwordDrafts([]);
    setHotwordChecks([]);
    setFailedHotwordIdx(new Set());
  };

  const toggleHotwordPick = (id: string) => {
    setPickedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const startHotwordExtraction = async () => {
    if (pickedIds.size === 0 || extractBusy) return;
    setExtractBusy(true);
    try {
      const result = await extractHotwordCandidates([...pickedIds]);
      if (result.length === 0) {
        showExtractStatus('failed', t('vocab.extract.extractEmptyResult'));
        return;
      }
      setHotwordDrafts(result);
      setHotwordChecks(result.map(() => true));
      setFailedHotwordIdx(new Set());
      setExtractView('review');
    } catch (error) {
      // 错误原文可能很长：截断，避免 toast 药丸（nowrap）溢出屏幕。
      const detail = String(error);
      const clipped = detail.length > 80 ? `${detail.slice(0, 80)}…` : detail;
      showExtractStatus('failed', t('vocab.extract.extractFailed', { error: clipped }));
    } finally {
      setExtractBusy(false);
    }
  };

  const patchHotwordDraft = (index: number, patch: Partial<GhostwriterHotwordDraft>) => {
    setHotwordDrafts((current) =>
      current.map((draft, i) => (i === index ? { ...draft, ...patch } : draft)),
    );
  };

  const toggleHotwordCheck = (index: number) => {
    setHotwordChecks((current) => current.map((checked, i) => (i === index ? !checked : checked)));
  };

  const toggleAllHotwordDrafts = () => {
    const allChecked = hotwordChecks.every(Boolean);
    setHotwordChecks(hotwordDrafts.map(() => !allChecked));
  };

  // 保存所选：逐条 addVocab（只进词、不加备注——词典 note 是内部学习标记）。
  // 词条为空或词典已有同词条（不区分大小写）就地标红留原地可改；其余失败同样
  // 标红；全部处理完 toast 汇总，全存完即关向导。有保存成功就对当初所选会话
  // 写提取标记（两个向导共用，仅视觉淡化）。
  const saveHotwordSelection = async () => {
    if (savingExtract) return;
    setSavingExtract(true);
    const markedSessionIds = [...pickedIds];
    const failures = new Set<number>();
    const known = new Set(
      entries.map((entry) => entry.phrase.trim().toLowerCase()),
    );
    const remaining: GhostwriterHotwordDraft[] = [];
    const remainingChecks: boolean[] = [];
    let saved = 0;
    for (let index = 0; index < hotwordDrafts.length; index++) {
      if (!hotwordChecks[index]) {
        remaining.push(hotwordDrafts[index]);
        remainingChecks.push(true);
        continue;
      }
      const draft = hotwordDrafts[index];
      const hotword = draft.hotword.trim();
      if (!hotword || known.has(hotword.toLowerCase())) {
        failures.add(remaining.length);
        remaining.push(draft);
        remainingChecks.push(hotwordChecks[index]);
        continue;
      }
      try {
        await addVocab(hotword);
        known.add(hotword.toLowerCase());
        saved += 1;
      } catch {
        failures.add(remaining.length);
        remaining.push(draft);
        remainingChecks.push(hotwordChecks[index]);
      }
    }
    setHotwordDrafts(remaining);
    setHotwordChecks(remainingChecks);
    setFailedHotwordIdx(failures);
    if (saved > 0) {
      try {
        await refresh();
      } catch {
        // 汇总 toast 已提示保存结果；列表刷新失败不打断收尾。
      }
      try {
        await markHistoryExtracted(markedSessionIds);
      } catch {
        // 标记只影响淡化样式，写失败不打断保存收尾。
      }
    }
    if (saved > 0 && failures.size === 0) {
      closeHotwordExtract();
      showExtractStatus('saved', t('vocab.extract.savedSummary', { count: saved }));
    } else if (failures.size > 0) {
      showExtractStatus(
        'failed',
        t('vocab.extract.partialSummary', { count: saved, failed: failures.size }),
      );
    }
    setSavingExtract(false);
  };

  const onAdd = async () => {
    const phrase = inputRef.current?.value.trim();
    if (!phrase) return;
    try {
      const entry = await addVocab(phrase);
      // 乐观插入头部（addVocab 返回新 entry，浏览器 mock 下也能立刻看到）。
      setEntries((prev) => [entry, ...prev]);
      flashSaved();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    if (inputRef.current) inputRef.current.value = '';
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      void onAdd();
    }
  };

  const removeEntries = async (ids: string[]) => {
    if (batchBusy || ids.length === 0) return;
    setBatchBusy(true);
    setError(null);
    const removed = new Set<string>();
    let failures = 0;
    try {
      // Bound IPC concurrency and retain unsuccessful selections for retry.
      for (let start = 0; start < ids.length; start += 8) {
        const batch = ids.slice(start, start + 8);
        const results = await Promise.allSettled(batch.map((id) => removeVocab(id)));
        results.forEach((result, index) => {
          if (result.status === 'fulfilled') removed.add(batch[index]);
          else failures += 1;
        });
      }
      await Promise.all([...removed].map((id) => fadeOutCard(id)));
      setEntries((current) => current.filter((entry) => !removed.has(entry.id)));
      setSelectedIds((current) => new Set([...current].filter((id) => !removed.has(id))));
      if (failures) setError(t('vocab.batchDeleteFailed', { count: failures }));
      else flashSaved();
    } finally {
      setRemovingIds(new Set());
      setBatchBusy(false);
    }
  };

  const toggleSelection = (id: string) => {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const onToggle = async (entry: DictionaryEntry) => {
    const next = !entry.enabled;
    // 乐观更新 UI；后端失败时回滚 + 让用户看到错误，避免 UI 显示「已禁用」但 ASR/polish
    // 仍在注入此词条造成的诡异状态。issue #60。
    setEntries((prev) => prev.map((e) => (e.id === entry.id ? { ...e, enabled: next } : e)));
    try {
      await setVocabEnabled(entry.id, next);
    } catch (err) {
      setEntries((prev) =>
        prev.map((e) => (e.id === entry.id ? { ...e, enabled: entry.enabled } : e)),
      );
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const openEdit = (entry: DictionaryEntry) => {
    setEditingEntry(entry);
    setEditDraft(entry.phrase);
    setEditError(null);
  };

  const saveEdit = async () => {
    if (!editingEntry) return;
    const phrase = editDraft.trim();
    if (!phrase) {
      setEditError(t('vocab.editEmpty'));
      return;
    }
    if (phrase === editingEntry.phrase) {
      setEditingEntry(null);
      return;
    }
    try {
      await updateVocab(editingEntry.id, phrase);
      // 乐观改名：id / hits / enabled 保持不变（后端 update_vocab 原地改 phrase）。
      setEntries((prev) => prev.map((e) => (e.id === editingEntry.id ? { ...e, phrase } : e)));
      setEditingEntry(null);
      flashSaved();
    } catch (err) {
      setEditError(err instanceof Error ? err.message : String(err));
    }
  };

  const addNewWord = async () => {
    const phrase = newWordDraft.trim();
    if (!phrase) return;
    try {
      const entry = await addVocab(phrase);
      setEntries((prev) => [entry, ...prev]);
      setNewWordDraft('');
      flashSaved();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const togglePreset = (id: string) => {
    setSelectedPresetIds((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id],
    );
  };

  const startEditPreset = (preset: VocabPreset) => {
    setEditingPresetId(preset.id);
    setPresetNameDraft(preset.name);
    setPresetPhrasesDraft(preset.phrases.join(', '));
  };

  const savePreset = async () => {
    if (!editingPresetId) return;
    const name = presetNameDraft.trim();
    if (!name) return;
    const phrases = Array.from(
      new Set(
        presetPhrasesDraft
          .split(/[,\n]/)
          .map((s) => s.trim())
          .filter(Boolean),
      ),
    );
    const next =
      editingPresetId === NEW_PRESET_DRAFT_ID
        ? [...presets, { id: `user-${Date.now()}`, name, phrases }]
        : presets.map((p) => (p.id === editingPresetId ? { ...p, name, phrases } : p));
    try {
      await persistVocabPresets(next);
      setPresets(next);
      setEditingPresetId(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const createPreset = () => {
    setEditingPresetId(NEW_PRESET_DRAFT_ID);
    setPresetNameDraft(t('vocab.presets.newPreset'));
    setPresetPhrasesDraft('');
  };

  /** 把一组模板的词条并入词典（已存在的按需启用）。返回失败条数。 */
  const applyPresets = async (selected: VocabPreset[]) => {
    const byPhrase = new Map<string, DictionaryEntry[]>();
    const addedPhrases = new Set<string>();
    for (const entry of entries) {
      const key = entry.phrase.trim().toLowerCase();
      if (!byPhrase.has(key)) byPhrase.set(key, []);
      byPhrase.get(key)?.push(entry);
    }
    let failures = 0;
    for (const p of selected) {
      for (const phrase of p.phrases) {
        const key = phrase.trim().toLowerCase();
        if (addedPhrases.has(key)) continue;
        const existing = byPhrase.get(key) || [];
        if (existing.length === 0) {
          try {
            const entry = await addVocab(phrase);
            addedPhrases.add(key);
            setEntries((prev) => [entry, ...prev]);
          } catch {
            failures += 1;
          }
          continue;
        }
        for (const item of existing) {
          if (!item.enabled) {
            try {
              await setVocabEnabled(item.id, true);
            } catch {
              failures += 1;
            }
          }
        }
      }
    }
    return failures;
  };

  const applySelectedPresets = async () => {
    const selected = presets.filter((p) => selectedPresetIds.includes(p.id));
    if (selected.length === 0) return;
    const failures = await applyPresets(selected);
    await refresh();
    if (failures > 0) {
      setError(`部分词条添加失败（${failures}）`);
    } else {
      flashSaved();
    }
  };

  const applyNewWordTemplates = async () => {
    const selected = presets.filter((p) => newWordTemplateIds.includes(p.id));
    if (selected.length === 0) return;
    const failures = await applyPresets(selected);
    setNewWordTemplateIds([]);
    setNewWordOpen(false);
    if (failures > 0) {
      setError(`部分词条添加失败（${failures}）`);
    } else {
      flashSaved();
    }
  };

  // 自动收集的单独一区。不给每个词条挂 badge —— 混在一堆里要逐个看；
  // 分段筛选一眼就看得完，「全部删除」也自然地只管自动这一块。
  // 用户随时能看清、能整块撤销，是自动收集能被信任的前提。
  const sourceOf = (entry: DictionaryEntry): Exclude<SourceFilter, 'all'> =>
    entry.note === LEARNED_NOTE ? 'auto' : 'manual';
  const learnedEntries = entries.filter((e) => sourceOf(e) === 'auto');

  /** 删除退场动画（从哪来回到哪去）：先淡出收缩，动画结束再真正删。 */
  const fadeOutCard = async (id: string) => {
    const element = cardRefs.current.get(id);
    if (!element) return;
    setRemovingIds((prev) => new Set(prev).add(id));
    try {
      await element.animate(
        [
          { opacity: 1, transform: 'scale(1)' },
          { opacity: 0, transform: 'scale(0.92)' },
        ],
        { duration: 140, easing: 'ease-out', fill: 'forwards' },
      ).finished;
    } catch {
      /* 动画被打断（筛选切换/卸载）不阻塞删除 */
    }
  };

  const onRemoveAllLearnedEntries = () => removeEntries(learnedEntries.map((entry) => entry.id));

  const needle = query.trim().toLowerCase();
  const visibleEntries = entries.filter(
    (e) =>
      (filter === 'all' || sourceOf(e) === filter) &&
      (!needle || e.phrase.toLowerCase().includes(needle)),
  );

  // FLIP：只量布局位置（offsetTop），rect 会被飞行中的动画 transform 污染。
  useLayoutEffect(() => {
    const nextTops = new Map<string, number>();
    cardRefs.current.forEach((element, id) => nextTops.set(id, element.offsetTop));
    cardRefs.current.forEach((element, id) => {
      const current = nextTops.get(id);
      if (current == null) return;
      const previous = prevCardTops.current.get(id);
      if (previous != null && Math.abs(previous - current) > 1) {
        element.animate(
          [{ transform: `translateY(${previous - current}px)` }, { transform: 'translateY(0)' }],
          { duration: 280, easing: 'cubic-bezier(0.16, 1, 0.3, 1)' },
        );
      }
    });
    prevCardTops.current = nextTops;
  }, [visibleEntries]);

  // ── 热词提取向导渲染（两步，照 GhostwriterSnippets 的结构）──
  if (extractView !== 'closed') {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
        <PageHeader
          kicker={t('vocab.kicker')}
          title={extractView === 'pick' ? t('vocab.extract.pickTitle') : t('vocab.extract.reviewTitle')}
          right={
            <Btn variant="ghost" icon="close" onClick={closeHotwordExtract}>
              {t('common.close')}
            </Btn>
          }
        />

        <SavedToast saveState={extractSaveState} message={extractMessage} />

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
              {extractView === 'pick'
                ? t('vocab.extract.pickHint')
                : t('vocab.extract.reviewHint')}
            </div>
            {extractView === 'pick' ? (
              <Pill tone="outline">
                {t('vocab.extract.selectedCount', { count: pickedIds.size })}
              </Pill>
            ) : (
              <label
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  gap: 6,
                  fontSize: 12,
                  color: 'var(--ol-ink-3)',
                }}
              >
                <input
                  type="checkbox"
                  checked={hotwordChecks.every(Boolean)}
                  onChange={toggleAllHotwordDrafts}
                />
                {t('vocab.extract.selectedCount', {
                  count: hotwordChecks.filter(Boolean).length,
                })}
              </label>
            )}
          </div>

          <div className="ol-thinscroll" style={{ overflow: 'auto', flex: '1 1 0', minHeight: 0 }}>
            {extractView === 'pick' ? (
              voiceRecords.length === 0 ? (
                <div
                  style={{
                    padding: 48,
                    textAlign: 'center',
                    fontSize: 13,
                    color: 'var(--ol-ink-3)',
                  }}
                >
                  {t('vocab.extract.pickEmpty')}
                </div>
              ) : (
                voiceRecords.map((record) => (
                  <label
                    key={record.id}
                    style={{
                      display: 'flex',
                      gap: 12,
                      alignItems: 'flex-start',
                      padding: '10px 18px',
                      borderBottom: '0.5px solid var(--ol-line)',
                      cursor: 'pointer',
                      // 已提取过的仅视觉淡化（与常用语向导同一样式），仍可多选。
                      opacity: record.extractedAt ? 0.45 : 1,
                    }}
                  >
                    <input
                      type="checkbox"
                      checked={pickedIds.has(record.id)}
                      onChange={() => toggleHotwordPick(record.id)}
                      style={{ marginTop: 3 }}
                    />
                    <span
                      style={{
                        flex: 1,
                        minWidth: 0,
                        fontSize: 13,
                        color: 'var(--ol-ink)',
                        whiteSpace: 'nowrap',
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                      }}
                    >
                      {record.rawTranscript.length > 80
                        ? `${record.rawTranscript.slice(0, 80)}…`
                        : record.rawTranscript}
                    </span>
                    {record.extractedAt ? (
                      <Pill tone="outline" size="sm">
                        {t('vocab.extract.extractedMark')}
                      </Pill>
                    ) : null}
                    <span style={{ flexShrink: 0, fontSize: 12, color: 'var(--ol-ink-4)' }}>
                      {extractRelativeAgeLabel(record.createdAt)}
                    </span>
                  </label>
                ))
              )
            ) : hotwordDrafts.length === 0 ? (
              <div
                style={{
                  padding: 48,
                  textAlign: 'center',
                  fontSize: 13,
                  color: 'var(--ol-ink-3)',
                }}
              >
                {t('vocab.extract.extractEmptyResult')}
              </div>
            ) : (
              hotwordDrafts.map((draft, index) => (
                <div
                  key={index}
                  style={{
                    display: 'flex',
                    gap: 12,
                    alignItems: 'flex-start',
                    padding: '12px 18px',
                    borderBottom: '0.5px solid var(--ol-line)',
                    borderLeft: failedHotwordIdx.has(index)
                      ? '3px solid var(--ol-red, #ef4444)'
                      : '3px solid transparent',
                  }}
                >
                  <input
                    type="checkbox"
                    checked={hotwordChecks[index] ?? false}
                    onChange={() => toggleHotwordCheck(index)}
                    style={{ marginTop: 4 }}
                  />
                  <div
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      gap: 6,
                      flex: 1,
                      minWidth: 0,
                    }}
                  >
                    {/* 原文错误写法只读展示（提取证据，不可改）。 */}
                    <span
                      style={{
                        fontSize: 13,
                        fontWeight: 600,
                        color: 'var(--ol-ink)',
                        whiteSpace: 'nowrap',
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                      }}
                    >
                      {draft.error}
                    </span>
                    <input
                      value={draft.hotword}
                      onChange={(event) =>
                        patchHotwordDraft(index, { hotword: event.target.value })
                      }
                      placeholder={t('vocab.extract.hotwordPlaceholder')}
                      style={{
                        padding: '6px 10px',
                        borderRadius: 8,
                        border: '0.5px solid var(--ol-line-strong)',
                        background: 'var(--ol-surface)',
                        color: 'var(--ol-ink)',
                        fontSize: 13,
                        fontFamily: 'inherit',
                      }}
                    />
                    {draft.example ? (
                      <span style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>
                        {draft.example}
                      </span>
                    ) : null}
                    {failedHotwordIdx.has(index) ? (
                      <span style={{ fontSize: 12, color: 'var(--ol-red, #ef4444)' }}>
                        {t('vocab.extract.saveFailedDuplicate')}
                      </span>
                    ) : null}
                  </div>
                </div>
              ))
            )}
          </div>

          <div
            style={{
              padding: '10px 18px',
              borderTop: '0.5px solid var(--ol-line)',
              flexShrink: 0,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              gap: 10,
            }}
          >
            <Btn
              variant="ghost"
              icon="chevLeft"
              onClick={
                extractView === 'review'
                  ? () => setExtractView('pick')
                  : closeHotwordExtract
              }
            >
              {extractView === 'review' ? t('vocab.extract.back') : t('common.close')}
            </Btn>
            {extractView === 'pick' ? (
              <Btn
                variant="primary"
                icon="sparkle"
                onClick={() => void startHotwordExtraction()}
                disabled={pickedIds.size === 0 || extractBusy}
              >
                {extractBusy
                  ? t('vocab.extract.extracting')
                  : t('vocab.extract.start')}
              </Btn>
            ) : (
              <Btn
                variant="primary"
                icon="check"
                onClick={() => void saveHotwordSelection()}
                disabled={savingExtract || !hotwordChecks.some(Boolean)}
              >
                {t('vocab.extract.save')}
              </Btn>
            )}
          </div>
        </Card>
      </div>
    );
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
      <PageHeader
        kicker={t('vocab.kicker')}
        title={t('vocab.title')}
        desc={t('vocab.desc')}
        right={
          <div style={{ display: 'flex', gap: 8 }}>
            {selectedIds.size > 0 && (
              <button
                type="button"
                className="ol-vocab-delete-selected"
                disabled={batchBusy}
                onClick={() => void removeEntries([...selectedIds])}
              >
                <Icon name="trash" size={15} />
                {t('vocab.deleteSelected', { count: selectedIds.size })}
              </button>
            )}
            <Btn
              variant="soft"
              icon="sparkle"
              onClick={() => void openHotwordExtract()}
              disabled={loading}
            >
              {t('vocab.extract.entry')}
            </Btn>
            <Btn
              variant="primary"
              icon="plus"
              onClick={() => {
                setNewWordOpen(true);
                setNewWordDraft('');
                setNewWordTemplateIds([]);
              }}
            >
              {t('vocab.newWord')}
            </Btn>
          </div>
        }
      />

      <SavedToast saveState={saveState} message={t('common.saved')} />

      {/* 工具行：来源分段筛选 + 圆形搜索（点击向左展开）。 */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 12,
          marginBottom: 14,
          flexWrap: 'wrap',
          flexShrink: 0,
        }}
      >
        <div className="ol-seg" role="tablist" aria-label={t('vocab.title')}>
          {(
            [
              { id: 'all', icon: null },
              { id: 'auto', icon: 'sparkle' },
              { id: 'manual', icon: 'feather' },
            ] as const
          ).map((seg) => (
            <button
              key={seg.id}
              type="button"
              role="tab"
              aria-selected={filter === seg.id}
              className={filter === seg.id ? 'ol-seg-item ol-seg-item-active' : 'ol-seg-item'}
              onClick={() => setFilter(seg.id)}
            >
              {seg.icon && <Icon name={seg.icon} size={13} />}
              {t(`vocab.filter.${seg.id}`)}
            </button>
          ))}
        </div>
        <label className="ol-vocab-select-all">
          <input
            type="checkbox"
            disabled={batchBusy || visibleEntries.length === 0}
            checked={
              visibleEntries.length > 0 &&
              visibleEntries.every((entry) => selectedIds.has(entry.id))
            }
            ref={(element) => {
              if (element)
                element.indeterminate =
                  visibleEntries.some((entry) => selectedIds.has(entry.id)) &&
                  !visibleEntries.every((entry) => selectedIds.has(entry.id));
            }}
            onChange={(event) => {
              const checked = event.target.checked;
              setSelectedIds((current) => {
                const next = new Set(current);
                visibleEntries.forEach((entry) =>
                  checked ? next.add(entry.id) : next.delete(entry.id),
                );
                return next;
              });
            }}
          />
          {selectedIds.size
            ? t('vocab.selectedCount', { count: selectedIds.size })
            : t('vocab.selectAllVisible')}
        </label>
        <div style={{ flex: 1 }} />
        {/* 圆形控件原地展开成搜索框 —— 放大镜固定在右缘不动，
            占位文字「搜索」在框内；收起走同一条 width 过渡（从哪来回到哪去）。 */}
        <div className={searchOpen ? 'ol-search ol-search-open' : 'ol-search'}>
          <input
            className="ol-search-field"
            type="text"
            value={query}
            placeholder={t('vocab.searchPlaceholder')}
            aria-label={t('vocab.searchPlaceholder')}
            tabIndex={searchOpen ? 0 : -1}
            onChange={(e) => setQuery(e.target.value)}
            onBlur={() => {
              if (!query) setSearchOpen(false);
            }}
            onKeyDown={(e) => {
              if (e.key === 'Escape') {
                setQuery('');
                setSearchOpen(false);
              }
            }}
          />
          <button
            type="button"
            className="ol-search-icon"
            aria-label={t('vocab.searchPlaceholder')}
            aria-expanded={searchOpen}
            // 点图标不让输入框失焦：否则 blur 收起与 click 切换竞态，第二次点击
            // 会先收起再被 toggle 重新展开，永远收不起来。
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              if (searchOpen && query) {
                setQuery('');
                return;
              }
              setSearchOpen((prev) => !prev);
              if (!searchOpen) {
                window.setTimeout(() => inputRefSearchFocus(), 60);
              } else {
                // 收起后输入框不可见，别让焦点留在里面。
                const active = document.activeElement;
                if (active instanceof HTMLElement && active.closest('.ol-search')) active.blur();
              }
            }}
          >
            <Icon name="search" size={15} />
          </button>
        </div>
      </div>

      {error && (
        <div
          role="alert"
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            gap: 10,
            padding: '8px 12px',
            marginBottom: 12,
            borderRadius: 10,
            border: '0.5px solid rgba(239,68,68,0.22)',
            background: 'rgba(239,68,68,0.07)',
            color: 'var(--ol-red, #ef4444)',
            fontSize: 12,
            lineHeight: 1.5,
          }}
        >
          <span>{error}</span>
        </div>
      )}

      {/* 自动添加筛选下给「全部删除」留一个稳定的出口（信任前提，见上注释）。 */}
      {filter === 'auto' && learnedEntries.length > 0 && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 10 }}>
          <span style={{ fontSize: 12, color: 'var(--ol-ink-3)' }}>
            {t('vocab.learnedSection', { count: learnedEntries.length })}
          </span>
          <Btn size="sm" onClick={() => void onRemoveAllLearnedEntries()}>
            {t('vocab.removeAllLearned')}
          </Btn>
        </div>
      )}

      {/* 独立滚动区：词条多时只有这一格在滚，底部面板固定在视口
          底缘、白底天然遮挡滚过去的内容；之前网格作为页根 flex item 被压扁、内容
          溢出到下方区块背后的穿帮也从根上消除。 */}
      <div
        className="ol-thinscroll"
        style={{ flex: 1, minHeight: 0, overflowY: 'auto', paddingRight: 2, paddingBottom: 12 }}
      >
        {/* 词条网格：hover 变灰 + 右侧浮现编辑/删除。 */}
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: mobile
              ? 'minmax(0, 1fr)'
              : 'repeat(auto-fill, minmax(230px, 1fr))',
            gap: 10,
            minHeight: 80,
            alignContent: 'start',
          }}
        >
          {loading && (
            <div style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>{t('common.loading')}</div>
          )}
          {!loading && !error && visibleEntries.length === 0 && (
            <div style={{ fontSize: 12, color: 'var(--ol-ink-4)', gridColumn: '1 / -1' }}>
              {needle ? t('vocab.searchEmpty') : t('vocab.empty')}
            </div>
          )}
          {visibleEntries.map((entry) => (
            <WordCard
              key={entry.id}
              entry={entry}
              auto={sourceOf(entry) === 'auto'}
              removing={removingIds.has(entry.id) || batchBusy}
              selected={selectedIds.has(entry.id)}
              onSelect={() => toggleSelection(entry.id)}
              cardRef={(element) => {
                if (element) cardRefs.current.set(entry.id, element);
                else cardRefs.current.delete(entry.id);
              }}
              onToggle={() => void onToggle(entry)}
              onEdit={() => openEdit(entry)}
            />
          ))}
        </div>
      </div>

      {/* 底部面板：快速添加行 + 提示 + 场景预设固定成一块。场景预设展开时面板
          整体变高、向上生长，输入行与面板顶缘的距离恒定。 */}
      <div
        style={{
          flexShrink: 0,
          paddingTop: 14,
          background: 'var(--ol-surface)',
          boxShadow: '0 -18px 22px -18px rgba(15,17,22,0.14)',
        }}
      >
        {/* 快速添加行（保留原输入即添加的顺手路径）。 */}
        <div style={{ display: 'flex', gap: 8 }}>
          <input
            ref={inputRef}
            placeholder={t('vocab.placeholder')}
            onKeyDown={onKeyDown}
            style={{
              flex: 1,
              height: 36,
              padding: '0 12px',
              border: '0.5px solid var(--ol-line-strong)',
              borderRadius: 999,
              fontSize: 13,
              fontFamily: 'inherit',
              outline: 'none',
              background: 'var(--ol-surface-2)',
              transition:
                'border-color 0.16s var(--ol-motion-quick), box-shadow 0.18s var(--ol-motion-soft), background 0.16s var(--ol-motion-quick)',
            }}
          />
          <Btn variant="primary" icon="plus" onClick={onAdd}>
            {t('common.add')}
          </Btn>
        </div>
        <div style={{ marginTop: 8, fontSize: 12, color: 'var(--ol-ink-4)' }}>{t('vocab.tip')}</div>

        {/* 场景预设：卡片区块（与「新词」弹窗共享同一份模板数据）。
          可展开，顶部给一条分隔线与输入区分开。 */}
        <div
          style={{
            marginTop: 12,
            paddingTop: 12,
            borderTop: '0.5px solid var(--ol-line)',
            paddingBottom: 8,
          }}
        >
          <Card padding={0}>
            <Collapsible embedded title={t('vocab.presets.title')} desc={t('vocab.presets.tip')}>
              <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
                {presets.map((p) => (
                  <button
                    key={p.id}
                    onClick={() => togglePreset(p.id)}
                    style={{
                      border: '0.5px solid var(--ol-line-strong)',
                      borderRadius: 999,
                      padding: '4px 10px',
                      fontSize: 12,
                      background: selectedPresetIds.includes(p.id)
                        ? 'var(--ol-blue-soft)'
                        : 'var(--ol-surface-2)',
                    }}
                  >
                    {p.name}
                  </button>
                ))}
                <Btn size="sm" variant="ghost" onClick={createPreset}>
                  {t('vocab.presets.create')}
                </Btn>
                <Btn size="sm" variant="primary" onClick={applySelectedPresets}>
                  {t('vocab.presets.apply')}
                </Btn>
              </div>
              {editingPresetId && (
                <div style={{ marginTop: 10, display: 'grid', gap: 8 }}>
                  <input
                    value={presetNameDraft}
                    onChange={(e) => setPresetNameDraft(e.target.value)}
                    placeholder={t('vocab.presets.namePlaceholder')}
                  />
                  <textarea
                    value={presetPhrasesDraft}
                    onChange={(e) => setPresetPhrasesDraft(e.target.value)}
                    placeholder={t('vocab.presets.wordsPlaceholder')}
                    rows={3}
                  />
                  <div style={{ display: 'flex', gap: 8 }}>
                    <Btn size="sm" variant="primary" onClick={() => void savePreset()}>
                      {t('vocab.presets.save')}
                    </Btn>
                    <Btn size="sm" variant="ghost" onClick={() => setEditingPresetId(null)}>
                      {t('common.cancel')}
                    </Btn>
                  </div>
                </div>
              )}
              {!editingPresetId && presets.length > 0 && (
                <div style={{ marginTop: 10, display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                  {presets.map((p) => (
                    <Btn
                      key={`${p.id}-edit`}
                      size="sm"
                      variant="ghost"
                      onClick={() => startEditPreset(p)}
                    >
                      {t('vocab.presets.edit', { name: p.name })}
                    </Btn>
                  ))}
                </div>
              )}
            </Collapsible>
          </Card>
        </div>
      </div>

      {/* 编辑词条弹窗 */}
      {editMount.mounted && (
        <ModalShell
          title={t('vocab.editTitle')}
          closing={editMount.closing}
          onClose={() => setEditingEntry(null)}
        >
          <input
            autoFocus
            value={editDraft}
            onChange={(e) => {
              setEditDraft(e.target.value);
              setEditError(null);
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                void saveEdit();
              }
            }}
            style={{
              width: '100%',
              boxSizing: 'border-box',
              height: 40,
              padding: '0 12px',
              border: '1.5px solid var(--ol-ink)',
              borderRadius: 10,
              fontSize: 14,
              fontFamily: 'inherit',
              outline: 'none',
              background: 'var(--ol-surface)',
              color: 'var(--ol-ink)',
            }}
          />
          {editError && (
            <div style={{ marginTop: 8, fontSize: 12, color: 'var(--ol-red, #ef4444)' }}>
              {editError}
            </div>
          )}
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 18 }}>
            <Btn variant="ghost" onClick={() => setEditingEntry(null)}>
              {t('common.cancel')}
            </Btn>
            <Btn variant="primary" onClick={() => void saveEdit()}>
              {t('vocab.editSave')}
            </Btn>
          </div>
        </ModalShell>
      )}

      {/* 新词弹窗：直接输入 + 预设模板多选 */}
      {newWordMount.mounted && (
        <ModalShell
          title={t('vocab.newWordTitle')}
          desc={t('vocab.newWordDesc')}
          closing={newWordMount.closing}
          onClose={() => setNewWordOpen(false)}
        >
          <div style={{ display: 'flex', gap: 8 }}>
            <input
              autoFocus
              value={newWordDraft}
              onChange={(e) => setNewWordDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  void addNewWord();
                }
              }}
              placeholder={t('vocab.newWordInputPlaceholder')}
              style={{
                flex: 1,
                minWidth: 0,
                height: 38,
                padding: '0 12px',
                border: '0.5px solid var(--ol-line-strong)',
                borderRadius: 999,
                fontSize: 13.5,
                fontFamily: 'inherit',
                outline: 'none',
                background: 'var(--ol-surface-2)',
                color: 'var(--ol-ink)',
              }}
            />
            <Btn variant="primary" icon="plus" onClick={() => void addNewWord()}>
              {t('common.add')}
            </Btn>
          </div>
          <div style={{ marginTop: 16, fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink-2)' }}>
            {t('vocab.newWordTemplates')}
          </div>
          <div style={{ marginTop: 8, display: 'grid', gap: 8 }}>
            {presets.map((p) => {
              const checked = newWordTemplateIds.includes(p.id);
              return (
                <button
                  key={p.id}
                  type="button"
                  aria-pressed={checked}
                  onClick={() =>
                    setNewWordTemplateIds((prev) =>
                      prev.includes(p.id) ? prev.filter((x) => x !== p.id) : [...prev, p.id],
                    )
                  }
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 10,
                    textAlign: 'left',
                    padding: '10px 12px',
                    borderRadius: 10,
                    fontFamily: 'inherit',
                    border: checked
                      ? '1px solid var(--ol-blue)'
                      : '0.5px solid var(--ol-line-strong)',
                    background: checked ? 'var(--ol-blue-soft)' : 'var(--ol-surface)',
                    cursor: 'default',
                    transition:
                      'background 0.14s var(--ol-motion-quick), border-color 0.14s var(--ol-motion-quick)',
                  }}
                >
                  <span style={{ minWidth: 0, flex: 1 }}>
                    <span
                      style={{
                        display: 'block',
                        fontSize: 13,
                        fontWeight: 600,
                        color: 'var(--ol-ink)',
                      }}
                    >
                      {p.name}
                    </span>
                    <span
                      style={{
                        display: 'block',
                        marginTop: 2,
                        fontSize: 11.5,
                        color: 'var(--ol-ink-4)',
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {p.phrases.join(' · ')}
                    </span>
                  </span>
                  <span style={{ flexShrink: 0, fontSize: 11.5, color: 'var(--ol-ink-4)' }}>
                    {t('vocab.newWordTemplateCount', { count: p.phrases.length })}
                  </span>
                  {checked && <Icon name="check" size={14} />}
                </button>
              );
            })}
          </div>
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 18 }}>
            <Btn variant="ghost" onClick={() => setNewWordOpen(false)}>
              {t('common.close')}
            </Btn>
            <Btn
              variant="primary"
              onClick={() => void applyNewWordTemplates()}
              disabled={newWordTemplateIds.length === 0}
            >
              {t('vocab.newWordAddSelected')}
            </Btn>
          </div>
        </ModalShell>
      )}

      <style>{`
        @keyframes ol-chip-in {
          from { opacity: 0; transform: scale(.92); filter: blur(5px); }
          to   { opacity: 1; transform: scale(1); filter: blur(0); }
        }
      `}</style>
    </div>
  );
}

function inputRefSearchFocus() {
  const el = document.querySelector<HTMLInputElement>('.ol-search-field');
  el?.focus();
}

interface WordCardProps {
  entry: DictionaryEntry;
  auto: boolean;
  /** 删除退场动画进行中：屏蔽交互，避免重复点击。 */
  removing: boolean;
  cardRef: (element: HTMLDivElement | null) => void;
  onToggle: () => void;
  onEdit: () => void;
  selected: boolean;
  onSelect: () => void;
}

/** 词条卡片：默认只显图标+文字+命中数；hover/focus-within 变灰并浮现编辑/删除。 */
function WordCard({
  entry,
  auto,
  removing,
  selected,
  cardRef,
  onToggle,
  onEdit,
  onSelect,
}: WordCardProps) {
  const { t } = useTranslation();
  const enabled = entry.enabled;
  return (
    <div
      ref={cardRef}
      className="ol-word-card"
      data-disabled={enabled ? undefined : 'true'}
      data-selected={selected ? 'true' : undefined}
      style={removing ? { pointerEvents: 'none' } : undefined}
    >
      <span className="ol-word-card-icon" aria-hidden>
        <Icon name={auto ? 'sparkle' : 'feather'} size={14} />
      </span>
      <button
        type="button"
        className="ol-word-card-text"
        onClick={onToggle}
        title={enabled ? t('vocab.tipDisabled') : t('vocab.tipEnabled')}
      >
        {entry.phrase}
      </button>
      <span className="ol-word-card-hits">{entry.hits}</span>
      <span className="ol-word-card-actions">
        <Tooltip content={t('vocab.edit')} placement="top">
          <button
            type="button"
            className="ol-word-card-action"
            aria-label={t('vocab.edit')}
            onClick={onEdit}
          >
            <Icon name="pencil" size={14} />
          </button>
        </Tooltip>
      </span>
      <input
        type="checkbox"
        className="ol-word-card-select"
        checked={selected}
        disabled={removing}
        aria-label={t('vocab.selectWord', { phrase: entry.phrase })}
        onChange={onSelect}
      />
    </div>
  );
}

interface ModalShellProps {
  title: string;
  desc?: string;
  /** true 时反向播放入场动画（退出），配合 useExitMount 实现「从哪来回到哪去」。 */
  closing?: boolean;
  onClose: () => void;
  children: React.ReactNode;
}

/** 页面级小弹窗：backdrop 淡入 + 卡片 spring 弹出，Esc/点遮罩关闭。 */
function ModalShell({ title, desc, closing = false, onClose, children }: ModalShellProps) {
  const { t } = useTranslation();
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [onClose]);
  return (
    <div
      onClick={onClose}
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 80,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 24,
        background: 'rgba(15,17,22,0.28)',
        backdropFilter: 'blur(6px) saturate(140%)',
        WebkitBackdropFilter: 'blur(6px) saturate(140%)',
        animation: closing
          ? 'ol-prompt-fade 0.2s var(--ol-motion-soft) reverse both'
          : 'ol-prompt-fade 0.2s var(--ol-motion-soft)',
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 440,
          maxWidth: '100%',
          borderRadius: 16,
          background: 'var(--ol-surface)',
          border: '0.5px solid rgba(0,0,0,.08)',
          boxShadow: '0 24px 70px -24px rgba(15,17,22,.38), 0 0 0 0.5px rgba(0,0,0,.06)',
          padding: 20,
          animation: closing
            ? 'ol-prompt-pop 0.2s var(--ol-motion-soft) reverse both'
            : 'ol-prompt-pop 0.26s var(--ol-motion-spring)',
        }}
      >
        <div
          style={{
            display: 'flex',
            alignItems: 'flex-start',
            justifyContent: 'space-between',
            gap: 12,
            marginBottom: desc ? 4 : 14,
          }}
        >
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 15, fontWeight: 650, color: 'var(--ol-ink)' }}>{title}</div>
            {desc && (
              <div
                style={{ marginTop: 4, fontSize: 12.5, color: 'var(--ol-ink-3)', lineHeight: 1.5 }}
              >
                {desc}
              </div>
            )}
          </div>
          <button
            type="button"
            aria-label={t('common.close')}
            onClick={onClose}
            style={{
              width: 26,
              height: 26,
              flexShrink: 0,
              border: 0,
              borderRadius: 8,
              background: 'transparent',
              color: 'var(--ol-ink-4)',
              cursor: 'default',
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}
          >
            <Icon name="close" size={14} />
          </button>
        </div>
        <div style={{ marginTop: 12 }}>{children}</div>
      </div>
    </div>
  );
}
