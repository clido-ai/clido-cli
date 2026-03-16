You are tasked with designing and implementing a next-generation CLI AI coding agent.

The design must be based on the reverse-engineering findings of modern coding agents (e.g. Claude CLI and Cursor Agent), while improving their architecture where possible.

The goal is to build a robust, extensible system that can perform coding tasks, repository analysis, auditing, writing, and other developer workflows.


--------------------------------------------------

PRIMARY GOAL

Design and implement a developer-friendly CLI agent that:

- operates primarily locally
- performs multi-step reasoning workflows
- interacts with repositories and files
- executes tools
- iterates until tasks are complete

The system must be capable of replacing existing CLI coding agents while maintaining a clean architecture.


--------------------------------------------------

LOCAL-FIRST REQUIREMENT

Everything should run locally whenever possible.

The only external services allowed are model APIs.

This includes providers such as:

- OpenRouter
- OpenAI
- Anthropic
- Alibaba Cloud
- local models if available

All other components must run locally:

- agent runtime
- tool execution
- context building
- repository indexing
- memory storage
- session history
- CLI interface


--------------------------------------------------

CORE SYSTEM ARCHITECTURE

The system should follow a modular architecture:

Agent Runtime
│
├─ Agent Loop
│
├─ Context Engine
│   ├─ repository knowledge
│   ├─ relevant files
│   ├─ tool results
│   └─ conversation history
│
├─ Tool System
│
├─ Model Provider Layer
│
└─ Storage / Memory Layer


--------------------------------------------------

AGENT LOOP

Implement an agent loop similar to those discovered in the reverse-engineering process.

Execution pattern:

User input
↓
build context
↓
call model
↓
model response
↓
tool call(s)
↓
execute tools locally
↓
append results to history
↓
repeat until completion


Additional requirements:

- configurable max_turns
- optional cost limits
- graceful error handling
- ability to recover from failed tool calls


--------------------------------------------------

TOOL SYSTEM

Implement a modular tool system.

Minimum tools required:

Read
    read file contents

Write
    write full file

Edit
    replace text in file

Glob
    search files by pattern

Grep
    search inside files

Bash
    execute shell commands locally

Tools must expose structured schemas describing:

- tool name
- description
- parameters

Example:

{
  "name": "read_file",
  "description": "Read the contents of a file",
  "parameters": {
    "type": "object",
    "properties": {
      "file_path": { "type": "string" }
    }
  }
}


--------------------------------------------------

CONTEXT ENGINE

Instead of relying on Markdown files for everything, implement a structured context system.

The context builder should assemble model input dynamically from:

system instructions
+
user request
+
conversation history
+
relevant repository files
+
tool results
+
optional memory

The system should prioritize relevant files rather than sending entire repositories.


--------------------------------------------------

REPOSITORY INTERACTION

The agent must be able to explore repositories using tools such as:

Glob
Grep
Read

Optional improvements:

- repository indexing
- dependency graphs
- semantic search

All repository analysis must run locally.


--------------------------------------------------

MEMORY AND STORAGE

The agent should store session history and memory locally.

Use structured formats such as JSON or a local database.

Example structure:

{
  "role": "assistant",
  "tool_calls": [
    {
      "tool": "read_file",
      "args": { "file_path": "src/server.ts" }
    }
  ]
}

Markdown files may still be used for human-editable instructions, but they should not serve as the primary storage mechanism.


--------------------------------------------------

MODEL PROVIDER ABSTRACTION

The agent must support multiple LLM providers through a unified interface.

Supported providers should include:

- OpenRouter
- OpenAI
- Anthropic
- Alibaba Cloud
- local models

The provider layer must normalize:

- API request formats
- tool calling interfaces
- streaming responses
- authentication methods

Example interface:

class ModelProvider:
    def generate(self, messages, tools):
        pass

This allows switching models without changing the agent logic.


--------------------------------------------------

CLI INTERFACE

The agent must expose a CLI interface.

Example usage:

agent "audit this repository"

agent -p "fix failing tests"

Recommended features:

- streaming output
- JSON output mode
- session resume
- configurable model selection


--------------------------------------------------

EXTENSIBILITY

The architecture should allow easy extension.

It must support adding:

- new tools
- new model providers
- new memory systems
- additional reasoning modules

without large architectural changes.


--------------------------------------------------

FINAL OBJECTIVE

The final system should be a clean, extensible, local-first CLI agent that:

- reproduces the core behavior of modern coding agents
- avoids brittle patterns such as Markdown-only context storage
- supports multiple LLM providers
- runs primarily locally
- remains developer-friendly

The reverse-engineering findings should serve as the technical foundation for this implementation.
