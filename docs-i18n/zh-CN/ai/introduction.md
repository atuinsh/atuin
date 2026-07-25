# Atuin AI

Atuin AI 是一个子命令，让你可以直接在终端中通过 LLM 生成 shell 命令，并查找其他信息。

使用 Atuin AI 需要一个 [Atuin Hub](https://hub.atuin.sh/) 账户，Atuin 会在你首次使用该二进制文件时要求你登录。你也可以选择 [自托管 Atuin AI 后端](./self-hosting.md)。

目前 Atuin AI 可免费使用。

## 快速开始

Atuin AI 目前支持 zsh、bash 和 fish shell。完整的支持矩阵请参阅 [支持的平台](../support.md)。你 shell 配置中常规的 `atuin init` 调用会自动将问号键绑定到 Atuin AI 界面（仅在提示符为空时生效）。

!!! note "禁用 Atuin AI"

    你可以在 shell 的 `atuin init` 调用中传入 `--disable-ai`，或在 Atuin 配置中将 `ai.enabled` 设为 `false`，以禁用默认的问号键绑定。

## 设置

关于控制 Atuin AI 行为的设置，完整列表请参阅 [专门的设置文档](./settings.md)。

## 功能

### 命令生成

向 LLM 描述需求即可生成命令。按 `enter` 运行，或按 `tab` 插入。

[![Atuin AI 基本用法](./images/basic.png)](./images/basic.png)

### 追问

你可以通过追加提示词来更新即将插入的命令。

[![Atuin AI 优化用法基本示例](./images/basic-refine.png)](./images/basic-refine.png)

你也可以通过追问获取自然语言形式的回答。

[![Atuin AI 追问信息用法基本示例](./images/basic-followup-questions.png)](./images/basic-followup-questions.png)

即使建议命令是 LLM 在之前的对话轮次中提出的，你仍然可以按 `enter` 或 `tab` 来运行或插入这条最近建议的命令。

### 对话式与搜索式用法

如果你提出的问题并非意在生成命令，LLM 会用自然语言作答，并在必要时通过网络搜索获取所需数据。

[![向它提问](./images/question.png)](./images/question.png)

### 危险或低置信度命令检测

LLM 会对命令的置信度与危险程度进行评分；一旦超过阈值，就会显示这一信息，并且在通过 `enter` 自动运行前，需要额外确认一步。

Atuin Hub 服务器还会监测建议命令，以发现 LLM 未能识别的危险模式，并在 LLM 自身评估结果之后附加服务器端的评估。

[![可能存在危险的命令会被标记](./images/danger.png)](./images/danger.png)
