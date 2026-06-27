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

<!-- codebase-memory-mcp:start -->
## Codebase Knowledge Graph (codebase-memory-mcp)

This checkout uses codebase-memory-mcp. Prefer graph tools over grep/glob for
code discovery; use grep only for string literals, logs, config values, and
non-code files.

Discovery order:

1. `search_graph` - find symbols by natural-language query, regex name, label,
   or file pattern.
2. `trace_path` - trace callers/callees for exact symbol names.
3. `get_code_snippet` - read source for a qualified name found by
   `search_graph`.
4. `query_graph` - run Cypher-like queries for complex relationships.
5. `search_code` - graph-augmented text search inside indexed files.

Examples:

- Find a symbol:
  `search_graph(project="Volumes-Storage-dev-studiofs", name_pattern=".*StudioVfsAdapter.*", limit=10)`
- Read source:
  `get_code_snippet(project="Volumes-Storage-dev-studiofs", qualified_name="Volumes-Storage-dev-studiofs.crates.studiofs-macfuse.src.adapter.StudioVfsAdapter")`
- Find callers/callees:
  `trace_path(project="Volumes-Storage-dev-studiofs", function_name="read_into_fh", direction="both", mode="calls", include_tests=true)`
- Query graph directly:
  `MATCH (f:Function)-[:CALLS]->(g:Function) WHERE g.name = 'read_into_fh' RETURN f.qualified_name, g.qualified_name LIMIT 20`
- Text search with graph context:
  `search_code(project="Volumes-Storage-dev-studiofs", pattern="deferred_release_inflight", path_filter="crates/studiofs-vfs/src/.*\\.rs$", limit=20)`

Notes:

- Use `get_graph_schema`; stored procedures such as `CALL db.labels()` are not
  supported by this MCP query engine.
- Common labels in this repo include `Function`, `Method`, `Class`, `Field`,
  `Enum`, `Interface`, `File`, `Module`, `Route`, and `EnvVar`.
- Common code-node properties include `name`, `qualified_name`, `file_path`,
  `start_line`, `end_line`, `is_test`, `is_exported`, `complexity`, and
  `signature`.
<!-- codebase-memory-mcp:end -->
