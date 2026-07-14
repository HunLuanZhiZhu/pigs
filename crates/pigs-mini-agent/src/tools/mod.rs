//! 内置工具集 —— Agent 可用的工具集合。
//!
//! 教学要点：
//! - 每个工具实现 `Tool` trait，注册到 `ToolRegistry`
//! - 工具是 Agent "动手"的能力——没有工具，Agent 只能聊天
//! - 添加新工具只需实现 `Tool` trait 并注册，无需修改 Agent 循环
//!
//! 借鉴对比：
//! - 对比 CoreCoder: 有 7 个工具（bash, read_file, write_file, edit_file, grep, glob, agent）
//! - 对比 pigs-tools: 有 11 个工具（加了 list_files, web_fetch, ask_user, todo_write, sleep）
//! - 本 crate 精选 4 个最核心工具，足以展示 Agent 的完整能力：
//!   - bash: 执行命令（与系统交互）
//!   - read_file: 读文件（获取信息）
//!   - write_file: 写文件（创建新文件）
//!   - edit_file: 编辑文件（修改已有文件，CoreCoder 的核心创新）
//!
//! 为什么选这 4 个：
//! 它们覆盖了编程 Agent 最核心的"读-改-验证"循环：
//! 读代码 → 改代码 → 运行命令验证。这是 CoreCoder 行为规则的核心。

use crate::tool::ToolRegistry;
use crate::tools::{
    bash::BashTool, edit_file::EditFileTool, read_file::ReadFileTool, write_file::WriteFileTool,
};

// === 工具子模块声明 ===
// 每个子模块实现一个具体的工具，都实现了 `Tool` trait。

/// bash 工具 —— 执行 shell 命令，支持跨平台和超时。
pub mod bash;

/// edit_file 工具 —— 通过搜索替换编辑文件，要求唯一匹配。
pub mod edit_file;

/// read_file 工具 —— 读取文件内容，带行号和范围读取。
pub mod read_file;

/// write_file 工具 —— 写入文件，自动创建父目录。
pub mod write_file;

/// 创建默认工具集 —— 注册所有内置工具到一个新的注册表。
///
/// 返回一个包含 4 个内置工具的 `ToolRegistry`。
/// Agent 初始化时调用这个函数来设置工具集。
///
/// 教学要点：这是工厂函数模式——把工具的创建和注册封装在一个函数里。
/// 如果想自定义工具集，可以不用这个函数，手动创建注册表。
pub fn create_default_tools() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    // 注册 bash 工具 —— 执行 shell 命令
    registry.register(Box::new(BashTool));
    // 注册 read_file 工具 —— 读取文件内容
    registry.register(Box::new(ReadFileTool));
    // 注册 write_file 工具 —— 写入文件
    registry.register(Box::new(WriteFileTool));
    // 注册 edit_file 工具 —— 搜索替换编辑文件
    registry.register(Box::new(EditFileTool));
    registry
}

/// 创建空工具集 —— 不注册任何工具。
///
/// 教学用：展示 Agent 在没有工具的情况下也能工作（纯聊天模式）。
pub fn create_empty_tools() -> ToolRegistry {
    ToolRegistry::new()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    // 测试模块 —— 验证默认工具集的创建

    use super::*;

    /// 测试默认工具集包含 4 个工具
    #[test]
    fn test_default_tools_count() {
        let tools = create_default_tools();
        assert_eq!(tools.len(), 4);
    }

    /// 测试默认工具集包含特定工具名
    #[test]
    fn test_default_tools_names() {
        let tools = create_default_tools();
        let names = tools.names();
        assert!(names.contains(&"bash".to_string()));
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"write_file".to_string()));
        assert!(names.contains(&"edit_file".to_string()));
    }

    /// 测试空工具集
    #[test]
    fn test_empty_tools() {
        let tools = create_empty_tools();
        assert!(tools.is_empty());
        assert_eq!(tools.len(), 0);
    }
}
