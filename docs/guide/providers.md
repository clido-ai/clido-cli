# Providers & Models

clido supports multiple LLM providers. Each provider is configured as a profile in `config.toml`. You can switch providers at runtime with `--provider` and `--model`.

## Supported providers

| Provider | Key in config | API key variable | Notes |
|----------|--------------|-----------------|-------|
| Anthropic | `anthropic` | `ANTHROPIC_API_KEY` | Default; supports prompt caching |
| OpenAI-compatible | `openai` | `OPENAI_API_KEY` | Any OpenAI-API endpoint |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` | Multi-model aggregator |
| Alibaba Cloud | `alibabacloud` | `DASHSCOPE_API_KEY` | DashScope / Qwen models |
| Local (Ollama) | `local` | — | No API key required |

## Anthropic (Claude)

The default provider. Connects to `https://api.anthropic.com`.

```toml
[profile.default]
provider    = "anthropic"
model       = "claude-3-5-sonnet-20241022"
api_key_env = "ANTHROPIC_API_KEY"
```

### Recommended models

| Model | Description |
|-------|-------------|
| `claude-3-5-sonnet-20241022` | Best balance of capability and cost (default) |
| `claude-3-opus-20240229` | Highest capability, highest cost |
| `claude-3-haiku-20240307` | Fastest and cheapest; good for simple tasks |

### Prompt caching

When using Anthropic, clido automatically enables prompt caching for the system prompt and long conversation histories. Cache hits are billed at ~10% of the normal input token price, which significantly reduces costs for long sessions.

## OpenAI-compatible

Any server that implements the OpenAI Chat Completions API can be used. This includes Azure OpenAI, Together AI, Groq, Fireworks AI, and others.

```toml
[profile.azure]
provider    = "openai"
model       = "gpt-4o"
api_key_env = "AZURE_OPENAI_API_KEY"
base_url    = "https://my-resource.openai.azure.com/openai/deployments/gpt-4o"
```

### OpenAI

```toml
[profile.gpt4]
provider    = "openai"
model       = "gpt-4o"
api_key_env = "OPENAI_API_KEY"
```

## OpenRouter

OpenRouter provides access to many models (Claude, GPT, Mistral, Gemini, and more) through a single API key. Models are identified by `provider/model-name`.

```toml
[profile.openrouter]
provider    = "openrouter"
model       = "anthropic/claude-3-5-sonnet"
api_key_env = "OPENROUTER_API_KEY"
```

### Common OpenRouter models

| Model string | Description |
|-------------|-------------|
| `anthropic/claude-3-5-sonnet` | Claude 3.5 Sonnet via OpenRouter |
| `openai/gpt-4o` | GPT-4o via OpenRouter |
| `mistralai/mistral-large` | Mistral Large |
| `google/gemini-pro-1.5` | Gemini Pro 1.5 |
| `meta-llama/llama-3.1-70b-instruct` | Meta Llama 3.1 70B |

## Local (Ollama)

Run models locally with [Ollama](https://ollama.ai). No API key or network connection required after pulling the model.

```toml
[profile.local]
provider = "local"
model    = "llama3.2"
base_url = "http://localhost:11434"
```

Start Ollama and pull a model:

```bash
ollama serve              # start the server
ollama pull llama3.2      # download the model
```

::: warning Local model limitations
Local models generally have smaller context windows and weaker instruction-following than cloud models. Complex multi-step coding tasks may require a larger model (e.g. `llama3.1:70b`) for reliable tool use.
:::

## Alibaba Cloud (DashScope / Qwen)

Connects to the DashScope OpenAI-compatible endpoint. Set `DASHSCOPE_API_KEY` or store the key in your profile.

```toml
[profile.alibaba]
provider    = "alibabacloud"
model       = "qwen-max"
api_key_env = "DASHSCOPE_API_KEY"
```

You can override the endpoint with `base_url` if needed (defaults to `https://dashscope.aliyuncs.com/compatible-mode/v1`).

## Listing available models

List all models known to clido for a provider (from the built-in pricing table):

```bash
clido list-models
clido list-models --provider anthropic
clido list-models --provider openrouter --json
```

## Fetching the current model list from a provider's API

Retrieve the live model list from a provider:

```bash
clido fetch-models
clido fetch-models --provider openrouter
```

::: tip Updating pricing data
If model pricing changes, update the local pricing table:
```bash
clido update-pricing
```
:::

## Switching provider and model at runtime

Override the profile's provider and model for a single run:

```bash
# Use a different model
clido --model claude-3-haiku-20240307 "quick task"

# Use a different profile
clido --profile local "quick task"

# Override both
clido --provider openrouter --model anthropic/claude-3-5-sonnet "task"
```

These flags only affect the current invocation; they do not modify `config.toml`.

## Per-profile base_url

For providers that need a custom endpoint (Azure, self-hosted, Ollama):

```toml
[profile.custom]
provider = "openai"
model    = "my-model"
base_url = "https://my-server.internal/v1"
api_key_env = "MY_API_KEY"
```

The `base_url` is used as-is; clido appends `/chat/completions` (OpenAI-compatible providers) or the appropriate path for each provider.

## API key security

clido looks for API keys in this order:

1. `api_key` field in the profile (stored in plain text — not recommended for shared machines)
2. Environment variable named by `api_key_env` in the profile
3. Well-known environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`)

The recommended approach is to set the environment variable in your shell profile:

```bash
# ~/.zshrc or ~/.bashrc
export ANTHROPIC_API_KEY="sk-ant-..."
```
