# Bazel 适配调研：Krill 该不该用 Bazel，能借什么

日期：2026-10-14。只读调研。每条结论后附来源。

## 结论（先看这里）

- **不采用 Bazel。** 一人项目、约 6 个 SwiftPM 库 + 1 个 UI 测试 target，迁移和长期维护成本远大于收益；而且 Bazel 不能直接解决“多 worktree 共用一台 Mac”的核心痛点（见 1.4）。
- **借用它的概念**：用输入哈希做 key 的测试结果缓存、rdeps 式受影响测试选择、flaky 重试次数、size/timeout 分级、exclusive/资源标签、BEP 式事件流。都在 kr-cli 里用 Rust 实现即可（见第 4 节）。

## 1. 如果迁到 Bazel，能得到什么

### 1.1 缓存
- Action 结果按输入内容哈希缓存；`--disk_cache` 用一个本地目录当缓存，官方文档说它适合“在同一项目的多个 checkout 之间共享产物”，正对应多 worktree。https://bazel.build/remote/caching
- 测试结果缓存：`--cache_test_results=auto`（默认）时，只有测试或其依赖变了、测试标了 `external`、用了 `--runs_per_test`、或上次失败时才重跑。https://bazel.build/docs/user-manual
- 粒度是**测试 target**。Krill 现在 32 个 XCUITest 类都在一个 UI 测试 target 里，改任何依赖就整体失效；要拿到收益必须先把 UI 测试拆成多个 `ios_ui_test` target（推断，依据同上页的 per-target 缓存语义）。
- UI 测试依赖模拟器状态、网络、外部服务，通常要标 `external` 或关缓存，否则缓存会“记住”一次偶然的通过（推断，依据上页 `external` 规则）。

### 1.2 受影响测试选择
- `bazel query 'rdeps(//..., //KrillKit:Foo)'` 给出反向依赖；两个 commit 之间的受影响 target 用 target-determinator（bazel-contrib）或 bazel-diff：
  - https://github.com/bazel-contrib/target-determinator （“determine which Bazel targets changed between two git commits”）
  - https://registry.bazel.build/modules/bazel-diff/35.0.1/ （按图哈希比较，区分 direct / indirect impacted，并给出 target_distance）
- 同样受限于 target 粒度：图只到 target 级，不知道哪个 XCUITest 方法用到了哪段 SwiftUI 代码。

### 1.3 测试控制
- `--flaky_test_attempts`、`--runs_per_test`、`--test_timeout` 都是 `bazel test` 的标准选项。https://bazel.build/docs/user-manual
- `size`（small/medium/large/enormous）隐含 timeout：short 60s、moderate 300s、long 900s、eternal 3600s。https://bazel.build/reference/test-encyclopedia
- `tags = ["exclusive"]`：该测试串行运行，等所有构建和非 exclusive 测试结束后才跑，并禁用远程执行；`exclusive-if-local` 只在本地串行。https://bazel.build/reference/be/common-definitions
- `--local_resources` 限制本机 CPU/RAM 等资源，`resources:` 标签声明测试占用。https://bazel.build/docs/user-manual

### 1.4 能否替代机器级锁？——不能
- 客户端按 output base 找 server；output base 默认由 workspace 路径决定，“在多个 workspace 中构建，就会有多个 output base，也就是多个 Bazel server 进程”。https://bazel.build/run/client-server ；https://bazel.build/remote/output-directories
- 因此每个 git worktree = 一个独立 server。`exclusive`、`--local_resources`、`--jobs` 都只在**一个 server 内部**调度，不跨 worktree 协调（推断，依据上两页 + exclusive 定义）。多个 worktree 同时 `bazel test` 仍会同时起 xcodebuild 和模拟器。现有的 `/tmp/native-build.lock` 仍然需要。
- 唯一真正的跨 worktree 方案是把执行交给同一个远程执行集群（让调度器排队），但这对单机没有意义。

### 1.5 模拟器测试的实现细节
- rules_apple 默认 iOS 测试 runner 调用 xctestrunner 的 `simulator_test`，每次测试创建新模拟器；设置 `REUSE_GLOBAL_SIMULATOR` 才复用一个全局模拟器。https://github.com/bazelbuild/rules_apple/blob/master/apple/testing/default_runner/ios_test_runner.template.sh
- 第三方 runner rules_idb 的 README 称：默认 runner 用 `xcodebuild test-without-building`，内存占用大且增长快；每个 (device, OS) 复用一个模拟器，难以并发。https://registry.bazel.build/modules/rules_idb
- 结论：Bazel 不自带模拟器池、租约或数据隔离；Krill 的 `<App>-Verify-<N>` 池和锁逻辑仍要自己写。

### 1.6 远程缓存 / 远程执行
- Apple 目标的远程执行要求 macOS worker 池；MacStadium 文档示例为 BuildBuddy 或 bazel-buildfarm 做控制面，Orka macOS VM 做 worker。https://docs.macstadium.com/orka/orka-devops-integrations/bazel-rbe
- 远程构建需要用 `--xcode_config` 指向一个列出远端可用 Xcode 版本的 `xcode_config` target。https://bazel.build/docs/bazel-and-apple
- 商业选项：Namespace（https://namespace.so/docs/bazel/execution）、Bitrise（https://docs.bitrise.io/en/bitrise-build-cache/build-cache-for-bazel/remote-build-execution-for-bazel）。
- 编译 action 可以远程执行；模拟器测试需要 worker 上有模拟器，`exclusive` 测试直接禁用远程执行（见 1.3）。一人一台 Mac 时，远程缓存只有在第二台机器或 CI 出现后才有价值。

## 2. 2025–2026 年 Apple-on-Bazel 现状

- **rules_apple**：5.0.0（2026-09-10）起只支持 Bazel 8.x/9.x LTS，取消 WORKSPACE；5.0.0 添加 Xcode 27 DeviceHub.app 支持；最新 5.3.0（10-09）。https://github.com/bazelbuild/rules_apple/releases
- **rules_xcodeproj**：4.0.0 支持 Bazel 9 并取消 WORKSPACE；最新 4.1.0。https://github.com/MobileNativeFoundation/rules_xcodeproj/releases
- **SwiftUI Previews 是已知痛点**：Xcode 16 起 Previews 要求 `ENABLE_DEBUG_DYLIB=YES`，rules_xcodeproj 2024 年设为 NO（PR #3042）。修复在 draft PR #3325，2026-09-15 仍在征集外部测试，作者自报在 Xcode 26.5 可渲染。
  - https://github.com/MobileNativeFoundation/rules_xcodeproj/pull/3042
  - https://github.com/MobileNativeFoundation/rules_xcodeproj/issues/3201
  - 另有独立实验项目 rules_swift_previews 0.1.x：https://registry.bazel.build/modules/rules_swift_previews
- **SPM 依赖**：rules_swift_package_manager 可下载、构建、消费 Swift 包（替代 rules_spm）；资源等特性依赖 rules_apple；推荐 bzlmod。https://github.com/cgrindel/rules_swift_package_manager
- **Xcode 27 支持**：只在 rules_apple 5.0.0 的说明中看到 DeviceHub 一项，没有找到“完整支持 Xcode 27”的声明。规则集通常落后 Xcode 新版本，需要自己踩坑（推断）。
- **谁在用（来源为官方用户页或公司博客）**：
  - Lyft、Spotify、LinkedIn、Pinterest、Tinder、LINE、Tokopedia：https://bazel.build/community/users
  - Spotify：2020 年定为 iOS 官方构建系统，build+test P75 从 80 分钟降到约 20 分钟。https://engineering.atspotify.com/2023/10/switching-build-systems-seamlessly
  - Airbnb iOS 从 Buck 迁到 Bazel：https://www.infoq.com/news/2024/02/airbnb-bazel-migration-ios/
  - Lyft：Keith Smiley 领导多百万行 iOS 代码迁移，后加入 BuildBuddy：https://buildbuddy.io/blog/welcoming-keith-smiley
  - Uber/Snap 的 iOS Bazel 迁移没有找到一手公开来源（Uber 用户页条目指 Go monorepo）。
- **共同点**：全是几十到上百名 iOS 工程师、有专职 DevEx/构建团队的公司（推断，依据上述文章描述的团队与规模）。

## 3. 一人项目的成本 vs 收益

- **成本**：
  - 为每个 KrillKit 模块、App、3 个平台、测试写 BUILD 文件；
  - 用 rules_xcodeproj 生成工程替代 XcodeGen；
  - 跟随 Bazel 8→9、rules_apple 4→5 的破坏性升级（两者都在 2026 年取消 WORKSPACE，见上）；
  - 处理 Previews 失效；
  - 接受与 Xcode 原生构建分叉。Delivery Hero（2026-05）明确说 Bazel/Buck “typically requires diverging from Xcode's native build system”。https://deliveryhero.jobs/blog/scaling-ios-application-development-with-tuist/
- **同类评估**：Asana 评估后认为 Bazel “very heavyweight… requires new mental models”，且遇到断点等开发体验问题，最后选 Tuist。https://asana.com/nl/inside-asana/scaling-a-mature-ios-codebase-with-tuist.md
- **收益在本项目很小**：
  - 编译本身不是瓶颈，瓶颈是串行的模拟器 UI 测试；
  - 单 UI target 让测试缓存几乎不命中；
  - 跨 worktree 锁仍要自己做；
  - 没有第二台机器，远程缓存/执行无用武之地。
- **对照痛点**：
  | 痛点 | Bazel 能否解决 |
  |---|---|
  | 锁排队 | 否（1.4） |
  | 编译错误发现晚 | 部分：增量 + 缓存会更快，但 SwiftPM 本地包的增量编译已经接近 |
  | 受影响测试太粗 | 需先拆 target，且 SwiftPM 的 `swift package describe` / 依赖图同样能得到 rdeps |
  | merge 冲突 | 否，还会多出 BUILD 文件冲突 |
  | flaky | 只有重试，不提供诊断 |
  | 手工计时 | 是，BEP 和 `--profile` |

## 4. 判定与可借鉴概念（kr-cli，Rust 包装 xcodebuild）

**判定：不采用。** 半年内只有出现以下情况才重新评估：第二台 Mac 或 CI 需要共享缓存，且模块数大于 30。

可借鉴的概念：

1. **内容寻址的 action key**：对（源文件哈希 + project.yml + Package.resolved + Xcode 版本 + 目标 + destination）求哈希，作为构建产物和 `test-without-building` 的 xctestrun 缓存 key，多 worktree 共享一个目录（对应 `--disk_cache`）。https://bazel.build/remote/caching
2. **测试结果缓存**：key = 测试 bundle 哈希 + 场景名 + 模拟器 runtime；只缓存 PASS；标记 external 的场景（依赖 Emby/fnOS/WebDAV）永不缓存（对应 `--cache_test_results` 和 `external`）。https://bazel.build/docs/user-manual
3. **rdeps 查询**：从 `swift package dump-package` / XcodeGen 依赖构建模块图；改动文件 → 模块 → 反向依赖 → 场景映射表（场景声明所依赖的模块）。参考 target-determinator 的“两 commit 比较图哈希”做法。https://github.com/bazel-contrib/target-determinator
4. **flaky_test_attempts + runs_per_test**：失败场景单独重跑 N 次，记录 flaky 率；用 runs_per_test 主动探测 flaky。https://bazel.build/docs/user-manual
5. **size/timeout 分级**：每个场景声明 small/medium/large，映射到硬超时，超时就杀进程并保存 xcresult。https://bazel.build/reference/test-encyclopedia
6. **exclusive / resources 标签**：场景声明占用的资源（模拟器数、外部服务、端口），调度器按资源排队；跨 worktree 的这一层仍由机器级锁实现（Bazel 本身做不到，见 1.4）。https://bazel.build/reference/be/common-definitions
7. **BEP 式事件流**：每次运行输出结构化 NDJSON 事件（排队开始、拿锁、构建开始/结束、每个测试开始/结束、xcresult 路径），用来自动计时、生成报告，取代手工计时。https://bazel.build/remote/bep
8. **远程执行作为以后多 Mac 的故事**：事件和 key 设计好之后，可以把“拿锁 + 执行”换成“提交到另一台 Mac 的队列”；协议可参考 REAPI，但现在不实现。https://bazel.build/remote/caching
9. **密封性（hermeticity）的轻量版**：固定 Xcode 路径和版本（写进 key）、固定模拟器 runtime、每个测试启动时清空数据。这些是缓存可信的前提，不需要完整沙箱。https://bazel.build/docs/bazel-and-apple
