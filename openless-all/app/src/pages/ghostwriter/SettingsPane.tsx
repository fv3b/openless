// SettingsPane.tsx — 「设置」页签：Ghostwriter 卡片（候选/推荐开关＋背景落点
// radio＋两枚节流间隔输入）与「对话」卡片（总开关/热键/回话时机/追问深度/
// 推荐显示）各占一张，两张独立卡片纵向排列（同设置弹窗卡片间距惯例）。
// 输入越界（500–10000 外）在失焦时红字提示并回弹上次合法值；合法即保存、立即生效。

import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ShortcutRecorder } from '../../components/ShortcutRecorder';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import type { GhostwriterPreferences, GhostwriterProbeDepth, GhostwriterReplyTiming } from '../../lib/types';
import { THROTTLE_MAX_MS, THROTTLE_MIN_MS, parseThrottleMs } from '../../lib/ghostwriterThrottle';
import {
  hasConversationHotkey,
  isConversationModifierPrimaryAllowed,
  isModifierOnlyPrimary,
  parseConversationHotkey,
  serializeConversationHotkey,
} from '../../lib/ghostwriterConversation';
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
    <div
      className="ol-thinscroll"
      style={{ display: 'flex', flexDirection: 'column', gap: 16, flex: '1 1 0', minHeight: 0, overflow: 'auto' }}
    >
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
      <Card>
        <ConversationSection
          ghostwriter={prefs.ghostwriter}
          onSave={saveGhostwriter}
        />
      </Card>
    </div>
  );
}

/** 「对话」卡片：对话模式总开关＋一键两用热键＋回话时机/追问深度/推荐显示。
 *  热键录制仅总开关开启后可用；热键序列化成小写串存 ghostwriter.conversationHotkey，
 *  Tauri 宿主按同一契约解析注册全局键（见 lib/ghostwriterConversation.ts）。 */
function ConversationSection({
  ghostwriter,
  onSave,
}: {
  ghostwriter: GhostwriterPreferences;
  onSave: (patch: Partial<GhostwriterPreferences>) => Promise<void> | void;
}) {
  const { t } = useTranslation();

  const saveHotkey = async (binding: { primary: string; modifiers: string[] } | null) => {
    // macOS 对话热键允许单修饰键 primary（挂主监听器 modifier-only 槽位，短按触发）；
    // 其余单修饰键 / 非 macOS 照旧拒绝。
    if (
      binding &&
      isModifierOnlyPrimary(binding.primary) &&
      !isConversationModifierPrimaryAllowed(binding.primary)
    ) {
      throw new Error(`hotkeyModifierOnly:${t('ghostwriter.conversation.hotkeyModifierOnly')}`);
    }
    await onSave({
      conversationHotkey: binding ? serializeConversationHotkey(binding) : null,
    });
  };

  return (
    <>
      <SectionTitle>{t('ghostwriter.conversation.title')}</SectionTitle>
      <SettingRow
        label={t('ghostwriter.conversation.enable')}
        desc={t('ghostwriter.conversation.enableDesc')}
      >
        <Toggle
          on={ghostwriter.conversationEnabled}
          onToggle={(next) => void onSave({ conversationEnabled: next })}
        />
      </SettingRow>
      <SettingRow
        label={t('ghostwriter.conversation.hotkey')}
        desc={t('ghostwriter.conversation.hotkeyDesc')}
      >
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, width: '100%' }}>
          <ShortcutRecorder
            value={parseConversationHotkey(ghostwriter.conversationHotkey)}
            comboOnly
            allowBareModifierOnMac
            disabled={!ghostwriter.conversationEnabled}
            onSave={saveHotkey}
            onDisable={() => saveHotkey(null)}
          />
          {ghostwriter.conversationEnabled && !hasConversationHotkey(ghostwriter.conversationHotkey) ? (
            <div style={{ fontSize: 11, color: 'var(--ol-ink-4)' }}>
              {t('ghostwriter.conversation.hotkeyUnsetHint')}
            </div>
          ) : null}
        </div>
      </SettingRow>
      <SettingRow
        label={t('ghostwriter.conversation.timing')}
        desc={t('ghostwriter.conversation.timingDesc')}
      >
        <RadioGroup
          name="ghostwriter-conversation-timing"
          value={ghostwriter.conversationReplyTiming}
          options={[
            ['pause', 'ghostwriter.conversation.timingPause'],
            ['explicit', 'ghostwriter.conversation.timingExplicit'],
          ]}
          onChange={(value) => void onSave({ conversationReplyTiming: value as GhostwriterReplyTiming })}
        />
      </SettingRow>
      <SettingRow
        label={t('ghostwriter.conversation.depth')}
        desc={t('ghostwriter.conversation.depthDesc')}
      >
        <RadioGroup
          name="ghostwriter-conversation-depth"
          value={ghostwriter.conversationProbeDepth}
          options={[
            ['single', 'ghostwriter.conversation.depthSingle'],
            ['untilClear', 'ghostwriter.conversation.depthUntilClear'],
            ['echo', 'ghostwriter.conversation.depthEcho'],
          ]}
          onChange={(value) => void onSave({ conversationProbeDepth: value as GhostwriterProbeDepth })}
        />
      </SettingRow>
      <SettingRow
        label={t('ghostwriter.conversation.recommendations')}
        desc={t('ghostwriter.conversation.recommendationsDesc')}
      >
        <Toggle
          on={ghostwriter.conversationRecommendations}
          onToggle={(next) => void onSave({ conversationRecommendations: next })}
        />
      </SettingRow>
    </>
  );
}

function RadioGroup({
  name,
  value,
  options,
  onChange,
}: {
  name: string;
  value: string;
  options: Array<[string, string]>;
  onChange: (value: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 14, flexWrap: 'wrap' }}>
      {options.map(([optionValue, labelKey]) => (
        <label
          key={optionValue}
          style={{ display: 'flex', alignItems: 'center', gap: 6, cursor: 'default' }}
        >
          <input
            type="radio"
            name={name}
            checked={value === optionValue}
            onChange={() => onChange(optionValue)}
          />
          <span style={{ fontSize: 12.5, color: 'var(--ol-ink)' }}>{t(labelKey)}</span>
        </label>
      ))}
    </div>
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
