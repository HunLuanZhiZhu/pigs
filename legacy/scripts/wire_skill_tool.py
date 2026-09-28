#!/usr/bin/env python3
"""Wire catalog-only skills + on-demand skill tool into pigs."""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main() -> None:
    # --- pigs-config exports ---
    lib = ROOT / "crates/pigs-config/src/lib.rs"
    t = lib.read_text(encoding="utf-8")
    t = t.replace(
        "pub use skills::{format_skills_for_prompt, load_skills, skill_search_dirs, Skill};",
        "pub use skills::{find_skill, format_skill_body, format_skills_for_prompt, load_skills, skill_search_dirs, Skill};",
    )
    if "find_skill" not in t:
        t = t.replace(
            "pub use skills::{format_skills_for_prompt, load_skills, Skill};",
            "pub use skills::{find_skill, format_skill_body, format_skills_for_prompt, load_skills, skill_search_dirs, Skill};",
        )
    lib.write_text(t, encoding="utf-8")
    print("lib exports ok")

    # --- main.rs ---
    main_rs = ROOT / "crates/pigs-cli/src/main.rs"
    mt = main_rs.read_text(encoding="utf-8")
    if "mod skill_tool" not in mt:
        mt = mt.replace("mod agent_tool;\n", "mod agent_tool;\nmod skill_tool;\n")
        main_rs.write_text(mt, encoding="utf-8")
        print("main module ok")
    else:
        print("main already has skill_tool")

    # --- agent.rs ---
    agent_path = ROOT / "crates/pigs-cli/src/agent.rs"
    at = agent_path.read_text(encoding="utf-8")

    if "skill_catalog" not in at:
        at = at.replace(
            "    pub skills: Vec<pigs_config::Skill>,\n",
            "    pub skills: Vec<pigs_config::Skill>,\n"
            "    /// Shared skill catalog for the on-demand `skill` tool.\n"
            "    pub skill_catalog: crate::skill_tool::SkillCatalog,\n",
        )
        print("added skill_catalog field")

    old_block = """        let skills = pigs_config::load_skills(&workspace_root);
        system_prompt.push_str(&pigs_config::format_skills_for_prompt(&skills));

        // Create tool registry (with shared todo list)
        let (mut tool_registry, todo_list): (pigs_core::ToolRegistry, TodoList) = if args.no_tools {
            (
                pigs_core::ToolRegistry::new(),
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            )
        } else {
            create_default_registry_with_todos()
        };

        // Register the sub-agent tool (needs the API client)
        tool_registry.register(Box::new(crate::agent_tool::AgentTool::new(Arc::clone(&api_client))));
"""
    new_block = """        let skills = pigs_config::load_skills(&workspace_root);
        // Catalog only (names + short descriptions); full bodies load via `skill` tool.
        system_prompt.push_str(&pigs_config::format_skills_for_prompt(&skills));
        let skill_catalog: crate::skill_tool::SkillCatalog =
            std::sync::Arc::new(std::sync::Mutex::new(skills.clone()));

        // Create tool registry (with shared todo list)
        let (mut tool_registry, todo_list): (pigs_core::ToolRegistry, TodoList) = if args.no_tools {
            (
                pigs_core::ToolRegistry::new(),
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            )
        } else {
            create_default_registry_with_todos()
        };

        // Register the sub-agent tool (needs the API client)
        tool_registry.register(Box::new(crate::agent_tool::AgentTool::new(Arc::clone(&api_client))));
        // On-demand skill loader (claw-code style)
        if !args.no_tools {
            tool_registry.register(Box::new(crate::skill_tool::SkillTool::new(std::sync::Arc::clone(
                &skill_catalog,
            ))));
        }
"""
    if old_block not in at:
        raise SystemExit("agent new() skills block not found")
    at = at.replace(old_block, new_block)

    if "skill_catalog," not in at:
        at = at.replace(
            """            skills,
            rules,
            memory,
            snapshots: crate::snapshots::SnapshotStore::load_from_workspace(&workspace_root),
            workspace_root,
        })
""",
            """            skills,
            skill_catalog,
            rules,
            memory,
            snapshots: crate::snapshots::SnapshotStore::load_from_workspace(&workspace_root),
            workspace_root,
        })
""",
        )

    # permission for skill tool in Agent::new
    if 'with_tool_requirement("skill"' not in at:
        at = at.replace(
            """        for (name, required) in pigs_tools::tool_permission_modes() {
            policy = policy.with_tool_requirement(name, required);
        }

        // Build system prompt (base + AGENTS.md + rules + skills)
""",
            """        for (name, required) in pigs_tools::tool_permission_modes() {
            policy = policy.with_tool_requirement(name, required);
        }
        // CLI-local tools
        policy = policy.with_tool_requirement("skill", PermissionMode::ReadOnly);

        // Build system prompt (base + AGENTS.md + rules + skills)
""",
        )
        at = at.replace(
            """        for (name, required) in pigs_tools::tool_permission_modes() {
            policy = policy.with_tool_requirement(name, required);
        }

        let base_prompt = config
""",
            """        for (name, required) in pigs_tools::tool_permission_modes() {
            policy = policy.with_tool_requirement(name, required);
        }
        policy = policy.with_tool_requirement("skill", PermissionMode::ReadOnly);

        let base_prompt = config
""",
        )

    old_rebuild = """    fn rebuild_prompt_context(&mut self) {
        self.skills = pigs_config::load_skills(&self.workspace_root);
        self.rules = pigs_config::load_rules(&self.workspace_root);
        self.memory = pigs_config::load_memory(&self.workspace_root);
        let base_prompt = self
            .config
            .system_prompt
            .as_deref()
            .unwrap_or(DEFAULT_SYSTEM_PROMPT);
        let agents_md = pigs_config::load_agents_md(&self.workspace_root);
        let mut system_prompt = pigs_config::build_system_prompt(base_prompt, agents_md.as_deref());
        system_prompt.push_str(&pigs_config::format_rules_for_prompt(&self.rules));
        system_prompt.push_str(&pigs_config::format_memory_for_prompt(&self.memory));
        system_prompt.push_str(&pigs_config::format_skills_for_prompt(&self.skills));
        self.system_prompt = system_prompt;
    }
"""
    new_rebuild = """    fn rebuild_prompt_context(&mut self) {
        self.skills = pigs_config::load_skills(&self.workspace_root);
        if let Ok(mut guard) = self.skill_catalog.lock() {
            *guard = self.skills.clone();
        }
        self.rules = pigs_config::load_rules(&self.workspace_root);
        self.memory = pigs_config::load_memory(&self.workspace_root);
        let base_prompt = self
            .config
            .system_prompt
            .as_deref()
            .unwrap_or(DEFAULT_SYSTEM_PROMPT);
        let agents_md = pigs_config::load_agents_md(&self.workspace_root);
        let mut system_prompt = pigs_config::build_system_prompt(base_prompt, agents_md.as_deref());
        system_prompt.push_str(&pigs_config::format_rules_for_prompt(&self.rules));
        system_prompt.push_str(&pigs_config::format_memory_for_prompt(&self.memory));
        system_prompt.push_str(&pigs_config::format_skills_for_prompt(&self.skills));
        self.system_prompt = system_prompt;
    }
"""
    if old_rebuild not in at:
        raise SystemExit("rebuild_prompt_context not found")
    at = at.replace(old_rebuild, new_rebuild)

    old_reload_skills = """        let skills = pigs_config::load_skills(&self.workspace_root);
        system_prompt.push_str(&pigs_config::format_skills_for_prompt(&skills));

        self.api_client = api_client;
        self.session.model = model.clone();
        self.max_turns = config.max_turns;
        self.permission_policy = policy;
        self.system_prompt = system_prompt;
        self.skills = skills;
"""
    new_reload_skills = """        let skills = pigs_config::load_skills(&self.workspace_root);
        system_prompt.push_str(&pigs_config::format_skills_for_prompt(&skills));

        self.api_client = api_client;
        self.session.model = model.clone();
        self.max_turns = config.max_turns;
        self.permission_policy = policy;
        self.system_prompt = system_prompt;
        self.skills = skills.clone();
        if let Ok(mut guard) = self.skill_catalog.lock() {
            *guard = skills;
        }
"""
    if old_reload_skills in at:
        at = at.replace(old_reload_skills, new_reload_skills)
        print("reload_config catalog sync ok")
    else:
        print("WARN: reload_config skills block not exact")

    if '"skill" =>' not in at:
        at = at.replace(
            """        "apply_patch" => {
            let dry = input.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
            Some(if dry { "dry-run".into() } else { "apply".into() })
        }
        _ => None,
""",
            """        "apply_patch" => {
            let dry = input.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
            Some(if dry { "dry-run".into() } else { "apply".into() })
        }
        "skill" => input
            .get("name")
            .or_else(|| input.get("skill"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
""",
        )
        print("format_tool_input skill ok")

    agent_path.write_text(at, encoding="utf-8")
    print("agent.rs updated")

    # tools permission table
    tools_lib = ROOT / "crates/pigs-tools/src/lib.rs"
    tt = tools_lib.read_text(encoding="utf-8")
    if '("skill"' not in tt:
        tt = tt.replace(
            '        ("agent", PermissionMode::ReadOnly),\n',
            '        ("agent", PermissionMode::ReadOnly),\n'
            '        ("skill", PermissionMode::ReadOnly),\n',
        )
        tools_lib.write_text(tt, encoding="utf-8")
        print("tools permission table ok")

    # commands help
    cmd = ROOT / "crates/pigs-cli/src/commands.rs"
    ct = cmd.read_text(encoding="utf-8")
    ct = ct.replace(
        '    println!("  /skills [reload]  List loaded skills (or reload from disk)");\n',
        '    println!("  /skills [reload]  List skill catalog (full body via `skill` tool)");\n',
    )
    ct = ct.replace(
        """                println!("Loaded skills ({}):", agent.skills.len());
                for skill in &agent.skills {
                    let desc = if skill.description.is_empty() {
                        "(no description)"
                    } else {
                        skill.description.as_str()
                    };
                    println!("  - {}  {}", skill.name, desc);
                    println!("    {}", skill.path.display());
                }
""",
        """                println!(
                    "Skill catalog ({}): names+descriptions in system prompt; full body via tool `skill`",
                    agent.skills.len()
                );
                for skill in &agent.skills {
                    let desc = if skill.description.is_empty() {
                        "(no description)"
                    } else {
                        skill.description.as_str()
                    };
                    println!("  - {}  {}", skill.name, desc);
                    println!("    {}", skill.path.display());
                }
""",
    )
    cmd.write_text(ct, encoding="utf-8")
    print("commands ok")

    # docs
    replacements = [
        (
            ROOT / "docs/agent-design.md",
            "技能内容会追加到系统提示词的 `--- Available Skills ---` 段。\n",
            "系统提示词仅注入 **技能目录**（名称 + 短描述）。完整正文通过工具 `skill` 按需加载（对齐 claw-code），避免把全部 SKILL.md 塞进上下文。\n",
        ),
        (
            ROOT / "README.md",
            "- **Skills** — 从 `~/.pigs/skills`、`~/.agents/skills`、`.pigs/skills`、`.agents/skills`、`skills/` 加载技能并注入系统提示\n",
            "- **Skills** — 从 `~/.pigs/skills`、`~/.agents/skills`、`.pigs/skills`、`.agents/skills`、`skills/` 加载技能目录；system 仅注入索引，全文由工具 `skill` 按需加载\n",
        ),
        (
            ROOT / "AGENTS.md",
            "- Skills 目录（优先级从高到低，同名先到先得）：`~/.pigs/skills/`、`~/.agents/skills/`、`.pigs/skills/`、`.agents/skills/`、`skills/`（支持 `SKILL.md` 与 frontmatter；`.agents/skills` 为通用 Agent 技能路径，含 ARIS 等）\n",
            "- Skills：扫描目录（优先级从高到低，同名先到先得）`~/.pigs/skills/`、`~/.agents/skills/`、`.pigs/skills/`、`.agents/skills/`、`skills/`；**system prompt 只放 catalog（name+description）**，完整正文用工具 `skill` 按需加载（非全量注入）\n",
        ),
    ]
    for path, old, new in replacements:
        t = path.read_text(encoding="utf-8")
        if old in t:
            path.write_text(t.replace(old, new), encoding="utf-8")
            print("doc", path.name)
        else:
            print("doc skip", path.name)

    print("done")


if __name__ == "__main__":
    main()
