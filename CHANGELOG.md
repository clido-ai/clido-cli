# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0-beta.9] - 2026-04-04

### Added

- **Solidity audit workflow**: Comprehensive Solidity smart contract audit workflow (`workflows/solidity-audit.yaml`) with automated security analysis, common vulnerability checks, gas optimization suggestions, and report generation.
- **Workflow `save_to` output**: Workflow steps can now specify a `save_to` field to write step output to a file on disk.
- **Workflow prerequisites**: Workflow steps can declare `prerequisites` — a list of files that must exist before the step runs.
- **Workflow `--profile` CLI flag** (`-p`): Override the default profile when running workflows, allowing different provider/model combinations per run.
- **Alibaba Cloud, MiniMax, and local providers**: New provider integrations in `clido-providers`.

### Changed

- **`list_models()` error propagation**: Changed from returning `Vec<ProviderInfo>` to `Result<Vec<ProviderInfo>, ProviderError>`. Callers now receive structured error information instead of silently dropping failures.
- **Model fetch failure handling**: `clido-cli` now surfaces model fetch errors gracefully instead of swallowing them.
- **Profile overlay improvements**: Profile configuration resolution and merging logic refined for more predictable behavior.
- **Removed stale providers**: Cohere, Together AI, Azure OpenAI, Custom, and LM Studio providers removed.

### Fixed

- **TUI workflow orchestration**: Major overhaul of workflow execution in the TUI — improved state management, event handling, and command dispatch for reliable workflow runs from the interactive interface.
- **Plan rendering in TUI**: Better handling of plan display with correct column/byte-width tracking.

## [0.1.0-beta.5] - 2026-04-03

### Changed

- Removed all references to competitor AI agents from code and documentation
- Replaced comparison table in introduction with feature highlights

## [0.1.0-beta.4] - 2026-04-03

### Added

- **Workflow TUI commands** (`/workflow`): Full workflow management from the interactive TUI — create, list, show, edit, save, and run YAML workflows without leaving the chat.
- **AI-guided workflow creation** (`/workflow new <desc>`): Describe what you want, and the agent walks you through designing the workflow step by step — asking about inputs, steps, tools, error handling, and parallelism. The generated YAML appears in the chat for review.
- **Workflow text editor** (`/workflow edit`): Nano-style full-screen YAML editor with validation on save (Ctrl+S). Opens saved workflows by name or the last YAML draft from chat.
- **Workflow save from chat** (`/workflow save`): Extracts the last YAML code block from assistant messages, validates it as a valid workflow, and saves to `.clido/workflows/`.
- **Workflow run from TUI** (`/workflow run <name>`): Sends workflow steps to the agent for execution directly from the chat.

### Documentation

- Updated `docs/guide/workflows.md` with TUI commands section
- Updated `docs/reference/slash-commands.md` with Workflow command table
- Updated `docs/reference/key-bindings.md` with Workflow editor keybindings
- Updated `FEATURES.md` with `/workflow` commands

## [0.1.0-beta.3] - 2026-04-03

### Added

- **In-app text selection**: Shift+drag to select text in the chat area, even with mouse capture enabled. Selection auto-copies to clipboard on mouse release. Works character-by-character with proper Unicode width handling.
- **Toast notifications**: Non-blocking overlay messages for clipboard copy confirmation and other status events. Positioned near the mouse cursor for copy actions, top-right for other toasts. Auto-dismiss after 2–3 seconds.
- **Prompt enhancement review** (`/enhance`): Enhanced prompts are now placed in the input field for review and editing before sending, instead of being auto-submitted. A spinner with cyan border shows progress while the utility model works.

### Fixed

- **Phantom session creation**: Fixed race condition where duplicate sessions could be created during startup. Recovery logic now falls back correctly when `current_session_id` is unset, and `find_recent_session` accepts meta-only sessions within a 2-second startup window.
- **Text selection coordinate mapping**: Fixed screen-to-content row mapping that was missing the chat area Y offset, causing selection to highlight wrong lines.
- **Selection column tracking**: Fixed byte-length vs display-width mismatch in selection highlighting that caused whole-line selection instead of character-level. Spans are now properly split at column boundaries.
- **Toast rendering**: Fixed width mismatch (content vs rect) that caused broken/shifted text. Toast background now fills the entire widget area.

### Changed

- **`/enhance` workflow**: No longer auto-submits. The enhanced prompt is placed in the input field so you can review, edit, then press Enter to send — or discard it.

## [0.1.0-beta.2] - 2025-01-20

### Fixed

- **Session Loss Bug**: Fixed critical bug where sessions would appear "lost" after restarting clido and switching workdirs. The initial workspace_root was not canonicalized, causing session path mismatches when workdir was switched (which always uses canonicalized paths).
- **Request Timeout**: Increased from 2 minutes to 7 minutes to handle large code generation. Added explicit timeout retry - if a request times out, it automatically retries up to 2 times without delay. Prevents indefinite hangs like the 25+ minute stuck requests.
- **Kimi Code User-Agent**: Changed default User-Agent for kimi-code provider to `RooCode/3.0.0` for better compatibility with the Kimi for Coding API.
- **Terminal Stability**: Fixed escape sequence leakage by disabling mouse tracking (DECSET 1002/1003) and bracketed paste mode on startup/exit. Added `stty sane` equivalent and stdin buffer flush to eliminate `^[[201~` garbage on init.
- **Queue Display**: Changed from showing "N queued '.........'" to displaying the truncated first line (50 chars) of EACH queued item, making it clear what's in the queue.
- **History Navigation**: Reset cursor to column 0 when displaying history items. Multiline items now show first line + "…" indicator.
- **OpenRouter Profile Flow**: Fixed profile configuration to prompt for API key when selecting providers that require one (OpenRouter, OpenAI, Anthropic, etc.). Fast model profile now properly saves the complete tuple (provider + key + model).

### Added

- **Queue Editing**: Press ↑ (Up arrow) to cycle through queued items (newest first) before falling back to history. Edit and resubmit to dequeue the original item.

### Changed

- **`/stop` Command**: Now executes immediately instead of being queued. If no agent is running, shows "No active run to stop".
