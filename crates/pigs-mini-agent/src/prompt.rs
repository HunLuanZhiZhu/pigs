//! 系统提示词 —— 把 LLM 变成 Agent 的"行为指令"。
//!
//! 教学要点：
//! - 系统提示词是对话的第一条消息，定义 Agent 的身份、能力、规则
//! - LLM 本身只是"续写文本"，是系统提示词让它"扮演"一个 Agent
//! - 系统提示词是**动态**的——工具列表变化时，提示词自动跟着变
//!
//! 借鉴对比：
//! - 对比 CoreCoder `prompt.py`（33 行）: 那里动态拼接工具列表到提示词中，
//!   本 crate 采用完全相同的设计——"工具增减提示词自动变"
//! - 对比 pigs: 那里用 `DEFAULT_SYSTEM_PROMPT` 常量 + `AGENTS.md` 拼接
//! - 对比 codex: 那里有 `base_instructions` + skills/plugins 注入，更复杂
//!
//! 教学理念（来自 CoreCoder README）：
//! > "改这 80 行里的一行就能看到 Agent 性格变化，是全项目最便宜的
//! > '改一处看结果'实验。"
//!
//! 如果你想让 Agent 更谨慎、更啰嗦、更注重测试——改这个文件就行。

use crate::tool::ToolRegistry;

/// 构建系统提示词 —— 动态拼接工具列表。
///
/// 系统提示词由四部分组成：
/// 1. 身份介绍 —— 告诉 LLM "你是谁"
/// 2. 环境信息 —— 当前工作目录、操作系统
/// 3. 工具清单 —— 遍历注册表中的工具，生成 Markdown 列表
/// 4. 行为规则 —— 定义 Agent 的工作准则
///
/// 参数:
/// - `tools`: 工具注册表，用于动态生成工具清单
///
/// 返回: 完整的系统提示词字符串
pub fn build_system_prompt(tools: &ToolRegistry) -> String {
    // --- 第 1 部分：身份介绍 ---
    let identity = "你是 Pigs Mini Agent，一个运行在用户终端中的 AI 助手。\n\
    你可以帮助用户完成软件工程任务：编写代码、修复 bug、重构、解释代码、执行命令等。\n\
    你拥有工具来与文件系统和 shell 交互，请主动使用工具完成任务。";

    // --- 第 2 部分：环境信息 ---
    // 获取当前工作目录
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string());
    // 获取操作系统信息
    let os = std::env::consts::OS; // "linux" / "macos" / "windows"

    let environment = format!(
        "# 环境信息\n\
        - 工作目录: {cwd}\n\
        - 操作系统: {os}"
    );

    // --- 第 3 部分：工具清单 ---
    // 遍历注册表中的所有工具，为每个工具生成一行 Markdown 描述
    let tool_lines: Vec<String> = tools
        .names()
        .iter()
        // 对每个工具名，从注册表获取 schema，提取描述
        .map(|name| {
            let schemas = tools.schemas();
            let desc = schemas
                .iter()
                .find(|s| s["function"]["name"].as_str() == Some(name.as_str()))
                .and_then(|s| s["function"]["description"].as_str())
                .unwrap_or("");
            format!("- **{name}**: {desc}")
        })
        .collect();
    let tool_list = tool_lines.join("\n");

    let tools_section = format!("# 可用工具\n{tool_list}");

    // --- 第 4 部分：行为规则 ---
    // 这些规则来自 CoreCoder 的 prompt.py，是经过实战验证的好规则
    let rules = "# 行为规则\n\
1. **先读后改。** 修改文件前先读取文件内容。\n\
2. **小改用编辑。** 小范围修改用 edit_file，完整重写或新建文件用 write_file。\n\
3. **验证你的工作。** 修改代码后，运行相关测试或命令确认正确性。\n\
4. **简洁明了。** 用代码代替废话，只解释必要的部分。\n\
5. **一步一脚印。** 多步任务按顺序执行，不要跳步。\n\
6. **edit_file 唯一性。** 使用 edit_file 时，old_string 要包含足够上下文保证唯一匹配。\n\
7. **尊重现有风格。** 匹配项目的代码风格和约定。\n\
8. **不确定就问。** 如果请求不明确，向用户提问而非猜测。";

    // 拼接所有部分
    format!("{identity}\n\n{environment}\n\n{tools_section}\n\n{rules}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    // 测试模块 —— 验证系统提示词的生成

    use super::*;
    use crate::tool::Tool;
    use async_trait::async_trait;
    use serde_json::Value;

    /// 测试用工具
    struct FakeTool;

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            "fake_tool"
        }
        fn description(&self) -> &str {
            "一个测试用的假工具"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(&self, _input: Value) -> crate::error::Result<String> {
            Ok("ok".to_string())
        }
    }

    /// 测试系统提示词包含身份介绍
    #[test]
    fn test_prompt_contains_identity() {
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(FakeTool));
        let prompt = build_system_prompt(&tools);
        // 验证包含身份介绍
        assert!(prompt.contains("Pigs Mini Agent"));
        assert!(prompt.contains("AI 助手"));
    }

    /// 测试系统提示词包含环境信息
    #[test]
    fn test_prompt_contains_environment() {
        let tools = ToolRegistry::new();
        let prompt = build_system_prompt(&tools);
        // 验证包含环境信息
        assert!(prompt.contains("工作目录"));
        assert!(prompt.contains("操作系统"));
    }

    /// 测试系统提示词包含工具列表
    #[test]
    fn test_prompt_contains_tools() {
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(FakeTool));
        let prompt = build_system_prompt(&tools);
        // 验证包含工具名和描述
        assert!(prompt.contains("fake_tool"));
        assert!(prompt.contains("一个测试用的假工具"));
    }

    /// 测试空注册表的提示词
    #[test]
    fn test_prompt_empty_tools() {
        let tools = ToolRegistry::new();
        let prompt = build_system_prompt(&tools);
        // 空注册表也应该生成有效提示词
        assert!(prompt.contains("可用工具"));
        // 应该包含行为规则
        assert!(prompt.contains("先读后改"));
    }

    /// 测试提示词包含行为规则
    #[test]
    fn test_prompt_contains_rules() {
        let tools = ToolRegistry::new();
        let prompt = build_system_prompt(&tools);
        // 验证包含规则
        assert!(prompt.contains("行为规则"));
        assert!(prompt.contains("先读后改"));
        assert!(prompt.contains("验证你的工作"));
        assert!(prompt.contains("不确定就问"));
    }
}
