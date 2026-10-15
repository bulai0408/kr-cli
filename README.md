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

`kr run` / `kr gate` 怎么判定：
- 构建一次：输入（git 知道的文件内容、xcodebuild 命令行、Xcode）没变就复用上次的测试产物，不再构建。
- 每个场景先在租到的模拟器上真实枚举测试，0 条即失败；运行后从 xcresult 核对：实际跑的测试必须正好是枚举出的那些，xcodebuild 的退出码和测试结果必须一致。
- 失败的测试单独重跑一次：通过记为偶发（场景仍通过，输出里列出），仍失败记为失败。
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
