// GhostwriterView.tsx — Ghostwriter 主视图：一页三页签（常用语／任务书／设置）。
// 页签状态由 Shell 持有（切走再回来保持原页签）；设置页入口经 ghostwriter:open-tab
// 事件通知 Shell 切页，并把目标页签经 props 传进来。

import { useTranslation } from 'react-i18next';
import type { GhostwriterTab } from '../lib/ghostwriterTabs';
import { useSavedToastListener } from '../lib/savedEvent';
import { SavedToast } from '../components/SavedToast';
import { PageHeader } from './_atoms';
import { GhostwriterSnippets } from './GhostwriterSnippets';
import { SettingsPane } from './ghostwriter/SettingsPane';
import { TaskBriefsPane } from './ghostwriter/TaskBriefsPane';

const TAB_ORDER: GhostwriterTab[] = ['snippets', 'briefs', 'settings'];

const TAB_LABEL_KEYS: Record<GhostwriterTab, string> = {
  snippets: 'ghostwriter.view.tabSnippets',
  briefs: 'ghostwriter.view.tabBriefs',
  settings: 'ghostwriter.view.tabSettings',
};

interface GhostwriterViewProps {
  tab?: GhostwriterTab;
  onTabChange?: (tab: GhostwriterTab) => void;
}

export function GhostwriterView({ tab = 'snippets', onTabChange }: GhostwriterViewProps) {
  const { t } = useTranslation();
  const savedToast = useSavedToastListener();

  return (
    <div style={{ display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
      <PageHeader
        kicker={t('nav.ghostwriter')}
        title={t(TAB_LABEL_KEYS[tab])}
        right={
          // 页签用项目分段控件惯例（.ol-seg，同词典页）。
          <div className="ol-seg" role="tablist" aria-label={t('ghostwriter.view.title')}>
            {TAB_ORDER.map((id) => (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={tab === id}
                className={tab === id ? 'ol-seg-item ol-seg-item-active' : 'ol-seg-item'}
                onClick={() => onTabChange?.(id)}
              >
                {t(TAB_LABEL_KEYS[id])}
              </button>
            ))}
          </div>
        }
      />
      <SavedToast saveState={savedToast.state} message={savedToast.message} />
      {tab === 'snippets' ? (
        <GhostwriterSnippets embedded />
      ) : tab === 'briefs' ? (
        <TaskBriefsPane />
      ) : (
        <SettingsPane />
      )}
    </div>
  );
}
