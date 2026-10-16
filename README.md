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
kr plan --changed main --explain          # 按工作树相对 main 的改动选测（不运行）
kr run --changed main --also settings-about --explain
kr gate --changed main --explain
kr hash ratings --json                   # 查看构建缓存实际使用的输入和哈希

# 7. 用 kr 自己的调度代码重放记录的运行，比较设定（容量、通道顺序、缓存），带校准误差
kr simulate --sim 2 --sim 3 --order ipad-first --order name

# 8. 用当前的判定规则重判已保存的运行，只列出结论变化；--evidence 也重判旧脚本留下的结果包
kr rejudge
kr rejudge --evidence ~/orca/workspaces/Krill/v0-evidence

# 9. 交付报告初稿：机器判定 / 需人判断手感 / 未验证三段；--base 给出起点分支，
#    改动里的 schema（待部署）、Artboard（手感）、法律文本（owner 复核）自动列入
kr deliver --base main

# 10. 运行遇到 kr 规则覆盖不到的失败：跑完全部场景，不给结论，退出 4，列出判断点；
#     agent 按证据交回决定，kr 按已有结果给出最终结论，不重跑
kr decide <判断点 ID> product --reason "断言的是 App 的数据，不是环境"
kr run --decide <判断点 ID>=flaky --reason "..."   # 也可以在下一条运行命令上交回

# 11. 给调用 kr 的 agent 读的使用说明，与当前版本一致；项目 .kr/skill.md、个人 ~/.config/kr/skill.md 追加在后面
kr skill
kr skill --reference decisions

# 12. 一批的报告：排队和运行时间、返工轮数、偶发排行、agent 调了哪些命令、参数错几次、
#     绕了哪些弯、每类判断的判对率。批次 = 时间范围（默认最近 24 小时）里这个项目的运行；
#     并行的几批用 --worktree / --agent 分开。只给数据，不改任何东西
kr report --since 2026-10-14T10:55:00Z --until 2026-10-14T16:20:00Z
kr report --since 7d --worktree ~/work/krill-133
```

每个命令都支持 `--json`，给 agent 读。

## 接入 agent

`kr setup` 安装很小的 skill 入口和带 `<!-- kr -->` 标记的规则块。
入口引导 agent 先运行 `kr skill`，说明正文跟随当前 kr 版本。

```sh
kr setup --project           # 当前目录：Claude Code 和 Codex
kr setup --project --codex   # 当前目录：只装 Codex
kr setup --claude            # 全局：只装 Claude Code
kr setup                     # 全局：两种 agent
kr setup --project --remove  # 删除当前目录的入口和规则块
kr setup --remove --json     # 全局卸载，列出实际删除的文件
```

默认全局位置：Claude 使用 `~/.claude/skills/kr/SKILL.md` 和
`~/.claude/CLAUDE.md`；Codex 使用 `~/.agents/skills/kr/SKILL.md` 和
`~/.codex/AGENTS.md`（规则目录可用 `CODEX_HOME` 改写）。
`--project` 在当前目录写 `.claude/skills/kr/SKILL.md`、`CLAUDE.md`、
`.agents/skills/kr/SKILL.md`、`AGENTS.md`，不要求已有 `.kr.toml`。
Codex 若已有非空 `AGENTS.override.md`，使用这个生效文件。
两种 agent 一起安装，且 Claude 规则通过 `@` 导入 Codex 规则时，只在 Codex 文件写规则块，
输出说明跳过 Claude 规则的原因。

重复安装不改结果；替换和卸载保持标记外的内容逐字不变。
已有其他内容的同名 skill 或不成对的标记会报错，并说明路径和处理方法。
JSON 的 `written`、`removed` 是实际修改的文件，`skipped` 说明跳过原因。
全局安装会修改个人 agent 规则，应由 owner 确认后操作。
官方路径依据见 [agent 安装调研](docs/research/agent-setup.md) 与 ADR 0018。

`kr run` / `kr gate` 怎么判定：
- 构建一次：输入（git 知道的文件内容、xcodebuild 命令行、Xcode）没变就复用上次的测试产物，不再构建。
- 每个场景先在租到的模拟器上真实枚举测试，0 条即失败；运行后从 xcresult 核对：实际跑的测试必须正好是枚举出的那些，xcodebuild 的退出码和测试结果必须一致。
- 失败的测试单独重跑一次：通过记为偶发（场景仍通过，输出里列出），仍失败记为失败。
- 高负载时，只有逐个重跑前后均测得正常负载、同一用例仍明确失败，才跳过整组重跑；高负载或负载未知时保留整组重跑。事件记录逐个重跑的负载及跳过证据。命令套件首次日志 `<name>.log`、重跑日志 `<name>.again.log` 均保留，事件列出两份路径，人读输出指向首次失败日志（ADR 0003 补充）。
- 仍失败、而且这条测试的这种失败在过去 90 天里没见过时，kr 不替 agent 判断：跑完全部场景后不给结论，退出 4，`--json` 的 `decisions` 列出判断点（证据、选项、默认建议），人读输出单独一段写明下一步命令（ADR 0011、0015）。见过的失败照常记为失败。
- 每个 `--json` 输出都有 `schema_version`（现为 1）和 `decisions`；字段只加不改。
- 模拟器通道按固定顺序排队拿槽位：先 iPad，再其他类型（按名字）。
- 每个场景有硬超时（默认 20 分钟，`.kr.toml` 里 `timeout` 可改），超时终止整个进程组并判失败。
- 结果包在 `build/kr/results/<场景>/`，日志在 `~/.local/state/kr/runs/<运行 ID>/`，事件在 `~/.local/state/kr/events/`；每次 kr 调用（命令、参数、退出码）另记在 `~/.local/state/kr/invocations/`，给 `kr report` 用（ADR 0016）。
- 每个场景的判定依据（枚举、每次运行的测试结果、重跑结果）和结论存为运行目录里的 `<场景>/verdict.json`，几 KB，`kr rejudge` 用它重判。

## 它是什么、不是什么

- **是**：开发流程的执行层。资源排队、环境体检、预检、构建与测试、结果核对、计时记录。
- **不是**：agent 编排工具（派单、沟通由 Orca 或其他工具负责，`kr` 不依赖它们）；也不是构建系统（底层仍是 `xcodebuild`、`simctl`、`swift`）。
- **只支持 Apple 客户端项目**（iOS / iPadOS / tvOS / macOS）。

## 怎么配置

- 机器级：`~/.config/kr/config.toml`（可选。不写时由 `kr doctor` 检测的机器能力推算）。
- 项目级：仓库根目录 `.kr.toml`（Xcode 版本、平台、测试套件、UI 场景）。

## 按改动选测

`--changed REF` 比较 REF 和当前工作树，包括暂存、未暂存、删除及未跟踪文件。
SwiftPM 的 `describe` 和 XcodeGen 的只读 `dump` 提供模块图；场景声明覆盖的模块。
页面关系可以补在项目配置里，例如：

```toml
[modules.TitleDetail]
paths = ["Krill/TitleDetail"]
depends_on = ["Metadata", "Sources"]

[modules.Favorites]
paths = ["Krill/Favorites"]
depends_on = ["TitleDetail"]

[scenarios.favorites]
only = ["KrillUITests/FavoritesUITests"]
modules = ["Favorites"]

[selection]
ignore = ["docs/**", "**/*.md", ".github/**"]
```

改 Metadata 会经 TitleDetail 选中 favorites。必跑场景可以跨出当前 gate；显式目标和
`--also` 只能加跑，不能排除必跑项。gate 的套件保持执行。没有 `--changed` 时保留原选择。
忽略名单完全由项目声明，kr 没有内置名单；`--explain` 列出 `ignored by <pattern>`。

图缺失、路径未映射、场景未声明覆盖时，kr 默认加跑所有候选场景，并输出效率类
`additional_scenarios` 判断点。它不阻止 PASS/FAIL；运行事件记为默认继续、待复核。
证据附过去 90 天本项目的共同失败次数、共同运行分母和偶发场景运行率。空样本比例为 null，
不把没有记录当作没有风险。`kr plan` 只展示，不写事件；SwiftPM 描述缓存放在 kr 状态目录，
不在项目里构建。没有 project.yml 的工程目前保守标为图不完整，可用 `[modules]` 补映射。

`kr hash` 与构建缓存共用计算入口，显示文件内容摘要、缺失文件、完整命令、Xcode 路径和生成命令。
它是现有保守构建哈希的解释，不是场景结果缓存，也不保证每个场景有独立哈希。

## 接入一个项目

1. 安装：`cargo install --git https://github.com/bulai0408/kr-cli kr`（或在本仓库 `cargo install --path crates/kr`）。
2. 在项目仓库根目录写 `.kr.toml`。从 [examples/krill.kr.toml](examples/krill.kr.toml) 改起，最少要有：
   - `[project]`：`name`（模拟器池名 `<name>-Verify-<N>`）、`xcodeproj` 或 `workspace`、`scheme`、`configuration`、要求的 `xcode` 版本；工程靠生成的写 `generate`。
   - `[simulators]`：运行时与设备类型。设备类型名照 `xcrun simctl list devicetypes` 写。
   - `[scenarios.<名字>]`：`only = ["UITests目标/类"]`，iPad 场景加 `device = "iPad"`。
   - `[suites.<名字>]`：`build` / `test` / `command` 三种，对应原来脚本里的每一步。
   - `[gates.default]`、`[gates.smoke]`：门禁由哪些套件和场景组成。
   - `[isolation]`：测试用 App 的合成 bundle id 和允许的权限环境。
   - 可选：`[project]` 里写 `compilation_cache = true`，同一项目的各个 worktree 共用 Xcode 编译缓存（默认关）。在 Krill 上，新 worktree 的构建时间少约三分之一，第一次构建多约四分之一（ADR 0013）。worktree 不要放在 `/tmp`、`/var` 下，那里是符号链接，缓存不会命中。
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
# kr = "kr"               # 远程主机上的 kr，按登录 shell 的 PATH 找；找不到时写全路径
```

```sh
git push -u origin <分支>
kr run --remote mini ratings
kr gate --remote mini
```

- kr 只连接这里声明的主机，用系统的 `ssh`（加 `BatchMode=yes`，不弹密码提示）。地址和连接安全由主机和 owner 负责：kr 不检查地址，不保存也不读取凭证。
- 开始前检查，有问题时一次列出全部原因和下一步，以退出码 1 结束，不连远程跑：分支没推送或推送的不是 HEAD、有未提交的改动、没有 `origin`、主机连不上、主机不是 macOS、没装 Xcode、没有 kr 或 kr 版本不一致、选中的项需要开发者证书签名而主机签不了。
- 远程主机自己用 git 拉取已推送的分支，需要能不弹提示地访问 `origin`（例如钥匙串在 SSH 会话里可用，或给 git 服务器配 SSH 密钥）（每个项目一个克隆，每个分支一个 worktree：`<dir>/<项目>@<分支>`），在用户的登录 shell 里（`$SHELL -l`，和在那台 Mac 上开终端时的 PATH 一样，例如能找到 Homebrew 的 xcodegen）运行它自己的 `kr --json`；资源由远程 kr 自己排队。结果回传本机，输出和本机运行一样，`--json` 多了 `host` 和 `remote`。xcresult 和日志路径在远程主机上。
- 事件两边都写：远程 kr 写它自己的，本机在自己的运行 ID 下写每项的开始和结束（带 `host`）以及 `run_end`。
- 运行途中失联（ssh 以 255 结束、没拿到结果）：每项记「未验证」（`tests.unverified`，原因在 `unverified_reason`），不算通过，退出码 1。本机收到 SIGINT / SIGTERM 时 ssh 断开，远程 kr 收到 SIGHUP 后自己停下、归还资源。
- 签名（ADR 0014）：在仓库根目录为真机或 macOS 构建 App 的项（例如 `destination = "macos"` 的测试套件、根目录的 `build` 套件、`--device`）需要开发者证书。这时 kr 先在要签名的机器上试签一次：远程运行在主机上试签，在 SSH 会话里直接运行 `kr run` / `kr gate` / `kr check` 在本机试签。试签给一个临时文件签名，签完删除；不改钥匙串，不读不存凭证，输出里只有身份序号、指纹前 8 位和团队 ID，没有证书名。签得了（且团队等于 `[signing] team`，没写时任意团队）就照常运行；签不了就提前拒绝，写明原因和要在那台机器上做什么，例如 SSH 会话里钥匙串锁着（`errSecInternalComponent`）时，让那台 Mac 在 SSH 登录时解锁钥匙串，或在那台 Mac 的登录会话里运行。kr 不替用户解锁钥匙串。模拟器构建和单独构建的包（`dir`）不需要试签；`[project].build_settings` 里写了 `CODE_SIGNING_ALLOWED = "NO"` 的项目也不需要。

## 在真机上运行

模拟器测不准的（硬件、系统行为）放到一台已配对的 iPhone 或 iPad 上跑：

```toml
# .kr.toml
[signing]
team = "K4AXH95DCF"       # 真机构建应签的团队；kr 只回读核对，不设定
# register_devices = true # 允许 xcodebuild 把还没登记的设备登记到团队（默认不开）
```

```sh
kr doctor --device "蔡煜恒的 iPad Mini"   # 只检查：配对、开发者模式、能否连上、是否锁屏、Xcode 是否认识它的系统
kr check --device "蔡煜恒的 iPad Mini"    # 构建真机测试产物并回读签名
kr run --device "蔡煜恒的 iPad Mini" launch
```

- 设备按名字、UDID 或 CoreDevice ID 找（`xcrun devicectl list devices` 里的）。kr 只用这一台，不抹掉、不重启、不管理设备。
- 开始前检查，缺什么就说在设备上怎么做，以退出码 1 结束：没配对（连线、解锁、点「信任」）、开发者模式没开（设置 > 隐私与安全性 > 开发者模式，重启后点「启用」）、连不上、锁屏、设备系统的主版本比 Xcode 的 SDK 新。只新一个小版本时给警告，照常运行。
- 「设置 > 开发者 > 启用 UI 自动化」devicectl 读不到，kr 不能事先检查；UI 测试失败且出现 `kAXError` 时，失败项下会提示去打开它。
- 构建一次（`-destination 'platform=iOS,id=<UDID>' -allowProvisioningUpdates`，自动签名为这台设备准备描述文件），再依次在设备上跑各项。设备还没登记到团队时，设备会拒绝安装测试 runner（`0xe8008012`）：在 Xcode 里登记，或在 `[signing]` 写 `register_devices = true` 让 xcodebuild 登记（加 `-allowProvisioningDeviceRegistration`，会改动团队的开发者账号，所以默认不开）。每台设备是容量为 1 的资源，同一台设备上同时只跑一个运行，后来的排队等待（`kr queue` 还不列出设备）。
- 安装前回读签名（`codesign`）：签名团队必须等于 `[signing] team`（没写时提示补上）；`[isolation]` 里配置的 entitlement 必须一致；App 带 iCloud entitlement 时，`com.apple.developer.icloud-container-environment` 必须写明，且在允许的值里（`[isolation.entitlements]` 没写时只允许 `Development`）。不对就停下，说明是哪一项、应该是什么。
- 运行途中设备掉线或锁屏（每项开始前和失败后用 `devicectl device info lockState` 确认能否连上、是否锁屏，并比较连接方式：拔线后 devicectl 常常还能经 Wi-Fi 连上设备，但测试的连接已经断了，所以连接方式从开始时的 `wired` 变成别的也算掉线）：这一项和后面没跑的项记「未验证」（`tests.unverified`，原因在 `unverified_reason`），不算通过，退出码 1。事件和模拟器运行一样写 #29 的字段，身份里是设备的型号和系统。
- 设备插在远程主机上时，`kr run --device <名字> --remote <主机> <场景>`：主机上的 kr 做设备检查、签名回读、掉线记「未验证」，结果按远程运行的方式传回，本机的 `item_end` 带 `device`、`tests.unverified` 和 `unverified_reason`。主机上设备的开始前问题（例如锁屏）逐条传回，写明在设备上怎么做。
- 不支持：真机截图、性能分析、多台设备并行（本机或远程）、CloudKit Production、注入真实服务凭证。

## 状态

第一阶段开发中。路线见 [docs/spec.md](docs/spec.md)，每个设计决定的理由见 [docs/adr/](docs/adr/)，参考过的开源项目与调研见 [docs/research/](docs/research/)。术语见 [CONTEXT.md](CONTEXT.md)。

## 安装

```sh
cargo install --git https://github.com/bulai0408/kr-cli kr
```

### 场景依赖的准备与清理（ADR 0020）

`kr run` 和 `kr gate` 在场景测试前按顺序执行 `prepare`。准备失败、就绪超时或清理失败记为「未验证」，退出 1；不记测试失败，不触发失败分类判断点。先检查准备日志和服务配置，再重试。

```toml
[[scenarios.shelf.prepare]]
shared = "local-backend" # 可选；同一次 run/gate 跨设备通道只启动一次
run = "python3 service.py start"
dir = "tools"          # 可选，相对项目根目录；默认根目录
background = true       # 默认 false：一次性准备命令，等待退出 0
ready = "python3 service.py ready" # 后台步骤必填，退出 0 表示就绪
timeout = "30s"        # 可选，准备/就绪截止时间；默认 30s
cleanup = "python3 service.py cleanup" # 可选，进程终止后的状态清理，硬超时 5s
```

同名 `shared` 的 run/dir/ready/cleanup/background/timeout 必须相同；不同时配置错误指出两个场景和键路径。共享实例在第一个依赖场景前启动，在最后一个结束后清理；失败只尝试一次，所有依赖场景记相同的环境原因。不带 `shared` 的步骤只属于当前场景。套件暂不支持准备步骤。

后台服务必须保持前台，不要自行 daemonize。kr 在正常结束、准备失败、就绪超时、Ctrl-C 和 SIGTERM 时，逆序终止它启动的进程组，5 秒后升级 SIGKILL，并回收子进程。清理命令在失败和中断时也运行。kr 自身被 SIGKILL 时无法保证清理。

每个准备、就绪和清理命令都收到以下环境变量：

| 变量 | 值 |
|---|---|
| `KR_RUN_ID` | 本次运行 ID |
| `KR_PROJECT` | 项目名 |
| `KR_SCENARIO` | 场景名；共享步骤为空 |
| `KR_DEVICE_UDID` | 设备 UDID；共享步骤为空 |
| `KR_RUN_DIR` | 本次运行的产物目录 |

准备步骤在原有 sim/device 槽位内执行，不增加槽位。独立实例用临时端口或独立端口文件；共享实例可用固定端口。就绪检查应核对本实例身份，kr 不杀占用端口的其他进程。带准备步骤的场景等同 `external`，不得复用测试结论；构建产物仍可缓存（当前 kr 本就每次运行测试）。JSON 的 `preparation` 和 `preparation_start` / `preparation_end` 事件带共享名、使用场景、日志、就绪探测次数和清理结果。`item_end` 的 `tests.unverified` 与 `unverified_reason` 是结论契约。

真实小服务示例见 `tests/sample/service.py`，以端口 0 绑定回环地址，再通过运行目录传递端口；清理后断言 PID 消失、端口关闭。该样例只用于显式真实验收，不加入默认回放场景。

## 合并辅助

`kr merge` 先算事实，agent 判断逻辑冲突。不会自动执行 `git merge`。

```sh
kr merge predict --base main feature-a feature-b feature-c --json
kr decide <decision-id> combine --reason "preserve both requirements" --json
kr merge install --json
kr merge check --base <before-merge-commit> --json
kr merge check --base <before-merge-commit> --decision <decision-id> --verification-run <run-id> --json
```

- `predict` 用 `git merge-tree` 两两预测，建议先祖先、后少冲突分支。同文件的两侧改动即使文本能干净合并，也产生逻辑冲突判断点（退出 4），附两边提交说明和其中的票号；缺票号时明确标记。祖先已包含于后继时不重复提问。它不保证累计合并或跨文件逻辑兼容。
- `kr decide` 接受 `combine`、`ours`、`theirs`、`manual`、`reject`。这里 ours/theirs 指判断点的 left/right。选项只记录解法，不替你改代码；全部交回后仍退出 1 并显示 `verification: pending`，直到用合并后的编译和测试证据对账。
- `install` 显式给当前仓库的全部已有 worktree 开启 rerere 并注册驱动，不修改 global 配置。**它会修改当前 worktree 的已跟踪文件 `.gitattributes`，请审阅并有意提交这个改动**；其他 worktree 也需包含此 attributes 提交。新 worktree 存在独立覆盖时重跑安装。移动 kr 可执行文件后也需重跑安装，以更新驱动路径。
- `kr merge xcstrings <base> <ours> <theirs>` 是驱动入口：完整 key 条目三方合并，同 key 分歧、删除/修改、元数据分歧、重复 JSON key 都拒绝；校验成功才替换 ours。需要当前 Xcode 的 `xcrun xcstringstool print`。GitHub 服务端不运行本地驱动。
- `check` 扫描相对 base 的已提交及工作区改动（含未忽略的未跟踪文件）：JSON 和目录、场景清单、迁移版本和重复 DROP、有限凭证模式。错误只报告文件、行号和下一步，凭证值不输出。源代码改动按已配置 build suite 目录增量编译，未知目录回退 App；没有配置就未验证。编译走资源队列，保留现有 DerivedData，不 clean；编译预算 120 秒，静态失败时不编译。秒级只指静态检查，不保证排队和编译耗时。
- 对账还要求：干净且包含两分支提交的合并树、决定后同一 tree 的非零 `kr run/gate` 结果。缺证据不写成功，编译或测试失败写不支持；同一判断点只对账一次。跨文件逻辑冲突、全部间接依赖与真实服务仍需其他验证。

设计见 [ADR 0017](docs/adr/0017-merge-assistance.md)，真实 Krill 样本、离线前后对比与验证边界见 [调研和证据](docs/research/merge-assistance.md)。
## 旧脚本接入资源队列

没有 `.kr.toml` 的脚本也能使用资源队列。将原来的 `/tmp/native-build.lock`
排队、`mkdir`、写 owner 和 trap 删除，整体替换为：

```sh
kr lock build --task conflux-build -- bash scripts/build.sh
kr lock sim --task legacy-test -- bash scripts/test-one-simulator.sh
kr lock light --task module-test -- swift test --filter MyModuleTests
```

一条 `kr lock` 占一个对应槽位。模拟器脚本优先用
`kr pool lease -- <command>`：它还会租用项目池设备，传入 `KR_SIM_UDID` 和
`KR_SIM_NAME`，结束后关机归还。需要项目 `.kr.toml`。不要在一个 sim 槽位内启动
多个模拟器；不要再在子脚本中拿旧全机锁。普通 `kr lock sim` 只限制数量，不分配设备。
构建和测试分开占各自槽位；容量和来源用 `kr doctor --json` 查看，配置覆盖优先。

指定池内设备时用 `kr pool lease --name Krill-Verify-1 -- <command>` 或
`kr pool lease --udid <UDID> -- <command>`。`--name`、`--udid`、显式 `--kind`
三者互斥；都不传时按 `default` 类型选空闲设备。指定设备忙时只等这一台，
不会换其他空闲设备；等待提示显示名称、UDID 和持有者的 task，`kr queue` 的
task 也包含目标。池外名称或 UDID 退出 2，用 `kr pool status` 查看当前池。
等待设备锁时仍占用 sim 槽位，保持负载 → 旧锁 → sim 槽 → 设备锁的顺序（ADR 0023）。
`--json` 在 stderr 输出租约通知，`selection` 为 `{"by":"name|udid|kind","value":"..."}`；
`sim_leased`、`sim_released` 事件的 `fields.selection` 记录同一选择。子命令 stdout 和退出码透传。

`kr lock` 原样传回子命令退出码（包括 4，这里不表示判断点）；子进程被信号 n
终止时返回 128+n。取消排队或 kr 自己出错返回 1，参数错误返回 2。stdout/stderr
透传给子命令；排队提示写 stderr，`--json` 不将子命令输出包装成 JSON。
SIGINT/SIGTERM/SIGHUP 会转发到子进程组，退出后归还槽位。嵌套同资源
调用只在槽位的持有者确为祖先进程时重入，不能靠伪造 `KR_HELD` 跳过排队。

桥接期间继续保留机器 `[legacy]`；新版本先取旧锁再取槽位，允许旧脚本持锁调用
kr。`kr doctor` 会列出七天内观察到的非 kr 使用者及下一步。观察不到不代表未接入
的脚本不存在，删除前须核对 [迁移步骤](docs/research/machine-concurrency.md)。

## 发版

源码仓库保持私有。公开的 `bulai0408/homebrew-tap` 只接收安装说明、
`Formula/kr.rb` 和 Release 二进制附件；本仓库的源码、测试素材、docs、Git 历史、
构建日志和私有词表都不公开。公开仓库的创建和第一次发布由协调者在 owner 确认后执行。

发版脚本需要 macOS Apple Silicon、Python 3.12+、Rust stable、`aarch64-apple-darwin`
target、已安装的 kr 和（仅发布时）gh。只支持稳定版 `MAJOR.MINOR.PATCH`。
不注入假版本：先把 workspace 的 Cargo 版本改成待发布版本并提交，协调者再创建
对应的 `v<version>` tag。脚本从该 tag 的已跟踪源码快照构建，要求 Cargo 版本和
`kr --version` 都等于版本参数。不会把未提交改动放进二进制。

1. 在仓库之外建立权限为 `600` 的 UTF-8 私有词表，每行一个要拦截的姓名、
   拼音变体、账户名或已知团队 ID。设置 `KR46_DENY_FILE` 指向它；不要提交词表，
   不要把词放到命令参数或日志里。脚本要求词表存在且非空，但不能自动证明名单完整。
2. 本地准备：`scripts/release.sh 0.1.0`。它要求本地 `v0.1.0` tag 已存在，
   经 `kr lock build --task kr46-release -- cargo build ...` 排队。输出目录默认为
   `target/release-dist/0.1.0`，必须不存在；失败产物保留供检查，再次执行使用新的
   `--output` 目录。
3. 本票的无 tag 演练：`scripts/release.sh 0.1.0 --ref HEAD --dry-run`。
   它仍构建和检查本地产物，只打印公开操作，不创建仓库、推送或上传。
   非 tag ref 只允许在 `--dry-run` 下使用；`--publish` 与 `--dry-run` 不能并用。
4. 运行 `python3 -m unittest discover -s scripts -p test_release.py -v`。
   再运行 `python3 scripts/rehearse-release.py target/release-dist/0.1.0`：
   临时 HOME、无 Rust 的 PATH、临时本地 tap、`file://` 二进制安装、
   `kr --version`、`kr skill`、`brew test`、临时全局 `kr setup`、
   `kr doctor --json`、移除入口、卸载和删除 tap。已有 brew 版 kr 时停止，
   不替换用户的安装。演练会临时写 Homebrew 的 tap 和 Cellar，不更改机器配置；
   不使用真实的 agent 配置目录。它不能证明无仓库访问权限的新用户或新 agent 已验收。
5. 已有二进制可独立检查：
   `scripts/release.sh 0.1.0 --verify-only target/release-dist/0.1.0/kr`。
   README 里的每个 `kr <子命令>` 必须存在于该二进制 `--help`。
   缺 `kr init` 或 `kr skill --reference config` 明确打印 `pending #47`；
   本地准备和 dry-run 允许 pending，`--publish` 拒绝。其他缺失命令一律失败。

隐私检查先做 remap，再检查真实产物。编译时清除继承的 Rust flags，用
`--remap-path-prefix` 映射源码、HOME、Cargo home 和 Rustup home，并去掉符号。
`strings -a` 检查 `/Users/`、`/home/`、Cargo/Rustup 本机路径、邮箱和带上下文的
团队 ID；运行时读取构建机器的 HOME 和 `id -un` 用户名，明确拦截这些值；
私有词表再做大小写不敏感的 strings 检查与原始 UTF-8 字节检查。
团队 ID 不能直接用裸 `[A-Z0-9]{10}` 拦截：随机字节和符号常命中，误报太多；
已知 ID 放入词表，通用规则只拦 `TeamIdentifier=` / `DEVELOPMENT_TEAM=` 上下文。
命中时只输出规则名，不输出私有值。tar 只含 `kr`，清除 uid/gid 和用户/组名。
现有日志脱敏器通过运行时组合路径生成替换前缀，保持原有脱敏行为，避免把
用于替换的 `/Users/` 字面量编进二进制；这不是隐私检查的豁免。
这些检查不能代替人工审查；未知姓名拼写、拆开的字节或其他私密格式可能漏检。

默认只构建 Apple Silicon。不签 Developer ID，也不公证。Homebrew formula 安装通常
不带浏览器下载的隔离属性；浏览器下载可能被 Gatekeeper 阻止，公开 README 提示先核对
校验和，再由用户决定是否在系统设置中允许打开，不能自动关闭 Gatekeeper。
Intel 需额外安装 target、完整编译一套依赖并在 Intel Mac 上验收，构建时间和产物约为
再加一份，当前未实测。GitHub Actions 还需私有构建环境与跨仓库最小权限上传凭证，
会增加托管分钟、凭证配置和供应链维护成本；本票不新增 workflow，由 owner 决定。

协调者首次发布清单（本票不执行）：

- 等 #47 合入并重新构建、检查，完成真实新用户/agent 的 setup、init decisions、
  doctor、check 验收；按仓库规定完成 main/release 的全量门禁和审查。
- owner 确认后执行 `gh repo create bulai0408/homebrew-tap --public`。
  在私有 kr-cli 仓库提交正确版本后执行
  `git tag v0.1.0 <已验收的提交>`；如版本不是 0.1.0，所有命令使用同一版本。
- 先执行 `scripts/release.sh 0.1.0 --dry-run --output target/release-preview/0.1.0`，
  检查 `tap/README.md`、`tap/Formula/kr.rb`、tar 内容与校验和，再由协调者显式执行
  `scripts/release.sh 0.1.0 --publish --output target/release-publish/0.1.0`。
  gh 使用既有认证，脚本不读或打印令牌。这个分支会 clone **已有**公开 tap，
  只 add 两个公开文件、commit、push `HEAD:main`，再执行
  `gh release create v0.1.0 <tar.gz> <SHA256SUMS> --repo bulai0408/homebrew-tap ...`。
  首次仓库可以为空；该顺序先建立 main，Release 创建失败时 Formula 会暂时指向缺失附件，
  必须保留失败日志，由协调者检查远端状态后补全，不可盲目重复发布。
- 公开文件白名单：Git 的 `README.md`、`Formula/kr.rb`；Release 的
  `kr-0.1.0-aarch64-apple-darwin.tar.gz`、`SHA256SUMS`。`source-commit.txt`、
  裸 `kr`、本地日志和其他文件全部留在私有工作区。脚本不会创建公开仓库或推送私有仓库。
