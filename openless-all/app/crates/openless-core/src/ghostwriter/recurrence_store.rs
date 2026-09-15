//! 重复档（recurrence file）：跨会话累计的可复用说法底账。
//!
//! 持久化照抄 snippet_store 模式：Mutex 内存态 + 每次变更后整文件原子写，
//! 落 `data_dir/ghostwriter-sediment.json`；文件缺失按空库处理；文件损坏时
//! 先把原文件改名备份（.corrupt-<uuid4>）再回空库，避免下次变更覆写丢失。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::errors::{BackendError, BackendErrorCode};
use crate::ghostwriter::assist::RecurrenceSummary;
use crate::persistence::{atomic_write, persistence_error, read_or_default};

/// 重复档总量上限：超限淘汰 count 最小、最旧（插入序靠前）的条目。
const MAX_ENTRIES: usize = 50;

/// 重复档一条：说法（归一化后的统一键）、累计次数、最近例句、是否已提示过。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecurrenceEntry {
    pub phrase: String,
    pub count: u32,
    pub last_example: String,
    pub prompted: bool,
}

pub struct RecurrenceStore {
    path: Option<PathBuf>,
    state: Mutex<Vec<RecurrenceEntry>>,
}

impl RecurrenceStore {
    /// 打开数据目录下的 ghostwriter-sediment.json；文件缺失按空库处理；
    /// 文件损坏时先把原文件改名备份（.corrupt-<uuid4>）再回空库。
    pub fn at_data_dir(dir: &std::path::Path) -> Self {
        let path = dir.join("ghostwriter-sediment.json");
        let entries = match read_or_default::<Vec<RecurrenceEntry>>(&path) {
            Ok(entries) => entries,
            Err(_) => backup_corrupt_file(&path),
        };
        Self {
            path: Some(path),
            state: Mutex::new(entries),
        }
    }

    pub fn in_memory() -> Self {
        Self {
            path: None,
            state: Mutex::new(Vec::new()),
        }
    }

    /// 收下一次沉淀抽取的说法：(说法, 例句)。说法归一化（trim＋连续空白折叠）
    /// 后合并：已存在 → count+=1、更新最近例句；新说法 → 追加到末尾。总量超
    /// 50 时淘汰 count 最小、最旧（插入序靠前）的条目。锁损坏或落盘失败仅记
    /// 日志（变更留在内存，下次变更重试落盘）。
    pub fn apply_extraction(&self, phrases: Vec<(String, String)>) {
        let Ok(mut entries) = self.state.lock() else {
            log::warn!("[ghostwriter] recurrence store lock poisoned, extraction dropped");
            return;
        };
        for (phrase, example) in phrases {
            let phrase = normalize_phrase(&phrase);
            if phrase.is_empty() {
                continue;
            }
            if let Some(existing) = entries.iter_mut().find(|entry| entry.phrase == phrase) {
                existing.count += 1;
                existing.last_example = example;
            } else {
                entries.push(RecurrenceEntry {
                    phrase,
                    count: 1,
                    last_example: example,
                    prompted: false,
                });
            }
        }
        while entries.len() > MAX_ENTRIES {
            let victim = entries
                .iter()
                .enumerate()
                .min_by_key(|(index, entry)| (entry.count, *index))
                .map(|(index, _)| index);
            if let Some(index) = victim {
                entries.remove(index);
            }
        }
        self.persist_locked(&entries);
    }

    /// 未提示过的说法摘要（给实时助手注入重复档用），按 count 降序、至多
    /// limit 条；count 相同按插入序（稳定排序）。
    pub fn summary(&self, limit: usize) -> Vec<RecurrenceSummary> {
        self.lock()
            .map(|entries| {
                let mut pending: Vec<&RecurrenceEntry> =
                    entries.iter().filter(|entry| !entry.prompted).collect();
                pending.sort_by_key(|entry| std::cmp::Reverse(entry.count));
                pending
                    .into_iter()
                    .take(limit)
                    .map(|entry| RecurrenceSummary {
                        phrase: entry.phrase.clone(),
                        count: entry.count,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 标记某条说法已提示过（本次会话不再重复提醒）；没有该条时 no-op。
    pub fn mark_prompted(&self, phrase: &str) {
        let phrase = normalize_phrase(phrase);
        self.mutate_locked(move |entries| {
            if let Some(entry) = entries.iter_mut().find(|entry| entry.phrase == phrase) {
                entry.prompted = true;
            }
        });
    }

    /// 从重复档移除某条说法（确认存为常用语后不再提）；没有该条时 no-op。
    pub fn mark_saved(&self, phrase: &str) {
        let phrase = normalize_phrase(phrase);
        self.mutate_locked(move |entries| entries.retain(|entry| entry.phrase != phrase));
    }

    /// 全量条目快照（插入序），测试/调试用。
    pub fn pending_matches(&self) -> Vec<RecurrenceEntry> {
        self.lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Vec<RecurrenceEntry>>, BackendError> {
        self.state
            .lock()
            .map_err(|_| BackendError::new(BackendErrorCode::Internal, "recurrence store lock poisoned"))
    }

    /// 变更后落盘的统一入口：锁损坏丢弃本次变更（log::warn），落盘失败保留
    /// 内存态、下次变更重试。
    fn mutate_locked<F: FnOnce(&mut Vec<RecurrenceEntry>)>(&self, mutate: F) {
        let Ok(mut entries) = self.state.lock() else {
            log::warn!("[ghostwriter] recurrence store lock poisoned, change dropped");
            return;
        };
        mutate(&mut entries);
        self.persist_locked(&entries);
    }

    fn persist_locked(&self, entries: &[RecurrenceEntry]) {
        if let Some(path) = &self.path {
            if let Err(error) = write_entries(path, entries) {
                log::warn!("[ghostwriter] failed to persist recurrence store: {error}");
            }
        }
    }
}

/// 归一化：trim＋连续空白折叠为单个空格（split_whitespace 恰好两者兼得），
/// 作为合并匹配与存储的统一键。
fn normalize_phrase(phrase: &str) -> String {
    phrase.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn write_entries(path: &Path, entries: &[RecurrenceEntry]) -> Result<(), BackendError> {
    let bytes = serde_json::to_vec_pretty(entries).map_err(|_| persistence_error("encode recurrence entries"))?;
    atomic_write(path, &bytes)
}

/// 解码失败时把损坏文件改名备份到 .corrupt-<uuid4> 后回空库；
/// 改名失败则删除原文件兜底（保住可手工恢复的备份优先）。
fn backup_corrupt_file(path: &Path) -> Vec<RecurrenceEntry> {
    let backup = path.with_file_name(format!(
        "{}.corrupt-{}",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        uuid::Uuid::new_v4().simple()
    ));
    if std::fs::rename(path, &backup).is_err() {
        let _ = std::fs::remove_file(path);
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(store: &RecurrenceStore, phrase: &str, example: &str) {
        store.apply_extraction(vec![(phrase.to_string(), example.to_string())]);
    }

    #[test]
    fn extraction_merges_normalized_and_counts() {
        // 「把  日志\n清一下 」与「把 日志 清一下」归一化后是同一条：合并、
        // count+=1、最近例句更新；不同说法正常追加。
        let store = RecurrenceStore::in_memory();
        apply(&store, "  把  日志\n清一下  ", "例句一");
        apply(&store, "把 日志 清一下", "例句二");
        let entries = store.pending_matches();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].phrase, "把 日志 清一下");
        assert_eq!(entries[0].count, 2);
        assert_eq!(entries[0].last_example, "例句二");
        assert!(!entries[0].prompted);
        apply(&store, "以后都用测试环境跑", "例句三");
        let entries = store.pending_matches();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].phrase, "以后都用测试环境跑");
        assert_eq!(entries[1].count, 1);
    }

    #[test]
    fn summary_excludes_prompted_and_sorts_by_count() {
        // 三条 count 1/2/3；提示过丙后 summary 只剩乙、甲，按 count 降序；
        // limit 生效（1 条取最高的乙，0 条为空）。
        let store = RecurrenceStore::in_memory();
        apply(&store, "说法甲", "例句甲");
        apply(&store, "说法乙", "例句乙");
        apply(&store, "说法乙", "例句乙二");
        apply(&store, "说法丙", "例句丙");
        apply(&store, "说法丙", "例句丙二");
        apply(&store, "说法丙", "例句丙三");
        store.mark_prompted("说法丙");
        assert_eq!(
            store.summary(20),
            vec![
                RecurrenceSummary {
                    phrase: "说法乙".to_string(),
                    count: 2,
                },
                RecurrenceSummary {
                    phrase: "说法甲".to_string(),
                    count: 1,
                },
            ]
        );
        assert_eq!(
            store.summary(1),
            vec![RecurrenceSummary {
                phrase: "说法乙".to_string(),
                count: 2,
            }]
        );
        assert!(store.summary(0).is_empty());
        assert!(store.pending_matches()[2].prompted);
    }

    #[test]
    fn mark_saved_removes() {
        // mark_saved 后条目从重复档消失；不存在的说法是 no-op。
        let store = RecurrenceStore::in_memory();
        apply(&store, "说法甲", "例句甲");
        apply(&store, "说法乙", "例句乙");
        store.mark_saved(" 说法乙 ");
        let entries = store.pending_matches();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].phrase, "说法甲");
        store.mark_saved("不存在的说法");
        assert_eq!(store.pending_matches().len(), 1);
        store.mark_saved("说法甲");
        assert!(store.pending_matches().is_empty());
    }

    #[test]
    fn cap_fifty_evicts_lowest_count() {
        // 51 条时淘汰 count 最小、最旧（插入序靠前）的条目：50 个 count-1
        // 说法里最先插入的「说法01」出局；把「说法02」升到 count 2 后，再来
        // 新说法淘汰的是下一个 count-1 最旧的「说法03」，高频老条目存活。
        let store = RecurrenceStore::in_memory();
        for i in 1..=50 {
            apply(&store, &format!("说法{i:02}"), "例句");
        }
        assert_eq!(store.pending_matches().len(), 50);
        apply(&store, "说法甲", "例句甲");
        let entries = store.pending_matches();
        assert_eq!(entries.len(), 50);
        assert!(entries.iter().all(|entry| entry.phrase != "说法01"));
        assert_eq!(entries[0].phrase, "说法02");

        apply(&store, "说法02", "例句二");
        apply(&store, "说法乙", "例句乙");
        let entries = store.pending_matches();
        assert_eq!(entries.len(), 50);
        assert!(entries
            .iter()
            .any(|entry| entry.phrase == "说法02" && entry.count == 2));
        assert!(entries.iter().all(|entry| entry.phrase != "说法03"));
        assert!(entries.iter().any(|entry| entry.phrase == "说法乙"));
    }

    #[test]
    fn at_data_dir_roundtrip() {
        // temp dir：at_data_dir 落盘 ghostwriter-sediment.json，重开读回全部
        // 条目（含 prompted 标记与归一化合并结果），插入序不变。
        let dir = std::env::temp_dir().join(format!(
            "openless-core-ghostwriter-recurrence-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = RecurrenceStore::at_data_dir(&dir);
        apply(&store, "  跨会话   说法  ", "例句一");
        apply(&store, "跨会话 说法", "例句二");
        apply(&store, "另一条", "例句三");
        store.mark_prompted("另一条");
        let path = dir.join("ghostwriter-sediment.json");
        assert!(path.exists());
        let reopened = RecurrenceStore::at_data_dir(&dir);
        assert_eq!(reopened.pending_matches(), store.pending_matches());
        assert_eq!(reopened.summary(20), store.summary(20));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
