// SettingsPane.tsx — 「设置」页签：候选/推荐开关＋背景落点 radio＋两枚节流间隔输入。
// 输入越界（500–10000 外）在失焦时红字提示并回弹上次合法值；合法即保存、立即生效。

import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import type { GhostwriterPreferences } from '../../lib/types';
import { THROTTLE_MAX_MS, THROTTLE_MIN_MS, parseThrottleMs } from '../../lib/ghostwriterThrottle';
import { Card } from '../_atoms';
import { SectionTitle, SettingRow, Toggle, inputStyle } from '../settings/shared';

export function SettingsPane() {
  const { t } = useTranslation();
  const { prefs, updatePrefs } = useHotkeySettings();

  if (!prefs) {
    return (
      <Card>
        <div style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>{t('common.loading')}</div>
      </Card>
    );
  }

  const saveGhostwriter = (patch: Partial<GhostwriterPreferences>) =>
    updatePrefs((current) => ({
      ...current,
      ghostwriter: { ...current.ghostwriter, ...patch },
    }));

  return (
    <Card>
      <SectionTitle>{t('nav.ghostwriter')}</SectionTitle>
      <SettingRow
        label={t('settings.ghostwriter.ghostwriterCandidate')}
        desc={t('settings.ghostwriter.ghostwriterCandidateDesc')}
      >
        <Toggle
          on={prefs.ghostwriter.candidatesEnabled}
          onToggle={(next) => void saveGhostwriter({ candidatesEnabled: next })}
        />
      </SettingRow>
      <SettingRow
        label={t('settings.ghostwriter.ghostwriterRecommendation')}
        desc={t('settings.ghostwriter.ghostwriterRecommendationDesc')}
      >
        <Toggle
          on={prefs.ghostwriter.recommendationsEnabled}
          onToggle={(next) => void saveGhostwriter({ recommendationsEnabled: next })}
        />
      </SettingRow>
      <SettingRow
        label={t('settings.ghostwriter.backgroundPlacement')}
        desc={t('settings.ghostwriter.backgroundPlacementDesc')}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 14, flexWrap: 'wrap' }}>
          {(
            [
              ['head', 'settings.ghostwriter.backgroundPlacementHead'],
              ['tail', 'settings.ghostwriter.backgroundPlacementTail'],
            ] as const
          ).map(([value, labelKey]) => (
            <label
              key={value}
              style={{ display: 'flex', alignItems: 'center', gap: 6, cursor: 'default' }}
            >
              <input
                type="radio"
                name="ghostwriter-background-placement"
                checked={prefs.ghostwriter.backgroundPlacement === value}
                onChange={() => void saveGhostwriter({ backgroundPlacement: value })}
              />
              <span style={{ fontSize: 12.5, color: 'var(--ol-ink)' }}>
                {t(labelKey)}
              </span>
            </label>
          ))}
        </div>
      </SettingRow>
      <ThrottleRow
        label={t('ghostwriter.settingsPane.throttleCandidate')}
        current={prefs.ghostwriter.candidateThrottleMs}
        onSave={(value) => void saveGhostwriter({ candidateThrottleMs: value })}
      />
      <ThrottleRow
        label={t('ghostwriter.settingsPane.throttleRecommendation')}
        current={prefs.ghostwriter.recommendationThrottleMs}
        onSave={(value) => void saveGhostwriter({ recommendationThrottleMs: value })}
      />
    </Card>
  );
}

function ThrottleRow({
  label,
  current,
  onSave,
}: {
  label: string;
  current: number;
  onSave: (value: number) => void;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState(String(current));
  const [invalid, setInvalid] = useState(false);

  // current 是合法值的唯一真相：保存成功、外部变更、保存失败回滚后都回到输入框。
  useEffect(() => {
    setDraft(String(current));
    setInvalid(false);
  }, [current]);

  const commit = (raw: string) => {
    const parsed = parseThrottleMs(raw);
    if (parsed === null) {
      setInvalid(true);
      setDraft(String(current));
      return;
    }
    setInvalid(false);
    setDraft(String(parsed));
    if (parsed !== current) onSave(parsed);
  };

  return (
    <SettingRow label={label}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
        <input
          type="number"
          inputMode="numeric"
          min={THROTTLE_MIN_MS}
          max={THROTTLE_MAX_MS}
          step={100}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={(event) => commit(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') event.currentTarget.blur();
          }}
          style={{ ...inputStyle, flex: '0 0 auto', width: 120, maxWidth: 120 }}
        />
        <span
          style={{
            fontSize: 11.5,
            lineHeight: 1.5,
            color: invalid ? 'var(--ol-err)' : 'var(--ol-ink-4)',
          }}
        >
          {t('ghostwriter.settingsPane.throttleRangeHint')}
        </span>
      </div>
    </SettingRow>
  );
}
