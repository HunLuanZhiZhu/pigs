# Rust style rules for this repo

- Prefer explicit error handling (`Result`) over panics.
- Keep crate boundaries clean: core traits, llm providers, tools, cli orchestration.
- Add focused unit tests for new pure helpers.
- Update README/AGENTS/docs when user-facing commands or tools change.
