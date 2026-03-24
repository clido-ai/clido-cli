# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Documentation: development plan, CLI interface specification, release plans, testing strategy, local development testing guide, schemas, and algorithm specs.
- **Agent Profiles**: multiple named profiles with `clido profile list/create/switch/edit/delete`. Each profile carries a main agent and optional per-profile `worker`/`reviewer` sub-agent slots that override the global agent config. Active profile is shown in the TUI header; `/profile <name>` and `/profiles` slash commands allow in-session switching. Profile switching takes effect immediately for the next interaction.
