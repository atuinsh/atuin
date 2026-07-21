# Self-Hosting the Atuin AI Backend

The core of Atuin AI's backend is open source, available [at atuinsh/atuin-ai-server](https://github.com/atuinsh/atuin-ai-server). It's based on [atuin-ai-core](https://github.com/atuinsh/atuin-ai-core), the same Gleam library that powers the production Atuin AI backend.

The Atuin AI server currently supports any **OpenAI-compatible, chat completions-style endpoint**. For local models, this includes Ollama, vLLM, LM Studio, llama.cpp, and LiteLLM, among others. You can also use OpenAI-compatible web services, like OpenRouter.

## Getting Started

After cloning the repository, copy the example config file, `config.example.toml`, to `config.toml`. Follow the configuration section of the readme to set up your instance.

Here's a very basic example of an Ollama-based setup:

```
port = 8080
endpoint = "http://localhost:11434/v1" # or host.docker.internal
api_key = "ollama"

default_model = "llama31"

[request.body]
stream_options = { include_usage = true }

[[models]]
alias = "llama31"
name = "Llama 3.1 70b"
description = "Ollama Llama 3.1 70b"
model = "llama3.1:70b"

[[models]]
alias = "gemma4"
name = "Gemma 4 r4b"
description = "Ollama Gemma 4 - Effective 4b"
model = "gemma4:e4b"
```

See the [repository readme](https://github.com/atuinsh/atuin-ai-server#readme) for more setup details, including configuring server-side tools, like web search and web content scraping.

Once done, you can start the server one of two ways:

## Running from Source

If you have Erlang, Elixir, and Gleam installed (see `.tool-versions` for required versions), you can run the server natively:

```
mix deps.get
mix run --no-halt
```

If your `config.toml` specifies API keys via environment variables, remember to set them when you start the server.

## Running with Docker

To run the server with docker, run the following:

```
docker run \
  -v ./config.toml:/etc/atuin-ai/config.toml \
  -p 8080:8080 \
  ghcr.io/atuinsh/atuin-ai-server:latest
```

If you're running via Docker and want the Atuin AI server to connect with a local LLM service running on the host, like Ollama, use `host.docker.internal` as the endpoint instead of `localhost` (which would resolve to the container's own loopback interface).

## Configuring Atuin AI

Once your server is running, you can configure Atuin AI to connect to it by setting the endpoint config:

```
[ai]
endpoint = "http://localhost:8080"
```
