# Ghostwriter M3/M4 实施计划（候选流＋推荐流＋沉淀＋任务书＋Ghostwriter 视图＋历史扩展）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 M2 的指令台升级为三流齐备：说话中 LLM 自判卡词出候选＋扫库推荐＋跨会话沉淀提醒（确认后入库），任务书（提示词正文）可查看编辑，自有内容收进一个「Ghostwriter」视图，M4 给历史补明细。

**Architecture:** 核心新模块全在 `openless-core/src/ghostwriter/`（任务书注册表/覆写存储、实时助手 assist 调用、重复档存储、沉淀抽取）；session 保持纯逻辑（时钟与 LLM 都在 api/dispatcher 侧）；段润色/assist/抽取三条 LLM 链全走 `resolve_session_provider + DictationContext::capture + polisher.polish + DiscardTextStream` 既有模式；前端新「Ghostwriter」视图（三页签）收编常用语/任务书/设置，浮框加候选区。

**Tech Stack:** Rust (tokio, serde, uuid 已有) ＋ Tauri 2 ＋ React/TS（无 vitest：自定义 assert＋tsx，`npm test`）。

**Spec:** `docs/design/ghostwriter/requirements-m3m4.md`（执行者必须先读）；界面契约 `docs/design/ghostwriter/ui-m3m4.md`；词汇 `CONTEXT.md`；纪律 `docs/adr/0001`、`docs/adr/0002`。

## Global Constraints

- 内部代号 `ghostwriter`；显示名「Ghostwriter 流式浮框」8 门 i18n 已定案勿改；capsuleStyle 值 `"fluid"` 不在改名范围。
- **润色常开：机械模式已砍**。`polish_enabled` 字段、开关、分支、测试一律移除，勿再引用；`design.md` 相应条目随 Task 1 订正。
- 低侵入三纪律（ADR 0001）：宿主界面每处最多一枚入口；入口统一带「Ghostwriter」名称＋同一图标；自有内容集中在「Ghostwriter」视图（页签：常用语／任务书／设置）。
- 任务书边界（ADR 0002）：可编辑的只有任务正文；数据注入与输出结构由代码固定（常量拼接，界面只读展示）；覆写存 `data_dir/ghostwriter-prompts.json`，缺省＝代码内置默认。
- 指令预览与 `GhostwriterSession::assembled_text()` 同源；口头命令「浮着才认、失败不剔」；沉淀确认后才入库；候选上限 ≤2 组、每组 ≤5、总 ≤8；推荐 ≤3 条。
- LLM 调用模式照抄 `segment_polisher.rs:47-96`：`resolve_session_provider(credential_store, ProviderSlot::Llm, active_llm_provider)` → `DictationContext::capture` 覆写 → `polisher.polish(..., DiscardTextStream)`；provider/凭据解析不新造。
- 同步上游一律 rebase（弃 merge）；推送用 SSH。
- ghostwriter 窗口属性不动（560×420、focus:false、透明无边框、alwaysOnTop）。
- 旧文件只做加法式小改，不改既有 trait 签名；每任务后 `cargo check -p openless-core`、`cargo check -p openless-linux-egui` 必须过。
- 测试：core 单测内嵌 `#[cfg(test)] mod tests`；api 集成测试用 `crate::testing::Fixture*`＋`backend_with_dictation_engine` 模式（**注意**：同一个 FixtureTextPolisher 同时服务段润色与 assist，按 session_id 前缀路由输出，见 Task 3）；前端 node＋tsx 无 DOM。
- 事件 payload camelCase、枚举 tag snake_case（`events.rs:363` 既有规则）。

## File Structure

**openless-core/src/ghostwriter/**（`mod.rs` 加声明）：

| 文件 | 职责 |
|---|---|
| `prompts.rs`【新建】 | 任务书注册表：TaskBriefId 枚举、五份默认正文（含迁来的 `GHOSTWRITER_INSTRUCTION_PROMPT`）、标题/说明、ASSIST_OUTPUT_CONTRACT 固定输出契约 |
| `task_brief_store.rs`【新建】 | 覆写存储：`data_dir/ghostwriter-prompts.json`，Mutex 内存态＋原子写（照 `snippet_store.rs` 模式） |
| `assist.rs`【新建】 | 实时助手 LLM 调用：拼 prompt（任务书正文×3＋固定契约＋内容/库/重复档注入）→ 解析 JSON → AssistOutcome |
| `recurrence_store.rs`【新建】 | 重复档：`data_dir/ghostwriter-sediment.json`，说法/次数/例句/已提示，归一化合并 |
| `sediment_extractor.rs`【新建】 | 会话后抽取：final_text → 0-3 条可复用说法（JSON） |
| `session.rs`（扩展） | 批次视图＋口头命令识别剔除＋选中队列＋cancel_last_action＋assist 快照 |
| `types.rs`（扩展） | Selection/SelectionKind/LiveRecommendation/AssistSnapshot/事件 payload/删 GhostwriterConfig |
| `dispatcher.rs`（扩展） | assist 触发（静默/句毕＋节流＋在飞）、推荐节流、抽取触发、任务书正文接线 |

**修改**：`events.rs`（+GhostwriterAssistChanged）、`api.rs`（静默计时、触发点、抽取点、命令转发、snapshot 记录）、`shared_types.rs`（删 polish_enabled）、`dictation_context.rs`（快照瘦身）、`src-tauri/src/commands/ghostwriter.rs`、`src-tauri/src/lib.rs`、前端 `src/lib/ipc/ghostwriter.ts`、`src/lib/ghostwriterCapsule.ts(.test)`、`src/pages/GhostwriterPanel.tsx`、`src/pages/GhostwriterView.tsx`【新建】、`src/pages/ghostwriter/TaskBriefsPane.tsx`【新建】、`src/pages/ghostwriter/SettingsPane.tsx`【新建】、`src/pages/GhostwriterSnippets.tsx`（嵌入化）、`src/pages/settings/RecordingInputSection.tsx`（三开关→按钮）、`src/components/FloatingShell.tsx`、`src/components/MobileMoreSheet.tsx`、`src/pages/History.tsx`、`src/lib/types.ts`、`src/lib/ipc/mock-data.ts`、`src/i18n/*.ts`（8 门）、`docs/design/ghostwriter/design.md`（订正）。

---

### Task 1: 机械模式清理（润色常开）

**Files:**
- Modify: `crates/openless-core/src/shared_types.rs`（GhostwriterPreferences `polish_enabled` 字段 :328、Default :339 附近、内嵌测试 :3436/:3452/:3461；`UserPreferencesWire` :793 起字段映射、Default :1029、Deserialize 映射 :1184）
- Modify: `crates/openless-core/src/dictation_context.rs`（GhostwriterSnapshot :6-11 瘦身、capture :302-304）
- Modify: `crates/openless-core/src/ghostwriter/types.rs`（删 GhostwriterConfig :14-23）
- Modify: `crates/openless-core/src/ghostwriter/session.rs`（删 config 字段与全部分支；`new()` 无参）
- Modify: `crates/openless-core/src/ghostwriter/dispatcher.rs`、`crates/openless-core/src/api.rs`（:4827-4832 改 `GhostwriterSession::new()`；删相关引用）
- Modify: `src/lib/types.ts`（删 polishEnabled :330 附近）、`src/lib/ipc/mock-data.ts`（:50-55）、`src/pages/settings/RecordingInputSection.tsx`（删「指令化润色」开关行 :476-481 与 mechanicalHint 块 :494-504；**候选/推荐两行暂留**，Task 9 再迁）
- Modify: `src/i18n/*.ts` 8 门（删 `settings.ghostwriter.ghostwriterPolishEnabled`、`ghostwriterMechanicalHint`）
- Modify: `docs/design/ghostwriter/design.md`（:30 三流开关语义句改为「润色常开」；:73-74 已拍板偏好删「纯净模式一键关」）
- Test: 删 `session.rs` 内 `mechanical_mode_skips_segments_but_keeps_hits_and_inline_append`；删 api.rs 内 `ghostwriter_mechanical_mode_keeps_raw_text_and_inline_append`

**Interfaces:**
- Produces: `GhostwriterSession::new()`（无参）；`GhostwriterPreferences` 仅剩 candidates_enabled/recommendations_enabled/candidate_throttle_ms/recommendation_throttle_ms；`GhostwriterSnapshot { active: bool }`。
- 行为：feed 恒产出 PolishableSegment（不再判 polish_enabled）；`assembled_text` 的「未消化材料文末追加」保留（转为润色失败兜底，行为不变）。

- [ ] **Step 1:** 先删测试（session 机械测试、api 集成机械测试），跑 `cargo test -p openless-core ghostwriter` 确认其余编译失败（引用 polish_enabled 处）。
- [ ] **Step 2:** 按 Files 清单逐处删除（prefs 三处 wire 同步删，缺一处旧配置字段会被静默丢弃——照 `UserPreferencesWire` 相邻字段写法删干净）。
- [ ] **Step 3:** 前端删字段/开关/hint/i18n key（8 门同步）。
- [ ] **Step 4:** `cargo test -p openless-core` 全绿＋`cargo check -p openless-linux-egui`＋`npm test`＋`npm run build` 过。
- [ ] **Step 5:** design.md 订正后 Commit：`git commit -am "ghostwriter: remove mechanical mode (polish always on)"`

---

### Task 2: 任务书注册表＋覆写存储

**Files:**
- Create: `crates/openless-core/src/ghostwriter/prompts.rs`
- Create: `crates/openless-core/src/ghostwriter/task_brief_store.rs`
- Modify: `crates/openless-core/src/ghostwriter/mod.rs`、`segment_polisher.rs`（GHOSTWRITER_INSTRUCTION_PROMPT 迁往 prompts.rs，本任务先保留 re-export 兼容）

**Interfaces:**
- Produces:
```rust
// prompts.rs
pub enum TaskBriefId { InstructionPolish, Candidates, Recommendations, SedimentNotice, SedimentExtraction }
impl TaskBriefId {
    pub fn key(self) -> &'static str;        // "instruction_polish" | "candidates" | "recommendations" | "sediment_notice" | "sediment_extraction"
    pub fn title(self) -> &'static str;      // 指令化润色／候选生成／推荐挑选／沉淀提醒／沉淀抽取
    pub fn description(self) -> &'static str; // 每份一句"管什么/改了会怎样"（中文，ui 文案同步用）
    pub fn default_body(self) -> &'static str;
}
pub const ASSIST_OUTPUT_CONTRACT: &str = "只输出 JSON，不要任何解释或代码块标记：\n{\"candidateGroups\":[{\"kind\":\"term|phrase|naming\",\"items\":[\"候选文本\",…]}],\"recommendations\":[\"常用语id\",…],\"sediment\":{\"phrase\":\"说法\",\"count\":出现次数,\"suggestedTrigger\":\"触发词\"}}\n没有的键给空数组或 null；候选最多 2 组、每组最多 5 条、总数最多 8 条；推荐最多 3 个 id；kind：term=精准词、phrase=候选表述、naming=命名。";
// Candidates/Recommendations/SedimentNotice 默认正文在本任务写初稿（中文，语义见 requirements 三.1/三.4；Candidates≈"判断说话人是否卡词…给出更准的词/整理表述/命名建议"；Recommendations≈"从库中挑与当前内容相关的常用语…"；SedimentNotice≈"判断当前内容是否在重复某个未入库说法…"）
```
```rust
// task_brief_store.rs
pub struct TaskBriefInfo { pub id: String, pub title: String, pub description: String, pub modified: bool, pub body: String }
pub struct TaskBriefStore { /* path: Option<PathBuf>, state: Mutex<HashMap<String,String>> */ }
impl TaskBriefStore {
    pub fn at_data_dir(dir: &std::path::Path) -> Self  // dir/ghostwriter-prompts.json，缺文件=空覆写
    pub fn in_memory() -> Self
    pub fn body(&self, id: TaskBriefId) -> String      // 覆写优先，否则 default_body
    pub fn set_body(&self, id: TaskBriefId, body: &str) -> Result<TaskBriefInfo, BackendError> // trim 非空，否则 InvalidArgument；写盘照 snippet_store.rs 持久化模式
    pub fn reset(&self, id: TaskBriefId) -> TaskBriefInfo
    pub fn is_modified(&self, id: TaskBriefId) -> bool
    pub fn list(&self) -> Vec<TaskBriefInfo>           // 固定 5 份顺序
}
```

- [ ] **Step 1: 失败测试**（两文件内嵌）：`set_body_persists_and_reports_modified`（in_memory：set → body=覆写、modified=true、list 标记；重开 at_data_dir(temp) 读回）、`blank_body_rejected`、`reset_restores_default`、`list_returns_five_in_fixed_order`。
- [ ] **Step 2:** 跑 `cargo test -p openless-core ghostwriter::task_brief` 确认失败。
- [ ] **Step 3:** 实现（照 `snippet_store.rs` 的 Mutex＋atomic_write＋read_or_default 模式）。
- [ ] **Step 4:** 测试过＋两 cargo check 过。Commit：`git commit -am "ghostwriter: task brief registry + override store"`

---

### Task 3: 实时助手调用（assist）

**Files:**
- Create: `crates/openless-core/src/ghostwriter/assist.rs`
- Modify: `crates/openless-core/src/ghostwriter/mod.rs`
- Test: `crates/openless-core/src/testing.rs`（如 FixtureTextPolisher 需按 session_id 前缀路由输出，扩展之；先读 :632 附近确认捕获接口）

**Interfaces:**
- Consumes: Task 2 的正文与契约；`segment_polisher.rs:47-96` 的 LLM 调用模式（照抄，含 `DiscardTextStream`）。
- Produces:
```rust
pub struct RecurrenceSummary { pub phrase: String, pub count: u32 }
pub struct SedimentMatch { pub phrase: String, pub count: u32, pub suggested_trigger: String }
pub struct AssistInput {
    pub session_id: SessionId,          // 固定前缀 "ghostwriter-assist-"（fixture 路由依据）
    pub context_text: String,           // 说话缓冲尾部 ≤800 chars（截取调用方做）
    pub snippets: Vec<Snippet>,         // enabled
    pub recurrence: Vec<RecurrenceSummary>, // ≤20，仅未 prompted
    pub include_candidates: bool,
    pub include_recommendations: bool,
    pub instruction_candidates: String,
    pub instruction_recommendations: String,
    pub instruction_sediment: String,
}
pub struct AssistOutcome {
    pub candidate_groups: Vec<(String, Vec<String>)>, // (kind: "term"|"phrase"|"naming", texts)
    pub recommendation_ids: Vec<String>,
    pub sediment: Option<SedimentMatch>,
}
pub async fn run_assist(
    polisher: &Arc<dyn TextPolisher>,
    credential_store: &Arc<dyn CredentialStore>,   // 类型以 segment_polisher.rs 实际签名为准
    active_llm_provider: &str,
    input: &AssistInput,
) -> Result<AssistOutcome, BackendError>
```
- system prompt 拼装：`instruction_candidates`（include_candidates 时）＋`instruction_recommendations`（include_recommendations 时）＋`instruction_sediment`＋`ASSIST_OUTPUT_CONTRACT`；user 输入＝`当前内容：{context_text}`＋`常用语库（id|触发词|标题|文本）：…`＋`重复档：{phrase}×{count} …`；其余 context 覆写照 segment_polisher（mode=Light、清 hotwords/prior/cursor、关翻译、style_system_prompt=拼接结果）。
- JSON 解析：`serde_json::from_str`；失败/空 → 返回空 Outcome（不报错，log::warn）。候选组上限裁剪（≤2 组、每组 ≤5、总 ≤8）、推荐 ≤3 在解析侧强制。
- 空 outcome 也是合法返回（LLM 判定"没什么可给"）。

- [ ] **Step 1: 失败测试**（assist.rs 内嵌，FixtureTextPolisher 按前缀路由）：`assist_parses_canned_json`（fixture 对 "ghostwriter-assist" 前缀返回契约样例 JSON → 断言 groups/ids/sediment、超限裁剪）、`assist_invalid_json_yields_empty_outcome`、`assist_prompt_contains_bodies_library_and_recurrence`（捕获 context.polish.style_system_prompt 与 raw 文本断言）。
- [ ] **Step 2:** 跑 `cargo test -p openless-core ghostwriter::assist` 确认失败。
- [ ] **Step 3:** 实现。
- [ ] **Step 4:** 测试过＋两 cargo check。Commit：`git commit -am "ghostwriter: assist LLM call (fused candidates/recommendations/sediment)"`

---

### Task 4: 重复档存储＋沉淀抽取

**Files:**
- Create: `crates/openless-core/src/ghostwriter/recurrence_store.rs`
- Create: `crates/openless-core/src/ghostwriter/sediment_extractor.rs`
- Modify: `crates/openless-core/src/ghostwriter/mod.rs`

**Interfaces:**
- Produces:
```rust
// recurrence_store.rs
#[derive(Serialize, Deserialize)] pub struct RecurrenceEntry { pub phrase: String, pub count: u32, pub last_example: String, pub prompted: bool }
pub struct RecurrenceStore { /* path: Option<PathBuf>, state: Mutex<Vec<RecurrenceEntry>> */ }
impl RecurrenceStore {
    pub fn at_data_dir(dir: &std::path::Path) -> Self  // dir/ghostwriter-sediment.json
    pub fn in_memory() -> Self
    pub fn apply_extraction(&self, phrases: Vec<(String, String)>) // (说法, 例句)；归一化(trim+连续空白折叠)合并，count+=1；新条目追加；总量 cap 50（超限淘汰 count 最小、最旧）
    pub fn summary(&self, limit: usize) -> Vec<RecurrenceSummary>  // 仅未 prompted，按 count 降序
    pub fn mark_prompted(&self, phrase: &str)
    pub fn mark_saved(&self, phrase: &str)   // 从重复档移除（已入库不再提）
    pub fn pending_matches(&self) -> Vec<RecurrenceEntry> // 测试/调试用
}
```
```rust
// sediment_extractor.rs
pub async fn extract_phrases(
    polisher: &Arc<dyn TextPolisher>,
    credential_store: &Arc<dyn CredentialStore>,
    active_llm_provider: &str,
    session_id: SessionId,          // 前缀 "ghostwriter-extract-"（fixture 路由）
    final_text: &str,
    instruction_body: &str,         // TaskBriefId::SedimentExtraction 正文
) -> Result<Vec<(String, String)>, BackendError>
// 输出契约（固定，拼在 system 末尾）："只输出 JSON 数组，至多 3 项：[{\"phrase\":\"可复用说法\",\"example\":\"原话例句\"}]；没有就输出 []。挑长期可能重复的背景/偏好/约束类说法，忽略一次性内容。"
// 解析失败→空 Vec；空 final_text→直接返回空
```

- [ ] **Step 1: 失败测试**：store——`extraction_merges_normalized_and_counts`、`summary_excludes_prompted_and_sorts_by_count`、`mark_saved_removes`、`cap_fifty_evicts_lowest_count`、`at_data_dir_roundtrip`；extractor——`extract_parses_canned_array`、`extract_invalid_json_yields_empty`。
- [ ] **Step 2:** 跑 `cargo test -p openless-core ghostwriter::recurrence ghostwriter::sediment` 确认失败。
- [ ] **Step 3:** 实现（store 照 snippet_store 持久化模式；extractor 照 assist 的调用模式）。
- [ ] **Step 4:** 测试过＋两 cargo check。Commit：`git commit -am "ghostwriter: recurrence store + sediment extraction"`

---

### Task 5: session v3（批次视图＋口头选＋选中队列＋撤销扩展）

**Files:**
- Modify: `crates/openless-core/src/ghostwriter/types.rs`、`session.rs`
- Test: `session.rs` 内嵌 tests

**Interfaces:**
- Produces:
```rust
// types.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum SelectionKind { Candidate, Recommendation }
#[derive(Debug, Clone)] pub struct LiveRecommendation { pub snippet_id: String, pub title: String, pub text: String }
#[derive(Debug, Clone)] pub struct Selection { pub kind: SelectionKind, pub index: usize, pub snippet_id: Option<String>, pub text: String } // index 1-based
pub struct CandidateGroupView { pub kind: String, pub items: Vec<CandidateItemView> }       // kind: "term"|"phrase"|"naming"
pub struct CandidateItemView { pub index: usize, pub text: String, pub selected: bool }
pub struct RecommendationView { pub snippet_id: String, pub title: String, pub selected: bool }
pub struct AssistSnapshot { pub candidate_groups: Vec<CandidateGroupView>, pub recommendations: Vec<RecommendationView> }
// FeedOutcome 增字段： pub new_selections: Vec<Selection>
```
```rust
// session.rs
pub fn set_live_batch(&mut self, candidates: Vec<Vec<String>>, recommendations: Vec<LiveRecommendation>)
    // 替换式：新批次到达即清 active selections（已入待融的材料不受影响）
pub fn assist_snapshot(&self) -> Option<AssistSnapshot>   // 无批次 → None
pub fn toggle_selection(&mut self, kind: SelectionKind, index: usize) -> Option<Selection>
    // 未选中→选中：文本进待融队列（同 inline 命中路径）、记 active；已选中→取消：从待融按文本 rposition 移除；越界→None
pub fn cancel_last_action(&mut self) -> Option<LastAction>  // 替代 cancel_last_hit
pub enum LastAction { Hit(GhostwriterSnippetHit), Selection(Selection) } // types.rs
```
- **口头命令识别**（feed 内部，新私有 `scan_commands`）：对**新完成段文本**与**缓冲尾部新增量**扫描「用候选N」/「用常用语N」（N 支持中文数字一二两三四五六七八与 ASCII 1-8，助手函数 `parse_command_number`）；有 live 批次且序号可解析命中 → 从文本剔除该短语及紧邻的一个标点（，。、）；段文本剔除发生在产出 PolishableSegment **之前**；随后走 `toggle_selection`（已选中则仅剔除不重复入队）；产出 `new_selections`。**无可解析批次/序号越界→不剔除、当普通话**。
- 既有 `cancel_last_hit` 更名为 `cancel_last_action`（hits 与 selections 混合按生效时间取最新；撤销 Selection 时从待融移除并标记未选中）；`inline_hit_order` 扩展为统一 action 序（或并列一列，取最新语义不变）。

- [ ] **Step 1: 失败测试**（session.rs 内嵌）：
  - `voice_command_strips_from_tail_and_selects`（set_live_batch 后 feed "……用候选2"，断言 buffer 无"用候选2"、new_selections 含文本、待融含该文本）
  - `voice_command_in_segment_strips_before_polish`（段完成含"用候选二"→ PolishableSegment.text 不含、selection 产出）
  - `unresolvable_command_keeps_text`（无批次 feed "用候选二" → buffer 原样）
  - `out_of_range_command_keeps_text`（批次 2 条，说"用候选五"→ 原样）
  - `toggle_selection_adds_and_removes_material`
  - `new_batch_clears_selections_but_keeps_materials`
  - `cancel_last_action_mixed_order`（命中→选中→取消 返回 Selection；再取消返回 Hit）
- [ ] **Step 2:** 跑 `cargo test -p openless-core ghostwriter::session` 确认新测试失败。
- [ ] **Step 3:** 实现（`match_snippet` :430 旁加命令扫描；中文数字映射写死 8 个）。
- [ ] **Step 4:** 全部过＋既有 session 测试不回归＋两 cargo check。Commit：`git commit -am "ghostwriter: session v3 (live batch, voice select, selections, unified cancel)"`

---

### Task 6: dispatcher 扩展（触发＋节流＋在飞＋抽取＋任务书接线）

**Files:**
- Modify: `crates/openless-core/src/ghostwriter/dispatcher.rs`、`crates/openless-core/src/ghostwriter/types.rs`（事件 payload）、`crates/openless-core/src/events.rs`（:395-397 处加变体）
- Modify: `crates/openless-core/src/api.rs`（构造处 :2290-2304 传入新依赖；MutableState :1372 附近加 `ghostwriter_last_delta: std::collections::HashMap<crate::types::SessionId, std::time::Instant>`）

**Interfaces:**
- Consumes: Task 2/3/4/5 全部产出；backend 的 `clock: Arc<dyn Clock>`（api.rs:1865 已有）。
- Produces（dispatcher 新增/变更）:
```rust
pub struct GhostwriterPolishDispatcher {
    // 既有字段保留；新增：
    task_briefs: Arc<TaskBriefStore>,
    recurrence: Arc<RecurrenceStore>,
    clock: Arc<dyn Clock>,
    assist_in_flight: std::sync::atomic::AtomicBool,
    last_assist: Mutex<Option<std::time::Instant>>,
    last_rec: Mutex<Option<std::time::Instant>>,
    suggestion: Mutex<Option<(crate::types::SessionId, crate::ghostwriter::types::GhostwriterSedimentSuggestion)>>,
}
pub const ASSIST_PAUSE_MS: u64 = 1500;
pub const ASSIST_CONTEXT_CHARS: usize = 800;
// types.rs 事件 payload（camelCase；Task 7 命令与前端直接消费）
pub struct GhostwriterAssistChanged { pub candidate_groups: Vec<GhostwriterCandidateGroup>, pub recommendations: Vec<GhostwriterRecommendationItem>, pub sediment: Option<GhostwriterSedimentSuggestion> }
pub struct GhostwriterCandidateGroup { pub kind: String, pub items: Vec<GhostwriterCandidateItem> } // kind: "term"|"phrase"|"naming"
pub struct GhostwriterCandidateItem { pub index: u32, pub text: String, pub selected: bool }
pub struct GhostwriterRecommendationItem { pub snippet_id: String, pub title: String, pub selected: bool }
pub struct GhostwriterSedimentSuggestion { pub phrase: String, pub count: u32, pub suggested_trigger: String }
// events.rs 枚举加变体： GhostwriterAssistChanged(crate::ghostwriter::types::GhostwriterAssistChanged) → wire 名 ghostwriter_assist_changed
impl GhostwriterPolishDispatcher {
    pub fn maybe_trigger_assist(&self, session_id: &SessionId, reason: AssistTrigger) // AssistTrigger { Pause, SegmentEnd }
        // 1) prefs 门：candidates_enabled || recommendations_enabled，全关直接返回
        // 2) 节流：clock.now() - *last_assist >= candidate_throttle_ms 才继续
        // 3) 在飞：assist_in_flight.swap(true) 已 true → 返回
        // 4) spawn：锁 state 读 session（debug_text 尾部 800 chars）+ snippets store enabled + recurrence.summary(20) + 任务书正文（task_briefs.body(Candidates/Recommendations/SedimentNotice) 填 AssistInput）
        //    include_recommendations = 首次 || clock.now()-*last_rec >= recommendation_throttle_ms
        //    推荐缓存：include_recommendations=false 时用 dispatcher 缓存的上次推荐填 set_live_batch（推荐行不闪失）；true 时更新缓存
        //    run_assist(...) → 成功：锁 state 写 session.set_live_batch(...)；sediment 命中→recurrence.mark_prompted + suggestion 锁写入
        //    publish GhostwriterAssistChanged（快照来自 session.assist_snapshot()，无批次→空数组+suggestion）；失败 publish GhostwriterNotice{level:"error"} 轻提示＋log::warn
        //    收尾：last_assist/last_rec 更新、in_flight=false
    pub fn refresh_assist_event(&self, session_id: &SessionId) // 按 session 当前快照重发 GhostwriterAssistChanged（toggle/cancel/save/dismiss 后调）
    pub fn trigger_extraction(&self, session_id: &SessionId, final_text: &str)
        // prefs 门同上；spawn：extract_phrases(正文取 task_briefs.body(SedimentExtraction)) → recurrence.apply_extraction；失败静默
    pub fn take_suggestion(&self, session_id: &SessionId) -> Option<GhostwriterSedimentSuggestion>
}
```
- `dispatch_segments`（:59-87）改：instruction 正文从 `task_briefs.body(TaskBriefId::InstructionPolish)` 取，填进 `SegmentPolishRequest`（该 struct 加 `pub instruction: String` 字段，Task 2 的存储就位后接上）。
- 会话已删/静默丢弃语义保持（apply_polished 找不到会话即丢）。

- [ ] **Step 1: 失败集成测试**（api.rs mod tests，照 `ghostwriter_segments_get_polished_and_preview_event_fires` 模式；FixtureTextPolisher 按前缀路由：`ghostwriter-assist` → canned JSON，其余 → 润色文本）：
  - `ghostwriter_assist_fires_on_segment_end`（capsule_style=Fluid、candidates/recommendations 开、喂含句末标点的两段 → 事件流出现 `ghostwriter_assist_changed` 且 candidate_groups 非空）
  - `ghostwriter_assist_skips_when_both_switches_off`
- [ ] **Step 2:** 跑 `cargo test -p openless-core ghostwriter_assist` 确认失败。
- [ ] **Step 3:** 实现（Clock 从 `new_with_repositories_and_clock` 的 clock 参数克隆给 dispatcher）。
- [ ] **Step 4:** 测试过＋全 crate 测试＋两 cargo check。Commit：`git commit -am "ghostwriter: dispatcher assist trigger, throttle, extraction"`

---

### Task 7: 事件变体＋api 接线＋Tauri 命令

**Files:**
- Modify: `crates/openless-core/src/api.rs`（feed 点 :1765-1831 加静默计时与触发；stop 点 :5236-5264 后加 `trigger_extraction`；cancel :5787-5817 换 cancel_last_action；toggle/save/dismiss 转发；`pub fn task_brief_store()`/`pub fn recurrence_store()` 访问器）
- Modify: `src-tauri/src/commands/ghostwriter.rs`、`src-tauri/src/lib.rs`（注册 :289-294；mobile 宏不加）

**Interfaces:**
- Consumes: Task 6 的 `GhostwriterAssistChanged` payload 与事件变体、dispatcher 的 `maybe_trigger_assist/refresh_assist_event/trigger_extraction/take_suggestion`、Task 5 的 `cancel_last_action/toggle_selection`。
- api 行为：
  1. feed 点：TranscriptDelta 臂里，`clock` 取 now，与 `state.ghostwriter_last_delta` 上一值比 → 间隔 ≥ASSIST_PAUSE_MS → `dispatcher.maybe_trigger_assist(Pause)`；`outcome.new_segments` 非空 → `maybe_trigger_assist(SegmentEnd)`；`outcome.new_selections` 非空 → 刷新 `ghostwriter_assist_changed`（锁外发布，照既有 hits 发布 :1802-1807 模式）。
  2. stop 点：assembled 计算后、persist 前，若 ghostwriter 会话存在 → `dispatcher.trigger_extraction(session_id, &final_text)`（fire-and-forget）。
  3. `ghostwriter_cancel_last`：session 换 `cancel_last_action`；响应 payload 加 `action: String`（"hit"|"selection"|"none"）；为 selection 时 `refresh_assist_event`。
  4. 新命令转发：`ghostwriter_toggle_selection(session_id, kind /*"candidate"|"recommendation"*/, index)` → session.toggle_selection → `refresh_assist_event`；`ghostwriter_save_suggestion(session_id)` → take_suggestion → `snippet_store.create(Snippet{ trigger: suggested_trigger, text: phrase, mode: Inline, enabled: true, id: 空, aliases: 空 })`（trigger 重复 → Err 原样返回，前端提示）→ `recurrence.mark_saved` → `refresh_assist_event`；`ghostwriter_dismiss_suggestion(session_id)` → `recurrence.mark_prompted` → 清 suggestion → `refresh_assist_event`。
  5. 任务书命令：`list_ghostwriter_task_briefs() -> Vec<TaskBriefInfo>`、`save_ghostwriter_task_brief(id, body) -> TaskBriefInfo`、`reset_ghostwriter_task_brief(id) -> TaskBriefInfo`（均 `.map_err(|e| e.to_string())`）。
- [ ] **Step 1: 失败集成测试**：
  - `ghostwriter_voice_command_selects_and_merges`（feed 含「用候选1」＋canned assist JSON 已就位 → assembled 含候选文本、命令短语被剔除）
  - `ghostwriter_toggle_selection_command_roundtrip`（命令选中 → assist 事件 selected=true → assembled 含材料 → 再 toggle 取消 → assembled 不含）
  - `ghostwriter_save_suggestion_creates_snippet`（assist canned JSON 带 sediment → save 命令 → list_ghostwriter_snippets 含新条目、重复档不再含）
  - `ghostwriter_stop_triggers_extraction`（stop 后 FixtureTextPolisher 收到 "ghostwriter-extract" 前缀调用）
- [ ] **Step 2:** 确认失败 → **Step 3:** 实现 → **Step 4:** `cargo test -p openless-core` 全绿＋两 cargo check。Commit：`git commit -am "ghostwriter: assist event, api wiring, tauri commands (select/suggest/briefs)"`

---

### Task 8: 前端 assist 状态＋候选区 UI

**Files:**
- Modify: `src/lib/ipc/ghostwriter.ts`（+toggleSelection/saveSuggestion/dismissSuggestion、类型）、`src/lib/types.ts`（Assist 类型）
- Modify: `src/lib/ghostwriterCapsule.ts(.test)`（assist reducer）、`src/pages/GhostwriterPanel.tsx`（候选区）

**Interfaces:**
- Consumes: Task 7 事件 `ghostwriter_assist_changed` 与命令。
- Produces:
```ts
// types.ts
export type GhostwriterCandidateKind = 'term' | 'phrase' | 'naming';
export interface GhostwriterCandidateItem { index: number; text: string; selected: boolean }
export interface GhostwriterCandidateGroup { kind: GhostwriterCandidateKind; items: GhostwriterCandidateItem[] }
export interface GhostwriterRecommendationItem { snippetId: string; title: string; selected: boolean }
export interface GhostwriterSedimentSuggestion { phrase: string; count: number; suggestedTrigger: string }
export interface GhostwriterAssistState { candidateGroups: GhostwriterCandidateGroup[]; recommendations: GhostwriterRecommendationItem[]; sediment: GhostwriterSedimentSuggestion | null }
// ghostwriterCapsule.ts
export function ghostwriterAssistReducer(state: GhostwriterAssistState, event: BackendEvent): GhostwriterAssistState
// 只认 ghostwriter_assist_changed：整体替换（无 revision，事件总线保序）；未知事件原样返回
```
- GhostwriterPanel（照 ui-m3m4.md §1）：
  - 候选区插在命中徽标行与预览区之间；三层：沉淀提醒条（「「{phrase}」最近说过 {count} 次」＋存为常用语→`saveSuggestion`＋✕→`dismissSuggestion`）、候选组（类型标签 i18n + chips `序号·文本`，点击→`toggleSelection('candidate', index)`，selected 高亮，选中 chip 尾部小[存]→ 调用现成命令 `saveGhostwriterSnippet` 新建 `{trigger: text, text, mode:'inline', enabled:true}`，重复 trigger 时 notice 提示）、推荐行（点击→`toggleSelection('recommendation', index)`）。
  - ✕（撤销）只在 `hits.length>0 || 任一 selected || 存在有效 selection` 时显示，调 `ghostwriterCancelLast`。
  - 卡片 `maxHeight` 由 340 提到 396（420−2×12），候选区出现时自然向上生长；预览区 `overflowY:auto` 已有即兜底；转写流不动。
  - 出现/消失 `transition: opacity .15s, max-height .2s`；`@media (prefers-reduced-motion: reduce)` 下 transition:none。
  - notice 复用现有药丸（saveSuggestion 失败也走它）。
- [ ] **Step 1: reducer 失败测试**（ghostwriterCapsule.test.ts）：`assist_event_replaces_state`、`assist_unknown_event_noop`。
- [ ] **Step 2:** 跑 `npm test` 确认失败 → **Step 3:** 实现 reducer＋面板 → **Step 4:** `npm test`＋`npm run build` 过。Commit：`git commit -am "ghostwriter: candidate area UI + assist state"`

---

### Task 9: Ghostwriter 视图（三页签）＋导航收敛＋设置页按钮

**Files:**
- Create: `src/pages/GhostwriterView.tsx`、`src/pages/ghostwriter/TaskBriefsPane.tsx`、`src/pages/ghostwriter/SettingsPane.tsx`
- Modify: `src/pages/GhostwriterSnippets.tsx`（加 `embedded?: boolean` prop：true 时跳过 PageHeader 外壳，列表照旧）、`src/components/FloatingShell.tsx`（:45/:61/:78 项换 `id:'ghostwriter', icon:'ghostwriter'`；组件映射 GhostwriterView）、`src/components/MobileMoreSheet.tsx`（同步）、图标映射组件（找到 'tag' 的渲染处，新增 'ghostwriter' 字形：笔/羽毛 SVG，从项目图标库挑，无则内联一个 16×16 stroke 图标）、`src/state/useAppState.ts`（'ghostwriterSnippets' → 'ghostwriter'）、`src/pages/settings/RecordingInputSection.tsx`（候选/推荐两行删，原位换一枚「Ghostwriter 设置」按钮，仅 `prefs.capsuleStyle==='fluid'` 时渲染，点击→导航到 ghostwriter 页设置页签）
- Modify: `src/lib/ipc/ghostwriter.ts`（+listTaskBriefs/saveTaskBrief/resetTaskBrief）、`src/lib/ipc/settings.ts` 不动

**Interfaces:**
- Produces:
  - GhostwriterView：读 URL/hash 无——页签状态内部 useState，接受外部 `initialTab?: 'snippets'|'briefs'|'settings'`；设置页按钮经 useAppState 的页面切换机制进入并带 tab（若 Shell 不支持带参导航，则 GhostwriterView 默认 snippets，设置按钮改为触发全局事件 `window.dispatchEvent(new CustomEvent('ghostwriter:open-tab',{detail:'settings'}))`，View 监听——二选一，执行时看 Shell 既有机制选简者）。
  - TaskBriefsPane：列表 5 行（title＋description＋modified 标记「已改」）→ 右侧抽屉（照 GhostwriterSnippets 抽屉 :421-669 布局）：description、textarea（body，字符数）、折叠区「固定部分（只读）」（内容＝该任务书的注入与输出契约说明文案，i18n 常量）、按钮 保存/恢复默认（`confirm` 确认）/取消。
  - SettingsPane：两枚 Toggle（candidatesEnabled/recommendationsEnabled，照 RecordingInputSection :482-493 写法 `savePrefs`）＋两枚数字输入（candidateThrottleMs/recommendationThrottleMs，500–10000，越界失焦时红字提示并回弹上次合法值）。
- [ ] **Step 1:** 实现 View＋三页签＋导航替换＋设置页按钮（页面骨架照 Style.tsx 的 PageHeader＋Card 惯例；页头 kicker=Ghostwriter）。
- [ ] **Step 2:** `npm run build`＋`npm test` 过（settings navigation tests 若断言 nav 列表需同步更新）。
- [ ] **Step 3:** 手动冒烟：nav 一项「Ghostwriter」→ 三页签切换；设置页按钮直达设置页签；非 fluid 样式时按钮不显示。
- [ ] **Step 4:** Commit：`git commit -am "ghostwriter: unified view (snippets/briefs/settings tabs) + single entries"`

---

### Task 10: i18n 收尾（新文案 8 门）

**Files:** `src/i18n/zh-CN.ts` 先行，其余 7 门照译。

- [ ] **Step 1:** zh-CN 增 key（命名空间）：
  - `ghostwriter.panel`：`sedimentSuggest`（「{{phrase}}」最近说过 {{count}} 次）、`saveSuggestion`（存为常用语）、`dismiss`（忽略）、`kindTerm`（精准词）、`kindPhrase`（表述）、`kindNaming`（命名）、`recommendLabel`（常用语）、`saveFailedDuplicate`（触发词已存在，去 Ghostwriter 页改一个）
  - `ghostwriter.view`：`title`（Ghostwriter）、`tabSnippets`（常用语）、`tabBriefs`（任务书）、`tabSettings`（设置）
  - `ghostwriter.briefs`：`listTitle`（任务书）、`desc`（这些是各功能交给 AI 的任务说明，改了立即生效）、`modified`（已改）、`fixedPart`（固定部分（只读））、`fixedPartHint`（数据注入与输出格式由系统固定，不在可改范围）、`save`/`saving`/`saved`/`reset`/`resetConfirm`（恢复默认？此操作不可撤销）/`cancel`
  - `ghostwriter.settingsPane`：`throttleCandidate`（候选刷新间隔（毫秒））、`throttleRecommendation`（推荐刷新间隔（毫秒））、`throttleRangeHint`（500–10000）
  - `settings.ghostwriter.openSettings`（打开 Ghostwriter 设置）
  - `history.ghostwriterDetail`（Ghostwriter 明细）、`history.ghostwriterHits`（本次命中）、`history.ghostwriterSelections`（选中的候选）
  - `nav.ghostwriter`（Ghostwriter）＋shell.navHint；删 `nav.ghostwriterSnippets`
- [ ] **Step 2:** 7 门照各文件相邻语气翻译；`npm run build`＋`npm test` 过。Commit：`git commit -am "ghostwriter: i18n for m3m4 (8 locales)"`

---

### Task 11: M4 历史扩展

**Files:**
- Modify: `crates/openless-core/src/types.rs`（DictationSession :73-113 加字段）、`api.rs`（persist_completed :5422-5500 快照填充）
- Modify: `src/lib/types.ts`（DictationSession 同步）、`src/pages/History.tsx`（按钮＋面板）

**Interfaces:**
- Produces:
```rust
// types.rs（均 #[serde(default)]）
pub ghostwriter_hits: Option<Vec<GhostwriterHistoryHit>>,          // { title: String, mode: String }
pub ghostwriter_selections: Option<Vec<GhostwriterHistorySelection>>, // { kind: String /*"candidate"|"recommendation"*/, text: String }
```
- api：stop 点组装 DictationSession 时（:5458-5485），从 session 取命中（active hits 的 title/mode）与生效 selections（toggle 后仍选中的），非空才填 Some。
- 前端：History.tsx 详情按钮组（:557-626）加「Ghostwriter 明细」（仅两字段任一非空渲染）；点击就地展开一块面板（再点收起）：命中行（✓ 标题·贴位文案「贴进正文/附在文末」照 `ghostwriter.snippets.modeInline/modeFootnote` 复用）＋选中行（类型 i18n ＋文本）。
- [ ] **Step 1: 失败测试**：core——`dictation_session_ghostwriter_fields_default_none`（serde roundtrip 旧 JSON 无字段照读）；api——`ghostwriter_stop_records_hits_and_selections`（含命中与选中的会话 stop → list_history 条目字段正确）。
- [ ] **Step 2:** 失败 → **Step 3:** 实现（core＋前端）→ **Step 4:** `cargo test -p openless-core`＋`npm test`＋`npm run build` 过。Commit：`git commit -am "ghostwriter: history detail (hits + selections)"`

---

### Task 12: 实机验收清单（macOS，run-macos.sh＋OPENLESS_LOG_LEVEL=debug，ASR 用火山流式）

- [ ] 1. 说话→停顿 ≥1.5s 或句毕 → 候选区出现（类型标签＋序号）；说「用候选二」→ 文本剔除、候选选中高亮、预览随下段融合；点 chip 同效；再点取消。
- [ ] 2. 口头命令在无候选时保留为普通话（日志确认未触发 selection）。
- [ ] 3. 库里建 2-3 条常用语 → 说话涉及 → 推荐行出现 → 「用常用语一」→ 融合。
- [ ] 4. 同一背景说两次以上（跨会话）→ 下次说话中出现沉淀提醒条 → 存 → 常用语页可查、说触发词能命中；✕ 忽略后不再提。
- [ ] 5. 任务书页签：改「候选生成」正文（如加"候选用英文"）→ 下次候选即变；恢复默认生效；「已改」标记正确。
- [ ] 6. 设置页签：节流改 800/6000 → 候选明显变勤/推荐变懒；越界 100 → 回弹提示。
- [ ] 7. 关候选+推荐 → 无 assist 事件（日志）；命中/润色照旧。
- [ ] 8. 主界面导航仅一个「Ghostwriter」项；设置页仅一枚按钮；历史明细按钮条件出现、展开正确。
- [ ] 9. 秒停/空说话/LLM 断开：不炸、notice 轻提示、贴出走原路径。
- [ ] 10. `cargo test -p openless-core` 全绿＋`cargo check -p openless-linux-egui`＋`npm test`＋`npm run build`；rebase 上游 beta 验证冲突面。
- [ ] **Step:** 逐条实机验证后 Commit＋`git push origin HEAD`（SSH）。
