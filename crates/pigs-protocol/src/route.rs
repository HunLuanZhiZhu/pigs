//! 裸路径协议识别与 PIGS 模型后缀规则的唯一定义处。 / Single source of truth for bare-path protocol detection and PIGS model suffixes.


/// PIGS 输出模式。 / PIGS output mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PigsMode {
    /// 模式 A：各阶段普通文本按顺序可见。 / Mode A: phase text remains visible in execution order.
    A,
    /// 模式 B：隐藏内部阶段文本，只提交被接受的业务答案。 / Mode B: hide internal phase text and return only the accepted business answer.
    B,
}

/// 支持的三种协议。 / Three supported protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// OpenAI 聊天补全协议。 / OpenAI Chat Completions protocol.
    OpenAI,
    /// Anthropic 消息协议。 / Anthropic Messages protocol.
    Anthropic,
    /// OpenAI 响应协议。 / OpenAI Responses protocol.
    Responses,
}

impl Protocol {
    /// 上游路径后缀；基础地址加此后缀构成完整 URL。 / Upstream path suffix appended to the base URL.
    pub fn append_suffix(self) -> &'static str {
        match self {
            Protocol::OpenAI => "/chat/completions",
            Protocol::Anthropic => "/v1/messages",
            Protocol::Responses => "/responses",
        }
    }
}

/// 根据裸路径识别协议；未知路径返回 None。 / Detect the protocol from a bare path; return None if unknown.
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

/// 解析 PIGS 模型后缀，返回上游模型名和输出模式。 / Parse the PIGS suffix into the upstream model name and output mode.
///
/// - `<model>-pigs`  → 模式 A，拼接阶段输出 / Mode A, combined phase output
/// - `<model>-pig`   → 模式 B，最终业务输出 / Mode B, final business output
/// - `<model>-pig3`  → 模式 B，强制三阶段 / Mode B, forced three-phase path
/// - `<model>-pigs3` → 模式 A，强制三阶段 / Mode A, forced three-phase path
pub fn parse_pigs_model(model: &str) -> Option<(String, PigsMode)> {
    if let Some(real) = model.strip_suffix("-pigs3") {
        Some((real.to_string(), PigsMode::A))
    } else if let Some(real) = model.strip_suffix("-pig3") {
        Some((real.to_string(), PigsMode::B))
    } else if let Some(real) = model.strip_suffix("-pigs") {
        Some((real.to_string(), PigsMode::A))
    } else {
        model
            .strip_suffix("-pig")
            .map(|real| (real.to_string(), PigsMode::B))
    }
}

/// 模型名是否有受支持的 PIGS 后缀。 / Whether the model name has a supported PIGS suffix.
pub fn has_pigs(model: &str) -> bool {
    parse_pigs_model(model).is_some()
}

/// 去掉 PIGS 后缀，返回上游模型名。 / Remove the PIGS suffix and return the upstream model name.
/// 需要区分 A/B 时使用 [`parse_pigs_model`]。 / Use [`parse_pigs_model`] when the output mode is needed.
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
        assert!(!has_pigs("claude-opus-5-pigsb"));
        assert!(has_pigs("claude-opus-5-pig"));
        assert!(has_pigs("claude-opus-5-pig3"));
        assert!(has_pigs("claude-opus-5-pigs3"));
        assert!(!has_pigs("claude-opus-5-pigfull"));
        assert!(!has_pigs("claude-opus-5"));

        assert_eq!(
            parse_pigs_model("claude-opus-5-pigs"),
            Some(("claude-opus-5".into(), PigsMode::A))
        );
        assert_eq!(
            parse_pigs_model("claude-opus-5-pig"),
            Some(("claude-opus-5".into(), PigsMode::B))
        );
        assert_eq!(parse_pigs_model("claude-opus-5-pigsb"), None);
        assert_eq!(strip_pigs_suffix("deepseek-v4.1-flash-pig").as_deref(), Some("deepseek-v4.1-flash"));
        assert_eq!(strip_pigs_suffix("deepseek-v4.1-flash-pig3").as_deref(), Some("deepseek-v4.1-flash"));
        assert_eq!(strip_pigs_suffix("deepseek-v4.1-flash-pigs3").as_deref(), Some("deepseek-v4.1-flash"));
        assert_eq!(parse_pigs_model("muse-spark-1.3-contributor-pig3"), Some(("muse-spark-1.3-contributor".into(), PigsMode::B)));
        assert_eq!(parse_pigs_model("muse-spark-1.3-contributor-pigs3"), Some(("muse-spark-1.3-contributor".into(), PigsMode::A)));
        assert_eq!(strip_pigs_suffix("deepseek-v4.1-flash-pigsb"), None);
        assert_eq!(strip_pigs_suffix("claude-opus-5"), None);
        assert_eq!(strip_pigs_suffix("-pigs").as_deref(), Some(""));
        assert_eq!(strip_pigs_suffix("-pig").as_deref(), Some(""));
    }

}
