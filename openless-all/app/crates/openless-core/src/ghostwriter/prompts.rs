//! 任务书注册表：Ghostwriter 各 LLM 功能的任务说明正文（默认内置，可在任务书存储里覆写）。
//!
//! 按任务书边界（ADR 0002）：可编辑的只有任务正文；数据注入与输出结构由代码固定
//! （[`ASSIST_OUTPUT_CONTRACT`] 为输出契约，逐字拼接，界面只读展示）。
//! 默认正文是中文数据本身，不走 i18n 键。

/// 段润色的指令化 system prompt（逐字使用，勿改动）。
pub const GHOSTWRITER_INSTRUCTION_PROMPT: &str = "你是语音指令整理器。用户在用语音给 AI 助手下指令，下面是一段口语转写。\n把它整理成清晰、直接、结构清楚的指令：\n- 去掉口头语、重复、语气词（嗯、啊、就是那种、类似什么的）\n- 理顺语句顺序，需要时整理成简短要点\n- 把口语化的说法换成准确表述，但绝不改变用户的意思，绝不添加用户没说的要求\n- 原话里的具体信息（名字、数字、路径、代码、命令）一字不改\n- 只修正明显的同音错字与语义不通处，绝不增加、删除或改写用户已说清的内容。\n- 如果给了「参考材料」，把材料内容自然融合进指令对应的位置\n只输出整理后的指令文本，不要任何解释或前缀。";

/// 实时助手一次调用协同产出（候选/推荐）的输出契约，逐字拼接，界面只读展示。
pub const ASSIST_OUTPUT_CONTRACT: &str = "只输出 JSON，不要任何解释或代码块标记：\n{\"candidateGroups\":[{\"kind\":\"term|naming\",\"items\":[{\"name\":\"名字\",\"note\":\"注释或理由\"},…]}],\"recommendations\":[\"常用语id\",…]}\n没有的键给空数组或 null；候选最多 2 组、每组最多 5 条、总数最多 8 条；推荐最多 3 个 id；kind：term=叫法（note=大白话解释，回指说话人的说法）、naming=命名（note=起名理由）。";

/// 对话会话一次调用双产出（回话＋推荐）的输出契约，逐字拼接，界面只读展示。
pub const CONVERSATION_OUTPUT_CONTRACT: &str = "只输出 JSON，不要任何解释或代码块标记：\n{\"reply\":\"给说话人的一句话，或 null\",\"recommendations\":[\"常用语id\",…]}\nreply 为 null 表示这次闭嘴；推荐最多 3 个 id；没有的键给空数组或 null。";

/// 热词提取（词典页向导）的输出契约，逐字拼接，界面只读展示。
pub const HOTWORD_EXTRACTION_CONTRACT: &str = "只输出 JSON，不要任何解释或代码块标记：\n[{\"error\":\"原文错误写法\",\"hotword\":\"建议正确写法\",\"example\":\"例句\"}]\n没有就给空数组。";

/// 任务书身份：Ghostwriter 的七个 LLM 功能各对应一份任务书。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBriefId {
    /// 段润色：把口语转写整理成指令。
    InstructionPolish,
    /// 命名校准：把说话里说不清的点校准成叫法/命名，与卡词无关。
    Candidates,
    /// 常用语推荐：从常用语库挑与当前内容相关的条目。
    Recommendations,
    /// 提取常用语：从语音记录批量提取候选常用语（常用语管理页按需触发）。
    SedimentExtraction,
    /// 对话回话：管对话会话里 AI 什么时候说什么、怎么校准怎么追问。
    ConversationReply,
    /// 对话出稿：把整份聊天记录润写成最终指令。
    ConversationFinalize,
    /// 提取热词：从语音记录 raw 原文找疑似识别混乱的词（词典页向导按需触发）。
    HotwordExtraction,
}

impl TaskBriefId {
    /// 存储与 JSON 里的稳定键（覆写文件的 map 键）。
    pub fn key(self) -> &'static str {
        match self {
            Self::InstructionPolish => "instruction_polish",
            Self::Candidates => "candidates",
            Self::Recommendations => "recommendations",
            Self::SedimentExtraction => "sediment_extraction",
            Self::ConversationReply => "conversation_reply",
            Self::ConversationFinalize => "conversation_finalize",
            Self::HotwordExtraction => "hotword_extraction",
        }
    }

    /// 从稳定键解析任务书身份（命令层字符串入口）；未知键 → None。
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "instruction_polish" => Some(Self::InstructionPolish),
            "candidates" => Some(Self::Candidates),
            "recommendations" => Some(Self::Recommendations),
            "sediment_extraction" => Some(Self::SedimentExtraction),
            "conversation_reply" => Some(Self::ConversationReply),
            "conversation_finalize" => Some(Self::ConversationFinalize),
            "hotword_extraction" => Some(Self::HotwordExtraction),
            _ => None,
        }
    }

    /// 界面列表展示用标题。
    pub fn title(self) -> &'static str {
        match self {
            Self::InstructionPolish => "指令化润色",
            Self::Candidates => "命名校准",
            Self::Recommendations => "常用语推荐",
            Self::SedimentExtraction => "提取常用语",
            Self::ConversationReply => "对话回话",
            Self::ConversationFinalize => "对话出稿",
            Self::HotwordExtraction => "提取热词",
        }
    }

    /// 每份一句「管什么/改了会怎样」（中文，ui 文案同步用）。
    pub fn description(self) -> &'static str {
        match self {
            Self::InstructionPolish => "管段润色怎么把口语转写整理成指令，改了会影响贴给 AI 的指令。",
            Self::Candidates => {
                "管把说话里说不清的点校准成叫法/命名，改了会影响候选区。"
            }
            Self::Recommendations => "管从常用语库里挑哪些条目推荐，改了会影响推荐区。",
            Self::SedimentExtraction => {
                "管从语音记录提取候选常用语，改了会影响提取结果。"
            }
            Self::ConversationReply => {
                "管对话会话里 AI 什么时候说什么、怎么校准怎么追问，改了会影响回话。"
            }
            Self::ConversationFinalize => {
                "管聊天记录怎么润写成指令，改了会影响最终贴出的指令。"
            }
            Self::HotwordExtraction => {
                "管从语音记录 raw 原文里找识别混乱的词，改了会影响提取结果。"
            }
        }
    }

    /// 代码内置默认正文；无覆写时任务书存储回退到这里。
    pub fn default_body(self) -> &'static str {
        match self {
            Self::InstructionPolish => GHOSTWRITER_INSTRUCTION_PROMPT,
            Self::Candidates => {
                "你是语音说话助手。用户在用语音给 AI 助手下指令，下面是说话人最近的口语转写。\n\
                 说话里总有一些说不清的点：可能是一个东西、一个东西里的某个部分、几样东西之间的关系等等——说话人只能绕着描述，绕的时候常会现造一些词。不管说话人卡不卡词，都把这些点和现造的词逐个找出来校准，生造的、说偏的、碰巧说准的都不放过：\n\
                 - 业界已有叫法的，给叫法（term）：给出本名，note 写一句外行能懂的大白话解释，回指说话人的说法（如「就是你说的那个排队的机制」），让人认出指的就是它\n\
                 - 还没有公认名字、要新造的，给命名（naming）：按领域惯例起名，note 写一句起名理由，尽量沿用相关本名作词根\n\
                 一个点只出一个名字，优先挑最影响理解的点；候选要贴合正在说的内容，宁缺毋滥；没有要校准的就全部留空。"
            }
            Self::Recommendations => {
                "你是语音说话助手。用户在用语音给 AI 助手下指令，下面是说话人最近的口语转写和已启用的常用语库。\n\
                 从库里挑出与当前内容真正相关的常用语：意图或用词对得上、马上就能用上的才推荐，最多 3 个。\n\
                 只回常用语 id，按相关程度从高到低排；没有相关的就返回空数组，宁缺毋滥。"
            }
            Self::SedimentExtraction => {
                "你是语音常用语助手。下面是从用户历史语音记录里选出的若干段口语转写。从这些转写里找出值得存成常用语的说法：说法固定、以后还会口头用到、贴着用户自己的措辞习惯。每条给出：整理好的说法（phrase）、一个便于口头触发的短触发词（suggestedTrigger）、一句原话例句（example）。没有值得收的就返回空数组。"
            }
            Self::ConversationReply => {
                "你是语音对话助手。用户在用语音跟你对一场话，目的是把一件他想交办的事说清。\n\
                 下面是你们目前的聊天记录和他的最新发言。\n\
                 你的职责：理解他的真实意图；发现说不清、没说全、用词含糊的地方，用一句短话回他（指出歧义、给出本行叫法并配一句外行能懂的解释、或补一句他没想到的要点）。\n\
                 一次只说一句，不超过 80 字；他没有问题你就闭嘴（reply 给 null）；拿不准他的意思就问，但同一个点他回应过就不再纠缠。\n\
                 不要替他做决定，不要复述他的话，不要客套。"
            }
            Self::ConversationFinalize => {
                "你是语音指令整理器。下面是一段用户与助手的对话记录：【我】开头的都是用户说的话，【助手】开头的是助手为了消歧而说的话。\n\
                 把用户的真实意图整理成清晰、直接、结构清楚的指令：\n\
                 - 只整理【我】的内容；【助手】的内容只当已澄清的上下文，里面的措辞不得当成需求写进指令\n\
                 - 对话里已经说清的决定（叫法、方案、边界）要体现在指令里\n\
                 - 去掉口语、重复、语气词；原话里的具体信息（名字、数字、路径、代码、命令）一字不改\n\
                 - 只修正明显的同音错字与语义不通处，绝不增加、删除或改写用户已说清的内容。\n\
                 - 如果给了「参考材料」，把材料内容自然融合进指令对应的位置\n\
                 只输出整理后的指令文本，不要任何解释或前缀。"
            }
            Self::HotwordExtraction => {
                "你是语音热词助手。下面是用户历史语音记录的 ASR 原文（未经润色）。找出其中**疑似识别混乱的词**：同音错字、明显不通顺、语义突兀的片段——这些通常是 ASR 把专有名词、术语、人名识别错了。\n\
                 每条给出：原文中的错误写法（error）、建议的正确写法（hotword，将作为热词喂给 ASR 提升下次识别）、所在的例句（example）。\n\
                 只提有把握是识别错误的，宁缺毋滥；话题范围类的一般词不提（那不归热词管）；没有值得提的就返回空数组。"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_brief_ids_have_stable_keys_and_metadata() {
        let expected = [
            (TaskBriefId::InstructionPolish, "instruction_polish", "指令化润色"),
            (TaskBriefId::Candidates, "candidates", "命名校准"),
            (TaskBriefId::Recommendations, "recommendations", "常用语推荐"),
            (TaskBriefId::SedimentExtraction, "sediment_extraction", "提取常用语"),
            (TaskBriefId::ConversationReply, "conversation_reply", "对话回话"),
            (TaskBriefId::ConversationFinalize, "conversation_finalize", "对话出稿"),
            (TaskBriefId::HotwordExtraction, "hotword_extraction", "提取热词"),
        ];
        assert_eq!(expected.len(), 7);
        for (id, key, title) in expected {
            assert_eq!(id.key(), key);
            assert_eq!(TaskBriefId::from_key(key), Some(id));
            assert_eq!(id.title(), title);
            assert!(!id.description().is_empty());
            assert!(!id.default_body().is_empty());
        }
        // 新增几份的描述逐字钉住（界面文案同步用）。
        assert_eq!(
            TaskBriefId::ConversationReply.description(),
            "管对话会话里 AI 什么时候说什么、怎么校准怎么追问，改了会影响回话。"
        );
        assert_eq!(
            TaskBriefId::ConversationFinalize.description(),
            "管聊天记录怎么润写成指令，改了会影响最终贴出的指令。"
        );
        assert_eq!(
            TaskBriefId::HotwordExtraction.description(),
            "管从语音记录 raw 原文里找识别混乱的词，改了会影响提取结果。"
        );
        // 退役的任务书身份：旧键不再可解析。
        assert_eq!(TaskBriefId::from_key("sediment_notice"), None);
        assert_eq!(TaskBriefId::from_key("unknown"), None);
    }

    #[test]
    fn hotword_extraction_contract_is_verbatim() {
        assert!(HOTWORD_EXTRACTION_CONTRACT.starts_with("只输出 JSON，不要任何解释或代码块标记："));
        assert!(HOTWORD_EXTRACTION_CONTRACT
            .contains("[{\"error\":\"原文错误写法\",\"hotword\":\"建议正确写法\",\"example\":\"例句\"}]"));
        assert!(HOTWORD_EXTRACTION_CONTRACT.ends_with("没有就给空数组。"));
    }

    #[test]
    fn hotword_extraction_default_body_is_verbatim() {
        // 用户裁决（2026-09-18）：热词提取读 raw 原文（不是润色文本），只提
        // 识别错误、不管话题范围；正文逐字钉死，防改动影响提取口径。
        assert_eq!(
            TaskBriefId::HotwordExtraction.default_body(),
            "你是语音热词助手。下面是用户历史语音记录的 ASR 原文（未经润色）。找出其中**疑似识别混乱的词**：同音错字、明显不通顺、语义突兀的片段——这些通常是 ASR 把专有名词、术语、人名识别错了。\n每条给出：原文中的错误写法（error）、建议的正确写法（hotword，将作为热词喂给 ASR 提升下次识别）、所在的例句（example）。\n只提有把握是识别错误的，宁缺毋滥；话题范围类的一般词不提（那不归热词管）；没有值得提的就返回空数组。"
        );
    }

    #[test]
    fn instruction_polish_default_body_is_the_migrated_prompt() {
        assert_eq!(
            TaskBriefId::InstructionPolish.default_body(),
            GHOSTWRITER_INSTRUCTION_PROMPT
        );
    }

    #[test]
    fn assist_output_contract_is_verbatim() {
        assert!(ASSIST_OUTPUT_CONTRACT.starts_with("只输出 JSON，不要任何解释或代码块标记："));
        assert!(ASSIST_OUTPUT_CONTRACT.contains(
            "{\"candidateGroups\":[{\"kind\":\"term|naming\",\"items\":[{\"name\":\"名字\",\"note\":\"注释或理由\"},…]}]"
        ));
        assert!(ASSIST_OUTPUT_CONTRACT
            .contains("\"recommendations\":[\"常用语id\",…]"));
        // 常用语提醒已退役：契约里不再有 sediment 槽。
        assert!(!ASSIST_OUTPUT_CONTRACT.contains("sediment"));
        assert!(ASSIST_OUTPUT_CONTRACT.contains("没有的键给空数组或 null"));
        assert!(ASSIST_OUTPUT_CONTRACT.ends_with(
            "kind：term=叫法（note=大白话解释，回指说话人的说法）、naming=命名（note=起名理由）。"
        ));
    }

    #[test]
    fn conversation_output_contract_is_verbatim() {
        assert!(CONVERSATION_OUTPUT_CONTRACT.starts_with("只输出 JSON，不要任何解释或代码块标记："));
        assert!(CONVERSATION_OUTPUT_CONTRACT.contains(
            "{\"reply\":\"给说话人的一句话，或 null\",\"recommendations\":[\"常用语id\",…]}"
        ));
        assert!(CONVERSATION_OUTPUT_CONTRACT
            .contains("reply 为 null 表示这次闭嘴；推荐最多 3 个 id"));
        assert!(CONVERSATION_OUTPUT_CONTRACT.ends_with("没有的键给空数组或 null。"));
    }

    #[test]
    fn correction_boundary_clause_is_pinned_in_both_finalize_bodies() {
        // 用户裁决（2026-09-18）：润色层有边界修正——只修同音错字与明显语义
        // 不通，绝不增删/改写用户已说清的内容。两份终稿任务书（指令化润色/
        // 对话出稿）都必须逐字带这条口径，且紧跟「具体信息一字不改」条款之后。
        let clause = "只修正明显的同音错字与语义不通处，绝不增加、删除或改写用户已说清的内容。";
        for body in [GHOSTWRITER_INSTRUCTION_PROMPT, TaskBriefId::ConversationFinalize.default_body()] {
            let pos_info = body.find("一字不改").expect("具体信息锚点");
            let pos_clause = body.find(clause).expect("纠错口径必须逐字存在");
            assert!(pos_info < pos_clause, "纠错口径应落在具体信息条款之后: {body}");
        }
    }

    #[test]
    fn conversation_default_bodies_are_verbatim() {
        assert_eq!(
            TaskBriefId::ConversationReply.default_body(),
            "你是语音对话助手。用户在用语音跟你对一场话，目的是把一件他想交办的事说清。\n\
             下面是你们目前的聊天记录和他的最新发言。\n\
             你的职责：理解他的真实意图；发现说不清、没说全、用词含糊的地方，用一句短话回他（指出歧义、给出本行叫法并配一句外行能懂的解释、或补一句他没想到的要点）。\n\
             一次只说一句，不超过 80 字；他没有问题你就闭嘴（reply 给 null）；拿不准他的意思就问，但同一个点他回应过就不再纠缠。\n\
             不要替他做决定，不要复述他的话，不要客套。"
        );
        assert_eq!(
            TaskBriefId::ConversationFinalize.default_body(),
            "你是语音指令整理器。下面是一段用户与助手的对话记录：【我】开头的都是用户说的话，【助手】开头的是助手为了消歧而说的话。\n\
             把用户的真实意图整理成清晰、直接、结构清楚的指令：\n\
             - 只整理【我】的内容；【助手】的内容只当已澄清的上下文，里面的措辞不得当成需求写进指令\n\
             - 对话里已经说清的决定（叫法、方案、边界）要体现在指令里\n\
             - 去掉口语、重复、语气词；原话里的具体信息（名字、数字、路径、代码、命令）一字不改\n\
             - 只修正明显的同音错字与语义不通处，绝不增加、删除或改写用户已说清的内容。\n\
             - 如果给了「参考材料」，把材料内容自然融合进指令对应的位置\n\
             只输出整理后的指令文本，不要任何解释或前缀。"
        );
    }

    #[test]
    fn default_bodies_do_not_leak_prompt_wording() {
        for id in [
            TaskBriefId::InstructionPolish,
            TaskBriefId::Candidates,
            TaskBriefId::Recommendations,
            TaskBriefId::SedimentExtraction,
            TaskBriefId::ConversationReply,
            TaskBriefId::ConversationFinalize,
            TaskBriefId::HotwordExtraction,
        ] {
            assert!(!id.default_body().contains("提示词"));
            assert!(!id.description().contains("提示词"));
        }
    }
}
