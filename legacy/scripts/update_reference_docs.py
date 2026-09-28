#!/usr/bin/env python3
"""Update README / AGENTS / docs/参考项目分析 for new reference projects."""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def update_analysis() -> None:
    path = ROOT / "docs" / "参考项目分析.md"
    text = path.read_text(encoding="utf-8")

    text = text.replace(
        """## 目录

1. [总览](#总览)
2. [CoreCoder — 极简教学 Agent（Python）](#1-corecoder)
3. [claw-code — Claude Code 的 Rust 端口（Rust）](#2-claw-code)
4. [cline — 多面编程 Agent（TypeScript）](#3-cline)
5. [codex — OpenAI Codex CLI（Rust）](#4-codex)
6. [deepseek-reasonix — DeepSeek 编程 Agent（Go）](#5-deepseek-reasonix)
7. [hermes-agent — 自我改进 Agent（Python）](#6-hermes-agent)
8. [kilocode — 多 IDE 编程 Agent（TypeScript）](#7-kilocode)
9. [openclaw — 自托管个人 AI 助手（TypeScript）](#8-openclaw)
10. [opencode — 开源编程 Agent 平台（TypeScript）](#9-opencode)
11. [pi — Agent Harness 框架（TypeScript）](#10-pi)
12. [横向对比与设计启示](#横向对比与设计启示)
""",
        """## 目录

1. [总览](#总览)
2. [CoreCoder — 极简教学 Agent（Python）](#1-corecoder)
3. [claw-code — Claude Code 的 Rust 端口（Rust）](#2-claw-code)
4. [cline — 多面编程 Agent（TypeScript）](#3-cline)
5. [codex — OpenAI Codex CLI（Rust）](#4-codex)
6. [deepseek-reasonix — DeepSeek 编程 Agent（Go）](#5-deepseek-reasonix)
7. [hermes-agent — 自我改进 Agent（Python）](#6-hermes-agent)
8. [kilocode — 多 IDE 编程 Agent（TypeScript）](#7-kilocode)
9. [openclaw — 自托管个人 AI 助手（TypeScript）](#8-openclaw)
10. [opencode — 开源编程 Agent 平台（TypeScript）](#9-opencode)
11. [pi — Agent Harness 框架（TypeScript）](#10-pi)
12. [fugu — Sakana 多模型编排系统（配置/接入层）](#11-fugu)
13. [oh-my-openagent — 多 Harness 插件式 Agent OS（TypeScript）](#12-oh-my-openagent)
14. [oh-my-pi — 带 IDE 能力的编程 Agent（TypeScript + Rust）](#13-oh-my-pi)
15. [横向对比与设计启示](#横向对比与设计启示)
""",
    )

    text = text.replace(
        """本仓库包含 **10 个参考项目**，涵盖 5 种编程语言：

| 语言 | 项目数 | 项目 |
|---|---|---|
| **Rust** | 2 | claw-code, codex |
| **TypeScript** | 5 | cline, kilocode, openclaw, opencode, pi |
| **Python** | 2 | CoreCoder, hermes-agent |
| **Go** | 1 | deepseek-reasonix |

所有项目都是独立的 git 仓库（各有自己的 `.git/`），作为参考克隆到 `pigs/` 目录下。

### 共同架构主题

所有参考项目都围绕以下核心概念构建：

1. **工具调用 Agent 循环** — 每个项目都实现了某种形式的 LLM ↔ 工具 ↔ 状态机循环
2. **多供应商 LLM 抽象** — 将不同模型供应商（OpenAI、Anthropic、Google 等）抽象到统一接口后
3. **项目记忆文件** — `AGENTS.md`、`CLAUDE.md`、`REASONIX.md` 等加载到系统提示词中
4. **可扩展架构** — 插件、技能、MCP 服务器、扩展系统
5. **多前端交付** — CLI、TUI、IDE 插件、Web、桌面应用、消息平台
""",
        """本仓库包含 **13 个参考项目**，涵盖 5 种编程语言（另含配置/接入型仓库）：

| 语言 / 形态 | 项目数 | 项目 |
|---|---|---|
| **Rust** | 2 | claw-code, codex |
| **TypeScript** | 6 | cline, kilocode, openclaw, opencode, pi, oh-my-openagent |
| **TypeScript + Rust** | 1 | oh-my-pi（Bun 前端 + Rust 本地核心） |
| **Python** | 2 | CoreCoder, hermes-agent |
| **Go** | 1 | deepseek-reasonix |
| **配置/接入（非完整运行时）** | 1 | fugu（Sakana 多模型编排的 Codex 接入与配置包） |

所有项目都是独立的 git 仓库（各有自己的 `.git/`），作为参考克隆到 `pigs/` 目录下，**不属于** pigs workspace 构建。

### 共同架构主题

所有参考项目都围绕以下核心概念构建：

1. **工具调用 Agent 循环** — 每个项目都实现了某种形式的 LLM ↔ 工具 ↔ 状态机循环
2. **多供应商 LLM 抽象** — 将不同模型供应商（OpenAI、Anthropic、Google 等）抽象到统一接口后
3. **项目记忆文件** — `AGENTS.md`、`CLAUDE.md`、`REASONIX.md` 等加载到系统提示词中
4. **可扩展架构** — 插件、技能、MCP 服务器、扩展系统
5. **多前端交付** — CLI、TUI、IDE 插件、Web、桌面应用、消息平台
6. **多 Agent / 多模型编排** — fugu（协调器+模型池）、oh-my-openagent（Team Mode / 多 harness）、oh-my-pi（swarm 扩展）等把「多专家」产品化
7. **以单模型 API 交付多 Agent 系统** — fugu 将内部编排包装为 Chat Completions / Responses 单一 model 外观
""",
    )

    new_sections = """
## 11. fugu

### 基本信息

| 属性 | 值 |
|---|---|
| **组织** | Sakana AI |
| **仓库形态** | 配置 / 安装 / Codex 接入包（**不是**完整开源 Agent 运行时） |
| **主要交付** | `codex-fugu` 启动器 + Codex `config.toml` 注入 + Sakana API 密钥 |
| **对外协议** | 用户侧像单个 LLM：Chat Completions + Responses |
| **技术报告** | [arXiv:2606.21228](https://arxiv.org/abs/2606.21228)（仓内 `Fugu_technical_report.pdf`） |
| **研究底座** | [TRINITY](https://arxiv.org/abs/2512.04695)、[Conductor](https://arxiv.org/abs/2512.04388)（ICLR 2026） |
| **本地路径** | `fugu/` |
| **仓库地址** | github.com/SakanaAI/fugu |

### 项目说明

**Sakana Fugu** 的产品定位是：**多 Agent / 多模型系统，以「一个模型」交付**。

- 内部：动态编排 frontier 模型池（协调器 + worker），处理复杂多步任务
- 外部：通过 [Sakana API](https://console.sakana.ai/get-started) 暴露为单个 model（如 `fugu` / `fugu-ultra`）
- 客户端：一键安装进 Codex（`curl … | bash` → `codex-fugu` ≈ `codex -p fugu`）

本仓库本地克隆主要包含：

- `configs/` — Codex 配置 bundle（`fugu.json` 模型目录、`injects/` 注入 provider 段）
- `scripts/install.sh` / `codex-fugu` — 安装、版本钉扎、配置备份与保护
- `docs/commands_details.md` — 安装与启动参数
- `Fugu_technical_report.pdf` 与研究论文链接

**协调器本体与训练权重不在此开源仓库中**；开源部分是「如何把托管 Fugu 接到 Codex」。

### 架构要点（可借鉴）

| 概念 | 含义 | 对 pigs 的启示 |
|---|---|---|
| **Coordinator / Conductor** | 小协调器逐轮分配角色，或设计通信拓扑 + 自然语言指令 | 编排层与 worker 分离；先可用提示词/规则实现，再谈训练 |
| **模型池** | 异构 frontier 模型持续更新 | 复用 `[[providers]]` / `[[models]]`，专家绑定不同端点 |
| **单 model 外观** | 内部多 agent，外部 `fugu` 一个 id | `pigs-server` 把编排暴露为 `/v1/responses` 或 `/v1/chat/completions` |
| **双协议接入** | Chat Completions + Responses | pigs 已实现两套 OpenAI 线格式 + Anthropic |
| **Harness 注入** | 改 Codex 配置而非重写 Codex | 编排可作为「特殊 model」挂进现有 Agent 循环 |

### 本地模型配置摘录

`configs/files/fugu.json` 定义对外模型 slug（如 `fugu`、`fugu-ultra`）、极大 `context_window`、reasoning effort、并行 tool calls 等——对 pigs 的 **模型级 `context_window` + catalog** 设计有直接对照价值。

### 独特价值

- **产品形态教科书**：专家团 / 多模型编排如何「卖成一个 LLM」
- **API 优先**：别人用标准 SDK 接入，无需理解内部拓扑
- **与 Codex 共生**：说明现代 Agent 常以 harness 插件/配置层存在

### 对 pigs 的优先级

**高（产品与编排方向）**。不必复制训练，应复制：

1. 单一 model id → 内部编排
2. OpenAI 兼容 API 外壳
3. 可配置模型池 + 上下文窗口

---

## 12. oh-my-openagent

### 基本信息

| 属性 | 值 |
|---|---|
| **语言** | TypeScript（Bun monorepo） |
| **npm 包** | `oh-my-opencode` / `oh-my-openagent`（过渡双名），CLI：`omo` / `lazycodex` |
| **版本（检出时）** | 约 4.16.x |
| **定位** | 多 Harness Agent OS / OpenCode 与 Codex 的增强插件 |
| **本地路径** | `oh-my-openagent/` |
| **仓库地址** | github.com/code-yeongyu/oh-my-openagent |
| **许可证** | SUL-1.0（见仓库） |

### 项目说明

oh-my-openagent（OmO）把「装完就能干活」的 harness 增强做到极致，并明确走向 **多 harness**（OpenCode、Codex、Pi 等）：

- **Ultimate Edition（omo for OpenCode）**：完整能力——约 11 个 agent、54+ lifecycle hooks、内置 MCP、Team Mode、`ultrawork` / `ulw-loop`、hashline 编辑等
- **Light Edition（omo for Codex / LazyCodex）**：适配 Codex 插件面的可移植组件（rules、comment-checker、git-bash、lsp、ultrawork、telemetry、若干 MCP），**不做**完整 agent 编排与 `team_*` 工具

安装口号级入口：`ultrawork` / `ulw` —— 一句话拉起整套工作流。

### 架构

工作空间约 **39 个 package**，分层大致为 Core → MCP → Skills → Harness 适配器 → 平台/Web。

| 包 / 区域 | 作用 |
|---|---|
| `packages/omo-opencode/` | OpenCode 插件适配器；agents / hooks / tools / features |
| `packages/omo-codex/` | Codex Light 版（lazycodex） |
| `packages/team-core/` 与 Team Mode | Lead + 并行成员、`team_*` 工具、tmux 可视化 |
| `packages/delegate-core/` | 委派核心 |
| `packages/*-core` | rules、hashline、lsp、mcp、agents-md、telemetry、boulder-state 等 |
| `packages/shared-skills/` | 跨 harness 的 `SKILL.md` 包 |

文档强调 **真实 harness QA + 证据落盘**（`.omo/evidence/`），OpenCode / Codex 各有专用 QA skill。

### 独特价值

- **Team Mode**：真·多 Agent 并行协作（lead + 成员 + 任务工具）
- **多 harness 适配**：同一套核心能力挂到 OpenCode / Codex（及规划中的 Pi）
- **生命周期 hooks 密度极高**：可配置开关
- **工作流产品化**：`ultrawork`、hyperplan、security-research 等模式

### 对 pigs 的启示

| 借鉴点 | 说明 |
|---|---|
| Team / Council | 与 fugu 编排互补：Fugu 偏模型池协调；OmO 偏 harness 内多 agent 团队 |
| Hooks 分层 | pigs 已有 pre/post tool hooks，可参考更细事件与开关 |
| 双发行版 | 「完整 CLI」vs「嵌入其他 harness 的轻量组件」 |
| QA 证据 | 对 agent 改动要求真实会话证据，适合后续 CI 文化 |

### 对 pigs 的优先级

**高（多 Agent 协作与 hooks）**。

---

## 13. oh-my-pi

### 基本信息

| 属性 | 值 |
|---|---|
| **语言** | TypeScript（Bun）+ **Rust** 本地核心 |
| **上游** | Fork of Pi / pi-mono（Mario Zechner） |
| **CLI** | `omp`（npm `@oh-my-pi/pi-coding-agent` 等） |
| **版本（检出时）** | workspace 约 16.4.x |
| **定位** | 「带 IDE 的 coding agent」— Pi harness 的电池级增强版 |
| **本地路径** | `oh-my-pi/` |
| **仓库地址** | github.com/can1357/oh-my-pi |
| **站点** | https://omp.sh |

### 项目说明

oh-my-pi（omp）在 Pi harness 上补齐真实开发工作流：

- **40+** 供应商、**32** 内置工具、**14** LSP ops、**28** DAP ops
- 约 **55k 行 Rust 核心**（`crates/pi-*`）：文本/搜索/shell/AST 等性能路径
- 强调 **harness 质量 > 盲目换模型**

### 架构

**TypeScript packages（节选）**

| 包 | 作用 |
|---|---|
| `packages/coding-agent` | 主 CLI 应用（文档默认焦点） |
| `packages/agent` | Agent 运行时（工具调用与状态） |
| `packages/ai` | 多供应商 LLM 客户端 + 流式 |
| `packages/catalog` | 模型目录与能力分类 |
| `packages/tui` | 终端 UI |
| `packages/natives` | 原生绑定（对接 Rust） |
| `packages/swarm-extension` 等 | 扩展与协作相关能力 |

**Rust crates（节选）**：`pi-natives`、`pi-shell`、`pi-ast`、`pi-walker`、`pi-uu-grep` 等；workspace 使用较新 edition 与严格 clippy。

### 独特价值

- **IDE 能力进 Agent**：LSP 贯穿写操作、DAP 真调试
- **代码执行 + tool-calling 回环**：Python/Bun kernel 可回调 agent 工具
- **编辑格式与 harness 调优**
- **TS + Rust 混合**：热路径 Rust、产品面 TS

### 对 pigs 的启示

| 借鉴点 | 说明 |
|---|---|
| 工具质量 | read 摘要、search 性能、edit 一次成功 |
| LSP/DAP | 长期可选工具后端 |
| catalog | 与 pigs `[[models]]` + context_window 同族 |
| 严格 lint | 与 codex/pigs clippy 纪律同向 |

### 对 pigs 的优先级

**高（工具与 harness 质量 + Rust 热路径）**。建议与上游 `pi/` 对照阅读。

---

"""

    marker = "## 横向对比与设计启示\n"
    if "## 11. fugu" not in text:
        if marker not in text:
            raise SystemExit("horizontal section missing")
        text = text.replace(marker, new_sections + marker)

    text = text.replace(
        """| 维度 | Rust 项目 | TS 项目 | Python 项目 | Go 项目 |
|---|---|---|---|---|
| **模块化** | claw-code: 9 crate；codex: ~126 crate | opencode: ~34包；kilocode: 27包；cline: 5层SDK | CoreCoder: 单包极简；hermes: 多模块 | reasonix: internal/ 单内核 |
| **Agent 循环** | codex `core/`；claw-code `runtime/` | cline `@cline/agents`（无状态）；opencode `session/` | CoreCoder 极简循环；hermes 闭环学习 | reasonix `control.Controller` |
| **多供应商 LLM** | codex `model-provider/`；claw-code `api/` | cline `@cline/llms`；pi `pi-ai`；opencode `llm/` | hermes 多端点 | reasonix DeepSeek 优先 |
| **沙箱/安全** | codex: landlock+seccomp+bubblewrap+Starlark；claw-code: 工作区边界+权限模式 | kilocode `kilo-sandbox` | — | — |
| **项目记忆** | codex `AGENTS.md`；claw-code `CLAUDE.md` | opencode `AGENTS.md`+`CONTEXT.md`；pi `AGENTS.md` | hermes `AGENTS.md`(72KB) | reasonix `REASONIX.md` |
| **可扩展性** | codex `ext/`+插件+技能+hooks；claw-code 插件+技能+hooks | opencode 34包；kilocode 27包；cline 插件+MCP；openclaw 150+扩展 | hermes 技能系统 | — |
""",
        """| 维度 | Rust / 混合 | TS 项目 | Python / Go | 编排/接入 |
|---|---|---|---|---|
| **模块化** | claw-code: 9 crate；codex: ~126 crate；oh-my-pi: TS packages + `crates/pi-*` | opencode: ~34包；kilocode: 27包；cline: 5层SDK；oh-my-openagent: ~39包分层 | CoreCoder: 单包；hermes: 多模块；reasonix: internal/ | fugu: 配置 bundle + 安装器 |
| **Agent 循环** | codex `core/`；claw-code `runtime/`；oh-my-pi `packages/agent` | cline `@cline/agents`；opencode `session/`；OmO agents+hooks | CoreCoder 极简；hermes 闭环；reasonix Controller | fugu: 服务端协调器（仓外） |
| **多供应商 LLM** | codex `model-provider/`；claw-code `api/`；oh-my-pi `packages/ai`+catalog | cline `@cline/llms`；pi `pi-ai`；opencode `llm/`；OmO model-core | hermes 多端点；reasonix DeepSeek 优先 | fugu: 模型池 + 单 model API |
| **多 Agent 编排** | oh-my-pi swarm 等扩展 | OmO Team Mode / ultrawork；openclaw 多渠道 | hermes 技能进化 | **fugu Coordinator/Conductor** |
| **沙箱/安全** | codex sandboxing；claw-code 工作区边界 | kilocode sandbox；OmO 权限/hooks | — | fugu 托管侧策略 |
| **项目记忆** | codex/claw-code AGENTS/CLAUDE | opencode/pi/OmO agents-md | hermes；reasonix REASONIX | — |
| **可扩展性** | codex ext；claw-code 插件；oh-my-pi 扩展包 | opencode/kilocode/OmO 包海；openclaw 150+ | hermes 技能 | fugu 作为 Codex profile 注入 |
| **IDE 深度** | **oh-my-pi LSP/DAP** | OmO lsp 组件；cline IDE 插件 | — | — |
""",
    )

    text = text.replace(
        """#### 3. 多供应商 LLM 抽象（参考 codex + cline + pi）

- 统一接口抽象不同供应商（OpenAI、Anthropic、Google、本地模型）
- 流式响应处理（SSE）
- 模型目录/清单管理
- 供应商特定认证流程
""",
        """#### 3. 多供应商 LLM 抽象（参考 codex + cline + pi + oh-my-pi）

- 统一接口抽象不同供应商（OpenAI、Anthropic、Google、本地模型）
- 流式响应处理（SSE）；OpenAI 侧区分 **Responses** 与 **Chat Completions**
- 模型目录/清单管理（context window、能力标签）
- 供应商特定认证流程

#### 3.5 多 Agent / 单模型 API（参考 fugu + oh-my-openagent）

- **Fugu 形态**：内部协调器 + 模型池，外部一个 model id + OpenAI 兼容 API
- **OmO Team Mode**：lead + 并行成员 + 任务工具；工作流入口（如 ultrawork）
- pigs 可渐进：experts 配置 → Trinity/Council 编排 → `pigs-server` 暴露为 `model=pigs-fugu`
""",
    )

    text = text.replace(
        """### 不建议直接复制的方面

- codex 的 Bazel 构建系统（过度工程，除非有类似 CI 规模需求）
- claw-code 的 OmX/clawhip/OmO 自治协调系统（理念有趣但超出初始范围）
- openclaw 的 150+ 扩展（初始阶段不需要如此大的扩展面）
- hermes 的六种终端后端（初始阶段 local 执行足够）

### 建议的初始实现路径

1. **先读 claw-code 的 `rust/crates/`** — 理解 Rust Agent 工作空间的 crate 分层
2. **再读 codex 的 `codex-rs/core/src/`** — 理解核心 Agent 循环的实现
3. **读 codex 的 `AGENTS.md`** — 理解 Rust 项目的编码规范和沙箱警告
4. **读 CoreCoder 的核心代码** — 理解最小 Agent 循环的概念验证
5. **设计新 Agent 的 crate 结构** — 结合 claw-code 的模块化 + codex 的深度安全
""",
        """### 不建议直接复制的方面

- codex 的 Bazel 构建系统（过度工程，除非有类似 CI 规模需求）
- claw-code 的 OmX/clawhip/OmO 自治协调系统（理念有趣但超出初始范围）
- openclaw 的 150+ 扩展（初始阶段不需要如此大的扩展面）
- hermes 的六种终端后端（初始阶段 local 执行足够）
- **Fugu 的闭源协调器训练**（学形态，不抄权重）
- **OmO 的全量 hooks/agents 矩阵**（先子集：team/council + 关键 hooks）

### 建议的阅读与借鉴路径（含新增参考）

1. **claw-code `rust/crates/`** — Rust Agent 工作空间分层
2. **codex `codex-rs/core/` + Responses API** — 核心循环与 OpenAI 最新线格式
3. **CoreCoder** — 最小 Agent 循环
4. **fugu README + 技术报告 + TRINITY/Conductor** — 多模型编排与「一个 model 交付」
5. **oh-my-openagent Team Mode / packages 分层** — harness 内多 Agent 与 hooks
6. **oh-my-pi（对照 pi/）** — 工具质量、LSP/DAP、TS+Rust 热路径
7. **回看 pigs 自身** — 在 multi-provider / tools / sub-agent 上叠编排与 API 外壳
""",
    )

    path.write_text(text, encoding="utf-8")
    print("updated", path, "lines", len(text.splitlines()))


def update_readme() -> None:
    path = ROOT / "README.md"
    text = path.read_text(encoding="utf-8")

    # Expand workspace tree / reference mention
    text = text.replace(
        "- 参考项目目录（`CoreCoder/`、`claw-code/`、`codex/` 等）是独立嵌套仓库，**不是** workspace members。",
        "- 参考项目目录（`CoreCoder/`、`claw-code/`、`codex/`、`fugu/`、`oh-my-openagent/`、`oh-my-pi/` 等）是独立嵌套仓库，**不是** workspace members。详见 `docs/参考项目分析.md`。",
    )

    tree_old = """```
pigs/
├── Cargo.toml                 # workspace 根配置
├── crates/
│   ├── pigs-core/              # 核心类型 + trait
│   ├── pigs-permissions/       # 权限系统
│   ├── pigs-config/            # 配置 / Skills / Rules / Memory
│   ├── pigs-session/           # 会话持久化 + 压缩
│   ├── pigs-llm/               # 多供应商 LLM + SSE
│   ├── pigs-tools/             # 内置工具 + .pigsignore
│   ├── pigs-mcp/               # MCP 客户端 + tool bridge
│   ├── pigs-cli/               # 产品二进制 pigs
│   └── pigs-mini-agent/        # 教学用最简 Agent（自包含）
├── skills/                     # 可选技能目录
├── .pigsignore                 # 工具搜索忽略模式
└── docs/
    ├── agent-design.md         # 架构设计文档
    └── 参考项目分析.md           # 参考项目综合分析
```
"""
    tree_new = """```
pigs/
├── Cargo.toml                 # workspace 根配置
├── crates/                    # pigs 产品代码（见上文 Crate 一览）
├── skills/                    # 可选技能目录
├── .pigsignore
├── docs/
│   ├── agent-design.md        # 架构设计文档
│   └── 参考项目分析.md          # 参考项目综合分析（13 个）
└── 参考项目/（独立 git，不参与 cargo workspace）
    ├── CoreCoder/ claw-code/ cline/ codex/
    ├── deepseek-reasonix/ hermes-agent/ kilocode/
    ├── openclaw/ opencode/ pi/
    ├── fugu/                  # Sakana 多模型编排 · Codex 接入
    ├── oh-my-openagent/       # 多 harness 插件式 Agent OS
    └── oh-my-pi/              # Pi fork · TS+Rust · IDE 级工具
```
"""
    if tree_old in text:
        text = text.replace(tree_old, tree_new)

    # Insert dedicated reference section before 开发 / 许可证
    ref_section = """
## 参考项目

仓库中还克隆了多个独立 Agent 实现，供架构借鉴（**不参与** `cargo` 构建）。完整分析见 [`docs/参考项目分析.md`](docs/参考项目分析.md)。

| 目录 | 语言 | 一句话 |
|---|---|---|
| `CoreCoder/` | Python | 极简教学 Agent（“coding agent 的 nanoGPT”） |
| `claw-code/` | **Rust** | Claude Code 的 Rust 端口；crate 分层与安全设计 |
| `cline/` | TypeScript | 多面 Agent + 分层 SDK |
| `codex/` | **Rust** | OpenAI Codex CLI；Responses API 与沙箱 |
| `deepseek-reasonix/` | Go | DeepSeek 向编程 Agent |
| `hermes-agent/` | Python | 自我改进 / 技能闭环 |
| `kilocode/` | TypeScript | 多 IDE；CLI fork 自 opencode |
| `openclaw/` | TypeScript | 自托管助手；渠道与扩展 |
| `opencode/` | TypeScript | 大型开源编程 Agent monorepo |
| `pi/` | TypeScript | Agent harness 框架 |
| `fugu/` | 配置/接入 | **Sakana Fugu**：多模型编排以单 model API 交付；Codex 安装包 |
| `oh-my-openagent/` | TypeScript | **OmO**：多 harness 插件、Team Mode、ultrawork |
| `oh-my-pi/` | TS + **Rust** | **omp**：Pi fork，LSP/DAP/工具质量与 Rust 热路径 |

对 pigs 当前方向特别相关：

- **fugu** — 专家团/协调器 +「一个 model 对外」的产品形态
- **oh-my-openagent** — Team Mode / hooks / 多 harness 适配
- **oh-my-pi** — 工具与 harness 质量、Rust 本地核心
- **codex / claw-code** — Rust 实现与 OpenAI Responses 线格式

"""

    if "## 参考项目" not in text:
        if "## 开发\n" in text:
            text = text.replace("## 开发\n", ref_section + "## 开发\n", 1)
        elif "## 许可证\n" in text:
            text = text.replace("## 许可证\n", ref_section + "## 许可证\n", 1)
        else:
            text = text.rstrip() + "\n" + ref_section

    path.write_text(text, encoding="utf-8")
    print("updated", path)


def update_agents() -> None:
    path = ROOT / "AGENTS.md"
    text = path.read_text(encoding="utf-8")

    old_table = """| 目录 | 语言 | 说明 | 状态 |
|---|---|---|---|
| `CoreCoder/` | Python | 极简（约1k行）教学用编程 Agent，"coding agent 的 nanoGPT"，适合阅读和 fork | 已检出 |
| `claw-code/` | **Rust** | Claude Code 的 Rust 端口，9 个 crate 的工作空间；安全优先、可观测性优先；附带 Python 对等审计层。**与本项目语言相同，最重要参考** | 已检出 |
| `cline/` | TypeScript | 多面 Agent（CLI + VS Code + JetBrains + Kanban + SDK）；严格分层 SDK 架构 | 已检出 |
| `codex/` | **Rust** | OpenAI Codex CLI，约126个内部 crate 的超大工作空间；双重构建系统（Cargo + Bazel）；跨平台沙箱。**与本项目语言相同，重要参考** | 已检出 |
| `deepseek-reasonix/` | Go | Go 重写的编程 Agent（1.0，前身为 TS），面向 DeepSeek 模型；多前端架构 | 已检出 |
| `hermes-agent/` | Python | Nous Research 的自我改进 Agent；闭环学习（技能创建/改进、记忆、跨会话召回） | 已检出 |
| `kilocode/` | TypeScript | 多 IDE 编程 Agent；CLI 是 opencode 的 fork；27 个工作空间包 | 已检出 |
| `openclaw/` | TypeScript | 自托管个人 AI 助手；渠道优先（22+ 消息平台）；150+ 可插拔扩展 | 已检出 |
| `opencode/` | TypeScript | 大型开源编程 Agent monorepo（Bun + Effect + SolidJS）；约34个工作空间包 | 已检出 |
| `pi/` | TypeScript | "Pi Agent Harness" monorepo；自扩展编程 Agent CLI + Agent 运行时 + 统一多供应商 LLM API | 已检出 |
"""
    new_table = """| 目录 | 语言 | 说明 | 状态 |
|---|---|---|---|
| `CoreCoder/` | Python | 极简（约1k行）教学用编程 Agent，"coding agent 的 nanoGPT"，适合阅读和 fork | 已检出 |
| `claw-code/` | **Rust** | Claude Code 的 Rust 端口，9 个 crate 的工作空间；安全优先、可观测性优先。**与本项目语言相同，最重要参考** | 已检出 |
| `cline/` | TypeScript | 多面 Agent（CLI + VS Code + JetBrains + Kanban + SDK）；严格分层 SDK 架构 | 已检出 |
| `codex/` | **Rust** | OpenAI Codex CLI，约126个内部 crate；Responses API、双重构建、跨平台沙箱。**与本项目语言相同，重要参考** | 已检出 |
| `deepseek-reasonix/` | Go | Go 重写的编程 Agent（1.0，前身为 TS），面向 DeepSeek 模型；多前端架构 | 已检出 |
| `hermes-agent/` | Python | Nous Research 的自我改进 Agent；闭环学习（技能创建/改进、记忆、跨会话召回） | 已检出 |
| `kilocode/` | TypeScript | 多 IDE 编程 Agent；CLI 是 opencode 的 fork；27 个工作空间包 | 已检出 |
| `openclaw/` | TypeScript | 自托管个人 AI 助手；渠道优先（22+ 消息平台）；150+ 可插拔扩展 | 已检出 |
| `opencode/` | TypeScript | 大型开源编程 Agent monorepo（Bun + Effect + SolidJS）；约34个工作空间包 | 已检出 |
| `pi/` | TypeScript | "Pi Agent Harness" monorepo；自扩展编程 Agent CLI + Agent 运行时 + 统一多供应商 LLM API | 已检出 |
| `fugu/` | 配置/接入 | **Sakana Fugu**：多模型编排以单 model API 交付；Codex 安装/配置包 + 技术报告 | 已检出 |
| `oh-my-openagent/` | TypeScript | **OmO**：多 harness 插件式 Agent OS；Team Mode、ultrawork、OpenCode/Codex 双发行版 | 已检出 |
| `oh-my-pi/` | TS + **Rust** | **omp**：Pi fork；LSP/DAP/工具质量；Rust `crates/pi-*` 热路径 | 已检出 |
"""
    if old_table not in text:
        raise SystemExit("AGENTS reference table not found")
    text = text.replace(old_table, new_table)

    # Architecture themes
    if "fugu（协调器" not in text:
        text = text.replace(
            "- **Agent 自治/协调**：claw-code 的 OmX + clawhip + OmO 多 Agent 协调系统；hermes-agent 的自我改进闭环。\n",
            "- **Agent 自治/协调**：claw-code 的 OmX + clawhip + OmO 多 Agent 协调系统；hermes-agent 的自我改进闭环。\n"
            "- **多模型编排 / 单 model 交付**：fugu（Coordinator/Conductor + 模型池，对外 Chat/Responses）；oh-my-openagent Team Mode。\n"
            "- **IDE 级工具面**：oh-my-pi 的 LSP/DAP 与 harness 调优；对照上游 pi harness。\n",
        )

    # Key files list
    if "fugu/README.md" not in text:
        text = text.replace(
            "- `docs/参考项目分析.md` — 本仓库中对所有参考项目的综合分析文档。\n",
            "- `docs/参考项目分析.md` — 本仓库中对所有参考项目的综合分析文档。\n"
            "- `fugu/README.md` + `Fugu_technical_report.pdf` — 多模型编排与单 model API 形态。\n"
            "- `oh-my-openagent/AGENTS.md` / Team Mode 文档 — 多 harness 与多 Agent 团队。\n"
            "- `oh-my-pi/AGENTS.md` + `crates/pi-*` — 工具质量与 Rust 热路径。\n",
        )

    # Build commands
    if "oh-my-openagent" not in text.split("## 参考项目的构建命令")[-1][:800]:
        text = text.replace(
            "- **pi**（npm/TS）：`npm install` 然后查看 `packages/*/package.json` 脚本\n",
            "- **pi**（npm/TS）：`npm install` 然后查看 `packages/*/package.json` 脚本\n"
            "- **fugu**（配置/接入）：主要是安装脚本与 Codex profile；`codex-fugu` / 见 `docs/commands_details.md`（需 Sakana API）\n"
            "- **oh-my-openagent**（Bun/TS）：`bun install`；OpenCode Ultimate 与 Codex Light（lazycodex）见 README\n"
            "- **oh-my-pi**（Bun + Rust）：`bun install`；Rust 侧 `cargo build`（`crates/pi-*`）；CLI 见 `omp` / README\n",
        )

    # Fix outdated "no source" if still present
    text = text.replace(
        "父仓库 `pigs` 目前**没有构建、测试或 lint 命令**——还没有源代码可构建。待新 Agent 的 Rust 代码开始后，将使用 `cargo` 作为构建工具。\n",
        "父仓库 `pigs` 使用 `cargo build` / `cargo test` / `cargo clippy` 构建与验证产品代码（`crates/pigs-*`）。参考项目各自独立构建。\n",
    )

    path.write_text(text, encoding="utf-8")
    print("updated", path)


def main() -> None:
    update_analysis()
    update_readme()
    update_agents()
    print("done")


if __name__ == "__main__":
    main()
