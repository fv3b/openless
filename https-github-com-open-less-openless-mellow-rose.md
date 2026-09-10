# 语音输入法「Fluid」— 基于 OpenLess fork 的实施计划

## Context

用户日常用语音给 AI 输入指令，现用 OpenLess（开源语音输入：按住说话→ASR→LLM 润色→光标处插入）。缺五样：①说话中就出字出润色（不等说完）②主题建议浮动框（边想边给思路拓展，只展示）③自动注入相关信息（说到配置过的条目自动展开，文末附注）④按内容类型推荐动作＋口头选择（说动作名即选中，提示词拼进结果）⑤动作/注入的后台管理。本计划在上游 fork 上加一个「fluid 层」，外加四个补充功能（口头编辑命令、注入自动沉淀、动作预览、纯净模式）。

需求经三轮用户决策（11 项，见下）；源码级调研由设计代理完成（gh api 实读 beta 分支关键源码），主代理亲手抽查 5 个承重事实全部复核通过。

## 已定决策（不可偏离）

1. 浮框交互：说话时浮框流式展示；**说话中口头选动作**（仅浮框已列出者可触发、高亮＋预览、「取消 X」撤销）；松开润色完**即自动贴**（主文本＋注入附注＋动作块一起）；贴后浮框残留数秒渐隐。
2. 云端流式 ASR（火山/讯飞等，openless 已支持多家）。
3. 分句增量润色，每段带已润色前文；尊重当前激活 style pack。
4. 润色/主题建议/动作匹配三条独立异步流，各自节流可调，不设硬延迟预算。
5. 注入＝文末附注块；动作＝文末提示词块；两者同构。
6. 注入条目、动作均本地管理（管理页照 Style 页模式）。
7. 口头编辑命令（「删掉上一句」「重说」）说话中生效，作用于流式缓冲。
8. 注入自动沉淀：未配置的重复项目，浮框提示一键存条目。
9. 纯净模式一键关：FluidSession 不创建，完全退化为上游纯听写。
10. 历史轻量扩展（记命中注入/选中动作）。
11. fork 跟上游：改动做成新增模块＋旧文件仅 8 处加法式小改；**同步上游一律 rebase（2026-09-10 用户拍板，弃 merge）**——把我们的提交重放到上游新基之上，`git diff 上游..我们的分支` 永远只含我们自己的差异（当前 35 文件），上游更新再大也不进我们的提交序列；推送用 SSH（HTTPS 的 gh token 无 workflow scope，会因 ci.yml 被拒）。每个里程碑后 rebase 一次上游 beta 验证冲突面。
12. Fluid 作为第三种胶囊样式（capsuleStyle='fluid'，样式驱动接管浮框；上游后来加的 typeless 与之并列，四样式共存）。**显示名定为「Ghostwriter 流式浮框」（2026-09-11 用户拍板）**，两段式分工是定名约定：前段专名管身份承诺（Ghostwriter＝你只管说、它代写成文并自备素材，对位注入背景/动作/提示词的"提前张罗"职能），后段管形态（流式浮框，设计终态边说边流字）。仅改 8 门语言 i18n 的 capsuleStyleFluid 值，内部代号 fluid（serde 值/窗口路由/文件名）全部不动。
13. MLX 改为可选 feature（`--features mlx` 才启用）：本地 MLX 识别是可选增强，默认构建不编 MLX 依赖；下轮 rebase 遇到相关冲突时不得把这套 feature 门控消掉。

## 对上游的真实认知（读码实证）

- 真正的管线：`api.rs` 的 `OpenLessBackend`（phase 状态机，~11.5k 行门面）＋ `dictation_engine.rs` 的 `PipelineDictationEngine`（资源编排）；`voice_session.rs` 只是会话门闩。
- **流式通道已存在**：流式 ASR partial 说话期间就到前端（`asr/volcengine.rs:704` → `EngineProgress::TranscriptDelta`（`dictation_engine.rs:928`）→ `api.rs` publish → `tauri_events.rs:44` 全量桥 → 前端 `backendEvent.ts` `applyTranscriptEvent`）。浮框流式展示零 ASR 改动。
- **Raw 透传门控已存在**：`dictation_engine.rs:484` `uses_polisher = context.uses_llm_polisher()`——Raw 模式下 finish 走透传分支不调 LLM；`api.rs:1423` 流式插入也由它门控：fluid 置 Raw 后松开即一次性贴拼装文本、不逐字漏插入。
- 浮窗先例：`tauri.conf.json` capsule 静态窗口（`alwaysOnTop:true`、`focus:false`、`index.html?window=capsule` 路由）。
- 四个现成模板：`style_pack_store.rs`（存储＋管理页）、`selection_voice_intent.rs`（三层意图解析：启发式→LLM 分类→解析兜底）、`prompt_compose.rs`（marker 分段）、`QaPanel.tsx`/`SelectionVoiceIntentPicker.tsx`（浮窗页面）。

## 架构（数据流）

```
按热键 → start_dictation_with_options (api.rs:4505)
  DictationContext::capture（含激活 style pack 系统提示词快照）
  fluid 激活 → context.polish.mode 置 Raw（跳过松开后整段重润）
  创建 FluidSession 挂 backend registry【新建】

说话中：ASR partial → api.rs progress 循环 TranscriptDelta 分支（~1784 行）
  ├→ 前端 TranscriptDelta（现有，浮框主文本流）
  └→ FluidSession::feed(delta)【新建，~3 行接线】
       ├→ segmenter 断句 → segment_polisher 段润色（带前文）→ FluidDictationText
       ├→ voice_command 命令解析（删上句/重说/选动作/取消 X）→ FluidCommandApplied
       ├→ injection_store 命中检测 → FluidInjectionsHit
       ├→ action_matcher 推荐动作（节流）→ FluidActions
       └→ suggestion_stream 主题建议（节流）→ FluidSuggestions

松开 → stop_dictation_session_with_options (api.rs:4751)
  尾段补润 → assembled_text()（已润段＋注入附注块＋动作块）替换 polished_text
  → finish_text_insertion (api.rs:5316) 自动贴（现有 unicode_keystroke＋剪贴板兜底）
  → 浮框残留渐隐（coordinator）
```

## 新建文件

**openless-core/src/fluid/**（照 `asr/` 子目录先例；`lib.rs` 加 `pub mod fluid;`）：

| 文件 | 职责 |
|---|---|
| `types.rs` | 5 个事件 payload（serde camelCase） |
| `session.rs` | FluidSession：流式会话协调器（缓冲/已润段/选中动作/命中注入；feed 驱动；assembled_text 拼装；取消三流） |
| `segmenter.rs` | 中英混合断句状态机（标点＋长度阈值），纯函数可单测 |
| `segment_polisher.rs` | 段润色：临时 DictationContext（style_system_prompt 取会话快照）调 `TextPolisher`，照 `selection_voice_service.rs:284` 模式；stop 尾段补润 |
| `voice_command.rs` | 口头命令：规则层先行、LLM 兜底（照 selection_voice_intent 三层模式）；输出缓冲编辑操作 |
| `action_store.rs` | 动作库（id/name/aliases/prompt_block/description/enabled）落 `data_dir/fluid-actions.json`，照 StylePackStore |
| `action_matcher.rs` | 按内容推荐动作（别名/关键词规则＋可选 LLM 排序），只推荐已启用动作 |
| `injection_store.rs` | 注入条目（trigger/aliases/expansion/title/enabled）落 `data_dir/fluid-injections.json`；命中扫流式缓冲 |
| `suggestion_stream.rs` | 主题建议 LLM 流（只展示不触发）＋沉淀候选识别 |
| `assembly.rs` | 最终拼装：主文本＋[附注]块＋[动作]块（marker 定界照 prompt_compose.rs） |
| `mod.rs` | 模块声明 |

**src-tauri**：`src/commands/fluid.rs`【新建】（list/save/delete 动作与注入，照 `commands/style_packs.rs` 的 CoreState 模式）。

**前端**【新建】：`src/pages/FluidPanel.tsx`（浮框：主文本流＋建议区＋动作 chips＋注入徽标＋预览，照 QaPanel/SelectionVoiceIntentPicker）、`src/pages/FluidActions.tsx`（动作管理页，照 Style.tsx）、`src/pages/FluidInjections.tsx`（注入管理页＋沉淀候选一键存）、`src/lib/ipc/fluid.ts`（ipc 封装）。

## 修改文件（8 处，全为加法式小改）

| 文件 | 改动 |
|---|---|
| `core/src/lib.rs` | `pub mod fluid;` 一行 |
| `core/src/events.rs` | `BackendEventKind` 加 5 个变体（payload 类型放 fluid/types.rs，旧文件不膨胀） |
| `core/src/api.rs` | ① TranscriptDelta 分支喂 FluidSession（~3 行）；② start 创建/注销 FluidSession＋fluid 激活置 Raw；③ stop 在 `apply_chinese_script_preference` 之后、`finish_text_insertion` 之前用 `assembled_text()` 替换 `polished_text`；④ M4 persist 新字段 |
| `core/src/dictation_context.rs` | `DictationPolishContext` 加 fluid 分项开关快照（capture 内从 preferences 读） |
| `core/src/shared_types.rs` | `UserPreferences` 加 fluid 总开关＋分项开关＋节流参数（`#[serde(default)]` 兼容旧配置） |
| `core/src/types.rs` | M4：`DictationSession` 加命中注入/选中动作字段（serde default，照 dictionary_entry_count 同款兼容） |
| `src-tauri/tauri.conf.json`＋`src-tauri/src/lib.rs` | fluid 窗口（照 capsule：无边框透明、alwaysOnTop、`focus:false` 不抢键盘，~560×420）＋注册命令 |
| `src-tauri/src/coordinator.rs` | fluid 窗口 show/hide/贴后渐隐编排（照现有卡片窗口模式，~40 行） |
| `src/main.tsx`＋`src/App.tsx` | `?window=fluid` 路由分支（各 2 行，照 isQa） |

**不碰**：`dictation_engine.rs`、`ports.rs`、`cloud_providers.rs`、`asr/*`、`tauri_events.rs`（`backend:event` 全量桥自动携带新变体）、`linux-egui`。

## 关键设计决策

1. **接入点在 api.rs progress 循环**，engine 零改动——上游 merge 时核心管线零冲突。
2. **fluid 激活置 Raw**（门控已验证）：松开后不重润整段、不逐字漏插入，一次性贴拼装文本。兜底：fluid 未激活或 FluidSession 无结果（秒停/段润全败）→ 不置 Raw，走上游原路径，行为与上游一致。
3. **防误触**：动作仅当名称/别名出现在最近下发的 FluidActions 列表才生效；「取消 X」只撤销最近一次选中；命令应用后锁定 offset，ASR partial 修正不得回退越过该点。
4. **命令词剥离**：`assembled_text()` 基于 FluidSession 缓冲（命令已应用）而非 engine transcript，命令词天然不进最终文本。
5. 三流节流参数进 `UserPreferences`（默认建议 1.5s/动作 2s，可调）。

## 里程碑与验证

## 里程碑与验证

- **M1 浮框＋流式展示＋自动贴**（转写流）：fluid 窗口＋FluidPanel 主文本流＋FluidSession 骨架（Accumulator＋segmenter）＋stop 替换 final_text。✅ 已完成（2026-09-10，ff9a50c0）。两点与原计划不同：①「贴后渐隐」按用户拍板改为「停止即收起，仅兜底 toast 短暂显示」；②stop 替换放在简繁转换**之前**（原计划在转换之后）——这样转换与纠错规则仍作用于最终插入文本，比原位更正确。验证实况：Raw 门控生效（会话窗口零 LLM 调用）、`[fluid] stop` 拼装替换 16 字、远程全链路 RC=0；「说话实时出字」受当前智谱批量 ASR 限制为降级形态（停止后一次性出全文，见 ASR 现实节）。观测通道：文件日志默认 info，设 `OPENLESS_LOG_LEVEL=debug` 放行 debug 探针（src-tauri lib.rs 加法改动）。
- **M2 增量润色＋口头命令＋动作系统**（含管理页；置 Raw 切换在此步上）。验证：浮框文本从转写切为润色段；「删掉上一句」生效且最终文本无该句无命令词；说已列动作名→高亮→松开动作块在文末；管理页 CRUD＋重启持久化；切 style pack 后段润风格跟随。
- **M3 注入系统＋自动沉淀**。验证：配置触发词后说到即命中、文末出附注块；未配置重复项目出沉淀提示、保存后下次生效；管理页 CRUD。
- **M4 主题建议＋纯净模式＋历史扩展**。验证：建议区节流更新且不可触发动作；纯净模式一键退化纯听写；历史展示命中注入/选中动作；旧 history.json 兼容不报错。

依赖：M2 用 M1 缓冲；M3 复用 M2 拼装；M4 建议流独立但 UI 寄生浮框。每段完即独立可用。

## 风险与对策

| 风险 | 对策/验证 |
|---|---|
| 各家 ASR delta 语义不一（追加式 vs 替换式 offset） | 全经 `TranscriptAccumulator::apply`（已处理 offset 语义，前端 backendEvent.ts 有同款）；单测两种序列＋实测火山/讯飞两家 |
| 命令剥离 vs partial 回退竞态 | offset 锁定机制；实测连说两次「删掉上一句」夹正常语句 |
| 跨段润色风格一致性（增量 vs 整段） | 段带已润前文尾部上下文；「松开后整段重润」偏好开关兜底；人工 A/B |
| 三流并发 LLM 成本/限流 | 每流独立开关＋节流参数；日志观测调用频率 |
| 浮框抢焦点干扰打字 | `focus:false` 静态窗口（capsule 同款）；实测浮框显示时在别的 app 打字 |
| 尾段未完即停（说完立刻松开） | stop 对尾段补润一次（带全部前文）；实测说完立即松开看最终文本完整 |
| 上游 merge 冲突 | 旧文件改动仅 8 处且均为加法；每里程碑后 merge beta 验证冲突面 |
| 破坏 linux-egui 编译 | 新模块独立、不改任何 trait 签名；`cargo check -p openless-core`＋`cargo check -p openless-linux-egui` |

## 实施第一步（获批后）

1. 克隆 Open-Less/openless（beta 分支）到本地（如 `~/LocalCode/openless`），fork 远端＋建 `fluid` 分支。
2. 跑通上游基线：`cd openless-all/app && npm ci && npm run tauri dev`（macOS；submodule 按需 `git submodule update --init --recursive`），确认现有功能正常。
3. 进 M1。开发遵循 TDD：segmenter/accumulator 等纯逻辑先写失败测试再实现；每个里程碑完成后在 macOS 实机按上面验证清单逐条验收，并 merge 一次上游 beta 验证冲突面。
