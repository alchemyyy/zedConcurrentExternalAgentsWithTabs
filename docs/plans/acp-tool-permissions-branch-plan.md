# Plan: `pr/acp-tool-permissions` Branch

**Base branch:** `origin/pr/tool-permissions-schema-redesign`
**Reference branch:** `origin/always-allow-revision`
**Draft PR into:** `pr/tool-permissions-schema-redesign`
**Purpose:** Port ACP/external agent permission enforcement from `origin/always-allow-revision` onto the schema-redesign branch.

> **Naming context:** The base branch (`pr/tool-permissions-schema-redesign`) has already renamed `always_allow_tool_actions` → `tool_permissions.default` and `default_mode` → `default`. All code in this plan uses the new naming. The `request_tool_call_authorization` method on that branch takes 3 params (no `respect_always_allow_setting`).

---

## What Already Landed on `origin/main` (and thus on the base branch)

- **Permission Option ID Separator Fix (`:` → `\n`)** — #48636
- **Shared `authorize_file_edit` Function** — #48641
- **Path normalization (`decide_permission_for_path`, `normalize_path`, `most_restrictive`)** — #48640
- **Strengthened hardcoded `rm` security rules** — #48640, #48647
- **Authorization send error handling** — #48639
- **Copy/move path pattern authorization, deferred authorization for save_file** — #48641

None of these need to be ported.

---

## What Remains: ACP/External Agent Permission Enforcement

This is the one major feature not yet on any branch. ~675 lines across 11 files.

### Problem

When an external agent (Claude Code, etc.) requests tool permission via ACP, Zed passes the request straight to the UI authorization prompt without checking `tool_permissions` settings. Regex patterns (`always_deny`, `always_allow`, `always_confirm`) and the global `default` have no effect on external agents.

### Overview of Changes

#### 1. Hardcoded security rules in `agent_settings` (prerequisite)

The ACP code in `agent_servers` needs access to the hardcoded security rules (e.g. blocking `rm -rf /`), but `agent_servers` cannot depend on the `agent` crate. The solution is to expose the hardcoded rules from `agent_settings`, which `agent_servers` already depends on.

**`crates/agent_settings/src/agent_settings.rs`:**
- Add `LazyLock` to the `std::sync` import
- Add `HARDCODED_SECURITY_DENIAL_MESSAGE` constant
- Add `HardcodedSecurityRules` struct with `pub terminal_deny: Vec<CompiledRegex>`
- Add `HARDCODED_SECURITY_RULES` static via `LazyLock` (use the same regex patterns that are in `tool_permissions.rs`)
- Add `pub fn check_hardcoded_security_rules(tool_name: &str, terminal_tool_name: &str, input: &str, extracted_commands: Option<&[String]>) -> Option<String>` — checks input and optional sub-commands against the hardcoded patterns, returns denial reason or `None`

> Note: `tool_permissions.rs` on main already has its own inline implementation with improved regex patterns and path normalization. The `agent_settings` version is simpler and used only by the ACP code path. These should be consolidated in a future cleanup.

**`crates/settings_ui/src/pages/tool_permissions_setup.rs`:**
- Update import of `HARDCODED_SECURITY_RULES` from `agent` to `agent_settings`.

#### 2. `deny_once_option_id` on `PermissionOptions` (prerequisite)

**`crates/acp_thread/src/connection.rs`:**
- Add `pub fn deny_once_option_id()` to `PermissionOptions`, parallel to the existing `allow_once_option_id()`. Uses `PermissionOptionKind::RejectOnce`.

#### 3. Shell parser for `agent_servers`

The ACP permission check needs to parse terminal commands into sub-commands (so chained commands like `cargo build && rm -rf /` are checked individually). Since `agent_servers` can't depend on `agent`, the shell parser is duplicated.

**`crates/agent_servers/Cargo.toml`:**
- Add dependencies: `agent_settings`, `brush-parser`, `paths`
- Add dev-dependency: `workspace` (with `test-support` feature)

**`crates/agent_servers/src/agent_servers.rs`:**
- Add `mod shell_parser;`

**`crates/agent_servers/src/shell_parser.rs`** (new file):
- Copy of `crates/agent/src/shell_parser.rs` (the `brush-parser`-based command extractor).

#### 4. ACP permission enforcement (the main change)

**`crates/agent_servers/src/acp.rs`** (~276 lines changed):

New imports: `agent_settings::AgentSettings`, `FutureExt`, `ToolPermissionMode`, `crate::shell_parser::extract_commands`.

**`AcpPermissionDecision` enum:**
```rust
enum AcpPermissionDecision {
    Allow,
    Deny(String),
    Confirm,
}
```

**`check_acp_tool_permission(tool_name, input, settings) -> AcpPermissionDecision`:**

Replicates the core logic of `decide_permission_from_settings` for ACP code paths:
1. Extracts sub-commands for terminal tool via `extract_commands`
2. Checks hardcoded security rules via `agent_settings::check_hardcoded_security_rules`
3. Looks up tool-specific rules; if none, falls back to `settings.tool_permissions.default`:
   - `Allow` → `AcpPermissionDecision::Allow`
   - `Deny` → `AcpPermissionDecision::Deny("Blocked by global default: deny")`
   - `Confirm` → `AcpPermissionDecision::Confirm`
4. Checks invalid patterns → Deny
5. For each sub-command, checks deny/confirm/allow patterns:
   - DENY: if ANY sub-command matches a deny pattern → immediate deny
   - CONFIRM: if ANY sub-command matches a confirm pattern → confirm
   - ALLOW: ALL sub-commands must match at least one allow pattern (disabled if terminal command parsing failed)
6. Falls back to `rules.default.unwrap_or(settings.tool_permissions.default)`

**`request_permission()` rewrite:**
- Extracts `has_own_permission_modes` from `session.session_modes.is_some()`
- Wraps options in `PermissionOptions::Flat(arguments.options)` before the closure
- Extracts tool name from `tool_call.meta` via `acp_thread::TOOL_NAME_META_KEY`
- Calls `check_acp_tool_permission` using the tool call's title as best-effort input for pattern matching
- On `Deny`: calls `upsert_tool_call_inner` with `Rejected` status, selects deny option via `options.deny_once_option_id()` or returns `Cancelled`
- On `Allow` when agent has no own permission modes: auto-allows via `options.allow_once_option_id()`
- On `Confirm` or when agent has own modes: falls through to UI prompt via `thread.request_tool_call_authorization(tool_call, options, cx)`
- Legacy fallback for when no tool name is available: checks `settings.tool_permissions.default` directly

**`write_text_file()` permission checks:**
- Calls `check_acp_tool_permission("edit_file", &path_str, settings)` before writing
- On `Deny` → error
- On `Confirm` → error telling agent to use `request_permission` first
- On `Allow` → proceed, but still check sensitive paths:
  - Blocks writes targeting local settings directories (`.zed/`)
  - Blocks writes targeting the global config directory (via `canonicalize` on parent)

#### 5. Doc comment updates

**`crates/settings_content/src/agent.rs`:**
- Update `tool_permissions` doc comment to state that patterns apply to both native and external agents. For external agents, patterns are matched against the tool call's title rather than raw tool input.

**`assets/settings/default.json`:**
- Update comments in `tool_permissions` section to document ACP applicability.
- Update `"tools"` comment to note that per-tool rules and regex patterns apply to both native and external agents, and that the per-tool `"default"` also applies to MCP tools.

#### 6. New tests

**`crates/agent/src/tests/mod.rs`** — 4 new integration tests:
- `test_edit_file_tool_allow_still_prompts_for_local_settings` — Sets a tool-specific `default: Allow` for `edit_file`, runs it on `.zed/settings.json`, asserts authorization is still required because it's a sensitive settings path.
- `test_create_directory_tool_deny_rule_blocks_creation` — Configures an `always_deny` pattern for `create_directory`, runs the tool with a matching path, asserts tool call fails.
- `test_copy_path_tool_deny_by_user` — Runs `copy_path` with `default: Confirm`, user denies in authorization prompt, asserts failure and that no files were copied.
- `test_move_path_tool_deny_by_user` — Same pattern as copy, but for `move_path`.

**`crates/agent/src/tool_permissions.rs`** — 1 new unit test:
- `always_confirm_works_for_file_tools` — Tests `always_confirm` patterns on `EditFileTool`, `DeletePathTool`, and `FetchTool`. Verifies confirm beats allow, deny beats confirm, and non-matching inputs fall through to the tool default.

---

## Tests to Run

```bash
# New tests
cargo test -p agent --lib -- test_edit_file_tool_allow_still_prompts_for_local_settings
cargo test -p agent --lib -- test_create_directory_tool_deny_rule_blocks_creation
cargo test -p agent --lib -- test_copy_path_tool_deny_by_user
cargo test -p agent --lib -- test_move_path_tool_deny_by_user
cargo test -p agent --lib -- always_confirm_works_for_file_tools

# Existing tests that should still pass
cargo test -p agent --lib -- test_terminal_tool_permission_rules
cargo test -p agent --lib -- test_mcp_tools
cargo test -p agent --lib -- test_permission_option_ids_for_terminal
cargo test -p agent --lib -- test_authorize test_needs_confirmation
cargo test -p agent --lib -- hardcoded
cargo test -p agent_servers --lib
cargo test -p agent_ui --lib -- test_option_id_transformation
```

---

## What NOT to Port

These changes exist on `origin/always-allow-revision` but should NOT be included:

- **`always_allow_tool_actions` → `tool_permissions.default` rename** — Already on `pr/tool-permissions-schema-redesign`
- **`default_mode` → `default` rename** — Already on `pr/tool-permissions-schema-redesign`
- **Settings UI changes** — Already on `pr/tool-permissions-schema-redesign`
- **Migration code** (`crates/migrator/`) — Already on `pr/tool-permissions-schema-redesign`
- **`settings_content` schema changes** (removing `always_allow_tool_actions` field, adding `default` field to `ToolPermissionsContent`, `serde(alias)` additions) — Already on `pr/tool-permissions-schema-redesign`
- **Format-on-save changes** in `streaming_edit_file_tool.rs` — Unrelated
- **`apply_edits` rewrite** in `streaming_edit_file_tool.rs` — Unrelated
- **Separator fix (`:` → `\n`)** — Already on `origin/main` (#48636)
- **Shared `authorize_file_edit`** — Already on `origin/main` (#48641)
- **`normalize_path` / `decide_permission_for_path`** — Already on `origin/main` (#48640)
- **Hardcoded security regex improvements** — Already on `origin/main` (#48640)
- **Path normalization tests, rm bypass tests** — Already on `origin/main` (#48640, #48647)