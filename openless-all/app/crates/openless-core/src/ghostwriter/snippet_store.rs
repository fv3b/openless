//! 常用语（snippet）存储：用户存下来的表述实体（触发词/别名 → 表述文本，带贴位与启用开关）。
//!
//! 持久化照抄 style_pack_store 模式：Mutex 内存态 + 每次变更后整文件原子写，
//! 落 `data_dir/ghostwriter-snippets.json`；文件缺失按空库处理。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::errors::{BackendError, BackendErrorCode};
use crate::persistence::{atomic_write, persistence_error, read_or_default};

/// 常用语贴位模式："inline" 进正文、"footnote" 附注。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnippetMode {
    Inline,
    Footnote,
}

/// 常用语：触发词/别名 → 表述文本，命中后按贴位进正文或附注。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: String,
    /// 触发词（非空，≤24 chars）
    pub trigger: String,
    pub aliases: Vec<String>,
    /// 表述文本（可多句）
    pub text: String,
    pub mode: SnippetMode,
    pub enabled: bool,
}

pub struct SnippetStore {
    path: Option<PathBuf>,
    state: Mutex<Vec<Snippet>>,
}

impl SnippetStore {
    /// 打开数据目录下的 ghostwriter-snippets.json；文件缺失按空库处理；
    /// 文件损坏时先把原文件改名备份（.corrupt-<uuid4>）再回空库，避免下次变更覆写丢失。
    pub fn at_data_dir(dir: &std::path::Path) -> Self {
        let path = dir.join("ghostwriter-snippets.json");
        let snippets = match read_or_default(&path) {
            Ok(snippets) => snippets,
            Err(_) => backup_corrupt_file(&path),
        };
        Self {
            path: Some(path),
            state: Mutex::new(snippets),
        }
    }

    pub fn in_memory() -> Self {
        Self {
            path: None,
            state: Mutex::new(Vec::new()),
        }
    }

    pub fn list(&self) -> Vec<Snippet> {
        self.lock()
            .map(|snippets| snippets.clone())
            .unwrap_or_default()
    }

    pub fn enabled(&self) -> Vec<Snippet> {
        self.list().into_iter().filter(|s| s.enabled).collect()
    }

    /// create：id 空则 `uuid::Uuid::new_v4()`；trigger.trim() 空→InvalidArgument；
    /// 同 trigger（含大小写折叠）已存在→InvalidArgument。
    pub fn create(&self, mut snippet: Snippet) -> Result<Snippet, BackendError> {
        let mut snippets = self.lock()?;
        snippet.id = snippet.id.trim().to_string();
        if snippet.id.is_empty() {
            snippet.id = uuid::Uuid::new_v4().to_string();
        }
        snippet.trigger = required_trigger(&snippet.trigger)?;
        ensure_trigger_available(&snippets, &snippet.trigger, None)?;
        normalize_aliases(&mut snippet.aliases);
        snippets.push(snippet.clone());
        self.persist_locked(&snippets)?;
        Ok(snippet)
    }

    /// update：id 必须已存在；trigger 规则与 create 相同（重复判定排除自身）。
    pub fn update(&self, mut snippet: Snippet) -> Result<Snippet, BackendError> {
        let mut snippets = self.lock()?;
        if !snippets.iter().any(|existing| existing.id == snippet.id) {
            return Err(not_found(&snippet.id));
        }
        snippet.trigger = required_trigger(&snippet.trigger)?;
        ensure_trigger_available(&snippets, &snippet.trigger, Some(snippet.id.as_str()))?;
        normalize_aliases(&mut snippet.aliases);
        if let Some(slot) = snippets
            .iter_mut()
            .find(|existing| existing.id == snippet.id)
        {
            *slot = snippet.clone();
        }
        self.persist_locked(&snippets)?;
        Ok(snippet)
    }

    pub fn remove(&self, id: &str) -> Result<(), BackendError> {
        let mut snippets = self.lock()?;
        let index = snippets
            .iter()
            .position(|existing| existing.id == id)
            .ok_or_else(|| not_found(id))?;
        snippets.remove(index);
        self.persist_locked(&snippets)?;
        Ok(())
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), BackendError> {
        let mut snippets = self.lock()?;
        let index = snippets
            .iter()
            .position(|existing| existing.id == id)
            .ok_or_else(|| not_found(id))?;
        snippets[index].enabled = enabled;
        self.persist_locked(&snippets)?;
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Vec<Snippet>>, BackendError> {
        self.state.lock().map_err(|_| {
            BackendError::new(BackendErrorCode::Internal, "snippet store lock poisoned")
        })
    }

    fn persist_locked(&self, snippets: &[Snippet]) -> Result<(), BackendError> {
        match &self.path {
            Some(path) => write_snippets(path, snippets),
            None => Ok(()),
        }
    }
}

fn write_snippets(path: &Path, snippets: &[Snippet]) -> Result<(), BackendError> {
    let bytes =
        serde_json::to_vec_pretty(snippets).map_err(|_| persistence_error("encode snippets"))?;
    atomic_write(path, &bytes)
}

/// 解码失败时把损坏文件改名备份到 .corrupt-<uuid4> 后回空库；
/// 改名失败则删除原文件兜底（保住可手工恢复的备份优先）。
fn backup_corrupt_file(path: &Path) -> Vec<Snippet> {
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

fn required_trigger(trigger: &str) -> Result<String, BackendError> {
    let trigger = trigger.trim();
    if trigger.is_empty() {
        return Err(BackendError::new(
            BackendErrorCode::InvalidArgument,
            "snippet trigger is empty",
        ));
    }
    Ok(trigger.to_string())
}

fn ensure_trigger_available(
    snippets: &[Snippet],
    trigger: &str,
    ignore_id: Option<&str>,
) -> Result<(), BackendError> {
    let folded = trigger.to_lowercase();
    if snippets
        .iter()
        .any(|existing| {
            Some(existing.id.as_str()) != ignore_id
                && existing.trigger.to_lowercase() == folded
        })
    {
        return Err(BackendError::new(
            BackendErrorCode::InvalidArgument,
            format!("snippet trigger {trigger} already exists"),
        ));
    }
    Ok(())
}

/// 别名逐条 trim 并丢弃空串；大小写原样保留（折叠留给命中匹配时做）。
fn normalize_aliases(aliases: &mut Vec<String>) {
    *aliases = aliases
        .iter()
        .map(|alias| alias.trim().to_string())
        .filter(|alias| !alias.is_empty())
        .collect();
}

fn not_found(id: &str) -> BackendError {
    BackendError::new(
        BackendErrorCode::InvalidArgument,
        format!("snippet {id} not found"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(id: &str, trigger: &str) -> Snippet {
        Snippet {
            id: id.to_string(),
            trigger: trigger.to_string(),
            aliases: Vec::new(),
            text: "表述文本".to_string(),
            mode: SnippetMode::Inline,
            enabled: true,
        }
    }

    #[test]
    fn create_assigns_id_and_persists_order() {
        // create 两条 → list() 顺序与 created 一致、id 非空
        let store = SnippetStore::in_memory();
        let first = store.create(snippet("", "发货")).unwrap();
        let second = store.create(snippet("", "退款")).unwrap();
        assert!(!first.id.is_empty());
        assert!(!second.id.is_empty());
        assert_ne!(first.id, second.id);
        assert_eq!(store.list(), vec![first, second]);
    }

    #[test]
    fn create_rejects_blank_trigger_and_duplicate() {
        // 空 trigger Err；同 trigger 二次 create Err（含空白与大写折叠变体）
        let store = SnippetStore::in_memory();
        assert_eq!(
            store.create(snippet("", "   ")).unwrap_err().code,
            BackendErrorCode::InvalidArgument
        );
        assert!(store.list().is_empty());
        let first = store.create(snippet("", "发货")).unwrap();
        assert_eq!(first.trigger, "发货");
        assert_eq!(
            store.create(snippet("", "  发货  ")).unwrap_err().code,
            BackendErrorCode::InvalidArgument
        );
        store.create(snippet("", "ok")).unwrap();
        for variant in ["OK", " Ok "] {
            assert_eq!(
                store.create(snippet("", variant)).unwrap_err().code,
                BackendErrorCode::InvalidArgument
            );
        }
        assert_eq!(store.list().len(), 2);
    }

    #[test]
    fn update_remove_set_enabled_roundtrip() {
        // update 改 text/mode；remove 后 list 空；set_enabled 生效；未知 id 均报错
        let store = SnippetStore::in_memory();
        let created = store.create(snippet("", "翻译")).unwrap();
        let mut changed = created.clone();
        changed.text = "已更新的表述".to_string();
        changed.mode = SnippetMode::Footnote;
        let updated = store.update(changed).unwrap();
        assert_eq!(updated.text, "已更新的表述");
        assert_eq!(updated.mode, SnippetMode::Footnote);
        assert_eq!(store.list(), vec![updated.clone()]);
        assert_eq!(
            store.update(snippet("missing", "某词")).unwrap_err().code,
            BackendErrorCode::InvalidArgument
        );
        store.set_enabled(&created.id, false).unwrap();
        assert!(!store.list()[0].enabled);
        assert!(store.enabled().is_empty());
        store.set_enabled(&created.id, true).unwrap();
        assert_eq!(store.enabled(), vec![updated]);
        assert_eq!(
            store.set_enabled("missing", true).unwrap_err().code,
            BackendErrorCode::InvalidArgument
        );
        assert_eq!(
            store.remove("missing").unwrap_err().code,
            BackendErrorCode::InvalidArgument
        );
        store.remove(&created.id).unwrap();
        assert!(store.list().is_empty());
    }

    #[test]
    fn at_data_dir_loads_and_atomic_writes() {
        // temp dir：先写一个合法 json → at_data_dir → list 读出；create → 文件存在且含新条目
        let dir = std::env::temp_dir().join(format!(
            "openless-core-ghostwriter-snippets-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let seeded = vec![snippet("seed-1", "预置")];
        let path = dir.join("ghostwriter-snippets.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&seeded).unwrap()).unwrap();
        let store = SnippetStore::at_data_dir(&dir);
        assert_eq!(store.list(), seeded);
        let created = store.create(snippet("", "新增")).unwrap();
        assert!(path.exists());
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw.as_array().unwrap().len(), 2);
        assert_eq!(raw[1]["id"], created.id);
        assert_eq!(raw[1]["trigger"], "新增");
        assert_eq!(raw[1]["mode"], "inline");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn at_data_dir_backs_aside_corrupt_file() {
        // 损坏 json → at_data_dir → 空库；原文件被改名备份（原路径消失，.corrupt-* 备份存在且内容原样）
        let dir = std::env::temp_dir().join(format!(
            "openless-core-ghostwriter-snippets-corrupt-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ghostwriter-snippets.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = SnippetStore::at_data_dir(&dir);
        assert!(store.list().is_empty());
        assert!(!path.exists());
        let backup = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .find(|name| name.starts_with("ghostwriter-snippets.json.corrupt-"))
            .unwrap();
        assert_eq!(std::fs::read(dir.join(backup)).unwrap(), b"{ not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_normalizes_aliases_trim_and_drop_empty() {
        // 别名逐条 trim、空串丢弃、顺序保留：[" a ", "", " b ", "   "] → ["a", "b"]
        let mut input = snippet("", "发货");
        input.aliases = vec![
            " a ".to_string(),
            String::new(),
            " b ".to_string(),
            "   ".to_string(),
        ];
        let created = SnippetStore::in_memory().create(input).unwrap();
        assert_eq!(created.aliases, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn enabled_filters_disabled() {
        // 2 条，1 禁用 → enabled() 只回启用的那条
        let store = SnippetStore::in_memory();
        let on = snippet("", "开启");
        let mut off = snippet("", "关闭");
        off.enabled = false;
        let kept = store.create(on).unwrap();
        store.create(off).unwrap();
        assert_eq!(store.enabled(), vec![kept.clone()]);
        store.set_enabled(&kept.id, false).unwrap();
        assert!(store.enabled().is_empty());
    }
}
