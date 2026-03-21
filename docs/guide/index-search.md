# Repository Index

The repository index is an optional file and symbol index that enables the `SemanticSearch` tool. When the index is built, the agent can quickly find relevant files and code symbols without reading the entire codebase.

## What the index is

The index is a SQLite database (`.clido/index.db`) that stores:

- **File records** — path, size, modification time, and language
- **Symbol records** — function names, struct names, type aliases, constants, etc., with their file and line number

When the agent calls the `SemanticSearch` tool, it queries this index using full-text search to find relevant code symbols and files. This is much faster than a grep over a large codebase and works for symbol-level queries like "find all implementations of the Serialize trait".

## Building the index

Index the current directory with default settings:

```bash
clido index build
```

Index a specific directory:

```bash
clido index build --dir /path/to/project
```

Index only specific file types (comma-separated extensions):

```bash
clido index build --ext sol,rs,py,js,ts
```

Default extensions include **Web3 and smart-contract languages first**, then general-purpose languages:

| Category | Extensions |
|----------|-----------|
| Smart contracts | `sol` (Solidity), `move` (Move/Aptos/Sui), `vy` (Vyper), `fe` (Fe), `yul` (Yul/Yul+), `rell` (Rell/Chromia), `cairo` (Cairo/StarkNet) |
| General purpose | `rs`, `py`, `js`, `ts`, `go`, `java`, `c`, `cpp`, `h`, `md` |

Building the index is idempotent — re-running it updates changed files and removes deleted ones.

::: tip Auto-build
The `SemanticSearch` tool **builds and refreshes the index automatically** before every search. You never need to run `clido index build` manually — the agent handles it. The index is refreshed if it is older than 1 hour.
:::

::: tip Incremental updates
`clido index build` performs an incremental update: only files that have changed since the last build are re-indexed. For large codebases this is much faster than a full rebuild.
:::

## Checking index statistics

```bash
clido index stats
```

```
Index: .clido/index.db
Files indexed: 247
Symbols indexed: 3,891
Last updated: 2026-03-21 14:32:11 UTC
Size: 2.1 MB
```

## Clearing the index

Delete the index database entirely:

```bash
clido index clear
```

This removes `.clido/index.db`. Rebuild with `clido index build`.

## How the agent uses the index

When the index is present, clido automatically enables the `SemanticSearch` tool. The agent can call it with a query string:

```
[SemanticSearch] query: "parse error handling"
→ src/parser.rs:42  fn parse_with_error_context
→ src/errors.rs:10  struct ParseError
→ src/errors.rs:25  impl Display for ParseError
```

The agent uses these results to navigate to the right files rather than reading the entire codebase. This reduces token usage and speeds up responses for large projects.

## Supported languages

### Smart contracts and Web3

| Language | Extensions | Notes |
|----------|-----------|-------|
| Solidity | `.sol` | Ethereum, EVM-compatible chains |
| Move | `.move` | Aptos, Sui |
| Vyper | `.vy` | Ethereum (Python-like) |
| Fe | `.fe` | Ethereum (Rust-like) |
| Yul / Yul+ | `.yul` | EVM assembly IR |
| Rell | `.rell` | Chromia blockchain |
| Cairo | `.cairo` | StarkNet |

### General-purpose languages

| Language | Extensions | Symbol types |
|----------|-----------|-------------|
| Rust | `.rs` | Functions, structs, enums, traits, type aliases, constants, modules |
| Python | `.py` | Functions, classes, methods |
| JavaScript | `.js` | Functions, classes, arrow functions |
| TypeScript | `.ts` | Functions, classes, interfaces, type aliases |
| Go | `.go` | Functions, types, methods, interfaces |
| Java | `.java` | Classes, interfaces, methods |
| C / C++ | `.c`, `.cpp`, `.h` | Functions, structs, typedefs |
| Markdown | `.md` | Headings (as navigation symbols) |

Additional languages can be added — see [Adding Tools](/developer/adding-tools) for extension points.

## Index storage location

The index is stored at `.clido/index.db` relative to the directory passed to `--dir` (or the current directory by default). This is a project-local file and can be added to `.gitignore`:

```
# .gitignore
.clido/index.db
```

## How the index is kept fresh

The `SemanticSearch` tool automatically:

1. **Builds** the index on first use if it does not exist yet
2. **Refreshes** it if the existing index is older than 1 hour

This means you can clone a repo and immediately ask the agent semantic questions — no setup step needed. The auto-build note is shown in the tool output:

```
(Building repo index: 312 files, 2,104 symbols — this is a one-time cost)
[SemanticSearch] query: "transfer ownership"
→ contracts/Token.sol:88  function transferOwnership
→ contracts/Ownable.sol:14  event OwnershipTransferred
```

The manual `clido index build` command is still available for CI pipelines or when you want to pre-warm the index before a session:

```bash
# Pre-warm for a large monorepo before starting a session
clido index build && clido "refactor the ERC-20 token to use EIP-2612 permits"
```
