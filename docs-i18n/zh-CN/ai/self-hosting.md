# 自托管 Atuin AI 后端

Atuin AI 后端的核心是开源的，可在 [`atuinsh/atuin-ai-server`](https://github.com/atuinsh/atuin-ai-server) 获取。它基于 [`atuin-ai-core`](https://github.com/atuinsh/atuin-ai-core) 构建，而这正是驱动生产环境 Atuin AI 后端的同一个 Gleam 库。

Atuin AI 服务器目前支持任何**兼容 OpenAI、采用 chat completions 风格的端点**。对于本地模型，这包括 Ollama、vLLM、LM Studio、llama.cpp 和 LiteLLM 等。你也可以使用兼容 OpenAI 的网络服务，例如 OpenRouter。

## 快速开始

克隆仓库后，将示例配置文件 `config.example.toml` 复制为 `config.toml`。按照 README 中的配置部分设置你的实例。

以下是一个基于 Ollama 的基础配置示例：

```toml
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

更多设置细节，包括配置网页搜索和网页内容抓取等服务器端工具，请参阅[仓库 README](https://github.com/atuinsh/atuin-ai-server#readme)。

完成后，你可以通过以下两种方式之一启动服务器：

## 从源代码运行

如果你已安装 Erlang、Elixir 和 Gleam（所需版本请参见 `.tool-versions`），就可以原生运行该服务器：

```bash
mix deps.get
mix run --no-halt
```

如果你的 `config.toml` 通过环境变量指定 API 密钥，请记得在启动服务器时设置这些变量。

## 使用 Docker 运行

要使用 Docker 运行服务器，请执行以下命令：

```bash
docker run \
  -v ./config.toml:/etc/atuin-ai/config.toml \
  -p 8080:8080 \
  ghcr.io/atuinsh/atuin-ai-server:latest
```

如果你通过 Docker 运行，并希望 Atuin AI 服务器连接到主机上的本地 LLM 服务（例如 Ollama），请使用 `host.docker.internal` 作为端点，而不是 `localhost`。否则，`localhost` 会解析为容器自身的回环接口，而不是主机。

## 配置 Atuin AI

服务器运行后，你可以通过配置端点，让 Atuin AI 连接到它：

```toml
[ai]
endpoint = "http://localhost:8080"
```
