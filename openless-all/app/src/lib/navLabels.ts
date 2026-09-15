import type { AppTab } from '../state/useAppState';

/** 分组子项标题的 i18n key：style → nav.polishMode，其余 nav.<id>。 */
export function subItemLabelKey(id: AppTab): string {
  if (id === 'style') return 'nav.polishMode';
  return `nav.${id}`;
}

/** Task 10 临时硬编码标签（Task 11 i18n 收编为 nav.* key 后删除）。 */
export const PENDING_I18N_NAV_LABELS: Partial<Record<AppTab, string>> = {
  fluidSnippets: '常用语',
};
