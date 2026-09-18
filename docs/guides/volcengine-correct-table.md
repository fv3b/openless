# 火山引擎替换词表（correct_table_id）配置指引

> 本文是指南性质：教你把「高频错字对」沉淀成火山的**替换词表**，让云端识别结果做确定性替换。OpenLess 当前版本**尚未接入**该参数——文末说明后续接入的落点。
>
> 关联：`docs/design/asr-accuracy/design.md`（决策 5）、调研报告（`.superpowers/sdd/plan/asr-research-report.md`）。

## 一、替换词表是什么、什么时候用

火山官方提供三条提准通道，分工不同：

| 通道 | 作用 | OpenLess 现状 |
| --- | --- | --- |
| 热词（hotwords） | 词级偏置：同音/近音片段优先选热词写法 | ✅ 已接入（词典热词直传） |
| 上下文 dialog_ctx | 语义场景偏置：缩小话题范围 | ✅ 已接入（最近语音＋固定场景条目） |
| **替换词表（correct_table_id）** | **识别结果强制替换**：说错的写法 → 你要的写法，确定性生效 | ❌ 未接入 |

适用场景：某个词 ASR 总是识别成固定错字（如「灰度发布」识别成「灰度发报」），热词偏置兜不住时，用替换词表直接强制替换。它管的是「字面替换」，不管话题范围。

## 二、在火山控制台配置替换词表

前提：已有火山引擎账号，且账号开通了「大模型流式语音识别」（OpenLess 默认 Resource ID 为 `volc.seedasr.sauc.duration`）。

1. 登录 [火山引擎控制台](https://console.volcengine.com)，进入**语音技术 → 自学习平台**。
2. 在自学习平台选择**语义替换词（替换词表）**模块，点「新建词表」。
3. 逐条添加词对：**错误写法 → 正确写法**（左列填 ASR 老识别出来的错字，右列填你要的写法）。一条词表可放多对。
4. 保存并**发布/上线**词表（未上线的词表对线上识别不生效）。
5. 上线后，在词表详情里复制**词表 ID**（即 `correct_table_id`，形如一串字母数字）。

注意事项：

- 词表对**线上实时识别**生效，配好后下一次会话即可验证，无需改代码。
- 替换是**确定性**的：只要命中等价替换条件，就按词表写法输出；若发现「该替换的没替换」，先确认词对已上线、错写列与实际识别字面一致（同音但写法不同的错字要按实际输出的字面填）。
- 替换词表只对火山云端识别生效；本地模型（Whisper / Qwen3-ASR / sherpa-onnx）与 Apple 听写不走该通道。

## 三、OpenLess 侧现状与后续接入落点

**现状（截至 2026-09-18 批次）**：OpenLess 首帧请求的 `context` 对象已携带 `hotwords`（热词直传）与 `context_type: "dialog_ctx"` / `context_data`（最近语音＋固定场景条目），见 `app/crates/openless-core/src/asr/volcengine.rs` 的 `context_payload()`；但**没有传** `corpus.correct_table_id`。

**后续接入**（官方文档：大模型流式语音识别 API，docs/6561/1354869；替换词：docs/6561/1206007）：

- 官方参数落点：首帧请求 `request.context.corpus` 内携带 `correct_table_id`（或 `correct_table_name`）。
- OpenLess 侧无需改请求结构：`context` 就是现在组装的这个对象，接入时在此处加 `corpus` 字段即可。
- `correct_table_id` 的存放建议：跟随现有「高级 ASR 配置」（`advanced asr config`）或凭据库新增一个 account 槽位——用户在设置里粘贴词表 ID，会话启动时冻结进请求，与其他凭据同模式管理。
