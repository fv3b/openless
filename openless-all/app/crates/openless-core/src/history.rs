//! Newest-first dictation history with retention and count caps.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::errors::{BackendError, BackendErrorCode};
use crate::persistence::{atomic_write, persistence_error, read_or_default};
use crate::types::DictationSession;

pub const HISTORY_CAP: usize = 200;

pub struct HistoryStore {
    path: PathBuf,
    /// `<data_dir>/recordings/`（仅 `at_data_dir` 构造时提供；`at_path` 的测试实例
    /// 不碰文件系统）。历史条目被单条删除、清空或因保留策略挤出时，同名
    /// `<session_id>.wav` 一并删除——录音文件生命周期与历史条目绑定。
    recordings_dir: Option<PathBuf>,
    lock: Mutex<()>,
}

impl HistoryStore {
    pub fn at_data_dir(data_dir: impl AsRef<Path>) -> Self {
        Self {
            recordings_dir: Some(data_dir.as_ref().join("recordings")),
            ..Self::at_path(data_dir.as_ref().join("history.json"))
        }
    }

    pub fn at_path(path: PathBuf) -> Self {
        Self {
            path,
            recordings_dir: None,
            lock: Mutex::new(()),
        }
    }

    pub fn list(&self) -> Result<Vec<DictationSession>, BackendError> {
        let _guard = self.lock_store()?;
        self.read_locked()
    }

    pub fn append_with_retention(
        &self,
        session: DictationSession,
        retention_days: u32,
        max_entries: Option<u32>,
    ) -> Result<(), BackendError> {
        let _guard = self.lock_store()?;
        let mut sessions = self.read_locked()?;
        sessions.insert(0, session);
        let mut evicted: Vec<String> = Vec::new();
        if retention_days > 0 {
            let cutoff = chrono::Utc::now() - chrono::Duration::days(i64::from(retention_days));
            let (kept, expired): (Vec<_>, Vec<_>) = sessions.into_iter().partition(|session| {
                chrono::DateTime::parse_from_rfc3339(&session.created_at)
                    .map(|time| time.with_timezone(&chrono::Utc) >= cutoff)
                    .unwrap_or(true)
            });
            sessions = kept;
            evicted.extend(expired.into_iter().map(|session| session.id));
        }
        let cap = max_entries
            .map(|count| (count as usize).clamp(5, HISTORY_CAP))
            .unwrap_or(HISTORY_CAP);
        if sessions.len() > cap {
            evicted.extend(sessions.drain(cap..).map(|session| session.id));
        }
        self.write_locked(&sessions)?;
        for id in &evicted {
            self.remove_recording_file(id);
        }
        Ok(())
    }

    pub fn recent_within_minutes(
        &self,
        minutes: u32,
    ) -> Result<Vec<DictationSession>, BackendError> {
        if minutes == 0 {
            return Ok(Vec::new());
        }
        let _guard = self.lock_store()?;
        let sessions = self.read_locked()?;
        let cutoff = chrono::Utc::now() - chrono::Duration::minutes(i64::from(minutes));
        Ok(sessions
            .into_iter()
            .take_while(|session| {
                chrono::DateTime::parse_from_rfc3339(&session.created_at)
                    .map(|time| time.with_timezone(&chrono::Utc) >= cutoff)
                    .unwrap_or(true)
            })
            .collect())
    }

    pub fn delete(&self, id: &str) -> Result<(), BackendError> {
        let _guard = self.lock_store()?;
        let mut sessions = self.read_locked()?;
        let before = sessions.len();
        sessions.retain(|session| session.id != id);
        if sessions.len() != before {
            self.write_locked(&sessions)?;
        }
        self.remove_recording_file(id);
        Ok(())
    }

    pub fn update_entry(&self, updated: DictationSession) -> Result<bool, BackendError> {
        let _guard = self.lock_store()?;
        let mut sessions = self.read_locked()?;
        let Some(slot) = sessions.iter_mut().find(|session| session.id == updated.id) else {
            return Ok(false);
        };
        *slot = updated;
        self.write_locked(&sessions)?;
        Ok(true)
    }

    pub fn clear(&self) -> Result<(), BackendError> {
        let _guard = self.lock_store()?;
        self.write_locked(&[])?;
        self.remove_all_recordings();
        Ok(())
    }

    /// 删除条目绑定的录音文件。session_id 必须是合法 UUID 才拼路径（与 host 和
    /// 远程输入归档的 `<uuid>.wav` 命名规约一致，杜绝路径越界）；文件不存在（可能
    /// 已被录音条数上限清掉）不算错误，其余删除失败仅告警——历史主流程不受影响。
    fn remove_recording_file(&self, session_id: &str) {
        let Some(dir) = self.recordings_dir.as_ref() else {
            return;
        };
        if uuid::Uuid::parse_str(session_id).is_err() {
            return;
        }
        let path = dir.join(format!("{session_id}.wav"));
        if let Err(error) = std::fs::remove_file(&path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!("[history] failed to remove recording {path:?}: {error}");
            }
        }
    }

    /// 清空历史时清掉整个录音目录的 wav（与 host 的 prune_recordings 同样只认 .wav）。
    fn remove_all_recordings(&self) {
        let Some(dir) = self.recordings_dir.as_ref() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("wav") {
                continue;
            }
            if let Err(error) = std::fs::remove_file(&path) {
                log::warn!("[history] failed to remove recording {path:?}: {error}");
            }
        }
    }

    fn lock_store(&self) -> Result<std::sync::MutexGuard<'_, ()>, BackendError> {
        self.lock.lock().map_err(|_| {
            BackendError::new(BackendErrorCode::Internal, "history store lock poisoned")
        })
    }

    fn read_locked(&self) -> Result<Vec<DictationSession>, BackendError> {
        read_or_default(&self.path)
    }

    fn write_locked(&self, sessions: &[DictationSession]) -> Result<(), BackendError> {
        let json = serde_json::to_vec_pretty(sessions)
            .map_err(|_| persistence_error("encode history entries"))?;
        atomic_write(&self.path, &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{HistoryInsertStatus, HistorySource, PolishMode};

    fn session(id: &str, created_at: String) -> DictationSession {
        DictationSession {
            id: id.into(),
            created_at,
            source: HistorySource::Voice,
            raw_transcript: "raw".into(),
            asr_transcript: None,
            final_text: "final".into(),
            mode: PolishMode::Light,
            style_pack_id: None,
            translation_active: false,
            polish_source: None,
            app_bundle_id: None,
            app_name: None,
            insert_status: HistoryInsertStatus::Inserted,
            error_code: None,
            duration_ms: Some(1000),
            dictionary_entry_count: None,
            has_audio_recording: None,
            recording_file: None,
            asr_provider: None,
            asr_model: None,
            llm_provider: None,
            llm_model: None,
            pipeline_mode: None,
            asr_ms: None,
            polish_ms: None,
            ghostwriter_hits: None,
            ghostwriter_selections: None,
            ghostwriter_chat: None,
        }
    }

    #[test]
    fn append_orders_caps_and_filters_retention() {
        let path = std::env::temp_dir().join(format!(
            "openless-core-history-{}.json",
            uuid::Uuid::new_v4().simple()
        ));
        let store = HistoryStore::at_path(path.clone());
        let old = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        store
            .append_with_retention(session("old", old), 0, None)
            .unwrap();
        for index in 0..7 {
            store
                .append_with_retention(
                    session(&format!("new-{index}"), chrono::Utc::now().to_rfc3339()),
                    7,
                    Some(5),
                )
                .unwrap();
        }
        let sessions = store.list().unwrap();
        assert_eq!(sessions.len(), 5);
        assert_eq!(sessions[0].id, "new-6");
        assert!(sessions.iter().all(|session| session.id != "old"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn update_delete_clear_and_recent_queries_are_stable() {
        let path = std::env::temp_dir().join(format!(
            "openless-core-history-{}.json",
            uuid::Uuid::new_v4().simple()
        ));
        let store = HistoryStore::at_path(path.clone());
        let mut entry = session("one", chrono::Utc::now().to_rfc3339());
        store.append_with_retention(entry.clone(), 0, None).unwrap();
        assert_eq!(store.recent_within_minutes(5).unwrap(), vec![entry.clone()]);
        entry.final_text = "updated".into();
        assert!(store.update_entry(entry.clone()).unwrap());
        assert_eq!(store.list().unwrap(), vec![entry]);
        store.delete("one").unwrap();
        store.delete("missing").unwrap();
        assert!(store.list().unwrap().is_empty());
        store.clear().unwrap();
        assert!(store.recent_within_minutes(0).unwrap().is_empty());
        let _ = std::fs::remove_file(path);
    }

    const UUID_A: &str = "0b0e5e6e-8c8d-4f4a-9a5b-1a2b3c4d5e01";
    const UUID_B: &str = "0b0e5e6e-8c8d-4f4a-9a5b-1a2b3c4d5e02";
    const UUID_C: &str = "0b0e5e6e-8c8d-4f4a-9a5b-1a2b3c4d5e03";

    fn store_with_recordings(root: &Path) -> HistoryStore {
        std::fs::create_dir_all(root.join("recordings")).unwrap();
        HistoryStore::at_data_dir(root)
    }

    fn write_recording(root: &Path, session_id: &str) -> PathBuf {
        let path = root.join("recordings").join(format!("{session_id}.wav"));
        std::fs::write(&path, b"RIFF....wav").unwrap();
        path
    }

    #[test]
    fn deleting_entry_removes_its_bound_recording_file() {
        let root = std::env::temp_dir().join(format!(
            "openless-core-history-rec-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = store_with_recordings(&root);
        let entry = session(UUID_A, chrono::Utc::now().to_rfc3339());
        store.append_with_retention(entry, 0, None).unwrap();
        let wav = write_recording(&root, UUID_A);

        store.delete(UUID_A).unwrap();
        assert!(!wav.exists(), "删除历史条目应连带删除录音文件");

        // 非 UUID 的 id 不允许拼路径删文件（路径越界防护）。
        let innocent = root.join("recordings").join("not-a-uuid.wav");
        std::fs::write(&innocent, b"x").unwrap();
        store.delete("../escape").unwrap();
        store.delete("not-a-uuid").unwrap();
        assert!(innocent.exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clearing_history_removes_all_recordings() {
        let root = std::env::temp_dir().join(format!(
            "openless-core-history-rec-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = store_with_recordings(&root);
        for id in [UUID_A, UUID_B] {
            store
                .append_with_retention(session(id, chrono::Utc::now().to_rfc3339()), 0, None)
                .unwrap();
            write_recording(&root, id);
        }
        store.clear().unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(
            std::fs::read_dir(root.join("recordings"))
                .unwrap()
                .flatten()
                .next()
                .is_none(),
            "清空历史应清空全部录音"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn retention_and_cap_eviction_remove_bound_recordings() {
        let root = std::env::temp_dir().join(format!(
            "openless-core-history-rec-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = store_with_recordings(&root);
        let old = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let now = chrono::Utc::now().to_rfc3339();
        let ids = [
            UUID_A,
            UUID_B,
            UUID_C,
            "0b0e5e6e-8c8d-4f4a-9a5b-1a2b3c4d5e04",
            "0b0e5e6e-8c8d-4f4a-9a5b-1a2b3c4d5e05",
            "0b0e5e6e-8c8d-4f4a-9a5b-1a2b3c4d5e06",
        ];
        for (index, id) in ids.iter().enumerate() {
            let created_at = if index == 0 { old.clone() } else { now.clone() };
            store
                .append_with_retention(session(id, created_at), 0, None)
                .unwrap();
            write_recording(&root, id);
        }
        const UUID_G: &str = "0b0e5e6e-8c8d-4f4a-9a5b-1a2b3c4d5e07";
        // 新写入触发清理（cap 下限 5）：retention=30 天挤出过期条目 UUID_A；
        // 剩余 6 条超 cap=5，最旧的 UUID_B 按条数挤出。
        store
            .append_with_retention(session(UUID_G, now), 30, Some(5))
            .unwrap();
        let wav_of = |id: &str| root.join("recordings").join(format!("{id}.wav"));
        assert!(!wav_of(UUID_A).exists(), "按保留期挤出的条目应连带删录音");
        assert!(!wav_of(UUID_B).exists(), "按条数挤出的条目应连带删录音");
        assert!(wav_of(UUID_C).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recording_file_round_trips_and_legacy_entries_default_to_none() {
        let mut entry = session(UUID_A, chrono::Utc::now().to_rfc3339());
        entry.recording_file = Some(format!("recordings/{UUID_A}.wav"));
        let json = serde_json::to_string(&entry).unwrap();
        assert!(
            json.contains(&format!("\"recordingFile\":\"recordings/{UUID_A}.wav\"")),
            "camelCase 字段名，实际: {json}"
        );
        let restored: DictationSession = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.recording_file.as_deref(),
            Some(format!("recordings/{UUID_A}.wav").as_str())
        );

        // 旧 JSON 无该字段照读为 None，且 None 时不写出该键（skip_serializing_if）。
        let legacy_json = serde_json::to_string(&session(UUID_B, chrono::Utc::now().to_rfc3339()))
            .unwrap();
        assert!(!legacy_json.contains("recordingFile"));
        let restored: DictationSession = serde_json::from_str(&legacy_json).unwrap();
        assert!(restored.recording_file.is_none());
    }
}
