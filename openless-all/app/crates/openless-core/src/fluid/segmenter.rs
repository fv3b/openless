//! 中英混合断句：把流式转写缓冲切成「可润色的完整句」。
//!
//! 使用方式：持有当前完整缓冲文本的一方（M2 起是 `FluidSession` 的
//! `TranscriptAccumulator`）每次文本变化后调 [`Segmenter::update`]，
//! 拿到本次新完成的段；未完成的尾巴用 [`Segmenter::tail`] 读取，
//! 会话结束时补润。
//!
//! 边界规则（按优先级）：
//! 1. 句末标点（。！？；!?;）后跟任意字符 → 断。要求后面还有内容，
//!    因为 ASR partial 随时可能在句末标点后继续追加；
//! 2. 英文句点 `.` 后跟空白 → 断（避开 `3.14`、URL 里的点）；
//! 3. 从段首起超过 `max_force_chars` 仍无边界 → 硬切（ASR 长时间不出标点）。
//!
//! 字符位置一律按 `char` 计（中文安全）；`end_char` 是该段结尾在缓冲中的
//! 字符下标（不含），供 M2 的「命令应用后锁定 offset」用。

/// 一段已完成的文本：段内容 ＋ 它在缓冲中的字符结束位置。
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub text: String,
    /// 该段结尾在缓冲文本中的字符下标（不含该下标本身）。
    pub end_char: usize,
}

const SENTENCE_ENDERS: &[char] = &['。', '！', '？', '；', '!', '?', ';'];

pub struct Segmenter {
    /// 已作为完整段发射出去的缓冲前缀长度（字符数）。
    finalized_char: usize,
    /// 无标点尾巴超过该字符数时硬切。
    max_force_chars: usize,
}

impl Segmenter {
    pub fn new(max_force_chars: usize) -> Self {
        Self {
            finalized_char: 0,
            max_force_chars: max_force_chars.max(1),
        }
    }

    /// 给定当前完整缓冲，返回本次新完成的段（可能为空）。
    pub fn update(&mut self, text: &str) -> Vec<Segment> {
        let chars: Vec<char> = text.chars().collect();
        let total = chars.len();
        // 修订可能把缓冲缩回已发射位置之前：钳制，交给下次长回来后继续。
        let mut seg_start = self.finalized_char.min(total);
        let mut segments = Vec::new();
        let mut i = seg_start;
        while i < total {
            let c = chars[i];
            let is_sentence_ender = SENTENCE_ENDERS.contains(&c);
            let is_english_period =
                c == '.' && i + 1 < total && chars[i + 1].is_whitespace();
            if (is_sentence_ender || is_english_period) && i + 1 < total {
                // 段文本吃到句末标点为止；发射位置跳过其后空白，
                // 尾巴不留前导空格（英文句点后必跟空格）。
                let mut end = i + 1;
                while end < total && chars[end].is_whitespace() {
                    end += 1;
                }
                segments.push(Segment {
                    text: chars[seg_start..i + 1].iter().collect(),
                    end_char: end,
                });
                seg_start = end;
                i = end;
                continue;
            }
            if i - seg_start + 1 >= self.max_force_chars && i + 1 < total {
                let end = i + 1;
                segments.push(Segment {
                    text: chars[seg_start..end].iter().collect(),
                    end_char: end,
                });
                seg_start = end;
            }
            i += 1;
        }
        self.finalized_char = seg_start;
        segments
    }

    /// 尚未完成的尾巴（从上次发射位置到缓冲末尾）。
    pub fn tail<'a>(&self, text: &'a str) -> &'a str {
        let total = text.chars().count();
        let start = self.finalized_char.min(total);
        let byte = text
            .char_indices()
            .nth(start)
            .map(|(b, _)| b)
            .unwrap_or(text.len());
        &text[byte..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, end_char: usize) -> Segment {
        Segment {
            text: text.to_string(),
            end_char,
        }
    }

    #[test]
    fn empty_buffer_yields_no_segments() {
        let mut s = Segmenter::new(40);
        assert_eq!(s.update(""), vec![]);
        assert_eq!(s.tail(""), "");
    }

    #[test]
    fn trailing_sentence_punct_without_following_text_is_not_final() {
        // 句末标点后没有更多字符：可能还会继续追加，不当完成。
        let mut s = Segmenter::new(40);
        assert_eq!(s.update("今天天气很好。"), vec![]);
        assert_eq!(s.tail("今天天气很好。"), "今天天气很好。");
    }

    #[test]
    fn emits_sentence_when_more_text_follows() {
        let mut s = Segmenter::new(40);
        let text = "今天天气很好。我们继续";
        assert_eq!(s.update(text), vec![seg("今天天气很好。", 7)]);
        assert_eq!(s.tail(text), "我们继续");
    }

    #[test]
    fn question_and_exclamation_are_boundaries() {
        let mut s = Segmenter::new(40);
        let text = "去哪？回家！后来";
        assert_eq!(s.update(text), vec![seg("去哪？", 3), seg("回家！", 6)]);
        assert_eq!(s.tail(text), "后来");
    }

    #[test]
    fn semicolon_is_boundary() {
        let mut s = Segmenter::new(40);
        let text = "第一；第二。尾";
        assert_eq!(s.update(text), vec![seg("第一；", 3), seg("第二。", 6)]);
        assert_eq!(s.tail(text), "尾");
    }

    #[test]
    fn comma_is_not_boundary() {
        let mut s = Segmenter::new(40);
        let text = "今天天气，很好，不错的";
        assert_eq!(s.update(text), vec![]);
        assert_eq!(s.tail(text), text);
    }

    #[test]
    fn english_period_with_space_is_boundary() {
        let mut s = Segmenter::new(40);
        let text = "Hello world. Next sentence";
        // 发射位置跳过句点后的空格：尾巴无前导空白。
        assert_eq!(s.update(text), vec![seg("Hello world.", 13)]);
        assert_eq!(s.tail(text), "Next sentence");
    }

    #[test]
    fn decimal_point_is_not_boundary() {
        let mut s = Segmenter::new(40);
        let text = "值是3.14159左右";
        assert_eq!(s.update(text), vec![]);
    }

    #[test]
    fn force_splits_long_punctuationless_tail() {
        let mut s = Segmenter::new(4);
        let text = "一二三四五六七八九十";
        // 4 字符硬切，切到剩余不足 4 字为止：两段各 4 字，剩 2 字为尾巴。
        assert_eq!(s.update(text), vec![seg("一二三四", 4), seg("五六七八", 8)]);
        assert_eq!(s.tail(text), "九十");
    }

    #[test]
    fn incremental_updates_do_not_reemit() {
        let mut s = Segmenter::new(40);
        assert_eq!(s.update("你好。"), vec![]);
        assert_eq!(s.update("你好。世界"), vec![seg("你好。", 3)]);
        assert_eq!(s.update("你好。世界。再"), vec![seg("世界。", 6)]);
        assert_eq!(s.tail("你好。世界。再"), "再");
    }

    #[test]
    fn revision_shrinking_buffer_clamps_without_panic() {
        let mut s = Segmenter::new(40);
        assert_eq!(
            s.update("一二三四五六七八九十。xyz"),
            vec![seg("一二三四五六七八九十。", 11)]
        );
        // ASR partial 修订把缓冲缩回已发射位置之前：钳制到末尾、不 panic、不重发。
        // 「已发射段与修订后缓冲对不上」的重建是 M2 FluidSession 的职责。
        assert_eq!(s.update("一二三"), vec![]);
        assert_eq!(s.tail("一二三"), "");
        // 缓冲重新长回去后，从钳制位置继续发射。
        assert_eq!(
            s.update("一二三四五六七八九十。new"),
            vec![seg("四五六七八九十。", 11)]
        );
        assert_eq!(s.tail("一二三四五六七八九十。new"), "new");
    }
}
