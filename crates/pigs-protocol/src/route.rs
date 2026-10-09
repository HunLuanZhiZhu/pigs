//! 裸路径 → 协议判定，与 `-pigs` / `-pig`（兼容 `-pigsb`）后缀规则（全仓库唯一定义处）。


/// PIGS 输出模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PigsMode {
    /// 模式 A：各阶段普通文本按执行顺序对客户端可见。
    A,
    /// 模式 B：内部阶段普通文本隐藏，只提交最终被接受的业务答案。
    B,
}

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

/// 解析 PIGS model 后缀，返回真正上游 model 名与输出模式。
///
/// - `<model>-pigs`  → 模式 A（拼接阶段输出）
/// - `<model>-pig`   → 模式 B（单一最终业务输出）
/// - `<model>-pigsb` → 模式 B（旧后缀兼容）
pub fn parse_pigs_model(model: &str) -> Option<(String, PigsMode)> {
    if let Some(real) = model.strip_suffix("-pigfull") {
        Some((real.to_string(), PigsMode::B))
    } else if let Some(real) = model.strip_suffix("-pigsb") {
        Some((real.to_string(), PigsMode::B))
    } else if let Some(real) = model.strip_suffix("-pigs") {
        Some((real.to_string(), PigsMode::A))
    } else {
        model
            .strip_suffix("-pig")
            .map(|real| (real.to_string(), PigsMode::B))
    }
}

/// model 名是否携带任一种 PIGS 后缀。
pub fn has_pigs(model: &str) -> bool {
    parse_pigs_model(model).is_some()
}

/// 剥掉任一种 PIGS 后缀，只返回真正的上游 model 名。
/// 需要区分 A/B 时使用 [`parse_pigs_model`]。
pub fn strip_pigs_suffix(model: &str) -> Option<String> {
    parse_pigs_model(model).map(|(real, _)| real)
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
        assert!(has_pigs("claude-opus-5-pigsb"));
        assert!(has_pigs("claude-opus-5-pig"));
        assert!(has_pigs("claude-opus-5-pigfull"));
        assert!(!has_pigs("claude-opus-5"));

        assert_eq!(
            parse_pigs_model("claude-opus-5-pigs"),
            Some(("claude-opus-5".into(), PigsMode::A))
        );
        assert_eq!(
            parse_pigs_model("claude-opus-5-pig"),
            Some(("claude-opus-5".into(), PigsMode::B))
        );
        assert_eq!(
            parse_pigs_model("claude-opus-5-pigsb"),
            parse_pigs_model("claude-opus-5-pig")
        );
        assert_eq!(strip_pigs_suffix("deepseek-v4.1-flash-pig").as_deref(), Some("deepseek-v4.1-flash"));
        assert_eq!(strip_pigs_suffix("deepseek-v4.1-flash-pigfull").as_deref(), Some("deepseek-v4.1-flash"));
        assert_eq!(parse_pigs_model("muse-spark-1.3-contributor-pigfull"), Some(("muse-spark-1.3-contributor".into(), PigsMode::B)));
        assert_eq!(strip_pigs_suffix("deepseek-v4.1-flash-pigsb").as_deref(), Some("deepseek-v4.1-flash"));
        assert_eq!(strip_pigs_suffix("claude-opus-5"), None);
        assert_eq!(strip_pigs_suffix("-pigs").as_deref(), Some(""));
        assert_eq!(strip_pigs_suffix("-pig").as_deref(), Some(""));
        assert_eq!(strip_pigs_suffix("-pigsb").as_deref(), Some(""));
    }

}
