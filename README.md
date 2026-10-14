# kr-cli

**卡米的 Apple 开发流程工具。** 让多个 AI agent 在一台（或几台）Mac 上并行开发时，排队有序、验证可信、数据可比。

命令名是 `kr`。日常由 agent 调用，人偶尔用它看状态。

## 它解决什么

几个 agent 各在自己的 worktree 里写代码，但构建和模拟器测试只能排队用同一台机器。没有统一的流程时会出现：

- 排了半小时队才发现代码编不过；
- 同一组 UI 测试被反复从头构建、反复整组跑；
- 测试一条都没跑，却报了「通过」；
- 机器负载太高，测试大面积假失败；
- 每批花了多少时间，只能事后从日志里拼。

`kr` 把这些做成固定、可检查的步骤。

## 三个例子

```sh
# 1. 进队列前，先确认代码能编过（不占模拟器）
kr check

# 2. 构建一次，在两台模拟器上跑指定场景；排队、租模拟器、核对测试数都自动完成
kr run ratings shelf-share

# 3. 看现在谁在用机器、谁在排队、还要等多久
kr queue
```

每个命令都支持 `--json`，给 agent 读。

## 它是什么、不是什么

- **是**：开发流程的执行层。资源排队、环境体检、预检、构建与测试、结果核对、计时记录。
- **不是**：agent 编排工具（派单、沟通由 Orca 或其他工具负责，`kr` 不依赖它们）；也不是构建系统（底层仍是 `xcodebuild`、`simctl`、`swift`）。
- **只支持 Apple 客户端项目**（iOS / iPadOS / tvOS / macOS）。

## 怎么配置

- 机器级：`~/.config/kr/config.toml`（可选。不写时由 `kr doctor` 检测的机器能力推算）。
- 项目级：仓库根目录 `.kr.toml`（Xcode 版本、平台、测试套件、UI 场景）。

## 状态

第一阶段开发中。路线见 [docs/spec.md](docs/spec.md)，每个设计决定的理由见 [docs/adr/](docs/adr/)，参考过的开源项目与调研见 [docs/research/](docs/research/)。术语见 [CONTEXT.md](CONTEXT.md)。

## 安装

```sh
cargo install --git https://github.com/bulai0408/kr-cli kr
```
