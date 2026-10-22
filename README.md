# kr-cli

**让 AI agent 有序地构建和测试 Apple 项目。**

`kr` 是用 Rust 编写的命令行工具。适合多个 agent 在同一台或多台 Mac 上并行开发
iOS、iPadOS、macOS 和 tvOS 项目，也适合个人开发者统一构建与测试流程。

## 为什么用 kr

多个 agent 各自写代码，却共用机器上的 Xcode 和模拟器。kr 帮它们：

- **有序排队**：共享构建资源和模拟器池，避免争抢。
- **减少重复**：复用相同输入的测试产物，构建一次，多次运行。
- **核对结果**：检查实际执行的测试，零测试不算通过，未验证不算通过。

kr 不负责给 agent 派单，也不替代 Xcode 构建系统。

## 安装

需要 **Apple Silicon Mac、Xcode 和 Homebrew**。当前 Homebrew 包不支持 Intel Mac。

```sh
brew install bulai0408/tap/kr
```

无需安装 Rust。

## 接入 agent

在你的 Apple 项目根目录运行：

```sh
kr setup --project
```

这会为当前项目的 **Claude Code 和 Codex** 安装 skill 入口与规则，不修改全局配置。
然后把任务交给 agent：

```text
使用 kr 接入这个项目：先读 kr skill，保留已有配置；没有配置时初始化，
处理需要确认的选项，再检查环境和项目。报告通过、失败和未验证项。
```

使用说明随 kr 一起发布，agent 按当前版本读取，不必从 README 复制操作规则。

## 进一步了解

- **日常操作**：`kr skill`
- **项目配置**：`kr skill --reference config`
- **命令帮助**：`kr --help`
- **安装与卸载说明**：[Homebrew tap](https://github.com/bulai0408/homebrew-tap)
- **参与开发**：[开发约定](AGENTS.md) · [设计决定](docs/adr/) · [发版](docs/release.md)
