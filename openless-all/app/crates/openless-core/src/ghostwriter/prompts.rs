//! 任务书注册表：Ghostwriter 各 LLM 功能的任务说明正文（默认内置，可在任务书存储里覆写）。
//!
//! 按任务书边界（ADR 0002）：可编辑的只有任务正文；数据注入与输出结构由代码固定
//! （[`ASSIST_OUTPUT_CONTRACT`] 为输出契约，逐字拼接，界面只读展示）。
//! 默认正文是中文数据本身，不走 i18n 键。

/// 段润色的指令化 system prompt（逐字使用，勿改动）。
pub const GHOSTWRITER_INSTRUCTION_PROMPT: &str = "你是语音指令整理器。用户在用语音给 AI 助手下指令，下面是一段口语转写。\n把它整理成清晰、直接、结构清楚的指令：\n- 去掉口头语、重复、语气词（嗯、啊、就是那种、类似什么的）\n- 理顺语句顺序，需要时整理成简短要点\n- 把口语化的说法换成准确表述，但绝不改变用户的意思，绝不添加用户没说的要求\n- 原话里的具体信息（名字、数字、路径、代码、命令）一字不改\n- 如果给了「参考材料」，把材料内容自然融合进指令对应的位置\n只输出整理后的指令文本，不要任何解释或前缀。";

/// 实时助手一次调用协同产出（候选/推荐/沉淀提醒）的输出契约，逐字拼接，界面只读展示。
pub const ASSIST_OUTPUT_CONTRACT: &str = "只输出 JSON，不要任何解释或代码块标记：\n{\"candidateGroups\":[{\"kind\":\"term|phrase|naming\",\"items\":[\"候选文本\",…]}],\"recommendations\":[\"常用语id\",…],\"sediment\":{\"phrase\":\"说法\",\"count\":出现次数,\"suggestedTrigger\":\"触发词\"}}\n没有的键给空数组或 null；候选最多 2 组、每组最多 5 条、总数最多 8 条；推荐最多 3 个 id；kind：term=精准词、phrase=候选表述、naming=命名。";

/// 任务书身份：Ghostwriter 的五个 LLM 功能各对应一份任务书。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBriefId {
    /// 段润色：把口语转写整理成指令。
    InstructionPolish,
    /// 候选生成：卡词时给精准词/候选表述/命名建议。
    Candidates,
    /// 推荐挑选：从常用语库挑与当前内容相关的条目。
    Recommendations,
    /// 沉淀提醒：判断当前内容是否在重复某个未入库说法。
    SedimentNotice,
    /// 沉淀抽取：从说话内容里抽取值得沉淀的说法。
    SedimentExtraction,
}

impl TaskBriefId {
    /// 存储与 JSON 里的稳定键（覆写文件的 map 键）。
    pub fn key(self) -> &'static str {
        match self {
            Self::InstructionPolish => "instruction_polish",
            Self::Candidates => "candidates",
            Self::Recommendations => "recommendations",
            Self::SedimentNotice => "sediment_notice",
            Self::SedimentExtraction => "sediment_extraction",
        }
    }

    /// 从稳定键解析任务书身份（命令层字符串入口）；未知键 → None。
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "instruction_polish" => Some(Self::InstructionPolish),
            "candidates" => Some(Self::Candidates),
            "recommendations" => Some(Self::Recommendations),
            "sediment_notice" => Some(Self::SedimentNotice),
            "sediment_extraction" => Some(Self::SedimentExtraction),
            _ => None,
        }
    }

    /// 界面列表展示用标题。
    pub fn title(self) -> &'static str {
        match self {
            Self::InstructionPolish => "指令化润色",
            Self::Candidates => "候选生成",
            Self::Recommendations => "推荐挑选",
            Self::SedimentNotice => "沉淀提醒",
            Self::SedimentExtraction => "沉淀抽取",
        }
    }

    /// 每份一句「管什么/改了会怎样」（中文，ui 文案同步用）。
    pub fn description(self) -> &'static str {
        match self {
            Self::InstructionPolish => "管段润色怎么把口语转写整理成指令，改了会影响贴给 AI 的指令。",
            Self::Candidates => {
                "管说话卡词时出不出候选（精准词/候选表述/命名建议），改了会影响候选区。"
            }
            Self::Recommendations => "管从常用语库里挑哪些条目推荐，改了会影响推荐区。",
            Self::SedimentNotice => "管判断当前内容是否在重复未入库说法，改了会影响沉淀提醒。",
            Self::SedimentExtraction => "管从说话内容里抽取哪些说法去沉淀，改了会影响沉淀抽取结果。",
        }
    }

    /// 代码内置默认正文；无覆写时任务书存储回退到这里。
    pub fn default_body(self) -> &'static str {
        match self {
            Self::InstructionPolish => GHOSTWRITER_INSTRUCTION_PROMPT,
            Self::Candidates => {
                "你是语音说话助手。用户在用语音给 AI 助手下指令，下面是说话人最近的口语转写。\n\
                 判断说话人此刻是否在卡词：重复、停顿、含糊，或用「那个」「就是那种」指代说不清的东西，都是卡词迹象。\n\
                 卡词时从转写里推断说话人想表达什么，给出现成的候选：\n\
                 - 需要更准的词，给精准词（term）\n\
                 - 说法绕、不顺口，给更顺的候选表述（phrase）\n\
                 - 正在给东西起名字，给命名建议（naming）\n\
                 候选要贴合正在说的内容，宁缺毋滥，不重复说话人已经说清楚的部分；没有卡词迹象就全部留空。"
            }
            Self::Recommendations => {
                "你是语音说话助手。用户在用语音给 AI 助手下指令，下面是说话人最近的口语转写和已启用的常用语库。\n\
                 从库里挑出与当前内容真正相关的常用语：意图或用词对得上、马上就能用上的才推荐，最多 3 个。\n\
                 只回常用语 id，按相关程度从高到低排；没有相关的就返回空数组，宁缺毋滥。"
            }
            Self::SedimentNotice => {
                "你是语音说话助手。用户在用语音给 AI 助手下指令，下面是说话人最近的口语转写和历史重复记录。\n\
                 判断说话人当前是否又在说某条还没入库的说法：拿本次转写和重复记录逐条比对，语义一致才算重复，只是用词相近不算。\n\
                 确认重复时给出说法原文、累计出现次数，和一个便于以后口头触发的短触发词；没有重复就留空。"
            }
            Self::SedimentExtraction => {
                "你是语音说话助手。用户在用语音给 AI 助手下指令，下面是说话人最近的口语转写。\n\
                 从转写里找出值得沉淀成常用语的说法：反复出现、说法固定、以后还会用到的表述才值得收。\n\
                 每条给出整理好的说法和一句原话例句；没有值得沉淀的就返回空数组。"
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
            (TaskBriefId::Candidates, "candidates", "候选生成"),
            (TaskBriefId::Recommendations, "recommendations", "推荐挑选"),
            (TaskBriefId::SedimentNotice, "sediment_notice", "沉淀提醒"),
            (TaskBriefId::SedimentExtraction, "sediment_extraction", "沉淀抽取"),
        ];
        for (id, key, title) in expected {
            assert_eq!(id.key(), key);
            assert_eq!(TaskBriefId::from_key(key), Some(id));
            assert_eq!(id.title(), title);
            assert!(!id.description().is_empty());
            assert!(!id.default_body().is_empty());
        }
        assert_eq!(TaskBriefId::from_key("unknown"), None);
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
            "{\"candidateGroups\":[{\"kind\":\"term|phrase|naming\",\"items\":[\"候选文本\",…]}]"
        ));
        assert!(ASSIST_OUTPUT_CONTRACT
            .contains("\"recommendations\":[\"常用语id\",…]"));
        assert!(ASSIST_OUTPUT_CONTRACT.contains(
            "\"sediment\":{\"phrase\":\"说法\",\"count\":出现次数,\"suggestedTrigger\":\"触发词\"}"
        ));
        assert!(ASSIST_OUTPUT_CONTRACT.contains("没有的键给空数组或 null"));
        assert!(ASSIST_OUTPUT_CONTRACT
            .ends_with("kind：term=精准词、phrase=候选表述、naming=命名。"));
    }

    #[test]
    fn default_bodies_do_not_leak_prompt_wording() {
        for id in [
            TaskBriefId::InstructionPolish,
            TaskBriefId::Candidates,
            TaskBriefId::Recommendations,
            TaskBriefId::SedimentNotice,
            TaskBriefId::SedimentExtraction,
        ] {
            assert!(!id.default_body().contains("提示词"));
            assert!(!id.description().contains("提示词"));
        }
    }
}
