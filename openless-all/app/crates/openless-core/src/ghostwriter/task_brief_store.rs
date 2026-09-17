//! 任务书覆写存储：每份任务书的用户覆写正文，落 `data_dir/ghostwriter-prompts.json`。
//!
//! 持久化照抄 snippet_store 模式：Mutex 内存态 + 每次变更后整文件原子写；
//! 文件缺失按「无覆写」处理。只存覆写，缺省回代码内置默认（ADR 0002），
//! 每份可恢复默认，保存即生效。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::errors::{BackendError, BackendErrorCode};
use crate::ghostwriter::prompts::TaskBriefId;
use crate::persistence::{atomic_write, persistence_error, read_or_default};

/// 任务书列表条目：给界面展示的单份任务书快照（身份＋描述＋是否覆写＋当前正文）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskBriefInfo {
    pub id: String,
    pub title: String,
    pub description: String,
    pub modified: bool,
    pub body: String,
}

pub struct TaskBriefStore {
    path: Option<PathBuf>,
    state: Mutex<HashMap<String, String>>,
}

/// 固定六份的注册表顺序（list() 与界面列表都按这个序）。
const ALL_IDS: [TaskBriefId; 6] = [
    TaskBriefId::InstructionPolish,
    TaskBriefId::Candidates,
    TaskBriefId::Recommendations,
    TaskBriefId::SedimentExtraction,
    TaskBriefId::ConversationReply,
    TaskBriefId::ConversationFinalize,
];

impl TaskBriefStore {
    /// 打开数据目录下的 ghostwriter-prompts.json；文件缺失按空覆写处理；
    /// 文件损坏时先把原文件改名备份（.corrupt-<uuid4>）再回空覆写。
    pub fn at_data_dir(dir: &std::path::Path) -> Self {
        let path = dir.join("ghostwriter-prompts.json");
        let overrides = match read_or_default::<HashMap<String, String>>(&path) {
            Ok(overrides) => overrides,
            Err(_) => backup_corrupt_file(&path),
        };
        Self {
            path: Some(path),
            state: Mutex::new(overrides),
        }
    }

    pub fn in_memory() -> Self {
        Self {
            path: None,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// 当前生效正文：覆写优先，否则代码内置默认。
    pub fn body(&self, id: TaskBriefId) -> String {
        self.lock()
            .map(|overrides| {
                overrides
                    .get(id.key())
                    .cloned()
                    .unwrap_or_else(|| id.default_body().to_string())
            })
            .unwrap_or_else(|_| id.default_body().to_string())
    }

    /// 覆写正文：trim 后非空才收，否则 InvalidArgument；成功后落盘并返回最新条目。
    pub fn set_body(&self, id: TaskBriefId, body: &str) -> Result<TaskBriefInfo, BackendError> {
        let body = body.trim();
        if body.is_empty() {
            return Err(BackendError::new(
                BackendErrorCode::InvalidArgument,
                "task brief body is empty",
            ));
        }
        let mut overrides = self.lock()?;
        overrides.insert(id.key().to_string(), body.to_string());
        self.persist_locked(&overrides)?;
        Ok(self.info(id, &overrides))
    }

    /// 恢复默认：删掉该份的覆写并落盘；本来就没有覆写时是 no-op。
    /// 落盘失败时回滚内存，保持内存与磁盘一致（覆写保留，仍算已修改）。
    pub fn reset(&self, id: TaskBriefId) -> TaskBriefInfo {
        match self.lock() {
            Err(_) => default_info(id),
            Ok(mut overrides) => {
                if let Some(previous) = overrides.remove(id.key()) {
                    if self.persist_locked(&overrides).is_err() {
                        overrides.insert(id.key().to_string(), previous);
                    }
                }
                self.info(id, &overrides)
            }
        }
    }

    /// 该份是否存在用户覆写。
    pub fn is_modified(&self, id: TaskBriefId) -> bool {
        self.lock()
            .map(|overrides| overrides.contains_key(id.key()))
            .unwrap_or(false)
    }

    /// 全部六份任务书快照，固定注册表顺序。
    pub fn list(&self) -> Vec<TaskBriefInfo> {
        match self.lock() {
            Ok(overrides) => ALL_IDS.iter().map(|id| self.info(*id, &overrides)).collect(),
            Err(_) => ALL_IDS.iter().map(|id| default_info(*id)).collect(),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, String>>, BackendError> {
        self.state.lock().map_err(|_| {
            BackendError::new(BackendErrorCode::Internal, "task brief store lock poisoned")
        })
    }

    fn persist_locked(&self, overrides: &HashMap<String, String>) -> Result<(), BackendError> {
        match &self.path {
            Some(path) => write_overrides(path, overrides),
            None => Ok(()),
        }
    }

    fn info(&self, id: TaskBriefId, overrides: &HashMap<String, String>) -> TaskBriefInfo {
        TaskBriefInfo {
            id: id.key().to_string(),
            title: id.title().to_string(),
            description: id.description().to_string(),
            modified: overrides.contains_key(id.key()),
            body: overrides
                .get(id.key())
                .cloned()
                .unwrap_or_else(|| id.default_body().to_string()),
        }
    }
}

fn write_overrides(path: &Path, overrides: &HashMap<String, String>) -> Result<(), BackendError> {
    let bytes = serde_json::to_vec_pretty(overrides)
        .map_err(|_| persistence_error("encode task brief overrides"))?;
    atomic_write(path, &bytes)
}

/// 锁损坏时的兜底快照：回默认正文、无覆写标记。
fn default_info(id: TaskBriefId) -> TaskBriefInfo {
    TaskBriefInfo {
        id: id.key().to_string(),
        title: id.title().to_string(),
        description: id.description().to_string(),
        modified: false,
        body: id.default_body().to_string(),
    }
}

/// 解码失败时把损坏文件改名备份到 .corrupt-<uuid4> 后回空覆写；
/// 改名失败则删除原文件兜底（保住可手工恢复的备份优先）。
fn backup_corrupt_file(path: &Path) -> HashMap<String, String> {
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
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "openless-core-ghostwriter-task-briefs-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn set_body_persists_and_reports_modified() {
        // in_memory：set → body=覆写、modified=true、list 标记；at_data_dir：重开读回覆写
        let store = TaskBriefStore::in_memory();
        let info = store
            .set_body(TaskBriefId::InstructionPolish, "  自定义润色正文  ")
            .unwrap();
        assert_eq!(info.id, "instruction_polish");
        assert_eq!(info.body, "自定义润色正文");
        assert!(info.modified);
        assert_eq!(store.body(TaskBriefId::InstructionPolish), "自定义润色正文");
        assert!(store.is_modified(TaskBriefId::InstructionPolish));
        let listed = &store.list()[0];
        assert_eq!(listed.id, "instruction_polish");
        assert_eq!(listed.body, "自定义润色正文");
        assert!(listed.modified);
        assert!(!store.is_modified(TaskBriefId::Candidates));
        assert_eq!(store.body(TaskBriefId::Candidates), TaskBriefId::Candidates.default_body());

        let dir = temp_dir("persist");
        let file_store = TaskBriefStore::at_data_dir(&dir);
        file_store
            .set_body(TaskBriefId::SedimentExtraction, "提取常用语覆写")
            .unwrap();
        let path = dir.join("ghostwriter-prompts.json");
        assert!(path.exists());
        let reopened = TaskBriefStore::at_data_dir(&dir);
        assert_eq!(reopened.body(TaskBriefId::SedimentExtraction), "提取常用语覆写");
        assert!(reopened.is_modified(TaskBriefId::SedimentExtraction));
        assert_eq!(
            reopened.body(TaskBriefId::Recommendations),
            TaskBriefId::Recommendations.default_body()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_body_rejected() {
        let store = TaskBriefStore::in_memory();
        for blank in ["", "   ", "\n\t "] {
            let error = store
                .set_body(TaskBriefId::Candidates, blank)
                .unwrap_err();
            assert_eq!(error.code, BackendErrorCode::InvalidArgument);
        }
        assert!(!store.is_modified(TaskBriefId::Candidates));
        assert_eq!(
            store.body(TaskBriefId::Candidates),
            TaskBriefId::Candidates.default_body()
        );
    }

    #[test]
    fn reset_restores_default() {
        let dir = temp_dir("reset");
        let store = TaskBriefStore::at_data_dir(&dir);
        store
            .set_body(TaskBriefId::Recommendations, "被覆写的推荐正文")
            .unwrap();
        assert!(store.is_modified(TaskBriefId::Recommendations));

        let info = store.reset(TaskBriefId::Recommendations);
        assert_eq!(info.body, TaskBriefId::Recommendations.default_body());
        assert!(!info.modified);
        assert_eq!(
            store.body(TaskBriefId::Recommendations),
            TaskBriefId::Recommendations.default_body()
        );
        assert!(!store.is_modified(TaskBriefId::Recommendations));
        assert!(!store
            .list()
            .iter()
            .any(|brief| brief.id == "recommendations" && brief.modified));

        // 重开确认覆写已从盘上移除；对没覆写的份 reset 是 no-op
        let reopened = TaskBriefStore::at_data_dir(&dir);
        assert_eq!(
            reopened.body(TaskBriefId::Recommendations),
            TaskBriefId::Recommendations.default_body()
        );
        assert!(!reopened.is_modified(TaskBriefId::Recommendations));
        reopened.reset(TaskBriefId::SedimentExtraction);
        assert!(!reopened.is_modified(TaskBriefId::SedimentExtraction));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_returns_six_in_fixed_order() {
        let store = TaskBriefStore::in_memory();
        let briefs = store.list();
        assert_eq!(
            briefs
                .iter()
                .map(|brief| brief.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "instruction_polish",
                "candidates",
                "recommendations",
                "sediment_extraction",
                "conversation_reply",
                "conversation_finalize",
            ]
        );
        assert_eq!(briefs.len(), 6);
        assert!(briefs.iter().all(|brief| !brief.modified));
        assert!(briefs.iter().all(|brief| !brief.title.is_empty()));
        assert!(briefs.iter().all(|brief| !brief.description.is_empty()));
        assert!(briefs
            .iter()
            .all(|brief| !brief.body.is_empty()));
        assert_eq!(
            briefs[0].body,
            crate::ghostwriter::prompts::GHOSTWRITER_INSTRUCTION_PROMPT
        );
    }

    #[test]
    fn unknown_stored_keys_are_skipped() {
        // 退役任务书的旧覆写键（如 sediment_notice）留在文件里不影响加载：
        // 未知键被静默忽略，已知键照常读回，list 仍是固定六份。
        let dir = std::env::temp_dir().join(format!(
            "openless-core-ghostwriter-briefs-stale-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ghostwriter-prompts.json");
        std::fs::write(
            &path,
            r#"{"sediment_notice":"旧提醒覆写","candidates":"旧候选覆写"}"#,
        )
        .unwrap();
        let store = TaskBriefStore::at_data_dir(&dir);
        assert_eq!(store.body(TaskBriefId::Candidates), "旧候选覆写");
        assert!(store.is_modified(TaskBriefId::Candidates));
        assert_eq!(
            store.body(TaskBriefId::SedimentExtraction),
            TaskBriefId::SedimentExtraction.default_body()
        );
        assert_eq!(store.list().len(), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
