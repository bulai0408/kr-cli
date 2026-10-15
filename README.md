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

# 8. 交付报告初稿：机器判定 / 需人判断手感 / 未验证三段；--base 给出起点分支，
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

机器级设置（可选）在 `~/.config/kr/config.toml`：资源容量覆盖 `[resources]`、按机器覆盖 `[machines.<LocalHostName>.resources]`、负载策略 `[load]`、旧全机锁桥接 `[legacy]`（ADR 0009）、远程机器 `[remotes.<名字>]`。

## 在另一台 Mac 上运行

把构建和测试放到局域网里的另一台 Mac 上，本机空出来做别的事：

```toml
# ~/.config/kr/config.toml
[remotes.mini]
ssh = "mini.local"        # 原样交给 ssh：主机名或 ~/.ssh/config 里的 Host 别名
# dir = "~/kr-remote"     # 远程主机上放克隆的目录（默认）
# kr = "kr"               # 远程主机上的 kr；ssh 命令的 PATH 很短，找不到时写全路径
```

```sh
git push -u origin <分支>
kr run --remote mini ratings
kr gate --remote mini
```

- kr 只连接这里声明的主机，用系统的 `ssh`（加 `BatchMode=yes`，不弹密码提示）。地址和连接安全由主机和 owner 负责：kr 不检查地址，不保存也不读取凭证。
- 开始前检查，有问题时一次列出全部原因和下一步，以退出码 1 结束，不连远程跑：分支没推送或推送的不是 HEAD、有未提交的改动、没有 `origin`、主机连不上、主机不是 macOS、没装 Xcode、没有 kr 或 kr 版本不一致、选中的项需要开发者证书签名。
- 远程主机用 git 拉取已推送的分支（每个项目一个克隆，每个分支一个 worktree：`<dir>/<项目>@<分支>`），运行它自己的 `kr --json`；资源由远程 kr 自己排队。结果回传本机，输出和本机运行一样，`--json` 多了 `host` 和 `remote`。xcresult 和日志路径在远程主机上。
- 事件两边都写：远程 kr 写它自己的，本机在自己的运行 ID 下写每项的开始和结束（带 `host`）以及 `run_end`。
- 运行途中失联（ssh 以 255 结束、没拿到结果）：每项记「未验证」（`tests.unverified`，原因在 `unverified_reason`），不算通过，退出码 1。本机收到 SIGINT / SIGTERM 时 ssh 断开，远程 kr 收到 SIGHUP 后自己停下、归还资源。
- 签名：SSH 会话拿不到登录钥匙串里的签名身份。在仓库根目录为真机或 macOS 构建 App 的项（例如 `destination = "macos"` 的测试套件、根目录的 `build` 套件）需要开发者证书，远程运行时提前拒绝；在 SSH 会话里直接运行 `kr run` / `kr gate` / `kr check` 也一样。模拟器构建和单独构建的包（`dir`）不受影响；`[project].build_settings` 里写了 `CODE_SIGNING_ALLOWED = "NO"` 的项目也不受影响。

## 状态

第一阶段开发中。路线见 [docs/spec.md](docs/spec.md)，每个设计决定的理由见 [docs/adr/](docs/adr/)，参考过的开源项目与调研见 [docs/research/](docs/research/)。术语见 [CONTEXT.md](CONTEXT.md)。

## 安装

```sh
cargo install --git https://github.com/bulai0408/kr-cli kr
```
