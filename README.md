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

## 常用命令

```sh
# 0. 体检：本机的 Xcode、模拟器运行时、模拟器池、磁盘、负载，以及推算出的资源容量
kr doctor

# 1. 进队列前，先确认代码能编过（不占模拟器）
kr check

# 2. 构建一次，在两台模拟器上跑指定场景；排队、租模拟器、核对测试数都自动完成
kr run ratings shelf-share

# 3. 跑门禁：.kr.toml 里 [gates.default] 的套件和场景，每项一行 PASS / FAIL
kr gate
kr gate smoke

# 4. 看现在谁在用机器、谁在排队、还要等多久
kr queue

# 5. 拿到一个 build 槽位后执行任意命令，结束立即归还，退出码原样返回
kr lock build -- xcodebuild build-for-testing …

# 6. 只算不跑：哪些构建命中缓存、每项为什么要跑、通道怎么分、按历史大约多久
kr plan
kr plan ratings shelf-share --exit-code   # 有事要做退出 3，无事可做退出 0

# 7. 用 kr 自己的调度代码重放记录的运行，比较设定（容量、通道顺序、缓存），带校准误差
kr simulate --sim 2 --sim 3 --order ipad-first --order name

# 8. 用当前的判定规则重判已保存的运行，只列出结论变化；--evidence 也重判旧脚本留下的结果包
kr rejudge
kr rejudge --evidence ~/orca/workspaces/Krill/v0-evidence

# 9. 交付报告初稿：机器判定 / 需人判断手感 / 未验证三段；--base 给出起点分支，
#    改动里的 schema（待部署）、Artboard（手感）、法律文本（owner 复核）自动列入
kr deliver --base main
```

每个命令都支持 `--json`，给 agent 读。

`kr run` / `kr gate` 怎么判定：
- 构建一次：输入（git 知道的文件内容、xcodebuild 命令行、Xcode）没变就复用上次的测试产物，不再构建。
- 每个场景先在租到的模拟器上真实枚举测试，0 条即失败；运行后从 xcresult 核对：实际跑的测试必须正好是枚举出的那些，xcodebuild 的退出码和测试结果必须一致。
- 失败的测试单独重跑一次：通过记为偶发（场景仍通过，输出里列出），仍失败记为失败。
- 模拟器通道按固定顺序排队拿槽位：先 iPad，再其他类型（按名字）。
- 每个场景有硬超时（默认 20 分钟，`.kr.toml` 里 `timeout` 可改），超时终止整个进程组并判失败。
- 结果包在 `build/kr/results/<场景>/`，日志在 `~/.local/state/kr/runs/<运行 ID>/`，事件在 `~/.local/state/kr/events/`。
- 每个场景的判定依据（枚举、每次运行的测试结果、重跑结果）和结论存为运行目录里的 `<场景>/verdict.json`，几 KB，`kr rejudge` 用它重判。

## 它是什么、不是什么

- **是**：开发流程的执行层。资源排队、环境体检、预检、构建与测试、结果核对、计时记录。
- **不是**：agent 编排工具（派单、沟通由 Orca 或其他工具负责，`kr` 不依赖它们）；也不是构建系统（底层仍是 `xcodebuild`、`simctl`、`swift`）。
- **只支持 Apple 客户端项目**（iOS / iPadOS / tvOS / macOS）。

## 怎么配置

- 机器级：`~/.config/kr/config.toml`（可选。不写时由 `kr doctor` 检测的机器能力推算）。
- 项目级：仓库根目录 `.kr.toml`（Xcode 版本、平台、测试套件、UI 场景）。

## 接入一个项目

1. 安装：`cargo install --git https://github.com/bulai0408/kr-cli kr`（或在本仓库 `cargo install --path crates/kr`）。
2. 在项目仓库根目录写 `.kr.toml`。从 [examples/krill.kr.toml](examples/krill.kr.toml) 改起，最少要有：
   - `[project]`：`name`（模拟器池名 `<name>-Verify-<N>`）、`xcodeproj` 或 `workspace`、`scheme`、`configuration`、要求的 `xcode` 版本；工程靠生成的写 `generate`。
   - `[simulators]`：运行时与设备类型。设备类型名照 `xcrun simctl list devicetypes` 写。
   - `[scenarios.<名字>]`：`only = ["UITests目标/类"]`，iPad 场景加 `device = "iPad"`。
   - `[suites.<名字>]`：`build` / `test` / `command` 三种，对应原来脚本里的每一步。
   - `[gates.default]`、`[gates.smoke]`：门禁由哪些套件和场景组成。
   - `[isolation]`：测试用 App 的合成 bundle id 和允许的权限环境。
   写错时 `kr config` 会指到具体行列并说明怎么改。
3. `kr doctor`：确认 Xcode、运行时、池设备都齐；缺池设备就 `kr pool sync`（只新建，不删不改已有设备）。
4. `kr check`：不占模拟器，确认能编过、过滤都能匹配到测试、隔离设置正确。
5. `kr gate smoke`，再 `kr gate`。逐项和原来的脚本对照一次（测试数、结果）。
6. 在项目 AGENTS.md 里把「怎么跑测试、怎么排队」改成「用 kr」，旧脚本保留到所有人都改用 kr。

机器级设置（可选）在 `~/.config/kr/config.toml`：资源容量覆盖 `[resources]`、按机器覆盖 `[machines.<LocalHostName>.resources]`、负载策略 `[load]`、旧全机锁桥接 `[legacy]`（ADR 0009）。

## 状态

第一阶段开发中。路线见 [docs/spec.md](docs/spec.md)，每个设计决定的理由见 [docs/adr/](docs/adr/)，参考过的开源项目与调研见 [docs/research/](docs/research/)。术语见 [CONTEXT.md](CONTEXT.md)。

## 安装

```sh
cargo install --git https://github.com/bulai0408/kr-cli kr
```
