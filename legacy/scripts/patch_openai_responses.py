#!/usr/bin/env python3
"""Wire OpenAI Responses API into pigs-llm / config / cli."""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def write_lib() -> None:
    path = ROOT / "crates/pigs-llm/src/lib.rs"
    path.write_text(
        """//! LLM provider clients — OpenAI Responses / Chat Completions and Anthropic Messages.

pub mod anthropic;
pub mod openai;
pub mod openai_responses;
pub mod provider;

pub use anthropic::AnthropicClient;
pub use openai::OpenAiClient;
pub use openai_responses::OpenAiResponsesClient;
pub use provider::{
    create_client, create_client_for_endpoint, create_client_with_config, detect_provider,
    resolve_model_alias, ClientConfig, Provider,
};
""",
        encoding="utf-8",
    )
    print("wrote", path)


def patch_provider() -> None:
    path = ROOT / "crates/pigs-llm/src/provider.rs"
    text = path.read_text(encoding="utf-8")

    if "openai_responses" not in text:
        text = text.replace(
            "use crate::anthropic::AnthropicClient;\nuse crate::openai::OpenAiClient;\n",
            "use crate::anthropic::AnthropicClient;\n"
            "use crate::openai::OpenAiClient;\n"
            "use crate::openai_responses::OpenAiResponsesClient;\n",
        )

    old_enum = """/// Wire format used to talk to an LLM endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// OpenAI Chat Completions (`/v1/chat/completions`), including compatible proxies.
    OpenAI,
    /// Anthropic Messages API (`/v1/messages`).
    Anthropic,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}
"""
    new_enum = """/// Wire format used to talk to an LLM endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// OpenAI Responses API (`/v1/responses`) — current OpenAI agent protocol (Codex default).
    OpenAI,
    /// OpenAI Chat Completions (`/v1/chat/completions`) — legacy / third-party compatible.
    OpenAIChat,
    /// Anthropic Messages API (`/v1/messages`).
    Anthropic,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::OpenAIChat => "openai-chat",
            Self::Anthropic => "anthropic",
        }
    }
}
"""
    if "OpenAIChat" not in text:
        if old_enum not in text:
            raise SystemExit("provider enum block not found")
        text = text.replace(old_enum, new_enum)

    old_base = """    let base_url = if base_url.trim().is_empty() {
        match api {
            Provider::OpenAI => "https://api.openai.com/v1",
            Provider::Anthropic => "https://api.anthropic.com",
        }
    } else {
        base_url
    };
"""
    new_base = """    let base_url = if base_url.trim().is_empty() {
        match api {
            Provider::OpenAI | Provider::OpenAIChat => "https://api.openai.com/v1",
            Provider::Anthropic => "https://api.anthropic.com",
        }
    } else {
        base_url
    };
"""
    if old_base in text:
        text = text.replace(old_base, new_base)

    old_match = """    match api {
        Provider::Anthropic => Ok(Arc::new(AnthropicClient::new(api_key, model, base_url))),
        Provider::OpenAI => Ok(Arc::new(OpenAiClient::new(api_key, model, base_url))),
    }
"""
    new_match = """    match api {
        Provider::Anthropic => Ok(Arc::new(AnthropicClient::new(api_key, model, base_url))),
        Provider::OpenAI => Ok(Arc::new(OpenAiResponsesClient::new(api_key, model, base_url))),
        Provider::OpenAIChat => Ok(Arc::new(OpenAiClient::new(api_key, model, base_url))),
    }
"""
    if old_match not in text:
        raise SystemExit("create_client_for_endpoint match not found")
    text = text.replace(old_match, new_match)

    # create_client_with_config still uses detect_provider -> OpenAI (Responses)
    # Add a comment in detect_provider if needed.
    path.write_text(text, encoding="utf-8")
    print("patched", path)


def patch_config() -> None:
    path = ROOT / "crates/pigs-config/src/config.rs"
    text = path.read_text(encoding="utf-8")

    old = """pub enum ApiFormat {
    /// OpenAI Chat Completions (`/v1/chat/completions`).
    #[default]
    OpenAI,
    /// Anthropic Messages (`/v1/messages`).
    Anthropic,
}

impl ApiFormat {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-compatible" | "chat" | "completions" => Ok(Self::OpenAI),
            "anthropic" | "claude" | "messages" => Ok(Self::Anthropic),
            other => Err(format!(
                "unknown api format '{other}' (expected openai or anthropic)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}
"""
    new = """pub enum ApiFormat {
    /// OpenAI Responses API (`/v1/responses`) — current OpenAI protocol.
    #[default]
    OpenAI,
    /// OpenAI Chat Completions (`/v1/chat/completions`) — legacy/compatible endpoints.
    OpenAIChat,
    /// Anthropic Messages (`/v1/messages`).
    Anthropic,
}

impl ApiFormat {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai" | "responses" | "openai-responses" => Ok(Self::OpenAI),
            "openai-chat" | "chat" | "completions" | "openai-compatible" => Ok(Self::OpenAIChat),
            "anthropic" | "claude" | "messages" => Ok(Self::Anthropic),
            other => Err(format!(
                "unknown api format '{other}' (expected openai, openai-chat, or anthropic)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::OpenAIChat => "openai-chat",
            Self::Anthropic => "anthropic",
        }
    }
}
"""
    if "OpenAIChat" not in text:
        if old not in text:
            raise SystemExit("ApiFormat block not found")
        text = text.replace(old, new)

    old_url = """        let default_url = match api {
            ApiFormat::OpenAI => "https://api.openai.com/v1",
            ApiFormat::Anthropic => "https://api.anthropic.com",
        };
"""
    new_url = """        let default_url = match api {
            ApiFormat::OpenAI | ApiFormat::OpenAIChat => "https://api.openai.com/v1",
            ApiFormat::Anthropic => "https://api.anthropic.com",
        };
"""
    if old_url in text:
        text = text.replace(old_url, new_url)

    # effective_providers fallback for non-claude should still look for openai name
    # or openai-chat api string
    old_find = """            providers
                .iter()
                .find(|p| p.name == "openai")
                .or_else(|| {
                    providers
                        .iter()
                        .find(|p| p.api.eq_ignore_ascii_case("openai"))
                })
"""
    new_find = """            providers
                .iter()
                .find(|p| p.name == "openai")
                .or_else(|| {
                    providers.iter().find(|p| {
                        let api = p.api.to_ascii_lowercase();
                        api == "openai"
                            || api == "responses"
                            || api == "openai-chat"
                            || api == "chat"
                            || api == "completions"
                            || api == "openai-compatible"
                    })
                })
"""
    if old_find in text:
        text = text.replace(old_find, new_find)

    path.write_text(text, encoding="utf-8")
    print("patched", path)


def patch_agent() -> None:
    path = ROOT / "crates/pigs-cli/src/agent.rs"
    text = path.read_text(encoding="utf-8")
    old = """    fn llm_provider_from_api(api: ApiFormat) -> LlmProvider {
        match api {
            ApiFormat::OpenAI => LlmProvider::OpenAI,
            ApiFormat::Anthropic => LlmProvider::Anthropic,
        }
    }
"""
    new = """    fn llm_provider_from_api(api: ApiFormat) -> LlmProvider {
        match api {
            ApiFormat::OpenAI => LlmProvider::OpenAI,
            ApiFormat::OpenAIChat => LlmProvider::OpenAIChat,
            ApiFormat::Anthropic => LlmProvider::Anthropic,
        }
    }
"""
    if "OpenAIChat" not in text:
        if old not in text:
            raise SystemExit("agent llm_provider_from_api not found")
        text = text.replace(old, new)
        path.write_text(text, encoding="utf-8")
        print("patched", path)
    else:
        print("agent already patched")


def patch_models_docs() -> None:
    models = ROOT / "crates/pigs-cli/src/models.rs"
    mt = models.read_text(encoding="utf-8")
    mt = mt.replace(
        "API formats: anthropic (Messages) | openai (Chat Completions)",
        "API formats: anthropic (Messages) | openai (Responses /v1/responses) | openai-chat (Chat Completions)",
    )
    models.write_text(mt, encoding="utf-8")

    readme = ROOT / "README.md"
    r = readme.read_text(encoding="utf-8")
    r = r.replace(
        "- **多供应商多模型** — 配置 `[[providers]]` / `[[models]]`；仅两种线格式（Anthropic Messages / OpenAI Chat Completions）；每模型可设 `context_window`",
        "- **多供应商多模型** — 配置 `[[providers]]` / `[[models]]`；线格式：`openai`=Responses(`/v1/responses`，Codex 同款)、`openai-chat`=Chat Completions、`anthropic`=Messages；每模型可设 `context_window`",
    )
    r = r.replace(
        '# api = "openai"\n# api_key = "sk-..."\n# base_url = "https://api.openai.com/v1"',
        '# api = "openai"          # Responses API (default for OpenAI)\n# api_key = "sk-..."\n# base_url = "https://api.openai.com/v1"',
    )
    r = r.replace(
        '# name = "deepseek"\n# api = "openai"\n',
        '# name = "deepseek"\n# api = "openai-chat"   # third-party often only supports Chat Completions\n',
    )
    # pigs-llm crate description if present
    r = r.replace(
        "仅两种 API 格式：Anthropic Messages + OpenAI Chat Completions（SSE）。`claude*` 走 Anthropic；其它模型走 OpenAI 格式，可用 `openai.base_url` 接兼容端点。",
        "API 格式：Anthropic Messages、OpenAI Responses（默认 `/v1/responses`）、OpenAI Chat Completions（`openai-chat`）。",
    )
    readme.write_text(r, encoding="utf-8")

    agents = ROOT / "AGENTS.md"
    ag = agents.read_text(encoding="utf-8")
    ag = ag.replace(
        "线格式：Anthropic Messages / OpenAI Responses(`/v1/responses`) / OpenAI Chat Completions + SSE",
        "线格式：Anthropic Messages / OpenAI Responses(`/v1/responses`) / OpenAI Chat Completions + SSE",
    )
    ag = ag.replace(
        "多供应商多模型（`[[providers]]`/`[[models]]` + context_window；线格式仅 Anthropic Messages / OpenAI Chat Completions + SSE）",
        "多供应商多模型（`[[providers]]`/`[[models]]` + context_window；线格式：Anthropic Messages / OpenAI Responses / OpenAI Chat Completions + SSE）",
    )
    ag = ag.replace(
        "- **pigs-llm** 实现 `ApiClient` trait：仅 Anthropic Messages 与 OpenAI Chat Completions 两种线格式。",
        "- **pigs-llm** 实现 `ApiClient` trait：Anthropic Messages、OpenAI Responses（默认 `/v1/responses`，对齐 Codex）、OpenAI Chat Completions（`openai-chat`）。",
    )
    ag = ag.replace(
        "| `pigs-llm` | Anthropic / OpenAI 双格式客户端与 SSE 流式 |",
        "| `pigs-llm` | Anthropic / OpenAI Responses / OpenAI Chat 客户端与 SSE 流式 |",
    )
    agents.write_text(ag, encoding="utf-8")
    print("docs patched")


def main() -> None:
    write_lib()
    patch_provider()
    patch_config()
    patch_agent()
    patch_models_docs()
    print("done")


if __name__ == "__main__":
    main()
