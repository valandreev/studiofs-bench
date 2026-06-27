# AGENTS.md

- Use codebase-memory-mcp first for code discovery; fall back to `rg` only for literals, configs, docs, or when the graph is insufficient.
- Use context7 for current library, framework, SDK, CLI, or cloud-service docs.
- Use the `rust-best-practices` skill for Rust implementation, review, tests, and documentation decisions.
- Keep Rust code on edition 2024 with MSRV 1.95.
- Keep workspace lint policy in root `[workspace.lints]`; member crates opt in with `[lints] workspace = true`.
- Keep shared lints low-noise. Stage broad lint or hygiene migrations separately.
- Put contracts in `docs/contracts/`.
- Put user-facing documentation in `docs/user-manual/`.
- Write Linear issues, PRs, and GitHub comments in English. Use `gh` for GitHub work.
- Before opening a PR, commit all intended workspace changes and do not create draft PRs unless asked.
- Do not mention product-specific reference benchmark names in UI, README, user docs, or implementation.
- Do not run full test suites unless explicitly asked; run focused checks and give the full-suite command for the user.
