# 开发流程坑与规则清单（供 kr-cli 承接评估）

> 2026-10-14 整理。只读调研，没有改任何仓库。不含凭证值。
> 每条格式：**现象** / 根因 / 现在靠什么规避 / CLI 能否承接（是 / 部分 / 否）+ 能力 / 来源。

## 来源缩写

| 缩写 | 路径或 URI |
|---|---|
| G | `/Users/cyh/.codex/AGENTS.md`（Batched Integration、Gate Failure Handling、Apple Platform Development） |
| KA | Krill `origin/v0/integration:AGENTS.md`（`## Verification`） |
| KE | Krill `origin/v0/integration:scripts/e2e.sh`、`scripts/e2e-modes.sh`、`scripts/e2e-impact.json` |
| KT | Krill `origin/v0/integration:scripts/test.sh` |
| KD/<n> | Krill `origin/v0/integration:docs/agents/<n>-*.md`（交付报告） |
| KH | Krill `origin/v0/integration:docs/agents/coordinator-handover.md` |
| KI | Krill `origin/v0/integration:docs/acceptance/icloud-sync.md`、`scripts/verify-icloud-development.py` |
| PR | `/Users/cyh/orca/workspaces/Krill/v0-evidence/process-review.md` |
| CD | `/Users/cyh/orca/workspaces/Krill/v0-evidence/batch5/coordinator-decisions.md` |
| MK/<f> | `/Users/cyh/.claude/projects/-Users-cyh-Workspace-Krill/memory/<f>.md` |
| MC/<f> | `/Users/cyh/.claude/projects/-Users-cyh-Workspace-Conflux/memory/<f>.md` |
| ML/<f> | `/Users/cyh/.claude/projects/-Users-cyh--lody-chats-2ec49ed6-ef9d-480b-81a5-8fe40d621faf/memory/<f>.md` |
| CA | `/Users/cyh/Workspace/Conflux/AGENTS.md` |
| CT | `/Users/cyh/Workspace/Conflux/Scripts/test.sh` |
| OV:… | OpenViking 记忆，`viking://user/default/…` 下的路径 |

## 能力词表（下文「能力」一栏用这些词）

资源锁 · 模拟器池 · 预检 · 构建一次多次运行 · 选测 · 结果校验 · 合并辅助 · 凭证扫描 · 环境体检 doctor · 证据收集/交付报告 · 计时 · 偶发隔离 · 真机流程 · 签名检查 · 系统负载检查 · 挂起看门狗 · 凭证注入 · 数据隔离检查

---

## 1. 环境与机器

**E1 高负载下出现与代码无关的红和卡死**
- 现象：load 30–640 时，xctest 进程被 SIGTRAP/SIGSEGV 崩掉，计时类单测假红，模拟器辅助功能快照卡死，录屏 20 秒无新帧。
- 根因：多会话并行构建、多台模拟器、后台进程同时抢 8 核 M3；Apple 框架在高负载下内部崩溃（如 `com.apple.coredata.cloudkit.queue`）。
- 现在：记忆规则「跑前看 `vm.loadavg` 第一个数 <10」「App CPU≈0 且负载高就判环境问题」；人工判断后重跑。
- CLI：**是** — 系统负载检查（拿锁前采样 1 分钟负载，过高时等待或警告；失败时把负载与 App CPU 写进结果，标「高负载期间失败」）+ 偶发隔离。
- 来源：MC/check-load-before-hang-diagnosis；MC/simulator-ops-serial；CA「#442 CloudKit」段；OV:peers/agent/memories/events/2026/09/18/gate_run_deferred_high_load.md；OV:peers/agent/memories/entities/项目/conflux_swift_test_exit_code_pitfall.md（坑六）；OV:peers/agent/memories/events/2026/08/29/gate_sigsegv_flaky_rerun.md

**E2 后台进程吃资源**
- 现象：codebase-memory-mcp 索引约 200% CPU、CleanMyMac 监控、Lody 1.5G、agent-browser 不关时 headless Chrome 占满一核；swap 4.7G/6G。
- 根因：与构建无关的常驻进程和测试同时跑。
- 现在：人工排查（`uptime`、`ps`）；记忆提醒用完 agent-browser 立即 close。
- CLI：**部分** — doctor 列出 CPU/内存大户并给建议；不应自动杀进程。
- 来源：OV:peers/agent/memories/events/2026/10/13/机器负载排查与承载能力判断.md；MC/check-load-before-hang-diagnosis

**E3 两个 Xcode 并存，选错版本**
- 现象：Xcode 27 打的包只能上 TF，提审被拒；Xcode 26.6 编译器更严（CGFloat 隐式转换、类型检查上限），Xcode 27 门禁拦不住；test.sh 曾给 Xcode 26 选了 tvOS 27 模拟器而假红。
- 根因：`xcode-select` 默认 27；提审需 26；模拟器 runtime 与 Xcode 版本不配。
- 现在：手写 `DEVELOPER_DIR=…Xcode_26.6.app`；Conflux #1165 修了模拟器选择。
- CLI：**是** — doctor + 预检（按用途 dev/release 选 Xcode，校验所选 runtime 与当前 Xcode 兼容）。
- 来源：MC/appstore-builds-need-xcode26-ci；MC/testflight-local-by-default

**E4 模拟器 runtime 或池设备缺失**
- 现象：iOS 18.6 runtime 只能用 Xcode 26.6 下载；iOS 27.1 只有 iPhone Duo；XcodeBuildMCP 报 No available simulator；smb-* 场景默认设备不在池里。
- 根因：runtime 安装依赖 Xcode 版本；脚本硬编码设备名。
- 现在：交接文档写明；#148 修 smb 默认设备；e2e.sh 校验池内名字。
- CLI：**是** — 模拟器池（按配置创建/校验 `<App>-Verify-<N>`）+ doctor（缺 runtime 时给安装指引）。
- 来源：KH §六；MK/batch-level-full-gate-trial（smb 默认设备不在池）；OV:peers/agent/memories/entities/工具/orca.md（2026-09-20 条目）

**E5 宿主环境挡住构建**
- 现象：CoreSimulatorService 连接失效；Swift/Clang 缓存目录落在沙箱不可写位置。
- 根因：agent 沙箱与系统服务状态。
- 现在：人工把缓存目录改到工作区。
- CLI：**部分** — doctor 检测 CoreSimulatorService 与缓存目录可写性。
- 来源：OV:peers/agent/memories/events/2026/09/11/iOS构建受阻于宿主环境.md

**E6 新 worktree 落后或工程文件不一致**
- 现象：Lody/Orca worktree 从本地 main 建，落后数天；Krill pbxproj 不入库要先 xcodegen；Conflux pbxproj 入库，改 yml 不重生成会漂移；恢复历史分支用旧 pbxproj 导致缺符号。
- 根因：worktree 创建不 fetch；生成物与声明不同步。
- 现在：Lody setup 钩子（已退役）；派单 prompt 要求 `git fetch` + `checkout -B … origin/<base>`。
- CLI：**是** — 预检（HEAD 是否包含 `origin/<base>`、xcodegen 生成物是否与 yml 一致）。
- 来源：MC/lody-worktree-hooks；KH §五；CA「工程文件生成」；OV:peers/agent/memories/entities/工具/orca.md（2026-09-20 pbxproj 漂移）

**E7 DerivedData 与缓存堆积**
- 现象：每个 worktree 一份 DerivedData（单票可达 1GB）；删 worktree 后成孤儿。
- 根因：缓存按路径存放，没人回收。
- 现在：Lody cleanup 钩子（已随 Lody 退役）；人工删。
- CLI：**是** — `gc`：清理已不存在 worktree 的 DerivedData/XcodeBuildMCP 缓存。
- 来源：MC/lody-worktree-hooks；OV:peers/agent/memories/entities/项目/conflux_924_ios_frame_snapshot.md

**E8 Xcode 包缓存处于冲突状态**
- 现象：rebase 时 Xcode 开着工程，之后报 Missing package product（本地包）。
- 根因：Xcode GUI 缓存了冲突中的包图。
- 现在：人工清 SourcePackages、Reset Package Caches。
- CLI：**部分** — doctor 识别该错误并给出清理命令。
- 来源：OV:peers/agent/memories/events/2026/08/25/双设备验证Xcode包缓存故障排查.md

**E9 私有配置文件的位置与权限**
- 现象：凭证只能放 `~/.config/krill/dev.env`（600），不能回显；Grok Bot 唤醒值也在其中。
- 根因：凭证规则靠每个 agent 自觉。
- 现在：记忆与 prompt 文字。
- CLI：**部分** — doctor 检查文件权限；凭证注入（只把需要的变量传给子进程，不打印）。
- 来源：MK/krill-dev-environment；MK/grokbot-wake-webhook；MK/self-provision-test-env

---

## 2. 构建

**B1 同一 scheme / 同一 DerivedData 并发构建**
- 现象：多份门禁抢同一 `build/Test/DerivedData*`，反复全量重建或假红。
- 根因：固定路径的构建目录被共享。
- 现在：CA「同一 scheme 同时只能有一个构建」文字规则；全机重活锁。
- CLI：**是** — 资源锁。
- 来源：CA「分支与验证时机」；G「Native work lock」

**B2 重活锁协议各写一份，且不是所有项目都接入**
- 现象：e2e.sh、verify-icloud-development.py、worker 自写脚本各自实现 mkdir 锁 + FIFO 队列 + owner + stale 判定 + trap；旧脚本曾用 `/tmp/krill-native-build.lock`（#102 才改）；Conflux `Scripts/test.sh` 没有接入全机锁，#65 期间观察到持锁时另一项目的 tvOS 测试在并发跑。
- 根因：协议只写在全局规则里，靠每个脚本复制实现。
- 现在：G 规则 + 各脚本内嵌 Python；`KRILL_NATIVE_LOCK_HELD=1` 让父进程持锁时子命令跳过加锁（owner 也允许过门禁与 worker 重活并行）。
- CLI：**是** — 资源锁（单一实现：`kr lock run -- <cmd>`，跨项目共用，支持父进程持锁可重入、轻/重两种锁、轻活超时转重活）。
- 来源：G「Native work lock」；KE；KI；Krill commit `5edf854`（#102）；KD/65（「another project's tvOS simulator test was observed concurrently」）；CT（无 native-build 引用）；MK/batch-level-full-gate-trial

**B3 测试失败后 Xcode 收集诊断，持锁空转**
- 现象：失败时 Xcode 跑 `simctl diagnose` 最长 10 分钟，CPU≈0 仍占着全机锁（#126 记录 600 秒超时）。
- 根因：xcodebuild 默认收集诊断。
- 现在：KA 要求每条 `xcodebuild test` 加 `-collect-test-diagnostics never`；写进每个 worker prompt。
- CLI：**是** — 统一的测试运行器强制注入该参数；失败先释放锁再读日志。
- 来源：KA；MK/batch-level-full-gate-trial；KD/126-viewing-environment（L84）

**B4 漏掉项目必需的构建参数**
- 现象：漏 `-skipMacroValidation`，构建取消，0 个测试执行，白排一次队。
- 根因：每个 worker 自己拼 xcodebuild 命令。
- 现在：test.sh/e2e.sh 内已写；交接文档提示。
- CLI：**是** — 项目配置里声明固定参数，由 CLI 统一拼命令。
- 来源：KD/134-player-controls-batch4-repair（L78）；KH §六

**B5 拿到重锁后才发现编译错误**
- 现象：#114 两次拿锁后编译失败（宏内 async 读取；缺 `import Modules`，因 App 开了 MemberImportVisibility，而预检没用同样的 upcoming-feature 设置），各白排约 30 分钟；#126 两次缺 import。
- 根因：轻量预检只编被过滤的模块，编译设置与 App 目标不一致。
- 现在：PR 建议「排队期间先用轻量锁做 build-for-testing」；owner 否决 `scripts/precheck.sh`，要求做成 Rust CLI。
- CLI：**是** — 预检（从 project.yml / build settings 取 App 同一组编译设置，对本票改到的测试目标做 typecheck/build-for-testing，通过才入重活队列）。
- 来源：PR「第 6 批观察」；KD/114-module-settings（L33）；KD/126-viewing-environment；MK/personal-rust-cli

**B6 同一份代码重复构建**
- 现象：旧 e2e 每个场景各构建一次；一批内同一组场景跑 5 次。
- 根因：没有按源码指纹复用构建产物。
- 现在：#148 一次构建 + 两台模拟器 + `e2e-build.signature` 指纹重放（只允许持锁父进程重放）；Conflux test.sh 按输入内容缓存 suite 结论。
- CLI：**是** — 构建一次多次运行（源码指纹 → 产物 → test-without-building）；结果缓存（输入未变沿用结论）。
- 来源：KD/148-e2e-speed；KE；PR；CT（`run_cached_suite`）；G「builds only once」

**B7 构建指纹定义容易漏**
- 现象：加密密钥预构建步骤在构建中改写掩码字节，需构建后重算签名。
- 根因：生成文件参与构建但不在源码树。
- 现在：e2e-modes.sh 手写指纹目录清单。
- CLI：**部分** — 指纹输入由项目配置声明，CLI 负责计算与校验。
- 来源：KE（`e2e_build_signature` 与构建后重算注释）

**B8 平台覆盖与「绿了但其他平台编不过」**
- 现象：Conflux iOS 无单元测试 target，没有构建验证时 iOS 编不过 test.sh 仍全绿；worker 只在主平台证明，其他平台由合并检查编。
- 根因：门禁只跑测试 target。
- 现在：G「main platform」规则；KT 三平台 build KrillKit；CA 发版清单第 5 条。
- CLI：**是** — 选测（改到平台专属路径时自动加该平台构建；合并检查编全部平台）。
- 来源：G「Batched Integration」；KT；CA「发版前检查清单」

**B9 构建缓存过期造成假红**
- 现象：`cannot find type 'ProgramCategory'`，`.build` 计划早于新增文件，重跑即绿。
- 根因：增量构建计划未更新。
- 现在：人工重跑判定。
- CLI：**部分** — 偶发隔离（识别这类错误后用干净缓存重跑一次再判定）。
- 来源：OV:peers/agent/memories/entities/工具/orca.md（2026-09-18 门禁首跑假红）

---

## 3. 测试与模拟器

**T1 模拟器开太多导致整机卡死**
- 现象：worker 同时开 iPhone+iPad，负载 499。
- 根因：每台模拟器约 1.5G 内存并拉起大量系统进程。
- 现在：G「全机最多 2 台」；e2e.sh 检查 booted+count>2 就失败。
- CLI：**是** — 模拟器池（全局上限、租约、用后 shutdown）。
- 来源：G「simulator pool」；OV:peers/agent/memories/events/2026/09/18/1024模拟器负载处理.md；MC/simulator-ops-serial

**T2 关掉或擦掉别人的模拟器**
- 现象：`pkill Simulator`、对池外设备 `simctl erase/delete`。
- 根因：多会话共用设备，无归属记录。
- 现在：G 规则；e2e.sh 只操作自己 lease 的设备；记忆禁止 pkill。
- CLI：**是** — 模拟器池（lease 记录 owner pid；只允许操作自有 lease；清理死进程的 lease）。
- 来源：G；OV:…/1024模拟器负载处理.md；OV:…/conflux_924_ios_frame_snapshot.md

**T3 Xcode 并行测试克隆出池外模拟器**
- 现象：并行测试会创建模拟器克隆，绕过池上限。
- 根因：xcodebuild 默认并行。
- 现在：test.sh/e2e.sh 写死 `-parallel-testing-enabled NO`。
- CLI：**是** — 运行器注入，并用两个 lease 显式并行。
- 来源：KD/148-e2e-speed「Scope」；KE；KT

**T4 test.sh 不走池、不拿锁**
- 现象：Krill `scripts/test.sh` 选「最新的任意 iPhone」，也不加全机锁。
- 根因：脚本早于池与锁规则。
- 现在：协调者自觉在锁内跑。
- CLI：**是** — 资源锁 + 模拟器池统一包住门禁。
- 来源：KT（读源码所见）

**T5 零测试或部分测试被当成通过**
- 现象：`swift test --filter` 不认中文显示名，零匹配 exit 0；`shelf-edit` 场景忽略方法过滤，实际跑整类 11 例；smoke 合并结果只有 5 例仍报 PASS；拿锁后编译失败 0 测试。
- 根因：只看退出码，不看测试计数。
- 现在：G「A run that runs zero tests is not a pass」；e2e.sh 入队前校验过滤器、运行后用 xcresult 校验 total>0、expected、skipped=0。
- CLI：**是** — 选测（入队前枚举匹配）+ 结果校验（xcresult 计数）。
- 来源：G；KE（`validate_result`）；OV:…/conflux_swift_test_exit_code_pitfall.md（坑四）；KD/124-shelf-share（L200）；KD/148-e2e-speed（smoke-merge-red）

**T6 退出码被吞**
- 现象：`| tee`、`| tail`、末尾 `; echo exit=$?` 把失败变成 0；`swift test` 末行只代表 Swift Testing 一侧。
- 根因：shell 管道语义。
- 现在：记忆中的坑列表。
- CLI：**是** — 结果校验（以 xcresult/结构化结果和真实退出码判定，不依赖管道）。
- 来源：OV:…/conflux_swift_test_exit_code_pitfall.md（坑一二三）；OV:peers/agent/memories/entities/工具/orca.md（PIPESTATUS）

**T7 自测层级不够**
- 现象：`swift test` 覆盖不到 xcodebuild App target；macOS 79/79 不能代替 iOS（无宿主 iOS 包没有 Keychain entitlement）。
- 根因：轻量测试与交付证据不是同一层。
- 现在：G「light work 不是交付证明」；KA「macOS 只做迭代」。
- CLI：**部分** — 选测时标出「本改动的交付证据层」，报告里区分轻量结果与主平台结果。
- 来源：OV:…/conflux_swift_test_exit_code_pitfall.md（坑五）；KD/116-ios-secret-store（L26）；KA

**T8 选测要么太宽要么漏**
- 现象：`--changed` 实际跑全部 ~88 个 UI（30–50 分）；`affected` 遇通配规则每票都选近全量（#144 选 28 个）；映射漏间接依赖（收藏页经详情页；第 4 批漏 modules/shelf-edit，合入后才发现）。
- 根因：影响映射手写、粒度粗。
- 现在：`e2e-impact.json` + 未匹配运行时路径选全部；协调者裁定「≥20 场景时 worker 只跑本票相关」。
- CLI：**是** — 选测（映射 + 间接依赖 + 阈值策略；新页面要求更新映射）。
- 来源：MK/batch-level-full-gate-trial；PR；CD（07:5x）；KD/148-e2e-speed；KE

**T9 排队是主要等待**
- 现象：#124 七轮排队 21–48 分；#114 排队累计 102 分，实际运行约 85 分。
- 根因：所有重活串行在一把锁上，失败后排到队尾。
- 现在：报告里人工记时间。
- CLI：**是** — 计时（分开记录排队/构建/测试）+ 排队时提示可做的轻量工作（预检）。
- 来源：KD/124-shelf-share（L240–256）；KD/114-module-settings（L18）；KD/144-module-item-daikan（L157–162）

**T10 失败后整轮重跑**
- 现象：修一个测试就重跑全部；验收要求「连续两轮全绿」，每轮约 40 分。
- 根因：没有上轮结果记录与源码等价判断。
- 现在：G「只重跑失败的」；协调者改规则「App 代码没变时：上一轮完整绿 + 最终一轮完整绿 + 改动用例单独多次」。
- CLI：**是** — `rerun --failed`；用源码指纹判断 App/包未变以支持证据等价。
- 来源：G；PR「流程问题 2」；CD（05:19）；KD/124-shelf-share（L205）

**T11 macOS UI 测试的输入方式**
- 现象：`typeText()`/`typeKey()` 触发 Input Monitoring 弹窗；合成按键落进用户正在用的 Claude Code 输入框；测试遗留 `conflux.player.muted=1` 写进正式版 defaults。
- 根因：合成事件作用于前台窗口；Debug 与正式版共用 defaults 域。
- 现在：G Platform Notes；CA「XCUITest 输入方式」；派单 prompt 禁令。
- CLI：**部分** — 预检静态扫描 macOS UI 测试中的 `typeText`/`typeKey`/CGEvent；运行前后对比用户 defaults 并报告遗留键。
- 来源：G「Platform Notes」；MC/no-synthetic-keys-on-macos；CA

**T12 computer-use / 桌面控制挂死**
- 现象：Computer Use `-10005 timeoutReached`，GUI 路径未实测；无人值守时停在权限弹窗。
- 根因：需要人工授权。
- 现在：G 规则禁止用于 Apple UI 验证。
- CLI：**否**（规则层面）。
- 来源：G「Browser and Desktop Interaction」；KD/36-library-shelf-validation（L32）；KD/37-jellyfin-source-validation（L32）

**T13 模拟器启动与取证细节**
- 现象：环境变量钩子必须加 `SIMCTL_CHILD_` 前缀否则静默无效；重装后 App 容器 UUID 变化；`simctl spawn defaults write` 没写进 App 容器；高负载下 `simctl io screenshot` 从 250ms 变 5–11s。
- 根因：simctl 行为细节。
- 现在：记忆。
- CLI：**部分** — 封装 launch（自动加前缀、每次现取容器路径）。
- 来源：OV:peers/agent/memories/entities/项目/conflux_simulator_ui_hooks.md；OV:…/conflux_924_ios_frame_snapshot.md；OV:peers/github.com-bulai0408-conflux/memories/events/2026/09/24/AGENTS.md 模拟器写入说明失效.md

**T14 Files 选择器与测试媒体播种**
- 现象：Files 选择器在 App 进程外，`snapshot-ui` 看不到；普通 tap 不生效，要 AXe `--tap-style physical`；需要往「我的 iPhone」放测试片源。
- 根因：系统 UI 在独立进程。
- 现在：记忆中的手工配方（APFS clone 到 File Provider Storage）。
- CLI：**部分** — 模拟器池附带「播种」命令（按配置把 staging 目录 clone 进 LocalStorage AppGroup）。
- 来源：MK/simulator-files-picker-automation

### 真机

**P1 真机 UI 自动化前置条件**
- 现象：kAXError；需 Developer Mode + 设置 › 开发者 › Enable UI Automation + 保持解锁亮屏；程序化转屏真机不支持。
- 根因：系统安全设置。
- 现在：G Platform Notes；人工。
- CLI：**是** — 真机流程（`devicectl` 列设备、检查配对/开发者模式，缺项时给出人工步骤）。
- 来源：G；OV:peers/agent/memories/events/2026/09/04/真机XCUITest排障.md；KD/121-airing-calendar（L44）

**P2 真机装包后掉线**
- 现象：`CoreDeviceService was unable to locate a device`、NWError 60；自动启动被开发通道拒绝；两设备 CloudKit 接力因 iPad 未解锁没跑完。
- 根因：设备连接不稳定、需人在场。
- 现在：按「安装成功、取证失败」如实记录。
- CLI：**部分** — 真机流程记录每步结果，把掉线归为取证缺口而不是失败/通过。
- 来源：OV:peers/agent/memories/entities/工具/orca.md（2026-09-18 devicectl）；KI；KD/108-profile

**P3 远程/SSH 环境不能签名**
- 现象：Grok Bot 经 SSH 可编译和跑模拟器，但设备构建、archive、签名 macOS 构建、macOS XCUITest 不行。
- 根因：SSH 会话拿不到钥匙串签名身份与 GUI 会话。
- 现在：MK 文字规则。
- CLI：**是** — doctor/签名检查（识别 SSH 会话，提前拒绝需要签名的任务）。
- 来源：MK/no-parallel-agents；KD/110-compliance-validation（L46）

**P4 签名与 entitlement 回读**
- 现象：签名能力通过 ≠ 功能通过；要回读 `.app` 签名与 embedded profile entitlement；团队应为 K4AXH95DCF。
- 根因：自动签名结果不透明。
- 现在：G「Signing」；人工 codesign 回读。
- CLI：**是** — 签名检查（团队 ID、entitlement、iCloud 容器环境 Development/Production）。
- 来源：G「Signing」；OV:peers/agent/memories/entities/工具/orca.md（2026-09-20 签名回读）；KI（核对团队、容器与环境）

**P5 Xcode 不支持设备系统版本 / 缺真机截图工具**
- 现象：Instruments 不能 profile 更新的 iOS；本机无 libimobiledevice，agent 看不到真机屏幕。
- 根因：工具链版本。
- 现在：G 要求在报告中写明限制。
- CLI：**是** — doctor 报告真机能力边界，写进交付报告「未验证」段。
- 来源：G「Platform Notes」；OV:…/conflux_simulator_ui_hooks.md

---

## 4. UI 测试稳定性

**U1 偶发失败的处理流程**
- 现象：module-watchlist、ShelfEditing 数字保存、player-controls 两例（XCTest 启动时终止旧 App 失败；PNG 元数据即时 exists 断言）、source-management（runner 在连接前收到 TERM）。
- 根因：模拟器基础设施抖动或测试写法。
- 现在：规则「单独复跑一次 → 记录到 `docs/agents/flaky-tests.md` + 跟进；再失败就修或隔离」。注意：`origin/v0/integration` 里**没有**这个文件，记录散落在交付报告和 issue（#153）。
- CLI：**是** — 偶发隔离（失败用例自动单独复跑一次，分类为「复现 / 偶发」，写入统一台账，偶发用例不计入通过）。
- 来源：MK/batch-level-full-gate-trial；KD/114-module-settings（L37）；KD/124-shelf-share（L196）；MK/overnight-plan-2026-10-14（#153）；G「When a check fails, run it again by itself」

**U2 测试写法类不稳定**
- 现象：即时 `exists` 断言（改 `waitForExistence`）；双击替换输入没替换（2 变 20）；键盘挡住可见性断言；自定义 Menu 产生重复 AX 节点 isHittable 失败。
- 根因：XCUITest 时序与 AX 树细节。
- 现在：逐例修。
- CLI：**部分** — 预检里对 UI 测试源码做轻量 lint（即时 exists、双击+typeText 替换）；不改测试。
- 来源：KD/114-module-settings；KD/148-e2e-speed「Intermittent failure record」；CD（10:3x）

**U3 合并后才出现的布局问题**
- 现象：#117 加两行后 modules 场景目标被挤出首屏。
- 根因：单票测不出组合效果。
- 现在：合并后跑选中场景；用滚动助手。
- CLI：**部分** — 合并辅助在组合 HEAD 上跑选测，归因到后合入的票（修复本身不归 CLI）。
- 来源：PR「第 5 批暴露的问题」

**U4 截图证据与实际不符**
- 现象：名为 dark 的截图实际是浅色（`AppleInterfaceStyle` 参数在 Catalog 不生效）。
- 根因：启动参数无效且无断言。
- 现在：改成 Catalog 显式注入并断言 `colorScheme`。
- CLI：**部分** — 证据收集时检查截图平均亮度与文件名声明的外观是否一致，不一致标警告。
- 来源：PR；KD/124-shelf-share（L131）

**U5 测试会话挂起**
- 现象：门禁卡在 test session clean-up，只能中断（exit 143）；诊断收集 600 秒超时。
- 根因：Xcode 测试会话清理挂起。
- 现在：人工发现后中断。
- CLI：**是** — 挂起看门狗（N 分钟无输出且 CPU≈0 → 标挂起、保留日志、释放锁）。
- 来源：KD/34-user-state-validation（L70）；KD/126-viewing-environment（L84）

**U6 排队期间脚本被改**
- 现象：排队中的临时 shell 脚本被本任务改写，轮到时语法错误，0 测试。
- 根因：入队的是路径不是内容。
- 现在：改用不可变副本。
- CLI：**是** — 入队时固定命令与参数（不依赖可变脚本文件）。
- 来源：KD/126-viewing-environment（L103）

**U7 外部服务让回归套件随机**
- 现象：共享 Emby 跨源 302 后外部目标超时；公网 HLS 一次性测试。
- 根因：依赖外部可用性。
- 现在：真实服务场景显式 opt-in，不进常规套件。
- CLI：**部分** — 场景分级（合成 / 真实服务 / 截图），真实服务场景需显式请求并检查凭证变量。
- 来源：KD/71-redirect-token-validation；KD/116-module-streams（L124）；KE

---

## 5. 数据隔离（CloudKit、钥匙串、沙盒）

**D1 测试启动连到真实 iCloud Development**
- 现象：不带界面参数的 acceptance 启动会创建真实 UserStateSync，可能写默认 zone；历史是否写入未知。
- 根因：App Root 的隔离门只认 UI 参数。
- 现在：#148 在 Root 加门禁 + 最低层回归测试（`eba7bd5`）；合成 bundle 用 `com.kami.krill.e2e148`。
- CLI：**部分** — 数据隔离检查（运行前确认测试包 bundle id 是合成包、entitlement 环境是 Development；不能证明 App 内部不写云）。
- 来源：PR；CD（03:xx）；KD/148-e2e-speed「Resource audit」

**D2 重启 App 时隔离参数被沿用**
- 现象：`XCUIApplication.open(URL)` 会重新启动并沿用「清数据」参数，「重开后数据还在」类用例失败。
- 根因：测试辅助方法设计。
- 现在：`launchIsolated()` 只在首次启动清数据。
- CLI：**否**（测试代码层面）。
- 来源：PR；KD/148-e2e-speed

**D3 macOS 共用沙盒容器与 defaults**
- 现象：所有 worktree 的 Conflux Debug 共用 `com.kami.conflux` 容器，一个分支启动就删掉另一分支装的识别模型；Debug 与正式版共用容器，可能触发 iCloud 桥崩溃并删用户关注列表。
- 根因：bundle id 相同。
- 现在：启动前备份 default.store、核对计数；改用独立 SwiftPM 小程序。
- CLI：**部分** — 数据隔离检查（macOS 运行前快照容器关键文件与 defaults，运行后对比并报告）。
- 来源：MC/shared-sandbox-container-across-worktrees；OV:memories/preferences/蔡煜恒（Kami）/macOS 真机验证安全.md；MC/no-synthetic-keys-on-macos

**D4 无宿主 iOS 包测试没有钥匙串权限**
- 现象：持久化用例在 iOS 包测试中报 `.executionFailed`（#128、#116）。
- 根因：无宿主 iOS 测试没有 Keychain entitlement；需注入内存 SecretStore，真实钥匙串只在 App 宿主测。
- 现在：#111 约定 + 交付报告说明。
- CLI：**否**（测试代码约定）；可在预检提示。
- 来源：KD/116-ios-secret-store；KD/128-module-encryption；OV:peers/agent/memories/events/2026/10/12/full_gate_simulator_failure.md

**D5 真实 CloudKit 测试的边界**
- 现象：真实 CloudKit 容器用例在高负载下 SIGTRAP；不得删除默认 zone，只清理随机测试 zone；禁止 Production；需空闲、已签名、测试账户可用。
- 根因：Apple 框架串行队列 + 共享云状态。
- 现在：opt-in 环境变量（`CONFLUX_RUN_REAL_CLOUDKIT_TESTS`、`KRILL46_CLOUDKIT_DEVELOPMENT`）；验收脚本自带锁与清理。
- CLI：**部分** — 真机流程 + 系统负载检查 + 签名检查作为真实云测试的前置门；清理逻辑留在项目脚本。
- 来源：CA「#442」；KI

**D6 改了 schema 门禁文件 ≠ 已部署**
- 现象：CloudKit schema 文件改了，但 Development 验证、Production 部署、留档还没做。
- 根因：部署是人工步骤。
- 现在：发版清单第 1 条；release 脚本强制 schema 检查。
- CLI：**部分** — 交付报告检测 schema 相关文件变动时自动加「待部署」项。
- 来源：OV:peers/agent/memories/events/2026/08/30/cloudkit_schema_files_confirmed.md；CA「发版前检查清单」

**D7 并行测试共享端口、文件或数据**
- 现象：两条测试通道并行时，固定端口或共享文件会冲突。
- 根因：fixture 写死资源。
- 现在：G 规则；#148 审计表（OS 分配端口、UUID 目录、每台模拟器独立沙盒）。
- CLI：**部分** — 预检扫描 UI/App 测试中的固定端口与固定路径。
- 来源：G「Isolate test data」；KD/148-e2e-speed「Resource audit」

**D8 真实服务场景的凭证写入与清理**
- 现象：webdav-real/smb-real 需要独立签名包、把私有配置写进容器、结束后删除；合成场景要 `env -u` 去掉凭证变量。
- 根因：凭证不能进仓库或日志。
- 现在：e2e.sh 内嵌 Python 写入与 cleanup。
- CLI：**是** — 凭证注入（从 dev.env 取指定变量，写入/清理容器文件，不回显）。
- 来源：KE；MK/krill-dev-environment

---

## 6. 合并与集成

**M1 每票单独跑门禁浪费时间**
- 现象：3 个 worker 各跑全量门禁，全机锁成瓶颈；协调者曾为 #113 单独浪费一次门禁。
- 根因：门禁不在组合 HEAD 上跑就证明不了集成。
- 现在：分层门禁（第 1 层阻塞合并、第 2 层合并后、第 3 层合入 main 前）；链式合并只门禁最终 HEAD。
- CLI：**是** — 合并辅助（建临时集成分支，按顺序合入多张票，在最终 HEAD 跑第 1 层）。
- 来源：MK/batch-level-full-gate-trial；MC/gates-centralized-at-coordinator；G「Batched Integration」

**M2 门禁失败归因**
- 现象：要判断是单票问题、基线问题、偶发还是只在组合时出现。
- 根因：多票合并。
- 现在：协调者手工在各分支与基线上复现。
- CLI：**是** — 合并辅助（失败用例在每个分支和基线上复跑，输出归因表）。
- 来源：MK/batch-level-full-gate-trial；G「Gate Failure Handling」

**M3 解冲突引入新错误**
- 现象：协调者合并 #114 时在冲突块内补了非冲突区已有的语句，导致重复 DROP，test.sh 失败。
- 根因：只看冲突块，不看整段函数。
- 现在：PR 教训「解冲突后看整段函数；先用轻锁跑受影响包测试」。
- CLI：**部分** — 合并辅助在解冲突后列出冲突所在的完整函数 diff，并自动跑受影响的包测试。
- 来源：PR「第 6 批观察」最后一条

**M4 叠放 PR 合并顺序**
- 现象：`gh pr merge --delete-branch` 删掉 base，依赖它的 PR 被直接关闭。
- 根因：GitHub 不自动改挂。
- 现在：记忆配方（先改挂 main 再合，最后删分支）。
- CLI：**是** — 合并辅助（叠放 PR 先 retarget 再合并，最后统一删分支）。
- 来源：MC/stacked-pr-merge-order

**M5 并行票的迁移版本号冲突**
- 现象：同批 #144/#114 都加 UserState 迁移，需预先分配 v17/v18；合并时排序。
- 根因：迁移版本号全局单调。
- 现在：协调者在派单时手工分配。
- CLI：**部分** — 预检/合并辅助检测多分支新增的同号迁移。
- 来源：MK/overnight-plan-2026-10-14；OV:peers/agent/memories/entities/项目/krill_114_module_settings.md

**M6 批次 diff 凭证扫描**
- 现象：合并前要扫描组合 diff 与证据文本中的凭证。
- 根因：凭证规则依赖自觉。
- 现在：G 规则；worker 自写扫描（#148 扫 735 个文本证据，#128 扫 56 个路径）。
- CLI：**是** — 凭证扫描（diff + 证据目录，常见 token/私钥格式；不打开私有 env 文件）。
- 来源：G；KD/148-e2e-speed；OV:peers/agent/memories/events/2026/10/12/Krill_128_iOS加密套件通过并交付.md；KH §五

**M7 第 2 层检查的计数规则**
- 现象：「合并后检查连续 3 批无缺陷 → 只在合入 main 前跑」需要人工计数；第 2 层只跑第 1 层没跑的场景。
- 根因：批次状态没有持久记录。
- 现在：PR 手工记录计数。
- CLI：**是** — 批次台账（每批记录第 1/2 层场景集合和结果，自动算出第 2 层剩余场景与连续计数）。
- 来源：KA；PR「合入后检查连续无问题计数」

**M8 同一批里重复跑同一组场景**
- 现象：worker 各跑 affected（近全量）+ 合并门禁再跑一次，一组场景一批跑 5 次。
- 根因：结果没有按源码指纹共享。
- 现在：协调者裁定缩小 worker 范围。
- CLI：**是** — 结果缓存（同一指纹 + 同一场景已绿则复用）。
- 来源：PR「第 6 批观察」；CD（07:5x）；CT（suite 缓存先例）

**M9 分支保护只有本地钩子**
- 现象：私有仓库免费计划没有服务端分支保护，只有 `pre-push`，`--no-verify` 可绕过。
- 根因：GitHub 计划限制。
- 现在：CA 文字。
- CLI：**部分** — 合并辅助只通过 PR 合并，拒绝直推保护分支。
- 来源：CA「分支」

---

## 7. 交付与证据

**R1 三段式交付报告**
- 现象：每个 worker 手写「机器判定 N/N + xcresult / 需人判断手感 / 未验证及原因」，格式和数字靠人工抄。
- 根因：没有从 xcresult 自动生成。
- 现在：G「Deliver」；派单 prompt 模板。
- CLI：**是** — 证据收集/交付报告（从本次运行的 xcresult、截图、日志生成骨架，人只补手感与未验证原因）。
- 来源：G「4. Deliver」；KD/*（全部交付报告）

**R2 「已编译」「已实测」「模拟器」「真机」混淆**
- 现象：多次被要求分清编译与实测；模拟器结果不能冒充真机；opt-in 跳过不算 PASS。
- 根因：证据层级靠文字。
- 现在：KH 沟通要求；交付报告逐条声明。
- CLI：**是** — 交付报告按证据层分栏（编译 / 轻量 / 主平台模拟器 / 真机），跳过数单列。
- 来源：KH §一；KD/35-player-controls-validation（L3）；KD/148-e2e-speed（skip 不计 PASS）

**R3 日志被覆盖、证据路径不可点**
- 现象：后续检查覆盖前次日志；路径用反引号不可点击。
- 根因：固定输出路径。
- 现在：G「Preserve full logs」「用 Markdown 绝对路径链接」；e2e.sh 每次 `build/e2e-<时间>-<pid>`。
- CLI：**是** — 每次运行独立证据目录，报告用绝对路径 Markdown 链接。
- 来源：G「Gate Failure Handling」「Help Humans Understand」；KE

**R4 用时统计靠手算**
- 现象：报告要分开写开始、Artboard 等待、实现、排队、验证；早报要对比各批开发/排队/门禁/合并后检查时长。
- 根因：没有自动计时。
- 现在：worker 和协调者人工整理。
- CLI：**是** — 计时（每条命令记录排队、持锁、构建、测试时间，按票和批汇总）。
- 来源：KD/114-module-settings（L18）；MK/overnight-plan-2026-10-14；PR「时间线对比」

**R5 计时数据被并发污染**
- 现象：#148 基线测量与协调者复验、#145 重叠，需空闲重测 11 个场景。
- 根因：计时不知道同时段谁在跑。
- 现在：人工记录重叠窗口并剔除。
- CLI：**是** — 计时同时记录同期锁持有者与负载，自动标「被污染」。
- 来源：KD/148-e2e-speed（coordinator-overlap）；PR「流程问题 5」

**R6 发布成功需回读权威来源**
- 现象：脚本输出 UPLOAD SUCCEEDED 但 ASC 仍 PROCESSING；同版本号某端已上架会被拒（90062）。
- 根因：上传 ≠ 处理完成 ≠ 可测。
- 现在：记忆规则，用 asc 回读。
- CLI：**部分** — 若将来覆盖发版：上传后轮询 ASC 状态，分层汇报（V1 范围可不含）。
- 来源：MC/tf-success-needs-asc-readback；MC/tf-check-approved-version-per-platform；MC/testflight-local-by-default

**R7 P2 遗留项被悄悄丢掉**
- 现象：P2 散落在 PR 正文和交付报告，没人汇总。
- 根因：无台账。
- 现在：协调者手工维护。
- CLI：**部分** — 从交付报告/PR 正文抽取 P2/P3/未覆盖项，汇总成台账（判断与处理仍由人）。
- 来源：MC/p2-not-ignorable；G「Review Convergence」

---

## 8. 协作与派单

**C1 worker 等后台任务时结束这一轮**
- 现象：worker 中途变空闲，靠协调者唤醒（#148 一次；交接文档称「多次」）。
- 根因：agent 在等待长任务时结束 turn。
- 现在：派单 prompt「不要结束这一轮；必须结束就先发 heartbeat 写明在等什么」。
- CLI：**部分** — 排队/运行命令在前台阻塞并周期性输出进度，减少 agent 以为「已无事可做」的情况；唤醒仍归 Orca。
- 来源：KH §五；PR「流程问题 1」

**C2 派单 prompt 里堆满机械规则**
- 现象：每个 worker prompt 都要重复：锁协议、池名、`-collect-test-diagnostics never`、不跑 test.sh/`--changed`、凭证位置、不碰 Production、交付格式。
- 根因：规则没有可执行载体。
- 现在：`~/orca/workspaces/Krill/v0-coord/prompts` 模板。
- CLI：**是** — 把这些变成 CLI 默认行为，prompt 只需写「用 kr verify / kr deliver」。
- 来源：OV:peers/agent/memories/entities/项目/krill_114_module_settings.md（完整派单要求）；MK/batch-level-full-gate-trial

**C3 worker 与协调者职责混淆**
- 现象：worker 各自跑全量门禁和压测，负载 170+，协调者门禁被挂住。
- 根因：命令对谁都一样。
- 现在：记忆规则「worker 只做实现、相关测试、自审；门禁由协调者串行跑」。
- CLI：**是** — 按角色区分子命令（worker 用 `verify`，协调者用 `gate`/`integrate`），worker 调用全量门禁时警告。
- 来源：MC/gates-centralized-at-coordinator；MK/batch-level-full-gate-trial

**C4 同时开几个 worker**
- 现象：上限从「串行」到 3，再到一次 5；多开只会加长排队。
- 根因：重活都在一把锁上。
- 现在：owner 逐次口头决定。
- CLI：**部分** — 提供锁队列长度、平均等待与负载数据，帮助协调者决定并发数；不负责派单。
- 来源：MK/no-parallel-agents；OV:…/机器负载排查与承载能力判断.md；ML/serial-agents-only

**C5 Orca 派发绑定问题**
- 现象：capability 被吊销、compact 后丢失 IDs、用户重开 continuation 终端、`waiter_exists`、`terminal_worktree_mismatch`。
- 根因：Orca 生命周期。
- 现在：记忆配方。
- CLI：**否** — 属于 Orca 边界，kr-cli 不应接管。
- 来源：OV:peers/agent/memories/entities/工具/orca.md；MC/user-restarts-worker-terminals；MC/orca-worker-agent-claude

**C6 派单通道与 agent 选择过时**
- 现象：照旧交接用 Lody 派单被撤回；照模板起 Codex 而用户要 Claude；astra 模型规则被取代。
- 根因：交接文档与当前设置不一致。
- 现在：记忆。
- CLI：**否**（派单不在 CLI 范围）；最多 doctor 提示当前配置。
- 来源：MK/orca-only-no-lody；MC/orca-worker-agent-claude；MK/worker-model-astra

**C7 远程运行环境休眠**
- 现象：Grok Bot 休眠，需先 POST 唤醒，再轮询直到不再 `remote_runtime_unavailable`；多次卡住被放弃。
- 根因：远程机器节能。
- 现在：记忆配方。
- CLI：**部分** — doctor 检查远程环境可达性；唤醒与派单仍归协调者。
- 来源：MK/grokbot-wake-webhook；MK/no-parallel-agents

**C8 收尾清理误删现场**
- 现象：worker 工作树有未提交文件时差点归档；终端未关时删 worktree 返回 `terminal_close_incomplete`。
- 根因：清理前没检查。
- 现在：人工检查。
- CLI：**部分** — `gc`/清理前检查未提交改动、运行中进程、自有锁与 lease。
- 来源：OV:peers/agent/memories/events/2026/10/08/worker 未提交文件归档暂停.md；OV:peers/agent/memories/entities/工具/orca.md（2026-09-19 清理）

---

## 汇总：CLI 能力 → 承接条目

| 能力 | 承接条目 |
|---|---|
| 资源锁（全机重/轻锁、FIFO、可重入、stale 清理） | B1 B2 B3 T4 U5 |
| 模拟器池（租约、上限 2、只动自有设备、播种） | E4 T1 T2 T3 T4 T14 |
| 预检（base、xcodegen、App 同编译设置、过滤器、源码 lint） | E6 B4 B5 T5 T11 U2 D7 M5 |
| 构建一次多次运行 / 结果缓存 | B6 B7 T10 M8 |
| 选测（影响映射、间接依赖、平台、分级） | B8 T7 T8 U7 |
| 结果校验（xcresult 计数、真实退出码） | T5 T6 R2 |
| 偶发隔离 / 挂起看门狗 | E1 B9 U1 U5 |
| 合并辅助（链式合并、归因、叠放 PR、迁移号） | M1 M2 M3 M4 M5 M9 U3 |
| 凭证扫描 / 凭证注入 | E9 M6 D8 |
| 环境体检 doctor | E2 E3 E4 E5 E7 E8 P3 P5 C7 |
| 系统负载检查 | E1 D5 R5 |
| 签名检查 / 真机流程 | P1 P2 P3 P4 D5 |
| 数据隔离检查 | D1 D3 T11 |
| 计时 | T9 R4 R5 C4 |
| 证据收集/交付报告 | R1 R2 R3 R7 D6 U4 |
| 批次台账 | M7 |
| 不由 CLI 承接 | T12 D2 D4 C5 C6 |

## 观察到的缺口（读源码时发现，未在规则中写明）

1. Krill `scripts/test.sh` 不拿全机锁、不用池设备（T4）。
2. Conflux `Scripts/test.sh` 没有接入 `/tmp/native-build.lock`（B2），与 Krill 共享机器时会并发。
3. 规则引用的 `docs/agents/flaky-tests.md` 在 `origin/v0/integration` 中不存在（U1），偶发记录分散。
4. `shelf-edit` 场景不支持方法级过滤（T5），e2e.sh 仍接受方法变量。
