# Ghostwriter M1′+M2 实施计划（基座改造＋润色流＋常用语系统）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 M1 的生转写浮框升级为「指令台」第一段：流式 delta 语义修复→说话中段指令化润色→常用语（触发词命中→inline 融合/footnote 附注）→指令预览所见即所贴闭环＋管理页。

**Architecture:** 新模块全在 `openless-core/src/ghostwriter/`；段润色 LLM 调用由 api.rs 侧 dispatcher spawn（GhostwriterSession 保持纯逻辑可单测）；常用语存储照 style_pack_store 的「Mutex 内存态＋JSON 原子写」模式；前端 GhostwriterPanel 改三区（候选区置顶/指令预览居中/转写流沉底），显隐仍自管。

**Tech Stack:** Rust (tokio, serde, uuid v4 已有依赖) ＋ Tauri 2 ＋ React/TS（无 vitest：自定义 assert＋tsx 测试栈，`npm test` 跑 `src/**/*.test.ts`）。

**Spec:** `/Users/cc1/LocalCode/vendor/openless/docs/design/ghostwriter/design.md`（执行者必须先读）；词汇表 `/Users/cc1/LocalCode/vendor/openless/CONTEXT.md`。

## Global Constraints

- 内部代号 `ghostwriter`（原代号 `fluid` 已于 2026-09-11 全局改名，勿再使用）；显示名「Ghostwriter 流式浮框」（8 门 i18n 已定案，不要改）。capsuleStyle 样式值 `"fluid"` 是样式系统的用户数据标识，不在改名范围。
- 旧文件只做加法式小改，不改任何既有 trait 签名；每任务后 `cargo check -p openless-core` 与 `cargo check -p openless-linux-egui` 必须通过。
- 同步上游一律 rebase（弃 merge）；推送用 SSH（HTTPS token 无 workflow scope）。
- ghostwriter 窗口照 `tauri.conf.json` 现状（focus:false、alwaysOnTop、透明无边框、560×420），不要动窗口属性。
- **指令预览与 `GhostwriterSession::assembled_text()` 同源**（所见即所贴）；预览含主文本＋footnote 附注块。
- 润色 prompt 语义＝指令化：去口水、理结构、口语转准确说法、**不改意图**、保留原话具体信息（名字/数字/路径/代码一字不改）。
- 机械模式（`ghostwriter.polishEnabled=false`）：仍置 Raw、仍建 GhostwriterSession、贴生转写；footnote 附注照拼；inline 常用语退化为文末拼接。
- 命中即生效＋「✕ 取消」撤销最近一次命中；同一 snippet 会话内命中去重（重复说到只生效一次）。
- UI 文案用「贴进正文」/「附在文末」，`inline`/`footnote` 只活在代码层（serde 值 `"inline"`/`"footnote"`）。
- 空态：候选区/徽标行无内容整行收起；管理页空库直通新建＋一句说明。错误态：LLM 流失败→预览保持上一有效状态＋notice 行轻提示，不弹窗、不阻塞贴出。
- 浮框内文案一律走 i18n（现状 GhostwriterPanel 硬编码中文，本段一并接入）。
- 段润色 session_id 用独立前缀 `ghostwriter-segment-{n}`，与主转写润色互不 cancel。
- 测试：core 单测内嵌 `#[cfg(test)] mod tests`；api.rs 集成测试用 `crate::testing::Fixture*`＋`backend_with_dictation_engine` 模式；前端测试 node＋tsx 无 DOM。

## File Structure

**openless-core/src/ghostwriter/**（`lib.rs:33` 已有 `pub mod ghostwriter;`）：

| 文件 | 职责 |
|---|---|
| `types.rs`【新建】 | GhostwriterConfig、Snippet/SnippetMode/SnippetHit、FeedOutcome/PolishableSegment、事件 payload（GhostwriterPreviewChanged/GhostwriterSnippetsHit/GhostwriterNotice） |
| `session.rs`（重构） | 自适应缓冲＋segmenter 断句＋段/润色对齐＋待融材料＋命中去重＋assembled 拼装 |
| `segmenter.rs`（不动） | 断句状态机（现有 11 测试全保留） |
| `snippet_store.rs`【新建】 | 常用语存储：Mutex 内存态＋`data_dir/ghostwriter-snippets.json` 原子写 |
| `segment_polisher.rs`【新建】 | 临时 DictationContext 调 TextPolisher 的段润色（照 selection_voice_service.rs:281-352 模式）＋指令化 prompt 常量 |
| `dispatcher.rs`【新建】 | GhostwriterPolishDispatcher：spawn 段润色任务→apply_polished→publish GhostwriterPreviewChanged（持 `Arc<RwLock<MutableState>>`＋EventPublisher＋polisher，照 api.rs:1663 BackendEngineProgress 模式） |
| `mod.rs` | 模块声明＋文档 |

**修改**：`events.rs`（3 变体）、`api.rs`（构造注入 snippet_store/dispatcher、feed 点升级、stop 补润、cancel 命令）、`shared_types.rs`（GhostwriterPreferences）、`dictation_context.rs`（ghostwriter 快照字段）、`src-tauri/src/commands/ghostwriter.rs`【新建】、`src-tauri/src/lib.rs`（命令注册）、前端 `src/lib/ipc/ghostwriter.ts`【新建】、`src/lib/ghostwriterCapsule.ts`、`src/pages/GhostwriterPanel.tsx`（三区）、`src/pages/GhostwriterSnippets.tsx`【新建】、`src/App.tsx`/`src/main.tsx`（路由）、`src/i18n/*.ts`（8 门）。

---

### Task 1: 流式 delta 语义实机验证＋GhostwriterSession 自适应缓冲

**Files:**
- Modify: `crates/openless-core/src/ghostwriter/session.rs`
- Test: `crates/openless-core/src/ghostwriter/session.rs`（内嵌 tests）

**Interfaces:**
- Produces: `GhostwriterSession` 内部缓冲保证「说话中缓冲始终是完整累计文本」（segmenter 断句可用）；`feed` 签名本任务不变（`(&mut self, &TranscriptDelta)`），Task 6 再扩。
- 背景：`TranscriptAccumulator::apply`（types.rs:281-298）是绝对替换位语义；而 volcengine 等 provider 的 partial 是「相对 last_partial 的后缀增量＋offset:0」（volcengine.rs:709 strip_prefix）。M1 靠智谱批量（无 partial、仅 final 全量）掩盖了这一点。

- [ ] **Step 1: 写自适应缓冲的失败测试**

替换 session.rs 内 `assembled_text_joins_partial_and_final_deltas` 等 6 个测试中对 TranscriptAccumulator 的依赖，新测试（缓冲逻辑直接测 session.feed 行为）：

```rust
#[test]
fn suffix_partial_deltas_accumulate_and_final_replaces() {
    // 后缀增量型 provider（火山/讯飞实测形态）：partial 全部 offset:0
    let mut s = GhostwriterSession::new(GhostwriterConfig::default());
    s.feed(&delta("你好", 0, false), &[]).unwrap();
    s.feed(&delta("，", 0, false), &[]).unwrap();
    assert_eq!(s.debug_buffer(), "你好，");
    // final 全量替换
    s.feed(&delta("你好，这是测试。", 0, true), &[]).unwrap();
    assert_eq!(s.debug_text(), "你好，这是测试。");
}

#[test]
fn full_snapshot_partial_deltas_replace_not_append() {
    // 全量快照型 partial：text 是累计全文
    let mut s = GhostwriterSession::new(GhostwriterConfig::default());
    s.feed(&delta("你好", 0, false), &[]).unwrap();
    s.feed(&delta("你好，", 0, false), &[]).unwrap();
    assert_eq!(s.debug_text(), "你好，");
}

#[test]
fn offset_revision_still_replaces() {
    // 替换式修订（offset>0）语义保留
    let mut s = GhostwriterSession::new(GhostwriterConfig::default());
    s.feed(&delta("ABC", 0, false), &[]).unwrap();
    s.feed(&delta("X", 1, false), &[]).unwrap();
    assert_eq!(s.debug_text(), "AX");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p openless-core ghostwriter::` 
Expected: 编译失败（GhostwriterConfig/debug_text 不存在）或断言失败。

- [ ] **Step 3: 实现自适应缓冲**

session.rs 弃用 TranscriptAccumulator，自持 `buffer: String`＋`last_partial_len: usize`，feed 判型：

```rust
fn apply_delta(&mut self, delta: &TranscriptDelta) -> Result<(), BackendError> {
    if delta.is_final {
        self.buffer = delta.text.clone();   // final 全量替换，offset 忽略
        return Ok(());
    }
    let cur_chars = self.buffer.chars().count();
    let offset = usize::try_from(delta.offset).map_err(|_| bad_offset())?;
    if offset > cur_chars { return Err(bad_offset()); }
    if offset == 0 && self.buffer.chars().count() <= delta.text.chars().count()
        && self.buffer.chars().zip(delta.text.chars()).all(|(a, b)| a == b)
    {
        // 全量快照型：text 以当前缓冲为前缀 → 整体替换
        self.buffer = delta.text.clone();
    } else if offset == 0 {
        // 后缀增量型 → 追加
        self.buffer.push_str(&delta.text);
    } else {
        // 替换式修订（offset>0）→ 照旧语义
        let kept: String = self.buffer.chars().take(offset).collect();
        self.buffer = format!("{kept}{}", delta.text);
    }
    Ok(())
}
```

提供 `pub fn debug_text(&self) -> &str`（`&self.buffer`）供测试与 M1′ 验证探针；`GhostwriterConfig` 本任务先建空结构（`#[derive(Default, Clone, Debug)]`，字段 Task 2 补）。`FeedOutcome` 本任务先为空结构占位（Task 6 填充），feed 返回它。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p openless-core ghostwriter::`
Expected: 全部 PASS（新 3 个＋保留的 segmenter 11 个测试）。

- [ ] **Step 5: 实机验证**

1. 设置 ASR 源为火山（`activeAsrProvider: "volcengine"`，凭据照 `credentials.rs:64-68` 键名配置）；`OPENLESS_LOG_LEVEL=debug` 启动（`run-macos.sh`）。
2. 说话 30 秒（中文长句＋英文术语），抓 `~/Library/Logs/OpenLess/openless.log` 的 partial delta 序列。
3. 确认：①说话中前端浮框转写流逐句增长（不回跳碎片）；②若出现快照型/修订型序列与单测假设不符，回改 `apply_delta` 判型并补单测。
4. Commit:

```bash
git add crates/openless-core/src/ghostwriter/session.rs
git commit -m "ghostwriter: adaptive transcript buffer (suffix-append / snapshot-replace / offset-revision)"
```

---

### Task 2: GhostwriterPreferences（三流开关＋节流参数）进 UserPreferences 与 DictationContext

**Files:**
- Modify: `crates/openless-core/src/shared_types.rs`（UserPreferences L333-335 `capsule_style` 字段附近）
- Modify: `crates/openless-core/src/dictation_context.rs`
- Modify: `crates/openless-core/src/ghostwriter/types.rs`
- Test: `crates/openless-core/src/shared_types.rs`（内嵌 tests）

**Interfaces:**
- Produces:
```rust
// shared_types.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GhostwriterPreferences {
    pub polish_enabled: bool,          // 默认 true
    pub candidates_enabled: bool,      // 默认 true（M3 消费）
    pub recommendations_enabled: bool, // 默认 true（M3 消费）
    pub candidate_throttle_ms: u64,    // 默认 2000（M3 消费）
    pub recommendation_throttle_ms: u64, // 默认 2000（M3 消费）
}
// UserPreferences 加字段：
#[serde(default)]
pub ghostwriter: GhostwriterPreferences,
```
```rust
// dictation_context.rs：DictationContext 加字段（capture 时从 preferences 快照）
#[serde(default)]
pub ghostwriter: crate::ghostwriter::types::GhostwriterSnapshot,
// ghostwriter/types.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterSnapshot { pub polish_enabled: bool, pub active: bool }
```
- `Default for GhostwriterPreferences`：全 true＋2000ms。

- [ ] **Step 1: 失败测试（shared_types.rs 内嵌 tests）**

```rust
#[test]
fn ghostwriter_preferences_default_all_enabled() {
    let p = GhostwriterPreferences::default();
    assert!(p.polish_enabled && p.candidates_enabled && p.recommendations_enabled);
    assert_eq!(p.candidate_throttle_ms, 2000);
}

#[test]
fn ghostwriter_preferences_missing_in_old_config_falls_back_to_default() {
    let json = r#"{}"#;
    let p: GhostwriterPreferences = serde_json::from_str(json).unwrap();
    assert!(p.polish_enabled);
}

#[test]
fn user_preferences_ghostwriter_roundtrips_camel_case() {
    let json = r#"{"capsuleStyle":"fluid","ghostwriter":{"polishEnabled":false}}"#;
    let prefs: UserPreferences = serde_json::from_str(json).unwrap();
    assert!(!prefs.ghostwriter.polish_enabled);
    assert!(prefs.ghostwriter.candidates_enabled); // 未写回落默认
}
```

- [ ] **Step 2: 跑确认失败** — `cargo test -p openless-core ghostwriter_preferences`，编译失败。

- [ ] **Step 3: 实现** — shared_types.rs 加 `GhostwriterPreferences`＋UserPreferences 字段；dictation_context.rs 加 `ghostwriter: GhostwriterSnapshot` 字段；在 `DictationContext::capture` 内从 preferences 填 `GhostwriterSnapshot { polish_enabled: prefs.ghostwriter.polish_enabled, active: prefs.capsule_style == CapsuleStyle::Ghostwriter }`（capture 签名不动，读 preferences 参数即可——capture 已接收 `&UserPreferences`）。

- [ ] **Step 4: 跑确认通过** — `cargo test -p openless-core` 全绿（注意既有 capture 相关测试可能因新字段报缺省——`#[serde(default)]`＋Default 实现兜底）。

- [ ] **Step 5: Commit** — `git commit -am "ghostwriter: GhostwriterPreferences switches + context snapshot"`

---

### Task 3: 事件变体＋payload（ghostwriter/types.rs）

**Files:**
- Modify: `crates/openless-core/src/events.rs`（`BackendEventKind` 枚举 L362-395 末尾加变体）
- Modify: `crates/openless-core/src/ghostwriter/types.rs`
- Test: `crates/openless-core/src/ghostwriter/types.rs`（内嵌 tests）

**Interfaces:**
- Produces:
```rust
// ghostwriter/types.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterPreviewChanged { pub text: String, pub revision: u64 }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterSnippetHit { pub snippet_id: String, pub title: String, pub mode: String } // "inline"|"footnote"

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostwriterNotice { pub message: String, pub level: String } // "info"|"error"

// events.rs 变体（枚举已有 snake_case tag 自动生效，前端收到 {"type":"ghostwriter_preview_changed",...}）
GhostwriterPreviewChanged(crate::ghostwriter::types::GhostwriterPreviewChanged),
GhostwriterSnippetsHit(crate::ghostwriter::types::GhostwriterSnippetHit),
GhostwriterNotice(crate::ghostwriter::types::GhostwriterNotice),
```
- 桥接零改动：`src-tauri/src/tauri_events.rs:36` 全量 `app.emit("backend:event", &event)` 自动携带新变体。

- [ ] **Step 1: 失败测试（types.rs 内嵌）**

```rust
#[test]
fn ghostwriter_event_payloads_serialize_camel_case() {
    let p = GhostwriterPreviewChanged { text: "你好".into(), revision: 3 };
    let v: serde_json::Value = serde_json::to_value(&p).unwrap();
    assert_eq!(v["revision"], 3);
    let h = GhostwriterSnippetHit { snippet_id: "s1".into(), title: "翻译".into(), mode: "footnote".into() };
    let v: serde_json::Value = serde_json::to_value(&h).unwrap();
    assert_eq!(v["snippetId"], "s1");
}
```

- [ ] **Step 2-4:** 实现→`cargo test -p openless-core ghostwriter_event` PASS→`cargo check -p openless-linux-egui`。注意：枚举加变体后，tauri_events.rs 若有穷尽 match 会报错——新变体落入 L377-387 的空分支（`_ => {}` 已存在则零改动；若编译器报穷尽缺失，在 legacy 转发 match 加空臂）。

- [ ] **Step 5: Commit** — `git commit -am "ghostwriter: preview/hit/notice event variants"`

---

### Task 4: SnippetStore（常用语存储）

**Files:**
- Create: `crates/openless-core/src/ghostwriter/snippet_store.rs`
- Modify: `crates/openless-core/src/ghostwriter/mod.rs`（加 `pub mod snippet_store;`）

**Interfaces:**
- Produces:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnippetMode { Inline, Footnote }   // 序列化为字符串 "inline"/"footnote"（外部 tag），前端 types.ts mode:'inline'|'footnote' 直接对上

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: String,
    pub trigger: String,        // 触发词（非空，≤24 chars）
    pub aliases: Vec<String>,
    pub text: String,           // 表述文本（可多句）
    pub mode: SnippetMode,
    pub enabled: bool,
}

pub struct SnippetStore { path: Option<PathBuf>, state: Mutex<Vec<Snippet>> }
impl SnippetStore {
    pub fn at_data_dir(dir: &std::path::Path) -> Self  // dir/ghostwriter-snippets.json，文件缺失=空库
    pub fn in_memory() -> Self
    pub fn list(&self) -> Vec<Snippet>
    pub fn enabled(&self) -> Vec<Snippet>
    pub fn create(&self, mut s: Snippet) -> Result<Snippet, BackendError>
    // create：id 空则 uuid::Uuid::new_v4()；trigger.trim() 空→InvalidArgument；同 trigger（含大小写折叠）已存在→InvalidArgument
    pub fn update(&self, s: Snippet) -> Result<Snippet, BackendError>   // id 必须已存在
    pub fn remove(&self, id: &str) -> Result<(), BackendError>
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), BackendError>
}
```
- 持久化模式照抄 `style_pack_store.rs:653-656`：`serde_json::to_vec_pretty`＋`crate::persistence::atomic_write(&path, &bytes)`；每次变更后整文件写。
- `BackendError::new(BackendErrorCode::InvalidArgument, msg)` 模式照 transcript accumulator（types.rs:288-292）。

- [ ] **Step 1: 失败测试（snippet_store.rs 内嵌，用 `SnippetStore::in_memory()`）**

```rust
#[test]
fn create_assigns_id_and_persists_order() { /* create 两条 → list() 顺序与 created 一致、id 非空 */ }
#[test]
fn create_rejects_blank_trigger_and_duplicate() { /* 空 trigger Err；同 trigger 二次 create Err */ }
#[test]
fn update_remove_set_enabled_roundtrip() { /* update 改 text/mode；remove 后 list 空；set_enabled 生效 */ }
#[test]
fn at_data_dir_loads_and_atomic_writes() {
    // temp dir：先写一个合法 json → at_data_dir → list 读出；create → 断言文件存在且含新条目
}
#[test]
fn enabled_filters_disabled() { /* 2 条，1 禁用 → enabled() 只回 1 */ }
```

- [ ] **Step 2-4:** 跑失败→实现（Mutex Vec＋每变更 persist_locked，照 style_pack_store 模式）→PASS。

- [ ] **Step 5: Commit** — `git commit -am "ghostwriter: SnippetStore (ghostwriter-snippets.json, atomic write)"`

---

### Task 5: 段润色 SegmentPolisher（临时 context 调 LLM）

**Files:**
- Create: `crates/openless-core/src/ghostwriter/segment_polisher.rs`
- Modify: `crates/openless-core/src/ghostwriter/mod.rs`

**Interfaces:**
- Consumes: `ports.rs:474-484 TextPolisher::polish(session_id, Arc<DictationContext>, String, Arc<dyn TextStreamSink>)`；provider 解析与 context 组装照 `selection_voice_service.rs:281-352`（`resolve_session_provider`→`DictationContext::capture`→逐项覆写→`polisher.polish(..., Arc::new(DiscardTextStream))`；`DiscardTextStream` 若非 pub，在本文件照抄 8 行实现）。
- Produces:
```rust
pub const GHOSTWRITER_INSTRUCTION_PROMPT: &str = "你是语音指令整理器。用户在用语音给 AI 助手下指令，下面是一段口语转写。\n把它整理成清晰、直接、结构清楚的指令：\n- 去掉口头语、重复、语气词（嗯、啊、就是那种、类似什么的）\n- 理顺语句顺序，需要时整理成简短要点\n- 把口语化的说法换成准确表述，但绝不改变用户的意思，绝不添加用户没说的要求\n- 原话里的具体信息（名字、数字、路径、代码、命令）一字不改\n- 如果给了「参考材料」，把材料内容自然融合进指令对应的位置\n只输出整理后的指令文本，不要任何解释或前缀。";

#[derive(Debug, Clone)]
pub struct SegmentPolishRequest {
    pub session_id: SessionId,   // 独立前缀：由 dispatcher 生成 "ghostwriter-segment-{index}-{uuid后8位}"
    pub segment_index: usize,
    pub prior: String,           // 已润前文尾部（≤200 chars，截断自 polished 缓冲）
    pub segment: String,         // 本段生转写
    pub materials: Vec<String>,  // 本段挂着的 inline 常用语材料
}

pub async fn polish_segment(
    polisher: &Arc<dyn TextPolisher>,
    credential_store: &Arc<dyn crate::ports::CredentialStore>,  // 类型照 selection_voice_service.rs 实际签名
    active_llm_provider: &str,
    request: &SegmentPolishRequest,
) -> Result<String, BackendError>
```
- 实现：mode=Light、`style_system_prompt = GHOSTWRITER_INSTRUCTION_PROMPT`、`hotwords.clear()`、`translation_active=false`、`cursor_context=None`、`prior_turns.clear()`；user 输入＝`prior`（若非空，前缀「（前文：…）」）＋segment＋materials（每条前缀「参考材料：」）；`output.text.trim()`，空→Provider 错（照 selection_voice L344-350）。
- credential_store 类型名以 `selection_voice_service.rs:295` 实际引用为准（执行时读该文件确认，不要猜）。

- [ ] **Step 1: 失败测试（内嵌；polisher 用 `crate::testing::FixtureTextPolisher`，读 testing.rs:632 附近了解其输入捕获接口）**

```rust
#[tokio::test]
async fn polish_segment_sends_instruction_prompt_and_materials() {
    // FixtureTextPolisher 捕获 (session_id, context, raw_text)
    // 断言: context.polish.style_system_prompt 含 "语音指令整理器"
    //       context.polish.translation_active == false
    //       raw 含 segment 与 "参考材料："
    // 返回值 = fixture 输出 trim 后
}
#[tokio::test]
async fn polish_segment_empty_output_is_provider_error() { /* fixture 返回 "" → Err(Provider) */ }
```

- [ ] **Step 2-4:** 实现→`cargo test -p openless-core ghostwriter::segment_polisher` PASS。

- [ ] **Step 5: Commit** — `git commit -am "ghostwriter: segment polisher (instructional prompt, material merge)"`

---

### Task 6: GhostwriterSession M2 重构（段/润色对齐＋命中＋拼装）

**Files:**
- Modify: `crates/openless-core/src/ghostwriter/session.rs`（在 Task 1 基础上扩）
- Modify: `crates/openless-core/src/ghostwriter/types.rs`

**Interfaces:**
- Produces:
```rust
// types.rs
pub struct GhostwriterConfig { pub polish_enabled: bool }  // Default: polish_enabled=true
#[derive(Debug, Clone)]
pub struct PolishableSegment { pub index: usize, pub prior: String, pub text: String, pub materials: Vec<String> }
#[derive(Debug, Clone)]
pub struct FeedOutcome {
    pub new_segments: Vec<PolishableSegment>,
    pub new_hits: Vec<GhostwriterSnippetHit>,
}
// session.rs
impl GhostwriterSession {
    pub fn new(config: GhostwriterConfig) -> Self
    pub fn feed(&mut self, delta: &TranscriptDelta, snippets: &[Snippet]) -> Result<FeedOutcome, BackendError>
    pub fn apply_polished(&mut self, index: usize, text: String) -> bool   // revision+1；越界/机械模式返回 false
    pub fn apply_tail_polished(&mut self, text: String) -> bool
    pub fn cancel_last_hit(&mut self) -> Option<GhostwriterSnippetHit>  // 撤销最近命中：inline→出待融；footnote→出附注
    pub fn tail_polish_input(&self) -> Option<String>             // segmenter.tail 非空时返回 (它, 待融材料)
    pub fn assembled_text(&self) -> String                        // 预览与最终贴出同源
    pub fn is_empty(&self) -> bool
    pub fn revision(&self) -> u64
    pub fn segments(&self) -> &[Segment]
}
```
**行为规格（测试即规格）：**
1. feed：缓冲更新（Task 1 自适应）→ segmenter.update 完成段入 `segments`（`polished` 同步 push None）→ 对每新段：`polish_enabled` 时产出 PolishableSegment（prior=已润文本尾部 200 chars；materials=从 `inline_pending` 取走全部，若 pending 为空则空）；`polish_enabled=false` 时不产出段（机械模式）。
2. 命中：对**新完成段文本**与缓冲尾部增量，扫 snippets 的 trigger＋aliases（`contains` 匹配，大小写折叠）；命中且未生效过（会话内 snippet_id 去重）→ Footnote 进 `hits`、Inline 进 `inline_pending` 并记入 `inline_hit_order`；产出 new_hits。
3. apply_polished：`polished[index]=Some(text)`。若该段材料未全融（材料进 prompt 是 dispatcher 职责，session 不管）——材料消耗在 feed 产出时已转移，apply_polished 不动材料。
4. assembled_text：主文本＝`polished[i].unwrap_or(segments[i].text)` join＋`tail_polished.unwrap_or(尾部原文)`；机械模式/兜底时 `inline_pending` 剩余材料以「材料文本」追加在主文本后；附注块＝`hits` 非空时 `"\n\n[附注]\n" + hits.map(|h| format!("- {}：{}", title, text)).join("\n")`（footnote 命中的 text 由 store 提供，session 在命中时存全量）。
5. cancel_last_hit：取 `inline_hit_order`/`hits` 中最近一个生效命中撤销（inline 从 pending 移除，footnote 从 hits 移除）；返回被撤销者供事件确认。
6. X 撤销与命中去重交互：被撤销的 snippet 恢复「未生效」状态（同会话再说到可再生效）。

- [ ] **Step 1: 失败测试**（把 M1 的 6 测试改为新签名；新增）：

```rust
fn snip(id: &str, trigger: &str, mode: SnippetMode) -> Snippet { /* enabled, aliases 空 */ }

#[test]
fn feed_emits_segments_with_prior_and_materials() {
    // feed "你好，帮我翻译一下。" （句末触发断句）+ snippets 含 inline "翻译"
    // → outcome.new_segments.len()==1；materials 含该常用语文本；pending 转移
}
#[test]
fn foot_note_hit_records_once_and_dedupes() {
    // 同 snippet 说到两次 → new_hits 只第一次；assembled 含一条附注
}
#[test]
fn assembled_joins_polished_with_fallback_and_footnote_block() {
    // 两段，第 1 段 apply_polished、第 2 段未润 → 第 2 段用原文；附注块格式 "[附注]\n- 标题：文本"
}
#[test]
fn mechanical_mode_skips_segments_but_keeps_hits_and_inline_append() {
    // config.polish_enabled=false：feed 不产段；inline 命中材料追加在生转写后；footnote 照拼
}
#[test]
fn cancel_last_hit_removes_latest_and_allows_re_hit() { /* 撤销后同 snippet 再说到 → 再次生效 */ }
#[test]
fn revision_increments_on_apply_polished() { /* apply_polished → revision+1；无效 index 不增 */ }
```

- [ ] **Step 2-4:** 实现（全纯逻辑，无 IO 无时钟）→`cargo test -p openless-core ghostwriter::` 全绿。

- [ ] **Step 5: Commit** — `git commit -am "ghostwriter: session v2 (aligned polish, hit dedup, assembled preview)"`

---

### Task 7: api.rs 接线（dispatcher＋feed 升级＋stop 尾段补润）

**Files:**
- Create: `crates/openless-core/src/ghostwriter/dispatcher.rs`
- Modify: `crates/openless-core/src/api.rs`（L1371 MutableState、L2028 构造、L1785 feed 点、L5158 stop 点）
- Modify: `crates/openless-core/src/ghostwriter/mod.rs`

**Interfaces:**
- Consumes: Task 2 的 `context.ghostwriter` 快照；Task 4 SnippetStore；Task 5 SegmentPolisher；Task 6 session v2。
- Produces:
```rust
// dispatcher.rs —— 照 api.rs:1663 BackendEngineProgress 的持有模式
pub struct GhostwriterPolishDispatcher {
    state: Arc<RwLock<crate::api::MutableState>>,   // 与 OpenLessBackend.state 同一 Arc
    events: EventPublisher,                          // 类型照 BackendEngineProgress 的 events 字段
    polisher: Arc<dyn TextPolisher>,
    credential_store: Arc<dyn crate::ports::CredentialStore>,
    active_llm_provider: Arc<std::sync::atomic::AtomicString>, // 若 backend 现有偏好读取路径不同，照 api.rs 读 prefs 的既有方式
}
impl GhostwriterPolishDispatcher {
    pub fn dispatch_segments(&self, session_id: SessionId, segments: Vec<PolishableSegment>)
    // 对每段 tokio::spawn：polish_segment(...) → 成功则 state.write() 中
    // session.apply_polished(index, text) 且 events.publish(GhostwriterPreviewChanged{text: assembled, revision})
    // 失败则 log::warn!("[ghostwriter] segment polish failed: {e}") + events.publish(GhostwriterNotice{level:"error"})
    pub fn dispatch_tail(&self, session_id: SessionId, request: SegmentPolishRequest)
    // stop 前调用：尾段补润 → apply_tail_polished → publish 预览
}
```
- api.rs 改动点（全加法）：
  1. `MutableState` 加 `ghostwriter_dispatcher: Option<Arc<GhostwriterPolishDispatcher>>`、`ghostwriter_snippets: crate::ghostwriter::snippet_store::SnippetStore`（`MutableState` 及 ghostwriter_sessions 字段已是 `pub(crate)` 可见性时不动；若非 pub(crate)，把两者标 `pub(crate)`——加法式可见性放宽，不改签名）；
  2. `new_with_repositories_and_clock`（L2028）：`state.ghostwriter_snippets = SnippetStore::at_data_dir(&data_dir)`（data_dir 取构造现有参数；若构造无 data_dir，照 style_pack_store 在 backend 的接入方式接入——执行时读 `style_pack_store` 如何进 backend，同款照抄）；`ghostwriter_dispatcher = deps.selection_polisher.clone().map(|polisher| Arc::new(GhostwriterPolishDispatcher::new(state_arc_clone, events_clone, polisher, credential_store, ...)))`；
  3. feed 点（L1785）：改为 `feed(&delta, &state.ghostwriter_snippets.enabled())`→拿到 FeedOutcome→`new_hits` 逐条 `events.publish(GhostwriterSnippetsHit)`；`polish_enabled` 且 dispatcher 存在→`dispatcher.dispatch_segments(session_id, outcome.new_segments)`；
  4. stop 点（L5158）：替换前若 session 有 `tail_polish_input` 且 dispatcher 存在→同步 `dispatch_tail`（await 完成，保证贴出前尾段已润）→再取 `assembled_text()`（现有替换逻辑不变，简繁转换照旧在其后）；
  5. `cancel_dictation`/`reset_dictation_session`：现有 `ghostwriter_sessions.remove` 保持（dispatcher 内 spawn 的任务在 apply_polished 时按 session_id 找不到会话即静默丢弃——session 被 remove 后 apply 无害）。

- [ ] **Step 1: 失败集成测试（api.rs `mod tests`，照 `ghostwriter_active_session_skips_polish_and_inserts_assembled_text` L9500 模式）**

```rust
#[tokio::test]
async fn ghostwriter_segments_get_polished_and_preview_event_fires() {
    // backend_with_dictation_engine + FixtureTextPolisher（成功返回"润后文本"）
    // capsule_style=Ghostwriter → start → feed_external_pcm（含句末标点的两段音频/文本序列，用 engine fixture 的转写注定制）
    // 断言1: events 收到 ≥1 次 GhostwriterPreviewChanged 且 text 含 "润后文本"
    // 断言2: stop 后 polished_text 为 assembled（含润后段）
}
#[tokio::test]
async fn ghostwriter_mechanical_mode_keeps_raw_text_and_inline_append() {
    // prefs.ghostwriter.polish_enabled=false；inline 常用语命中
    // 断言: stop 后 polished_text == 生转写＋材料拼接＋footnote 块；零 LLM 段润色调用（fixture 计数）
}
#[tokio::test]
async fn ghostwriter_hit_dedup_and_cancel_command() {
    // 同 snippet 两次 → GhostwriterSnippetsHit 一次；cancel 命令后 assembled 无附注
}
```

- [ ] **Step 2-4:** 实现→`cargo test -p openless-core`（全 crate）绿＋`cargo check -p openless-linux-egui`。

- [ ] **Step 5: Commit** — `git commit -am "ghostwriter: api wiring (dispatcher, snippet store, tail polish)"`

---

### Task 8: Tauri 命令层（snippets CRUD＋cancel）

**Files:**
- Create: `src-tauri/src/commands/ghostwriter.rs`
- Modify: `src-tauri/src/lib.rs`（`tauri::generate_handler!` 宏列表，照 style_pack 命令在 L275-286 的挂法）

**Interfaces:**
- Consumes: CoreState（`commands/mod.rs:113`）→ `core.get_snippet_store()`（api.rs 上 Task 7 需为 OpenLessBackend 暴露 `pub fn snippet_store(&self) -> &SnippetStore`——如果字段在 MutableState 后面读锁获取，签名用 `-> Result<...>` 返回克隆列表更简单：`pub fn list_snippets(&self) -> Vec<Snippet>` 等四个方法转发 store）。
- Produces:
```rust
#[tauri::command] pub fn list_ghostwriter_snippets(core: CoreState<'_>) -> Result<Vec<Snippet>, String>
#[tauri::command] pub fn create_ghostwriter_snippet(core: CoreState<'_>, snippet: Snippet) -> Result<Snippet, String>
#[tauri::command] pub fn save_ghostwriter_snippet(core: CoreState<'_>, snippet: Snippet) -> Result<Snippet, String>
#[tauri::command] pub fn delete_ghostwriter_snippet(core: CoreState<'_>, id: String) -> Result<(), String>
#[tauri::command] pub fn set_ghostwriter_snippet_enabled(core: CoreState<'_>, id: String, enabled: bool) -> Result<(), String>
#[tauri::command] pub fn ghostwriter_cancel_last(core: CoreState<'_>, session_id: String) -> Result<bool, String>
// 全部 .map_err(|e| e.to_string())；ghostwriter_cancel_last 走 MutableState.ghostwriter_sessions.get_mut(&session_id).cancel_last_hit()
```
- 注册进 desktop 宏列表（照 `delete_style_pack` 等 L167-186 的写法）；mobile 宏（L421 起）不加。

- [ ] **Step 1:** 写 5 个命令＋cancel；错误映射 String。core 侧若缺转发方法先补（4 行）。
- [ ] **Step 2:** `cargo check -p openless-core && cargo check`（src-tauri 编译过：`cargo check` 在 `src-tauri/` 下）。
- [ ] **Step 3: Commit** — `git commit -am "ghostwriter: tauri commands (snippet CRUD + cancel last)"`

---

### Task 9: 前端 ipc＋GhostwriterPanel 三区改造

**Files:**
- Create: `src/lib/ipc/ghostwriter.ts`
- Modify: `src/pages/GhostwriterPanel.tsx`、`src/lib/ghostwriterCapsule.ts`
- Test: `src/lib/ghostwriterCapsule.test.ts`

**Interfaces:**
- Consumes: Task 8 命令名；事件 `ghostwriter_preview_changed { text, revision }`、`ghostwriter_snippets_hit { snippetId, title, mode }`、`ghostwriter_notice { message, level }`（backendEvent 同结构：`kind: { type, payload }`）。
- Produces:
```ts
// ipc/ghostwriter.ts —— 照 src/lib/ipc/style-packs.ts 的 invokeOrMock 模式
export function listGhostwriterSnippets(): Promise<Snippet[]>
export function createGhostwriterSnippet(s: Snippet): Promise<Snippet>
export function saveGhostwriterSnippet(s: Snippet): Promise<Snippet>
export function deleteGhostwriterSnippet(id: string): Promise<void>
export function setGhostwriterSnippetEnabled(id: string, enabled: boolean): Promise<void>
export function ghostwriterCancelLast(sessionId: string): Promise<boolean>
// Snippet 类型加进 src/lib/types.ts（camelCase: id/trigger/aliases/text/mode:'inline'|'footnote'/enabled）
```
- GhostwriterPanel 布局改造（保持 M1 显隐机制、showPanel 定位、fallback toast 全部不动）：
  - 卡片内部改三区纵向：**顶区**（命中徽标行：`✓ {title}` pills＋右侧 `✕` 按钮→`ghostwriterCancelLast(sessionId)`；无内容整行不渲染）、**中区指令预览**（`ghostwriter_preview_changed` 最新 text；空时显示「指令预览将在说话后出现」i18n 占位；这是主视觉：fontSize 16-17、主文本色）、**底区转写流**（现有转写文本降为小字 fontSize 13、次级色，保留贴底滚动与光标）。
  - 事件处理：`revision` 比较丢弃旧值；`ghostwriter_notice` level=error → 复用现有 notice 药丸展示 2.5s；preview 收到后区高度自适应（maxHeight 340 内 flex 分配：预览区 flex:1）。
  - fallback toast 与「停止即收起」逻辑保持（ghostwriterCapsule.ghostwriterPanelActionFor 不动）。
- [ ] **Step 1: ghostwriterCapsule.test.ts 加失败测试**

```ts
// ghostwriterPreviewReducer(state, event) 纯函数进 ghostwriterCapsule.ts（node 测试无 DOM）
test('preview revision 丢弃旧值', () => {
  const s1 = ghostwriterPreviewReducer({ text: '', revision: 0 }, { type: 'ghostwriter_preview_changed', payload: { text: 'A', revision: 2 } });
  const s2 = ghostwriterPreviewReducer(s1, { type: 'ghostwriter_preview_changed', payload: { text: 'B', revision: 1 } });
  assert.equal(s2.text, 'A');
});
test('hit 徽标去重按 snippetId', () => { /* 同 id 两次 → 徽标数组长度 1 */ });
```
- [ ] **Step 2-4:** 实现 reducer＋组件改造→`npm test` 绿→`npm run build`（tsc）过。
- [ ] **Step 5: Commit** — `git commit -am "ghostwriter: three-zone panel (hits + preview + transcript), ipc wrappers"`

---

### Task 10: 常用语管理页 GhostwriterSnippets.tsx＋路由

**Files:**
- Create: `src/pages/GhostwriterSnippets.tsx`
- Modify: `src/App.tsx`（主窗口导航＋懒加载，照 Style.tsx 挂载点）、`src/main.tsx`（若 Style 页路由需要 kind 判断则照抄）

**Interfaces:**
- Consumes: `ipc/ghostwriter.ts` 全部函数；`Snippet` 类型。
- Produces: 管理页（照 Style.tsx 骨架简化：`PageHeader`＋Card 列表＋编辑抽屉）：
  - 列表行：trigger＋title 摘要＋「贴进正文/附在文末」tag＋启用 toggle＋删除；
  - 编辑抽屉字段：触发词（必填）、别名（逗号分隔）、表述文本（多行）、贴位（radio：「贴进正文」=inline /「附在文末」=footnote）、启用开关；
  - 空库空态：插图＋一句「存下你调优过的说法，用触发词随叫随到」＋「新建常用语」主按钮；
  - dirty 保护＋beforeunload 拦截照 Style.tsx L338-368 模式。
- [ ] **Step 1:** 实现页面＋路由＋导航项。
- [ ] **Step 2:** `npm run build` 过；手动冒烟（dev 跑起来建一条→重启→还在）。
- [ ] **Step 3: Commit** — `git commit -am "ghostwriter: snippets management page"`

---

### Task 11: i18n（浮框文案＋管理页＋8 门）

**Files:**
- Modify: `src/i18n/zh-CN.ts`、`zh-TW.ts`、`en.ts`、`ja.ts`、`ko.ts`、`es.ts`、`fr.ts`、`de.ts`

**Interfaces:**
- Produces: 新命名空间 `ghostwriter: { panel: {...}, snippets: {...} }`：
  - panel: `listening`(正在聆听…)/`recording`(语音输入中)/`preparing`(正在准备)/`previewPlaceholder`(指令预览…)/`cancelLast`(撤销)/`hitTitle`(命中)/notice 文案；
  - snippets: `title`(常用语)/`modeInline`(贴进正文)/`modeFootnote`(附在文末)/`trigger`(触发词)/`aliases`(别名)/`text`(表述)/`emptyTitle`/`emptyHint`/`create`(新建常用语)/CRUD 提示；
  - settings: `ghostwriterPolishEnabled`(指令化润色)/说明文案（M3 的开关文案一并加上：candidate/recommendation）。
- zh-CN 为源语言先写，其余 7 门照 capsuleStyleFluid 各语言风格翻译（读各文件相邻 key 的语气）。
- [ ] **Step 1:** 8 文件各加 key（zh-CN 先行定稿）。
- [ ] **Step 2:** GhostwriterPanel/GhostwriterSnippets 硬编码文案替换为 `t()`。
- [ ] **Step 3:** `npm run build`＋`npm test` 过。
- [ ] **Step 4: Commit** — `git commit -am "ghostwriter: i18n (panel + snippets page, 8 locales)"`

---

### Task 12: 设置开关 UI＋实机验收清单

**Files:**
- Modify: `src/pages/settings/RecordingInputSection.tsx`（capsule style 选择旁，L430-455 区域加 ghostwriter 分项开关；顺手补 `CapsuleStylePreview` 对 ghostwriter 分支缺失的 fallthrough——探查确认 Minor）
- Modify: `src/lib/ipc/settings.ts` 相关（若 ghostwriter prefs 需新 setter——若 UserPreferences 整体保存已覆盖则零改动，读 `settings.ts` 保存路径确认）

**Interfaces:**
- UI：ghostwriter 样式选中时显示子开关：「指令润色」（关＝机械模式）＋「候选建议」「常用语推荐」（M3 生效，先出 UI）＋灰字说明「关闭指令润色后，浮框将贴出原话并附上命中材料」。

**实机验收清单（macOS，run-macos.sh＋OPENLESS_LOG_LEVEL=debug）：**
1. ghostwriter 样式说话→浮框三区：转写流滚动、润色段完成时预览更新、预览与最终贴出一致（所见即所贴）。
2. 管理页建 inline 常用语「翻译」→ 说到触发词→徽标「✓ 翻译」出现→预览含融合材料→贴出文本含融合。
3. footnote 常用语→文末附注块出现在贴出文本。
4. 「✕」→ 最近命中撤销，贴出不含该材料；同 snippet 再说到可再生效。
5. 重复说到同一条→只生效一次（徽标只出现一次）。
6. 机械模式（关指令润色）→贴出生转写＋inline 文末拼接＋footnote 照拼；零段润色 LLM 调用（debug 日志确认）。
7. 管理页 CRUD＋重启持久化（ghostwriter-snippets.json）。
8. 秒停（无内容）→走上游原路径不炸。
9. `cargo test -p openless-core` 全绿＋`cargo check -p openless-linux-egui` 过＋`npm test`＋`npm run build` 过。
10. rebase 上游 beta 验证冲突面：`git fetch origin && git rebase origin/beta`（冲突预期仅在 events.rs/shared_types.rs/api.rs 的加法区）。

- [ ] **Step 1-9:** 按清单逐条实机验证，全部通过。
- [ ] **Step 10: Commit＋推送** — `git push origin HEAD`（SSH）。

---

## 后续段（M3/M4，本段验收后另出计划）

M3＝候选流（hesitation 检测→LLM 三类候选→口头「用候选二」解析）＋推荐流（LLM 从库挑选→同区展示→「用常用语二」）＋待融队列候选融合＋沉淀（推荐流顺带判重复模式→一键存）。M4＝纯净模式快捷开关＋历史扩展＋节流参数生效。M2 落地后接口（事件桥、invoke 模式、dispatcher）都已定型，届时按同一格式细化。
