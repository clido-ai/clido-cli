# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-beta.2] - 2025-01-20

### Fixed

- **Terminal Stability**: Fixed escape sequence leakage by disabling mouse tracking (DECSET 1002/1003) and bracketed paste mode on startup/exit. Added `stty sane` equivalent and stdin buffer flush to eliminate `^[[201~` garbage on init.
- **Queue Display**: Changed from showing "N queued '.........'" to displaying the truncated first line (50 chars) of EACH queued item, making it clear what's in the queue.
- **History Navigation**: Reset cursor to column 0 when displaying history items. Multiline items now show first line + "…" indicator.
- **OpenRouter Profile Flow**: Fixed profile configuration to prompt for API key when selecting providers that require one (OpenRouter, OpenAI, Anthropic, etc.). Fast model profile now properly saves the complete tuple (provider + key + model).

### Added

- **Queue Editing**: Press ↑ (Up arrow) to cycle through queued items (newest first) before falling back to history. Edit and resubmit to dequeue the original item.
- **Copy Line Mode**: Press **Ctrl+Shift+C** to enter copy mode. Navigate with ↑/↓, Space to select messages, Enter to copy to clipboard. Allows copying chat lines without mouse selection.

### Changed

- **`/stop` Command**: Now executes immediately instead of being queued. If no agent is running, shows "No active run to stop".
