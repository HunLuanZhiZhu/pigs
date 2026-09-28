# Local machine config (gitignored)

This directory and `../config-cli.local.toml` are **not** committed.

They hold machine-only test endpoints and API keys for pigs.

## Current local test provider

- Vendor: iFlytek MaaS coding API (华北)
- Model id: `auto`
- Context window: 200000 tokens
- Protocols (see `../config-cli.local.toml`):
  - OpenAI Chat Completions base: `.../v2`
  - OpenAI Responses base: `.../v1`
  - Anthropic Messages base: `.../anthropic`

## Usage

```bash
# from repo root
cargo run -p pigs-cli -- --model auto "ping"
# or catalog aliases from config-cli.local.toml
cargo run -p pigs-cli -- --model maas-auto "ping"
```

In REPL:

```
/models
/model auto
/status
```

If skills from `~/.agents/skills` blow context on small models:

```bash
# Windows PowerShell
$env:PIGS_DISABLE_COMMON_AGENT_SKILLS=1
# or disable all skills
$env:PIGS_DISABLE_SKILLS=1
```

Do **not** paste API keys into tracked files or commit `config-cli.local.toml`.
