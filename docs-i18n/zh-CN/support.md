# 支持的平台

Atuin 支持多种 shell 和操作系统，但并非每种组合都能获得同等程度的关注。本页定义了我们的支持等级，并记录了各项功能在各平台上的可用情况。

## 支持等级

- **一级** — 由 Atuin 团队积极维护。
- **二级** — 由社区维护。该等级下的复杂问题不会阻塞发布，支持力度为尽力而为。

## Shell

各 shell 的功能覆盖范围有所不同：

<div class="support-matrix">
<table>
  <thead>
    <tr>
      <th class="tier">等级</th><th>Shell</th><th>历史记录搜索</th><th>内联弹窗</th>
      <th>Dotfiles</th><th>Atuin AI</th><th>pty-proxy</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td rowspan="3" class="tier"><strong>1</strong></td>
      <td>zsh</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td>
    </tr>
    <tr><td>bash</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td></tr>
    <tr><td>fish</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td></tr>
    <tr>
      <td rowspan="3" class="tier"><strong>2</strong></td>
      <td>nushell</td><td class="support-yes">✓</td><td class="support-no">✗</td><td class="support-no">✗</td><td class="support-no">✗</td><td class="support-yes">✓</td>
    </tr>
    <tr><td>xonsh</td><td class="support-yes">✓</td><td class="support-no">✗</td><td class="support-yes">✓</td><td class="support-no">✗</td><td class="support-no">✗</td></tr>
    <tr><td>PowerShell</td><td class="support-yes">✓</td><td class="support-no">✗</td><td class="support-yes">✓</td><td class="support-no">✗</td><td class="support-no">✗</td></tr>
  </tbody>
</table>
</div>

## 操作系统与架构

Atuin 支持多种操作系统和 CPU 架构。下表展示了每个平台是否发布了官方预构建二进制文件，以及持续集成（CI）对其测试覆盖的程度。

<div class="support-matrix">
<table>
  <thead>
    <tr>
      <th class="tier">等级</th><th>操作系统</th><th>架构</th><th>预构建</th><th>CI</th><th>备注</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td rowspan="6" class="tier"><strong>1</strong></td>
      <td rowspan="2"><code>Linux</code></td>
      <td><code>x86_64</code></td><td class="support-yes">✓</td><td>已测试</td><td></td>
    </tr>
    <tr><td><code>arm64</code></td><td class="support-yes">✓</td><td>仅构建</td><td></td></tr>
    <tr>
      <td rowspan="2"><code>macOS</code></td>
      <td><code>arm64</code></td><td class="support-yes">✓</td><td>已测试</td>
      <td rowspan="2">无法使用 <kbd>alt</kbd>+<kbd>#</kbd> 重放结果。</td>
    </tr>
    <tr><td><code>x86_64</code></td><td class="support-yes">✓</td><td>仅构建</td></tr>
    <tr>
      <td><code>Windows</code></td><td><code>x86_64</code></td><td class="support-yes">✓</td><td>已测试</td>
      <td>不支持语法高亮，也不支持 <code>pty-proxy</code>。</td>
    </tr>
    <tr><td><code>WSL-2</code></td><td><code>x86_64</code></td><td class="support-yes">✓</td><td>已测试</td><td></td></tr>
    <tr>
      <td rowspan="3" class="tier"><strong>2</strong></td>
      <td><code>Linux</code></td><td><code>riscv64</code></td><td class="support-yes">✓</td><td>仅构建</td>
      <td>基于 <a href="https://riseproject.dev/">RISE Project</a> 的运行器构建。</td>
    </tr>
    <tr>
      <td><code>Illumos</code></td><td><code>x86_64</code></td><td class="support-no">✗</td><td>仅构建</td>
      <td></td>
    </tr>
    <tr>
      <td><em>其他</em></td><td>—</td><td class="support-no">✗</td><td class="support-no">✗</td>
      <td>需从源代码构建。</td>
    </tr>
  </tbody>
</table>
</div>

## 终端

有些键位绑定取决于终端，而非 shell。实现了 [kitty 键盘协议](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) 的终端能够向 Atuin 报告修饰键、功能键（++"Fn1"++–++"Fn24"++）、媒体键以及 super（Command）修饰键。没有实现该协议的终端——包括默认的 macOS 终端——则只能使用基本的组合键。详情请参阅[高级键位绑定](configuration/advanced-key-binding.md)。

## AI 代理

Atuin 会通过 AI 编程代理的 hook 系统记录它们运行的命令。设置方法请参阅 [AI 代理 hooks](guide/agent-hooks.md)。目前支持的代理有 _Claude Code_、_Codex_、_Copilot_、_opencode_ 和 _pi_。

## 获取帮助

如果遇到问题，[`atuin doctor`](reference/doctor.md) 会收集我们所需的相关信息。然后你可以在[论坛](https://forum.atuin.sh)发帖、加入我们的 [Discord](https://discord.gg/Fq8bJSKPHh)，或提交一个 [issue](https://github.com/atuinsh/atuin/issues)。
