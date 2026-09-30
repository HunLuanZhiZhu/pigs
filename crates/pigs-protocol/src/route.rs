//! 裸路径 → 协议判定，与 `-pigs` 后缀规则（全仓库唯一定义处）。

/// 支持的三种协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// OpenAI Chat Completions。
    OpenAI,
    /// Anthropic Messages。
    Anthropic,
    /// OpenAI Responses。
    Responses,
}

impl Protocol {
    /// 透传到上游时的路径后缀（base_url + 后缀 = 完整上游 URL）。
    pub fn append_suffix(self) -> &'static str {
        match self {
            Protocol::OpenAI => "/chat/completions",
            Protocol::Anthropic => "/v1/messages",
            Protocol::Responses => "/responses",
        }
    }
}

/// 按裸路径判定协议；未识别路径返回 None。
pub fn protocol_from_path(path: &str) -> Option<Protocol> {
    let p = path.trim_start_matches('/');
    if p == "chat/completions" || p.ends_with("/chat/completions") {
        Some(Protocol::OpenAI)
    } else if p == "v1/messages" || p.ends_with("/v1/messages") {
        Some(Protocol::Anthropic)
    } else if p == "responses" || p.ends_with("/responses") {
        Some(Protocol::Responses)
    } else {
        None
    }
}

/// model 名是否携带 `-pigs` 后缀。
pub fn has_pigs(model: &str) -> bool {
    model.ends_with("-pigs")
}

/// 剥掉 `-pigs` 后缀，返回真正的上游 model 名；无后缀返回 None。
///
/// 例：`"claude-opus-5-pigs"` → `Some("claude-opus-5")`；`"claude-opus-5"` → `None`。
pub fn strip_pigs_suffix(model: &str) -> Option<String> {
    model.strip_suffix("-pigs").map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_bare_paths() {
        assert_eq!(protocol_from_path("/chat/completions"), Some(Protocol::OpenAI));
        assert_eq!(protocol_from_path("/v1/messages"), Some(Protocol::Anthropic));
        assert_eq!(protocol_from_path("/responses"), Some(Protocol::Responses));
        assert_eq!(protocol_from_path("/v1/models"), None);
        assert_eq!(protocol_from_path("/"), None);
    }

    #[test]
    fn suffixes_for_upstream_url() {
        assert_eq!(Protocol::OpenAI.append_suffix(), "/chat/completions");
        assert_eq!(Protocol::Anthropic.append_suffix(), "/v1/messages");
        assert_eq!(Protocol::Responses.append_suffix(), "/responses");
    }

    #[test]
    fn pigs_suffix_rules() {
        assert!(has_pigs("claude-opus-5-pigs"));
        assert!(!has_pigs("claude-opus-5-pig"));
        assert!(!has_pigs("claude-opus-5"));
        assert_eq!(strip_pigs_suffix("claude-opus-5-pigs").as_deref(), Some("claude-opus-5"));
        assert_eq!(strip_pigs_suffix("claude-opus-5-pig"), None);
        assert_eq!(strip_pigs_suffix("deepseek-v4.1-flash-pigs").as_deref(), Some("deepseek-v4.1-flash"));
        assert_eq!(strip_pigs_suffix("claude-opus-5"), None);
        // 只有后缀本身时，剥离结果为空模型名
        assert_eq!(strip_pigs_suffix("-pigs").as_deref(), Some(""));
    }
}
