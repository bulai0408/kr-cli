# Tuist 是否适合 Krill 的测试选择与缓存（调研，2026-10-14）

只读调研。每条结论后附来源（URL 或本机命令）。

## 0. Krill 现状（本机核对）

- 工程由 XcodeGen 生成，`project.yml` 定义 `Krill`、`KrillTests`、`KrillUITests` 三个 target，本地包 `KrillKit` 由 `packages: KrillKit: path: KrillKit` 引入。来源：`git -C /Users/cyh/Workspace/Krill show origin/v0/integration:project.yml`
- `KrillKit` 有 8 个库 target、6 个测试 target（ModulesTests、AnalyticsTests、MetadataTests、LibraryTests、SourcesTests、UserStateTests），依赖图可直接拿到。来源：在 `git archive` 出的 `KrillKit/Package.swift` 上运行 `swift package dump-package`
- `KrillUITests` 是一个 target，下有 32 个 `*Tests.swift` 文件（场景）；`KrillTests` 有 26 个测试文件。来源：`git ls-tree -r --name-only origin/v0/integration | grep -cE '^KrillUITests/.*Tests\.swift$'`
- 本机没有安装 tuist（`which tuist` 输出 `tuist not found`）；XcodeGen 最新版 2.46.0（2026-07-16），来源：`gh api repos/yonaskolb/XcodeGen/releases/latest`

## 1. 选择性测试（Selective testing）

| 问题 | 结论 | 来源 |
|---|---|---|
| 粒度 | **最细只到 test target**：“the maximum granularity of selective testing is at the target level” | https://tuist.dev/en/docs/guides/features/selective-testing/generated-xcode-project |
| 定价页的说法 | “skipping test suites (test targets) whose tests are not impacted” | https://canary.tuist.dev/pricing （tuist.dev/pricing 对 curl 返回 Cloudflare 403，canary 镜像内容相同） |
| 哈希输入 | target 属性（名称、平台、产物类型等）+ target 的文件 + 依赖的哈希；缓存哈希还加 Swift 版本（`xcrun swift --version`）和所选构建配置的 build settings | https://tuist.dev/en/docs/guides/features/projects/hashing |
| 调试命令 | `tuist hash selective-testing`、`tuist graph --format json` | 同上 |
| 支持的工程 | 当前文档只列 “Generated Xcode project”，并写 “Support for other build systems is planned.” | https://tuist.dev/en/docs/guides/features/selective-testing |
| XcodeGen 工程 + `tuist xcodebuild test` | **已不支持**。2025-02 曾推出（博客 “Selective Testing for all Xcode projects”），2026-01-15 PR #9126 “remove selective testing support for vanilla Xcode projects”，理由是 “no-one has ever used the feature”；2026-09-24 PR #13569 删除剩余哈希代码。现在 `tuist xcodebuild test` 只做测试洞察、隔离（quarantine）、分片 | https://github.com/tuist/tuist/pull/9126 ；`gh api repos/tuist/tuist/commits/dd736e241`；博客源文件 `server/priv/marketing/blog/2025/02/18/selective-testing-for-xcode-projects.md` |
| 是否必须账号/服务器 | 不必须。`tuist test` 把成功的哈希存入 cache category `.selectiveTests`，用 `cacheStorageFallingBackToLocal`；`--no-upload` 时只存本地 | `cli/Sources/TuistKit/Services/TestService.swift`（约 1373–1390 行，`gh api repos/tuist/tuist/contents/...` 读取） |
| 需要账号的部分 | 测试分片：“Test sharding requires a Tuist account”；跨机器/CI 共享哈希、PR 评论、隔离测试列表都依赖 `fullHandle` | 同一文件第 90、617、697 行 |
| 覆盖率 | “Test coverage tools assume that the whole test suite runs at once, which makes them incompatible with selective test runs” | generated-xcode-project 页 |

**能否跳过一个 UI test target 里的单个测试类？不能。** 粒度是 target。Krill 的 32 个 UI 场景在同一个 `KrillUITests` 里，`KrillUITests` 依赖 `Krill` app，而 app 依赖几乎所有 KrillKit 模块，所以任何 app 或模块改动都会让整个 UI target 重跑。不拆 target 时，Tuist 只能在“完全没改动”时跳过全部 UI 测试，以及跳过未受影响的 KrillKit 单元测试 target——后者用 `swift package dump-package` 的依赖图自己算也能做到。

## 2. 模块/二进制缓存

| 项 | 结论 | 来源 |
|---|---|---|
| Module cache 前提 | “Requires Tuist project generation”；读取 `Project.swift` 背后的图 | https://tuist.dev/en/docs/guides/features/cache ；https://tuist.dev/en/docs/guides/features/cache/module-cache |
| 可缓存的产物 | framework、library、bundle、macro；“`unitTests` and `uiTests` targets are not cached as binaries”；不用于 release | module-cache 页 |
| 本地模式 | `tuist cache --no-upload` 只存本地 | module-cache 页 |
| Xcode cache（编译缓存） | “Works with any Xcode project, no project generation required”；基于 Xcode 26 引入的编译缓存，`tuist setup cache` 装 LaunchAgent 代理；需要 `Tuist.swift` 里的 `fullHandle`（即账号+项目），CI 需登录或 `TUIST_TOKEN` | https://tuist.dev/en/docs/guides/features/cache/xcode-cache |
| Xcode cache 已知问题 | 代理失败、旧 socket 导致 `deadlineExceeded`、Xcode 27 下覆盖率可能漏掉映射路径的源文件 | 同上 |
| 自托管 | 自托管缓存节点（Kura）“available on the Enterprise plan”；整套服务器自托管也列在 Enterprise | https://tuist.dev/en/docs/guides/features/cache/self-hosting ；pricing 页 |
| 隐私/上传内容 | 文档没有逐项列出上传数据。代码中有 `fullHandle` 时会上传测试结果、构建数据（`uploadBuildRunService`、test insights），远端缓存上传二进制和哈希 | `cli/Sources/TuistKit/Services/XcodeBuild/XcodeBuildTestCommandService.swift` 第 347、546 行；PR #10186 “upload build data from tuist xcodebuild build” |

## 3. 价格、账号、许可证

- 三档：Air 免费（“All features, no credit card required”）；Pro 用量计费，“$0 and up”；Enterprise 定制，含自托管。来源：https://canary.tuist.dev/pricing
- 免费额度：Cache 200 单位/月，Tests 200 单位/月，超出 Pro 每单位 $0.5；单位 = 一次与服务器交互的命令（缓存拉取或测试结果拉取）。超限时 “the plan will cap interactions to prevent unexpected charges”。Generated Projects、Previews、Registry、Insights 不限量。来源：同上
- 单人私有项目：Air 免费档足够；纯本地使用（`--no-upload`、不配 `fullHandle`）不消耗单位。账号只在跨机共享、Xcode cache、分片、洞察时需要。
- 许可证：仓库默认 MIT；`server/`、`kura/`、`atlas/` 为 MPL-2.0。来源：`gh api repos/tuist/tuist/contents/LICENSE.md`

## 4. 成熟度

- 仓库 2018-04 创建，5830 stars，最近推送 2026-10-14。来源：`gh api repos/tuist/tuist`
- 发布极频繁：稳定版 4.207.1（10-09）、4.211.1（10-11）、4.212.0（10-12），10-13 一天 11 个 4.214.0 canary。来源：`gh api repos/tuist/tuist/releases`
- 客户案例：Toss、Monzo（CI −70%）、monday.com（CI 减半）、Trendyol（构建 −65%）、Hyperconnect。来源：https://canary.tuist.dev/customers
- Xcode 26/27 问题（均有 issue，多数已关闭）：#7808 “Xcode 26 Beta 3 completely broken”、#9507 Xcode 26.4 beta 缓存失败、#11203 Xcode 27 beta 解析 SwiftPM 设置失败、#12740 Xcode 27 `tuist run` 起不了模拟器、#12846 Xcode 27 误报循环依赖；**仍开放**：#11916 Xcode 27 外部依赖部署目标错误。来源：`gh issue list -R tuist/tuist --search "Xcode 27 in:title"` 及 “Xcode 26”
- 风格：产品方向会快速收缩（vanilla Xcode 选择性测试上线 11 个月后被删），依赖它的功能需要跟版本。

## 5. 采用成本（针对 Krill）

1. 把 `project.yml` 手工改写为 `Tuist.swift` + `Project.swift`：官方迁移指南只讲概念对照，没有自动转换工具。来源：`server/priv/docs/en/guides/features/projects/adoption/migrate/xcodegen-project.md`
2. 迁移 `preGenCommand`、prebuild 脚本、`DebugUIAcceptance` 配置、多个自定义 scheme（KrillSubtitles、KrillPlayerControls、KrillFNOSUI）以及 KrillKit 包测试在 scheme 中的引用。
3. 想让 UI 测试真正被选择，需要把 `KrillUITests` 拆成多个 UI test target（每个都要编译、签名、拉起 app），这是一次结构性改动，且每个 target 仍依赖整个 app。
4. 跟随高频发布与 Xcode 大版本适配；团队里新增一个工具链依赖。
5. 收益上限：KrillKit 的 6 个测试 target 可跳过；UI 测试在不拆分时基本拿不到收益；模块缓存对单人本机的增量构建帮助有限（Xcode 本地增量已经缓存）。

## 6. 无需账号的替代方案

| 方案 | 做法 | 成本 | 来源 |
|---|---|---|---|
| SwiftPM 图 + XcodeGen spec + 自建映射 | `swift package dump-package`/`describe --type json` 得到 target→依赖；`project.yml` 得到 app→包产品；再维护一份“UI 测试类 → 涉及的模块/目录”映射，生成 `-only-testing:KrillUITests/<Class>` | 小（一个脚本 + 一份映射）；可做到**类级**，比 Tuist 更细 | 本机 `swift package dump-package` 输出（见 §0）；`-only-testing`/`-skip-testing` 是 xcodebuild 原生参数，Tuist 自己也是靠追加 `-skip-testing` 实现（2025 博客 “How it works”） |
| Xcode 26 原生编译缓存 | 直接开 `COMPILATION_CACHE_*` 设置，本地 CAS，不经 Tuist 服务器 | 很小；跨机共享才需要远端服务 | xcode-cache 页（“introduced in Xcode 26”，C/ObjC 与模块共享需 `COMPILATION_CACHE_REMOTE_SERVICE_PATH`） |
| Bazel + rules_apple | 精确的动作级缓存与测试选择 | 大：全部改写为 BUILD 文件，SwiftPM 依赖（KSPlayer、GRDB 等）要逐个接入；单人维护负担重 | https://github.com/bazelbuild/rules_apple （596 stars，2026-10-13 仍活跃） |

## 7. 结论与建议（单人开发、自研 Rust CLI kr-cli）

- **现在不采用 Tuist。** 原因：(1) 选择性测试只支持 Tuist 生成的工程，XcodeGen 路径已于 2026-01 删除；(2) 粒度只到 target，而 Krill 的 32 个 UI 场景在一个 target 里，最贵的部分拿不到收益；(3) 采用要先重写工程定义并拆 UI target。
- **借鉴其思路，在 kr-cli 中实现：**
  1. 哈希 = target 属性 + 文件内容 + 依赖哈希 + Swift 版本 + 构建配置（照搬 Tuist hashing 页的输入集合）。
  2. 成功后才落盘哈希；失败不写（Tuist `storeTestHashes` 同样只在成功后存）。
  3. 图来源：`swift package dump-package` + `project.yml`；UI 层再加“场景类 → 模块”映射，输出 `-only-testing`，做到类级选择。
  4. 提供 `kr hash` 类调试命令，对应 `tuist hash selective-testing`。
  5. 覆盖率任务始终跑全量（Tuist 文档明确两者不兼容）。
- **可选的小试点：** 只启用 Xcode 26 原生编译缓存（本地），不引入 Tuist 账号。
- **何时再评估 Tuist：** 若 Krill 拆成多个 Xcode 工程/多个 UI target，或需要 CI 跨机共享二进制缓存，且愿意迁移到 `Project.swift`。
