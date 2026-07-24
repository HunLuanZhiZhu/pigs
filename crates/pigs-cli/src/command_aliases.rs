//! Canonicalize slash-command names.
//!
//! English names remain primary. Chinese characters and pinyin aliases are
//! always accepted, independent of the configured UI language.

/// Map a raw command token (without leading `/`) to the canonical English name
/// used by the command dispatcher.
///
/// Returns the original token when no alias matches.
pub fn canonicalize_command(cmd: &str) -> &str {
    let raw = cmd.trim();
    if raw.is_empty() {
        return raw;
    }

    // Chinese character aliases (exact).
    match raw {
        "帮助" | "救命" => return "help",
        "退出" | "离开" | "结束" => return "quit",
        "模型" => return "model",
        "模式" | "权限" | "权限模式" => return "mode",
        "清空" | "清除" => return "clear",
        "保存" => return "save",
        "会话" | "会话列表" => return "ses",
        "工具" | "工具列表" => return "tools",
        "待办" | "任务" => return "todo",
        "状态" | "仪表盘" => return "status",
        "信息" | "会话信息" => return "info",
        "标题" => return "title",
        "费用" | "开销" | "成本" => return "cost",
        "初始化" => return "init",
        "重载" | "重新加载" | "热重载" => return "reload",
        "记忆" => return "memory",
        "规则" => return "rules",
        "技能" => return "skills",
        "撤销" | "回退" => return "undo",
        "导出" => return "export",
        "诊断" | "体检" | "健康检查" => return "doctor",
        "模型列表" | "模型目录" => return "models",
        "钩子" => return "hooks",
        "历史" | "历史记录" => return "history",
        "压缩" | "精简" => return "compact",
        "复制" => return "copy",
        "新建" | "新会话" => return "new",
        "恢复" => return "resume",
        "返回" | "后退" => return "back",
        "前进" => return "next",
        "快捷键" => return "hotkeys",
        "设置" => return "settings",
        "变更日志" | "更新日志" => return "changelog",
        "登录" => return "login",
        "登出" => return "logout",
        "命名" | "名称" => return "name",
        // Language commands: /语言 and bare /中文 both route to `lang`.
        // Bare `/中文` without args is handled as "switch to zh" in commands.rs.
        "语言" | "界面语言" | "中文" => return "lang",
        _ => {}
    }

    // Pinyin + English (ASCII, case-insensitive).
    match raw.to_ascii_lowercase().as_str() {
        "help" | "h" | "?" | "bangzhu" => "help",
        "quit" | "q" | "exit" | "tuichu" | "likai" => "quit",
        "model" | "moxing" => "model",
        "mode" | "moshi" | "quanxian" => "mode",
        "clear" | "qingkong" | "qingchu" => "clear",
        "save" | "baocun" => "save",
        "ses" | "sessions" | "list" | "huihua" => "ses",
        "back" | "fanhui" | "houtui" => "back",
        "next" | "qianjin" => "next",
        "tools" | "gongju" => "tools",
        "todo" | "todos" | "daiban" | "renwu" => "todo",
        "status" | "zhuangtai" | "yibiaopan" => "status",
        "info" | "xinxi" => "info",
        "title" | "name" | "biaoti" | "mingcheng" => "name",
        "cost" | "feiyong" | "kaixiao" | "chengben" => "cost",
        "init" | "chushihua" => "init",
        "reload" | "zhongzai" | "chongzai" | "chongxinjiazai" => "reload",
        "mcp" => "mcp",
        "memory" | "jiyi" => "memory",
        "rules" | "guize" => "rules",
        "skills" | "jineng" => "skills",
        "undo" | "chexiao" | "huitui" => "undo",
        "export" | "daochu" => "export",
        "doctor" | "zhenduan" | "tijian" => "doctor",
        "models" | "moxingliebiao" => "models",
        "hooks" | "gouzi" => "hooks",
        "history" | "lishi" => "history",
        "compact" | "yasuo" | "jingjian" => "compact",
        "copy" | "fuzhi" => "copy",
        "new" | "xinjian" | "xinhuihua" => "new",
        "resume" | "huifu" => "resume",
        "hotkeys" | "kuaijiejian" => "hotkeys",
        "settings" | "shezhi" => "settings",
        "changelog" | "biangengrizhi" | "gengxinrizhi" => "changelog",
        "login" | "denglu" => "login",
        "logout" | "dengchu" => "logout",
        "lang" | "language" | "yuyan" | "zhongwen" => "lang",
        "fork" | "fenzhi" => "fork",
        "clone" | "fuzhi2" => "clone",
        "tree" | "shu" => "tree",
        "import" | "daoru" => "import",
        "share" | "fenxiang" => "share",
        "sub" | "ziagent" | "zizhuti" => "sub",
        _ => raw,
    }
}

/// Canonicalize MCP subcommands (Chinese / pinyin → English).
pub fn canonicalize_mcp_sub(sub: &str) -> &str {
    let raw = sub.trim();
    match raw {
        "帮助" => return "help",
        "列表" | "列出" => return "list",
        "工具" => return "tools",
        "连接" => return "connect",
        "断开" | "断开连接" => return "disconnect",
        _ => {}
    }
    match raw.to_ascii_lowercase().as_str() {
        "help" | "h" | "?" | "bangzhu" => "help",
        "list" | "ls" | "liebiao" => "list",
        "tools" | "gongju" => "tools",
        "connect" | "lianjie" => "connect",
        "disconnect" | "duankai" => "disconnect",
        _ => raw,
    }
}

/// Canonicalize common session subcommands.
pub fn canonicalize_sessions_sub(sub: &str) -> &str {
    let raw = sub.trim();
    match raw {
        "列表" | "列出" => return "list",
        "删除" | "移除" => return "rm",
        "打开" | "切换" => return "open",
        "搜索" | "查找" => return "search",
        "当前" => return "current",
        _ => {}
    }
    match raw.to_ascii_lowercase().as_str() {
        "list" | "ls" | "liebiao" => "list",
        "rm" | "delete" | "shanchu" | "yichu" => "rm",
        "open" | "switch" | "dakai" | "qiehuan" => "open",
        "search" | "sousuo" | "chazhao" => "search",
        "current" | "dangqian" => "current",
        _ => raw,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn chinese_aliases() {
        assert_eq!(canonicalize_command("帮助"), "help");
        assert_eq!(canonicalize_command("退出"), "quit");
        assert_eq!(canonicalize_command("状态"), "status");
        assert_eq!(canonicalize_command("语言"), "lang");
        assert_eq!(canonicalize_command("中文"), "lang");
        assert_eq!(canonicalize_command("技能"), "skills");
    }

    #[test]
    fn pinyin_aliases() {
        assert_eq!(canonicalize_command("bangzhu"), "help");
        assert_eq!(canonicalize_command("tuichu"), "quit");
        assert_eq!(canonicalize_command("zhuangtai"), "status");
        assert_eq!(canonicalize_command("yuyan"), "lang");
        assert_eq!(canonicalize_command("zhongwen"), "lang");
        assert_eq!(canonicalize_command("HELP"), "help");
        assert_eq!(canonicalize_command("jineng"), "skills");
    }

    #[test]
    fn english_unchanged_semantics() {
        assert_eq!(canonicalize_command("help"), "help");
        assert_eq!(canonicalize_command("status"), "status");
        assert_eq!(canonicalize_command("mcp"), "mcp");
    }

    #[test]
    fn session_and_mcp_subs() {
        assert_eq!(canonicalize_sessions_sub("打开"), "open");
        assert_eq!(canonicalize_sessions_sub("shanchu"), "rm");
        assert_eq!(canonicalize_mcp_sub("连接"), "connect");
        assert_eq!(canonicalize_mcp_sub("liebiao"), "list");
    }
}
