# Apple 平台多 agent 并行开发：构建、测试选择与 UI 测试稳定性调研

日期：2026-10-14。本机环境：Xcode 27.1（27A9269）、Swift 6.4。
规则：每条结论后附来源。标 **[本机]** 的条目是在本机 Xcode 27.1 上直接查到的事实，并附同等内容的开源出处。标 **[推断]** 的条目是本文的推论，没有直接来源，使用前需要验证。

---

## 1. 构建一次，测试多次

### 1.1 build-for-testing 与 test-without-building

- `build-for-testing` 构建目标和测试，并在构建根目录生成 `.xctestrun` 文件；`test-without-building` 用 `-xctestrun` 直接运行已编译的测试包，不再编译。来源：https://keith.github.io/xcode-man-pages/xcodebuild.1.html
- 新的 `-testProductsPath` 把测试产物打成一个 `.xctestproducts` 归档：`build-for-testing` 写出归档，`test-without-building` 从归档读取。归档是独立文件，可以复制给别的进程或机器。来源：https://keith.github.io/xcode-man-pages/xcodebuild.1.html
- `-only-testing` / `-skip-testing` 可以和 `test-without-building` 一起用，所以“构建一次、按场景切片跑多次”在官方工具里就能做到。来源：https://keith.github.io/xcode-man-pages/xcodebuild.1.html
- `-enumerate-tests` 配合 `-test-enumeration-style flat -test-enumeration-format json` 输出可直接喂给 `-only-testing` 的测试 ID 列表，不运行测试。可用来在加锁前确认“这次会跑哪些测试、不是 0 个”。来源：https://keith.github.io/xcode-man-pages/xcodebuild.1.html
- Xcode 26 起，`build-for-testing` 生成的 `.xctestrun` 也包含被禁用的测试计划配置，可用 `-only-test-configuration <name>` 在 `test-without-building` 时选择性运行（例如单独跑一个“深色模式”配置）。来源：https://developer.apple.com/documentation/xcode-release-notes/xcode-26-release-notes
- 早在 Xcode 8 就支持把构建与测试分离；CircleCI 把 UI 测试拆到多个并行任务的做法就基于这两步。来源：https://circleci.com/blog/xcuitest-parallel-execution/

### 1.2 Xcode 编译缓存（Compilation Caching）

- Xcode 26 引入可选的编译缓存：检测到同一组源文件输入被重复编译时，直接复用之前的编译结果；对切换分支和 clean build 最有效。开关是构建设置 “Enable Compilation Caching” 或每用户的项目设置。来源：https://developer.apple.com/documentation/xcode-release-notes/xcode-26-release-notes
- 对应构建设置是 `COMPILATION_CACHE_ENABLE_CACHING = YES`；缓存存储默认在 `DerivedData` 下的 `CompilationCache.noindex`，可用 `COMPILATION_CACHE_CAS_PATH` 改到固定路径。来源：https://tuist.dev/en/docs/guides/features/cache/xcode-cache
- Apple 工程师（Steven Wu）在 Swift 论坛说明：该设置“设计上只用于本地缓存”；绝对路径导致的不命中，解决办法是用 prefix map 把路径映射到规范位置，设置为 `SWIFT_ENABLE_PREFIX_MAPPING=YES`（混编项目再加 `CLANG_ENABLE_PREFIX_MAPPING=YES`）；Xcode 26 有一个限制：module cache 目录在两台机器间不能不同。来源：https://forums.swift.org/t/about-swift-shared-cache-across-machines/81850
- **[本机]** Xcode 27.1 的 Swift 构建规格里还有 `SWIFT_ENABLE_PROJECT_PREFIX_MAPPING`：把 `$(PROJECT_DIR)` 映射为 `/^src`，`$(PROJECT_TEMP_DIR)`、`$(BUILT_PRODUCTS_DIR)` 等也映射为固定前缀；仅在编译缓存和 `SWIFT_ENABLE_PREFIX_MAPPING` 都开启时生效。开源出处：https://github.com/swiftlang/swift-build/blob/main/Sources/SWBUniversalPlatform/Specs/Swift.xcspec
- Tuist 文档提醒：Xcode 27+ 的 prefix mapping 会改变所有缓存键（开启后需要重新填充缓存），并且可能让覆盖率报告丢失源文件，除非在覆盖率构建里关掉它。来源：https://tuist.dev/en/docs/guides/features/cache/xcode-cache
- Bitrise 实例：源码每次恢复到不同临时目录时，缓存键包含绝对路径会导致几乎全部不命中；加 prefix mapping 后缓存键与检出路径无关。来源：https://discuss.bitrise.io/t/fixed-xcode-compilation-cache-misses-caused-by-changing-checkout-paths/26009
- 收益量级：Bitrise 在开源 iOS 项目上测得缓存生效后构建时间下降 19%–55%。来源：https://bitrise.io/blog/post/accelerate-ios-builds-with-xcode-26-compilation-caching-on-bitrise
- Xcode 26 起，Swift explicit modules 是所有 Swift target 的默认构建模式，可用 `SWIFT_ENABLE_EXPLICIT_MODULES=NO` 退出；编译缓存依赖 explicit module build（一位用户报告在纯 SwiftPM 场景出现 “-cache-compile-job cannot be used without explicit module build” 警告）。来源：https://developer.apple.com/documentation/xcode-release-notes/xcode-26-release-notes 、https://forums.swift.org/t/about-swift-shared-cache-across-machines/81850
- 对 worktree 的含义 **[推断]**：每个 worktree 路径不同，未开 project prefix mapping 时缓存键带路径、互不命中；开启 `COMPILATION_CACHE_ENABLE_CACHING` + `SWIFT_ENABLE_PREFIX_MAPPING` + `SWIFT_ENABLE_PROJECT_PREFIX_MAPPING`，并把 `COMPILATION_CACHE_CAS_PATH` 指向一个所有 worktree 共用的目录，理论上能让 A worktree 编过的模块在 B worktree 命中。需要用构建日志里的命中率（Tuist 文档提到可开 `COMPILATION_CACHE_ENABLE_DIAGNOSTIC_REMARKS`）实测。

### 1.3 DerivedData 与 worktree

- 多个构建同时写同一个构建数据库会报 “database is locked”。来源：https://discuss.bitrise.io/t/xcode-problem-with-database-locked-when-running-tests-on-2-targets-consecutively/21705 、https://bugs.webkit.org/show_bug.cgi?id=209890
- 2026 年 6 月有开发者在 Apple 论坛提出同样的 worktree 问题：DerivedData 冲突、两个 worktree 同时解析 SPM 依赖时全局缓存反复 fetch、同一模拟器上同一 bundle id 的 App 沙盒被另一个 worktree 覆盖。该帖目前没有回复。来源：https://developer.apple.com/forums/thread/833677
- **[推断]** 稳妥做法：每个 worktree 用独立 `-derivedDataPath`（不共享构建数据库），只共享内容寻址的 CAS 目录（`COMPILATION_CACHE_CAS_PATH`）和 SPM 依赖缓存（`swift build --cache-path` / `-clonedSourcePackagesDirPath`）。

### 1.4 SwiftPM 侧

- Apple 2025 年开源 Swift Build（即 Xcode 的构建引擎），目标是让 SwiftPM 与 Xcode 用同一个构建引擎。来源：https://swift.org/blog/the-next-chapter-in-swift-build-technologies/
- Swift 6.3 可选启用 Swift Build 集成；Swift 主分支已默认使用它。来源：https://swift.org/blog/whats-new-in-swift-march-2026/
- **[本机]** Swift 6.4 的 `swift build --help` 显示 `--build-system` 默认值已是 `swiftbuild`（native 标为 deprecated），并提供 `--explicit-target-dependency-import-check none|warn|error`，检查 target 是否只 import 了显式声明的依赖。开源出处：https://github.com/swiftlang/swift-package-manager

### 1.5 Tuist cache 与 selective testing

- Tuist 的 target 哈希输入：target 属性（名称、平台、产品类型等）、target 文件、所有依赖的哈希；缓存哈希还包含 `xcrun swift --version` 输出和所选构建配置的名称、构建设置、xcconfig 内容。来源：https://tuist.dev/en/docs/guides/features/projects/hashing
- Selective testing：`tuist test` 用与模块缓存相同的方式哈希项目，测试成功后保存哈希；下次只跑哈希变化的测试 target。粒度是 target 级，因为测试和源码之间的代码级依赖无法检测，所以建议 target 小而专注。来源：https://docs.tuist.dev/en/guides/develop/selective-testing
- 非 Tuist 生成的普通 Xcode 项目也可用：`tuist xcodebuild test -scheme App`；它计算所有模块哈希，并对未变化的模块加 `-skip-testing`；支持本地 package 模块化的项目。示例：改了 `Core`，则 `CoreTests`、`FeatureATests`、`FeatureBTests` 都跑；只改 `FeatureA`，只跑 `FeatureATests`。需要 Tuist 账号和项目。来源：https://tuist.dev/blog/2025/02/18/selective-testing-for-xcode-projects 、https://tuist.dev/yue_Hant/docs-markdown/guides/features/selective-testing/xcode-project
- 选择性运行时覆盖率数据可能不准。来源：https://docs.tuist.dev/en/guides/develop/selective-testing
- Tuist 测试分片：`tuist xcodebuild build-for-testing` 构建并读取 `.xctestrun` 里的测试模块，按近 30 天耗时生成分片计划，产物打成 `.xctestproducts`；粒度可选 module 或 suite（suite 模式最后一片是兜底，负责跑历史数据里没有的新 suite）。来源：https://tuist.dev/en/docs/guides/features/test-sharding/xcode

### 1.6 Bazel 的目标判定

- bazel-diff 在两个提交上分别为每个 target 计算 SHA256（包含 target 全部属性和输入，源文件按内容哈希），哈希不同的 target 即受影响集合。来源：https://github.com/Tinder/bazel-diff
- rules_apple 提供 `ios_unit_test`、`ios_ui_test` 等规则。来源：https://docs-legacy.aspect.build/bazelbuild/rules_apple/bb8eca42e44b6ed0cda9dcd70dcc20db5255b0de/doc/readme
- **[推断]** 对单人项目迁移 Bazel 成本过高；可借鉴的是它的思想：“target 哈希 = 自身输入 + 依赖哈希”，按图传播，不靠手写路径通配。

---

## 2. 加锁前的快速编译预检

- `xcodebuild -dry-run` 只打印构建命令，不执行，不能做类型检查；社区用它提取 `swiftc` 命令行。来源：https://forums.swift.org/t/how-can-i-do-a-dry-run-that-generates-headers-from-swift-for-objective-c/34594
- 只做类型检查：`swiftc -typecheck` 可行，但不会给出全部诊断（缺 return、未初始化变量等来自后续 SIL pass）；`-emit-sil` 覆盖这些，但未必更快；`-experimental-skip-non-inlinable-function-bodies` 让只生成模块很快，但会跳过函数体诊断。来源：https://forums.swift.org/t/how-to-do-type-check-only/73662
- 结论 **[推断]**：没有官方的 “cargo check”；实际最可靠的预检是“真编译但不跑测试”：对包内模块 `swift build --target <Module>`（macOS 主机，轻量）；对 App 用 `xcodebuild build-for-testing -destination 'generic/platform=iOS Simulator'`，这一步不需要模拟器（generic destination 不需要设备，见 https://keith.github.io/xcode-man-pages/xcodebuild.1.html ），开编译缓存后可以很便宜。
- 设置一致性：SE-0444 `MemberImportVisibility` 要求文件直接 import 定义成员的模块；Package.swift 用 `.enableUpcomingFeature("MemberImportVisibility")`。来源：https://www.hackingwithswift.com/swift/6.1/member-import-visiblity 、https://forums.swift.org/t/se-0444-member-import-visibility/74519
- **[本机]** Xcode 27.1 的对应构建设置名是 `SWIFT_UPCOMING_FEATURE_MEMBER_IMPORT_VISIBILITY`（见 Swift.xcspec）。Krill 当前 `project.yml` 和 `KrillKit/Package.swift` 都已开启，包模块用 `swift build` 预检时设置一致；但 `swift build` 在 macOS 主机编译，`#if os(iOS)` 分支和仅 iOS 的 API 不会被检查。开源出处：https://github.com/swiftlang/swift-build/blob/main/Sources/SWBUniversalPlatform/Specs/Swift.xcspec

---

## 3. 测试影响分析（不用 Tuist 时）

- `swift package describe --type json` 输出每个 target 的 name、path、type、target_dependencies，可以据此建反向依赖表；注意它会先做一次完整依赖解析（可能触发 fetch）。来源：https://forums.swift.org/t/showing-dependencies/15962 、https://bugs.swift.org/browse/SR-4554
- Xcode 构建日志会打印 “Target dependency graph” 段，也是依赖图来源之一；`xcodebuild -showBuildSettings` 可拿到每个 target 的源码根等设置。来源：https://keith.github.io/xcode-man-pages/xcodebuild.1.html
- 方法 **[推断]**：变更文件 → 所属 target（按 path 前缀）→ 沿反向依赖闭包 → 受影响 target 集合；UI 场景按“它导入/驱动了哪些模块”标注，而不是按目录通配。共享底层模块（Domain、UserState）被改时，全量回归是正确结果，不是误报——要降低成本应靠拆小 target，正如 Tuist 文档所建议。来源（粒度建议）：https://docs.tuist.dev/en/guides/develop/selective-testing

---

## 4. 模拟器并行

- `-parallel-testing-enabled`、`-parallel-testing-worker-count`（固定 runner 数）、`-maximum-parallel-testing-workers`（上限）；多个 `-destination` 时默认每个目的地跑全套，`-maximum-concurrent-test-simulator-destinations` 限制同时测试的模拟器数；`-parallelize-tests-among-destinations` 则把测试类分配到多个目的地而不是各跑全套。来源：https://keith.github.io/xcode-man-pages/xcodebuild.1.html
- 并行测试时 Xcode 自动克隆所选模拟器，并按“测试类”分发给各个 clone。来源：https://aztristian.medium.com/parallelizing-ui-tests-for-ios-applications-4343779c9181
- Apple 发布说明原话：“通过把一个 target 中的测试类分发到多个 runner 进程来并行”；同一个类的测试总在同一个模拟器上，`-only-testing` 传单个测试名也无法打破；Grab 用改名 + swizzle 让每个测试独立分组，解决了类粒度造成的不均衡。来源：https://engineering.grab.com/tackling-ui-test-execution-time-imbalance-for-xcode-parallel-testing
- 有报告称命令行下 UI 测试只用了一个 clone，或在 IDE 中均衡、在 `xcodebuild` 中不均衡。来源：https://developer.apple.com/forums/thread/733374 、https://developer.apple.com/forums/thread/710301
- iOS 不能在同一设备上同时运行同一 App 的多个实例，所以单设备内无法并行，必须多设备/多模拟器。来源：https://engineering.grab.com/tackling-ui-test-execution-time-imbalance-for-xcode-parallel-testing （引述 Apple 论坛回答）
- `xcrun simctl clone <device> <new name>` 可手工克隆设备，适合维护固定的模拟器池。**[本机]** `xcrun simctl help clone`。
- Swift Testing 默认在同一进程内用 task group 并行；`.serialized` 只串行化被标注的 suite/参数化测试，不影响与其他 suite 的相对执行；`swift test --no-parallel` 全局关闭。来源：https://developer.apple.com/documentation/testing/parallelization
- WWDC22：测试必须相互独立才能并行；PR 用一个小测试计划，全量放后台定时；“Retry on failure” 主要用于不可靠的外部服务，更好的办法是 mock；“Repeat until failure” 用于定位间歇性失败；“Test Timeouts” 默认 600 秒，防止单个测试卡死整套。来源：https://developer.apple.com/videos/play/wwdc2022/110361/

---

## 5. XCUITest 稳定性：针对五类问题

### 5.1 键盘遮挡输入框
- Apple 工程师：UI 测试像用户一样发事件；键盘挡住的控件用户点不到，测试也点不到，这是设计如此。来源：https://developer.apple.com/forums/thread/16088
- 做法：先收起键盘（点 Return/Done 或中性区域）再点下一个控件；App 侧保证内容随键盘滚动（放在 ScrollView/Form 中）。来源：https://developer.apple.com/forums/thread/16088 、https://codemia.io/knowledge-hub/path/iphone_keyboard_covers_uitextfield

### 5.2 懒加载行需要滚动
- `LazyVStack` 在需要渲染到屏幕前不会创建子项，所以屏外的行在无障碍树中根本不存在，`waitForExistence` 等不到。来源：https://developer.apple.com/documentation/swiftui/lazyvstack
- 常见模式：循环 `swipeUp()` 直到目标 `isHittable`，设上限次数。来源：https://developer.apple.com/forums/thread/16088
- **[推断]** 更稳的是从源头避免：用 `-ui-testing-*` 启动参数注入少量数据，让目标行在首屏；或用深链接直接打开目标详情。

### 5.3 isHittable 不稳定
- 社区报告：`isHittable` 在无法确定命中点时可能直接报错（“Activation point invalid and no suggested hit points”）而非返回 false（本条来自搜索摘要，未找到原帖链接，需复核）；`exists` 为真不代表可点，元素被遮挡或在屏外时不可点。来源：https://developer.apple.com/forums/thread/16088
- Xcode 16 新增 `waitForNonExistence(withTimeout:)` 与 `wait(for:toEqual:timeout:)`（等待某个属性变为期望值）。来源：https://www.jessesquires.com/blog/2024/07/09/uitest-improvements-in-xcode-16/
- WWDC25：用 `waitForExistence` / `wait(for:toEqual:)` 配合 XCTAssert；本地化文本和动态内容优先用 accessibility identifier；查询越短越抗变化。来源：https://developer.apple.com/videos/play/wwdc2025/344/

### 5.4 自定义 Menu 造成重复无障碍节点
- `accessibilityRepresentation` 用一个隐藏、不可交互的视图生成无障碍元素，替换原有元素；Apple 建议优先用自定义 Style（如自定义 `ToggleStyle` 自动继承 Toggle 的无障碍特性），实在不行再用它。来源：https://developer.apple.com/documentation/swiftui/view/accessibilityrepresentation(representation:)
- `accessibilityElement(children:)` 新建或修改元素的子元素行为（`.combine`/`.ignore`/`.contain`）。来源：https://developer.apple.com/documentation/swiftui/view-accessibility
- Apple 论坛回答：Button 默认合并子元素，无法“取消合并”；`accessibilityElement(children:)` 一般不影响 XCUITest 能否查到内部元素。来源：https://developer.apple.com/forums/thread/725008
- **[推断]** 若自定义 Menu 的 label 与内部 Button 都带同一个 identifier，会出现两个同名节点；identifier 只放在一个元素上（外层 Menu），并按 WWDC25 建议让 identifier 对实例唯一。来源（唯一性建议）：https://developer.apple.com/videos/play/wwdc2025/344/

### 5.5 深色模式启动参数不生效
- WWDC25 示例在 `setUp()` 中设置 `XCUIDevice.shared.appearance`、方向、位置，使每次运行从相同状态开始。来源：https://developer.apple.com/videos/play/wwdc2025/344/
- 但 2025–2026 有报告：Xcode 26.2 / iOS 26.2 上，`XCUIDevice.shared.appearance = .dark` 从 Xcode IDE 运行有效，用 `xcodebuild test` 运行被忽略，无任何警告；目前无 Apple 回复。来源：https://developer.apple.com/forums/thread/812656
- 启动参数 `-AppleInterfaceStyle Dark` 在系统为浅色时可以强制深色，但系统为深色时设 Light 无效。来源：https://developer.apple.com/forums/thread/726559 、https://developer.apple.com/forums/thread/687260
- **[推断]** 最稳的是 App 内自己处理：读取 `-ui-testing-appearance dark` 之类的启动参数，在根视图上 `.preferredColorScheme(.dark)`，并用一个可断言的 identifier/value 暴露当前外观，让测试断言“外观已应用”，而不是依赖系统设置。

---

## 6. Localizable.xcstrings 合并冲突

- 冲突原因：两个分支新增不同 key，插入位置落在同一个巨大 JSON 对象的同一区域，key 不冲突，但行冲突。来源：https://github.com/manaflow-ai/cmux/issues/15415
- cmux 的自定义 merge driver：按 key 三方合并而不是按行；两侧把同一 key 改成不同值、或一侧删除另一侧修改的 key 时，退出非 0，交还 git 写冲突标记；真实冲突案例从 42 处降为 0。`.gitattributes` 指定驱动，但驱动必须在每个 clone 的 git config 里注册，否则 git 静默忽略。来源：https://github.com/manaflow-ai/cmux/pull/13692
- 局限：GitHub 服务端不运行仓库自定义 merge driver，PR 页面仍显示冲突；后续 PR 修复了“冲突结果必须是合法 JSON”和格式保持问题。来源：https://github.com/manaflow-ai/cmux/issues/15415 、https://github.com/manaflow-ai/cmux/pull/13715 、https://github.com/manaflow-ai/cmux/pull/16071
- 有指南建议 `*.xcstrings merge=union`；它只适用于新增不同 key，同 key 改值会出问题，且 union 对 JSON 可能产生非法结构，合并后必须用 `jq`/`python3 -m json.tool` 校验，因为 Xcode 会静默加载失败。来源：https://simplelocalize.io/blog/posts/xcstrings-string-catalog-guide/
- `extractionState` 为 `stale` 的条目表示源码中已删除或改名，建议 CI 检查。来源：https://simplelocalize.io/blog/posts/xcstrings-string-catalog-guide/
- **[本机]** Xcode 自带 `xcrun xcstringstool`，子命令有 `print`、`sync`、`extract`、`compile`、`generate-symbols`；可在合并后用 `sync`/`print` 校验，或在 merge driver 中重新规范化。Xcode 26 发布说明也提到 String Catalog 字段变化会把文件版本自动升到 1.1。来源：https://developer.apple.com/documentation/xcode-release-notes/xcode-26-release-notes

---

## 7. 建议借鉴（痛点 → 具体做法）

| 痛点 | 做法 |
|---|---|
| agent 在锁队列里等几个小时 | 把“重活”拆成两段：编译（`build-for-testing` 产出 `.xctestproducts`）和运行（`test-without-building -testProductsPath`）。编译用编译缓存后变短；运行段可按场景切片排队，多个 agent 的产物轮流上模拟器，锁只覆盖真正需要模拟器的时间。依据 §1.1、§1.2 |
| 拿到锁才发现编译失败 | 入队前强制“无模拟器预检”：包模块 `swift build --target X --explicit-target-dependency-import-check error`；App 用 `xcodebuild build-for-testing -destination 'generic/platform=iOS Simulator'`（不占模拟器，可放到轻量锁或单独的编译锁）。预检失败不许入队。依据 §2 |
| 各 worktree 重复全量编译 | 每个 worktree 独立 `-derivedDataPath`；共用 `COMPILATION_CACHE_CAS_PATH`；开 `COMPILATION_CACHE_ENABLE_CACHING`、`SWIFT_ENABLE_PREFIX_MAPPING`、`SWIFT_ENABLE_PROJECT_PREFIX_MAPPING`（写进 `project.yml`）；用诊断 remark 实测跨 worktree 命中率后再定。依据 §1.2、§1.3 |
| 路径→场景通配表导致几乎每次 30 个场景 | 用 `swift package describe --type json` + project.yml 生成模块反向依赖图，按“变更模块的反向闭包”选测试；UI 场景标注它覆盖的功能模块而非目录。若接受外部账号，可直接试 `tuist xcodebuild test` 的 selective testing。共享底层模块变更仍全量，靠拆小 target 降成本。依据 §1.5、§3 |
| 30 个场景约 45 分钟 | 一次 `build-for-testing` 后在 2 台池内模拟器上用 `-parallel-testing-worker-count 2` 或两路 `-only-testing` 分片并行；把场景拆成小测试类（Xcode 按类分发）；按历史耗时均衡分片（参考 Tuist/Grab）。依据 §1.5、§4 |
| 键盘遮挡 | 测试辅助函数：输入后先收键盘再点下一个；App 侧表单放在可滚动容器。依据 §5.1 |
| 懒加载行需滚动 | 用启动参数注入小数据集或深链接直达；必要时有上限的 `swipeUp` 循环。依据 §5.2 |
| isHittable 抖动 | 改为 `waitForExistence` + `wait(for:toEqual:)` 断言状态；只在必要时检查 hittable。依据 §5.3 |
| 自定义 Menu 重复节点 | identifier 只挂在一个元素上；自定义控件优先用 Style，否则 `accessibilityRepresentation`。依据 §5.4 |
| 深色模式参数不生效 | App 读 `-ui-testing-*` 参数后 `.preferredColorScheme`，并暴露可断言的外观值；或在测试计划里建深色配置用 `-only-test-configuration` 运行。依据 §5.5、§1.1 |
| xcstrings 合并冲突 | 引入按 key 合并的 merge driver（参考 cmux），由仓库脚本为每个 worktree 注册；合并后用 `jq` 与 `xcstringstool` 校验；不要用 `merge=union`。依据 §6 |
| 测试文件合并冲突 | **[推断]** 每个场景一个文件/一个测试类，共享辅助代码放独立文件；同时也有利于 Xcode 按类并行。 |
