# 对话（Conversation Mode）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在代笔（Ghostwriter）里新增可选的「对话」模式：对话热键开一场带 AI 的会话，AI 按设置在停顿/显式交话时回话（命名校准并进回话），结束把整份聊天记录润写成指令。

**Architecture:** 引擎从现有 assist 触发点长出来——对话会话复用停顿/句毕触发与多任务书拼装，但换成对话输出契约（`{reply, recommendations}` 一次调用双产出）；会话记聊天记录（【我】/【助手】行），段润色照旧，stop 终稿改用「对话出稿」任务书吃聊天记录；回话以事件落到前端转写流，推荐在对话会话常驻一份。外壳（开关/快捷键/设置子视图）照划词追问模式。

**Tech Stack:** Rust（openless-core：session/dispatcher/assist/prompts/api；src-tauri 命令层）＋ TypeScript/React（浮框面板、设置、capsule 状态机、i18n×8）。

**Spec:** `docs/design/dialog/design.md`（计划从 spec 论证，执行者两份都读）。

## Global Constraints

- 普通代笔会话的一切行为不变（默认关、零迁移）；候选流/推荐流开关原样保留只管代笔会话。
- AI 回话不产生生效动作（不进待融、无撤销）；AI 的话绝不进最终指令（只当已消歧上下文）。
- 输出契约与聊天记录行语法由代码固定（任务书页只读展示），逐字测试钉住。
- 追问深度必须机制级实现（冷却/封顶），不依赖模型自觉；抑制时代码强制剥掉 reply（同 include_candidates 门控模式）。
- 所有新任务书进任务书注册表（任务书页可见可编辑可恢复默认）。
- 测试基线：`cargo test -p openless-core` 现有 865 过＋32 个预存 asr/net/provider 网络失败，新增不得引入非网络失败；`npm test` 退出码 0。
- i18n 八语言同步（zh-CN 为准，zh-TW/ja/ko/en/fr/es/de 跟译）。
- 存储/事件键新增不迁移旧数据；`fluid` 样式值等既有用户数据标识不动。

---

### Task 1: 偏好字段与设置同步（Core）

**Files:**
- Modify: `crates/openless-core/src/shared_types.rs`（GhostwriterPreferences 定义所在；先 grep 定位）
- Modify: `crates/openless-core/src/api.rs`（偏好 get/set 透传处）
- Test: 同文件 tests 模块

**Interfaces:**
- Produces: `GhostwriterPreferences` 新字段（serde camelCase）：
  `conversation_enabled: bool`（默认 false）、`conversation_hotkey: Option<String>`（默认 None）、
  `conversation_reply_timing: ConversationReplyTiming`（枚举 `Pause|Explicit`，默认 Pause）、
  `conversation_probe_depth: ConversationProbeDepth`（枚举 `Single|UntilClear|Echo`，默认 Single）、
  `conversation_recommendations: bool`（默认 true）。
  枚举派生 Debug/Clone/Copy/PartialEq/Serialize/Deserialize（camelCase：`"pause"|"explicit"`、`"single"|"untilClear"|"echo"`）。
- Produces: api get/set 偏好路径带上新字段（照 background_placement 的同步模式）。

- [ ] 写失败测试：默认值断言（enabled=false、timing=Pause、depth=Single、recommendations=true）＋ serde 往返（camelCase 键名）。
- [ ] 跑测试确认失败（字段不存在）。
- [ ] 实现字段与默认值。
- [ ] 跑测试通过；`cargo test -p openless-core` 基线不破。
- [ ] Commit: `feat(ghostwriter): conversation preferences fields`

### Task 2: 任务书注册表扩容＋对话输出契约（Core）

**Files:**
- Modify: `crates/openless-core/src/ghostwriter/prompts.rs`
- Test: 同文件 tests

**Interfaces:**
- Produces: `TaskBriefId::ConversationReply`（key `"conversation_reply"`，title「对话回话」，description「管对话会话里 AI 什么时候说什么、怎么校准怎么追问，改了会影响回话。」）；`TaskBriefId::ConversationFinalize`（key `"conversation_finalize"`，title「对话出稿」，description「管聊天记录怎么润写成指令，改了会影响最终贴出的指令。」）。注册表共六份。
- Produces: `pub const CONVERSATION_OUTPUT_CONTRACT: &str`，逐字内容：
  `"只输出 JSON，不要任何解释或代码块标记：\n{\"reply\":\"给说话人的一句话，或 null\",\"recommendations\":[\"常用语id\",…]}\nreply 为 null 表示这次闭嘴；推荐最多 3 个 id；没有的键给空数组或 null。"`
- Produces: 两份默认正文（存进 default_body）：
  - 对话回话：`"你是语音对话助手。用户在用语音跟你对一场话，目的是把一件他想交办的事说清。\n下面是你们目前的聊天记录和他的最新发言。\n你的职责：理解他的真实意图；发现说不清、没说全、用词含糊的地方，用一句短话回他（指出歧义、给出本行叫法并配一句外行能懂的解释、或补一句他没想到的要点）。\n一次只说一句，不超过 80 字；他没有问题你就闭嘴（reply 给 null）；拿不准他的意思就问，但同一个点他回应过就不再纠缠。\n不要替他做决定，不要复述他的话，不要客套。"`
  - 对话出稿：`"你是语音指令整理器。下面是一段用户与助手的对话记录：【我】开头的都是用户说的话，【助手】开头的是助手为了消歧而说的话。\n把用户的真实意图整理成清晰、直接、结构清楚的指令：\n- 只整理【我】的内容；【助手】的内容只当已澄清的上下文，里面的措辞不得当成需求写进指令\n- 对话里已经说清的决定（叫法、方案、边界）要体现在指令里\n- 去掉口语、重复、语气词；原话里的具体信息（名字、数字、路径、代码、命令）一字不改\n- 如果给了「参考材料」，把材料内容自然融合进指令对应的位置\n只输出整理后的指令文本，不要任何解释或前缀。"`
- Consumes: Task 1 无依赖；后续任务按 `TaskBriefId::ConversationReply.default_body()` 等取用。

- [ ] 写失败测试：六份任务书 key/title 断言表更新；契约逐字 contains/ends_with 断言；`default_bodies_do_not_leak_prompt_wording` 循环加入新枚举。
- [ ] 实现枚举、契约常量、两份正文。
- [ ] 测试通过；全量基线不破。
- [ ] Commit: `feat(ghostwriter): conversation briefs + conversation output contract`

### Task 3: 会话层——聊天记录、模式标志、回话门控（Core）

**Files:**
- Modify: `crates/openless-core/src/ghostwriter/session.rs`
- Modify: `crates/openless-core/src/ghostwriter/types.rs`
- Test: session.rs tests

**Interfaces:**
- Produces（types.rs）: `ChatTurn { role: ChatRole, text: String }`、`ChatRole { User, Assistant }`；`ReplyGate { Allow, Suppress, SuppressReason }` 不必公开——门控用简单枚举 `ReplyGate { Allow, Cooldown, Cap }`。
- Produces（session.rs）:
  - `GhostwriterSession::with_conversation(mut self, timing: ConversationReplyTiming, depth: ConversationProbeDepth) -> Self`（同时置 `conversational: bool = true`；不调用则 false，普通会话零变化）
  - `pub fn conversational(&self) -> bool`
  - 段完成处（现有 segments push 路径）追加 `chat.push(ChatTurn::user(segment_text))`；同段路径触发 `cooldown_cleared = true`
  - `pub fn reply_gate(&self, auto: bool) -> ReplyGate`：`auto=false`（显式交话）→ 恒 Allow（绕过冷却；封顶只数自动回话）；`auto=true` → Echo/Single 深度下 `reply_cooldown` 激活即 Cooldown；UntilClear 深度下 `auto_reply_count >= 5` 即 Cap；其余 Allow
  - `pub fn record_reply(&mut self, text: String)`：chat.push(assistant)、`reply_cooldown = true`、`auto_reply_count += 1`（仅自动触发时由调用方传 auto 标记决定是否计数）、`revision += 1`
  - `pub fn chat_transcript(&self) -> String`：`【我】{text}`／`【助手】{text}` 按序每行一条；空记录返回空串
  - stop 路径：尾巴文本进 chat（`record_tail_into_chat`，api stop 时调一次）
- 注意：普通会话（conversational=false）这些方法不改变任何现有行为；chat 仅对话会话维护。

- [ ] 写失败测试：普通会话 chat 恒空；对话会话段完成入 chat（【我】行）；record_reply 入【助手】行＋revision 推进；reply_gate 三档×auto 两态矩阵（冷却/封顶/放行）；chat_transcript 行格式逐字断言；UntilClear 封顶第 6 次 Cap。
- [ ] 实现最小代码。
- [ ] 测试通过；全量基线不破。
- [ ] Commit: `feat(ghostwriter): conversational session flag, chat transcript, reply gate`

### Task 4: assist 对话路径——输入拼装与 reply 解析（Core）

**Files:**
- Modify: `crates/openless-core/src/ghostwriter/assist.rs`
- Test: assist.rs tests

**Interfaces:**
- Consumes: Task 2 的 `CONVERSATION_OUTPUT_CONTRACT`、`TaskBriefId::ConversationReply`；Task 3 的会话上下文由 dispatcher 传入。
- Produces: `AssistInput` 新字段 `conversation: Option<ConversationAssistContext>`；`ConversationAssistContext { chat: String, suppress_reply: bool }`（derive Clone，字段 camelCase 不外序列化——内部结构）。`AssistJson/AssistOutcome` 增 `reply: Option<String>`。
- 拼装规则（compose_system_prompt）：`conversation=Some` 时 parts ＝ 对话回话正文 +（suppress_reply 时追加一行代码注入 `"（系统提示：本次不要回话，用户还没有回应你上一句——reply 给 null。）"）` + 推荐任务书正文 + `CONVERSATION_OUTPUT_CONTRACT`（**不含**候选/提醒任务书，**不含**代笔契约）。compose_user_input：`当前内容` 改为聊天记录（suppress 语义不变）＋原有常用语库块。
- 解析：`reply` 键取字符串；suppress 时代码强制置 None（机制级，不信任模型）。代笔路径（conversation=None）一切照旧。
- 测试：对话 prompt 以 CONVERSATION_OUTPUT_CONTRACT 结尾且含回话正文、不含代笔契约；suppress 注入行存在且 outcome.reply 被剥成 None；正常解析 reply；代笔路径回归不变。

- [ ] 失败测试→实现→通过（TDD 循环，每步跑 `cargo test -p openless-core assist`）。
- [ ] Commit: `feat(ghostwriter): conversational assist assembly + reply parsing`

### Task 5: dispatcher——对话触发分支与显式交话（Core）

**Files:**
- Modify: `crates/openless-core/src/ghostwriter/dispatcher.rs`
- Test: dispatcher.rs tests

**Interfaces:**
- Consumes: Task 3 会话门控；Task 4 拼装。
- Produces:
  - `maybe_trigger_assist` 对话会话分支：auto=true 路径——先查 `session.reply_gate(true)`，Allow → conversation 上下文 `suppress_reply=false`；Cooldown/Cap → `suppress_reply=true`（照常调用出推荐）。产出处理：`outcome.reply` 非 Some/空 → 无回话动作；Some(text) → `session.record_reply(text)`（计 auto 计数）＋发布 `GhostwriterReplyChanged { text }`；recommendations 照旧走 assist 事件。
  - `pub fn trigger_reply(&self, session_id: &SessionId)`：显式交话入口——会话须存在且 conversational；auto=false（绕过冷却）；同样发布回话事件/推荐事件。
  - Fixture 路由：沿用现有 assist 的 uuid5 精确会话 id 模式；对话会话的 canned JSON 改为 `{"reply":"…","recommendations":[…]}` 形状。
- 事件（types/events）：`GhostwriterReplyChanged { text: String }`（Serialize camelCase）＋ `BackendEventKind::GhostwriterReplyChanged`，事件发布照 assist 模式（含前端投影路径如有）。

- [ ] types/事件先写失败测试（序列化 camelCase）。
- [ ] dispatcher 分支实现＋测试：对话会话停顿触发产出回话事件（canned reply）；冷却期 suppress（canned 有 reply 但事件不发、session 未记）；显式 trigger_reply 绕过冷却；普通会话不受任何影响（回归）。
- [ ] 测试通过；全量基线不破。
- [ ] Commit: `feat(ghostwriter): conversational trigger branch + explicit reply trigger + reply event`

### Task 6: api——会话启动路由、显式命令、终稿吃聊天记录、历史明细（Core）

**Files:**
- Modify: `crates/openless-core/src/api.rs`（会话创建点、stop 路径、历史写入、命令方法）
- Modify: `crates/openless-core/src/types.rs`（DictationSession 增 `ghostwriter_chat: Option<String>`）
- Test: api.rs tests

**Interfaces:**
- Consumes: Task 1 偏好、Task 3 会话、Task 5 dispatcher。
- Produces:
  - `pub async fn start_ghostwriter_conversation(&self) -> Result<SessionId, BackendError>`：与现有 ghostwriter 会话启动同路径，但启动选项带 `ghostwriter_conversational: true`（启动选项结构加字段；会话创建点按它调 `with_conversation(timing, depth)`——timing/depth 从偏好取）。
  - `pub fn trigger_ghostwriter_reply(&self, session_id: SessionId) -> Result<(), BackendError>`：校验会话存在且 conversational，转 dispatcher.trigger_reply。
  - stop 路径：conversational 会话的**终稿润色**换装——润色输入文本＝`session.chat_transcript()`（把原转写全文替换；命中材料照旧并入），system prompt 用 `TaskBriefId::ConversationFinalize.default_body()`（或覆写）替换指令化润色正文。段润色路径不动。
  - 历史写入：conversational 会话 stop 时 `ghostwriter_chat = Some(session.chat_transcript())`；普通会话恒 None。
- 测试：对话会话端到端（canned）——回话事件落到会话、stop 后 `final_text` 来自 canned 终稿且润色输入含【我】/【助手】（用 Fixture 断言收到的 prompt）、历史条目带 ghostwriter_chat；普通会话 ghostwriter_chat=None 回归。

- [ ] 失败测试→实现→通过。
- [ ] `cargo test -p openless-core` 基线不破。
- [ ] Commit: `feat(ghostwriter): conversation session start/trigger/finalize + history chat detail`

### Task 7: Tauri 命令层

**Files:**
- Modify: `openless-all/app/src-tauri/src/commands/ghostwriter.rs`
- Modify: `openless-all/app/src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `ghostwriter_start_conversation`（async，返回 sessionId 字符串）、`ghostwriter_trigger_reply(sessionId)`；lib.rs 注册两条。
- [ ] 实现＋注册；`cargo check --workspace` 零错误。
- [ ] Commit: `feat(ghostwriter): tauri commands for conversation start/reply trigger`

### Task 8: 前端类型/IPC/状态机——回话行与常驻推荐

**Files:**
- Modify: `src/lib/types.ts`（GhostwriterPreferences 镜像字段；`GhostwriterReplyChanged`；会话载荷带 `conversational`——找到现有会话启动载荷结构加字段）
- Modify: `src/lib/ipc/ghostwriter.ts`、`src/lib/ipc/index.ts`（`startGhostwriterConversation`、`triggerGhostwriterReply`＋mock）
- Modify: `src/lib/backendEvent.ts`（`ghostwriter_reply_changed` 事件 apply：追加进转写视图状态的 `replyLines: Array<{ seq: number; text: string }>`，事件保序即时间序；会话结束清空）
- Modify: `src/lib/ghostwriterCapsule.ts`（对话会话的推荐行 sticky：有对话标志时推荐行不因空内容收起，保留最后非空内容；会话结束复位）
- Test: `src/lib/ghostwriterCapsule.test.ts`（回话行追加保序、会话结束清空、sticky 推荐不塌行）

**Interfaces:**
- Produces: 转写视图状态新增 `replyLines`；面板任务（Task 9）按它渲染。
- [ ] 失败测试→实现→`npm test` 通过。
- [ ] Commit: `feat(ghostwriter): frontend reply-line state + sticky recommendations`

### Task 9: 浮框渲染——回话行、对话标识、常驻推荐

**Files:**
- Modify: `src/pages/GhostwriterPanel.tsx`

**Interfaces:**
- Consumes: Task 8 的 `replyLines` 与会话 `conversational` 标志。
- 渲染规则：回话行在转写流里以「助手」前缀＋区别样式（引用色/斜体级小差异，具体值实现时与现有 token 一致）插入，按事件到达顺序与转写增量自然交织；对话会话卡片标题区加小型「对话」标识；推荐行对话会话下 sticky（Task 8 语义）。
- [ ] 实现；`npm test` 与 `npm run build` 通过（手动冒烟：mock 模式看渲染）。
- [ ] Commit: `feat(ghostwriter): panel reply lines + conversation badge`

### Task 10: 设置子视图＋快捷键接线＋i18n

**Files:**
- Modify: `src/pages/ghostwriter/SettingsPane.tsx`（现有内容之下新增「对话」子视图：总开关 Toggle、快捷键录制（复用应用现有快捷键录制组件——照划词追问设置的模式接线，快捷键仅在总开关开启后可录）、回话时机 radio（停顿即审/显式交话）、追问深度 radio（一点一问/追问到清/回声确认）、推荐显示开关（仅对话模式））
- Modify: 快捷键注册接线处（照 SelectionAsk 热键的注册/分发模式：按下→无活跃对话会话则 `ghostwriter_start_conversation`；有活跃对话会话且回话时机=explicit 则 `ghostwriter_trigger_reply`；其余无操作）
- Modify: `src/i18n/*.ts` ×8

**Interfaces:**
- Consumes: Task 1 偏好字段（读写走现有偏好保存路径）、Task 7 命令。
- [ ] 设置 UI＋偏好读写；快捷键录制与触发接线；i18n 八语言（新键集中 `ghostwriter.conversation.*`）。
- [ ] `npm test`、`npm run build` 通过。
- [ ] Commit: `feat(ghostwriter): conversation settings subview + hotkey wiring + i18n`

### Task 11: 历史明细——聊天记录块

**Files:**
- Modify: `src/pages/History.tsx`（Ghostwriter 明细区新增第三块：`ghostwriterChat` 存在时按行渲染【我】/【助手】，样式与现有明细一致）
- Test: 现有 History 测试模式跟随（如无可测纯函数则抽 `renderChatLines` 纯函数测）

**Interfaces:**
- Consumes: Task 6 写入的 `ghostwriter_chat`（前端字段名 `ghostwriterChat`）。
- [ ] 实现＋测试；`npm test` 通过。
- [ ] Commit: `feat(ghostwriter): history chat transcript detail block`

### Task 12: 全量验证与发布

- [ ] `cargo test -p openless-core`（865+新增−0，非网络失败 0）；`cargo check --workspace`；`npm test`；`npm run build`。
- [ ] `./scripts/run-macos.sh` 构建拉起。
- [ ] 手动冒烟清单：设置开对话→录快捷键→对话热键开会话（浮框有标识）→说话触发回话（停顿即审）→读触发词命中→显式交话模式热键交话→结束→贴出的是出稿指令（不含【助手】字样）→历史条目展开有聊天记录→普通代笔热键一切照旧。
- [ ] 档案同步（CONTEXT.md 如有新词；本计划不新增 ADR——决策已在 design.md）。
- [ ] Commit: `feat(ghostwriter): conversation mode complete`（如零散则按任务各自已提交）

## Self-Review 记录

- Spec 覆盖：设计文档三~七节逐条对到 Task 1-11；完成标准里的「延迟≤3s」由机制保证（停顿触发即调、v1 无流式）＋Task 12 冒烟；「拷问后由用户复核补充」→ 交付后验收。
- 类型一致性：`ConversationReplyTiming/ConversationProbeDepth`（Task 1 定义，Task 3/6/10 消费）；`ChatTurn/ReplyGate`（Task 3 定义，Task 5 消费）；`GhostwriterReplyChanged{text}`（Task 5 定义，Task 8/9 消费）；`ghostwriter_chat/ghostwriterChat`（Task 6 写、Task 11 读）。
- 无占位符；两处「先 grep 定位」（偏好定义处/热键接线处）属定位指令非空泛步骤。
