//! 语言自动选择：按用户问题中的 CJK 字符占比判定提示词语言。

/// 提示词语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// 中文模板。
    Zh,
    /// 英文模板。
    En,
}

/// 扫描文本前 2000 个字符：出现 CJK 字符即判为中文（用户群以中文为主，宁可选 zh）。
pub fn detect_lang(text: &str) -> Lang {
    let sample: Vec<char> = text.chars().take(2000).collect();
    let has_cjk = sample.iter().any(|&c| {
        ('\u{4E00}'..='\u{9FFF}').contains(&c)
            || ('\u{3400}'..='\u{4DBF}').contains(&c)
    });
    if has_cjk { Lang::Zh } else { Lang::En }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_by_cjk_presence() {
        assert_eq!(detect_lang("帮我修复这个 bug"), Lang::Zh);
        assert_eq!(detect_lang("fix this bug"), Lang::En);
        assert_eq!(detect_lang(""), Lang::En);
        assert_eq!(detect_lang("总结：summary of the task"), Lang::Zh);
    }
}
