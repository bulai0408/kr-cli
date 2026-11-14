# Xcode 编译缓存能否在多个 worktree 之间共用（调研，2026-10-15）

#37 的资料和排查记录。结论和数据见 ADR 0013。每条结论后附来源：网址、源码位置（读的版本：swift-build @89af69d、swift-driver @6a0da30、swift @79e04da、llvm-project `next` @da6e6e4）或本机命令。

## 1. 设置

| 设置 | 作用 | 来源 |
|---|---|---|
| `COMPILATION_CACHE_ENABLE_CACHING` | 打开编译缓存：同样的输入复用上次的编译结果 | Apple Build Settings Reference：https://developer.apple.com/documentation/xcode/build-settings-reference ；Xcode 26 发布说明（Swift 与 C 系语言可选开启，适合切分支和干净构建） |
| `COMPILATION_CACHE_CAS_PATH` | 缓存目录。默认 `$(DERIVED_DATA_DIR)/CompilationCache.noindex`，即每个 DerivedData 一份，默认不共用 | swift-build `Sources/SWBCore/Settings/Settings.swift`；本机 Xcode 27.1 的 SWBCore 字符串 |
| `COMPILATION_CACHE_LIMIT_SIZE` / `_PERCENT` | 缓存大小上限，默认可用磁盘的 50% | swift-build `CASOptions.swift` |
| `COMPILATION_CACHE_ENABLE_DIAGNOSTIC_REMARKS` | 每个编译任务写出命中或未命中 | Build Settings Reference |
| `SWIFT_ENABLE_PREFIX_MAPPING` / `CLANG_ENABLE_PREFIX_MAPPING` | 把 SDK、工具链、Xcode 路径换成固定前缀 | swift-build `Sources/SWBUniversalPlatform/Specs/Swift.xcspec` 1512–1551 行；`Clang.xcspec` 3391–3436 行 |
| `SWIFT_ENABLE_PROJECT_PREFIX_MAPPING` / `CLANG_…` | 把 `PROJECT_DIR`→`/^src`、`PROJECT_TEMP_DIR`→`/^derived`、`BUILT_PRODUCTS_DIR`→`/^built` 等换成固定前缀 | 同上 |
| `SWIFT_OTHER_PREFIX_MAPPINGS` / `CLANG_OTHER_PREFIX_MAPPINGS` | 追加任意 `旧前缀=新前缀` | 同上 |

前缀映射默认不开（xcspec 里没有默认值，`CompilationCachingInfo.swift` 把它单列）。Apple 文档没有写跨目录共用缓存。唯一的一手说明来自 Apple 的 Steven Wu（Swift 论坛 https://forums.swift.org/t/about-swift-shared-cache-across-machines/81850）：要开 `SWIFT_/CLANG_ENABLE_PREFIX_MAPPING`；模块缓存目录要相同；宏可能导致误判为未命中。Tuist 的 Xcode cache 文档（https://tuist.dev/en/docs/guides/features/cache/xcode-cache）也只要求打开这四个前缀映射设置。

## 2. 缓存键里有什么

- **Swift**：缓存键包含前端的全部参数，只排除标为 `CacheInvariant` 的参数（swift `lib/Frontend/CompileJobCacheKey.cpp` 173–177 行）。`-index-store-path`、`-index-unit-output-path`、`-o`、`-module-cache-path`、`-serialize-diagnostics-path` 不在键里（`include/swift/Option/Options.td` 1839、1847 行）。`-file-compilation-dir` 在键里（同文件 1323 行）。
- **swift-driver**：路径在某个映射前缀之下才会被换掉，否则原样进键（`Sources/SwiftDriver/Jobs/FrontendJobHelpers.swift` 1259–1287 行）。带 `-g` 时总会加 `-file-compilation-dir <工作目录>`（460–469 行）。映射按前缀长度取最长的（`Driver.swift` 354–356 行）。
- **Clang**：键里没有索引路径、输出路径、依赖文件路径和前缀映射本身（llvm-project `clang/lib/Frontend/CompileJobCacheKey.cpp` 81–109、197–215 行）。
- **swift-build**：`-working-directory` 来自 `COMPILER_WORKING_DIRECTORY`，为空时用默认值（`SwiftCompiler.swift` 1005–1007、1869 行）。边编译边建索引不会关掉缓存（1065–1071 行）。

## 3. 本机排查：为什么第一次测出 0 命中

方法：两个目录各编一次，同一份缓存；用 `llvm-cas --ls-node-refs`、`--ls-tree-recursive`、`--cat-blob` 取出缓存键里存的命令行，直接对比两次的差别（Xcode 27.1 自带 `llvm-cas`）。

1. **`/tmp` 是指向 `/private/tmp` 的符号链接。** Xcode 把 `PROJECT_DIR` 写成 `/tmp/…`，SwiftPM 给的源文件是真实路径 `/private/tmp/…`，`^src` 映射永远对不上，键里留着各自的绝对路径。最小验证（ZIPFoundation 0.9.20 单独一个包，两个目录）：放在 `/tmp` 下 Swift 编译 0/18 命中；放在 `/Users` 下 18/18。同理 `/var`（`/private/var`）下也不行。
2. **SwiftPM 包 target 的 `-file-compilation-dir` 没有被映射。** 在 Krill 里，包 target 的编译工作目录是 `<worktree>/Krill.xcodeproj`，而包 target 的 `PROJECT_DIR` 是包的检出目录，项目映射盖不到它。两个 worktree 的键只差这一项（`llvm-cas` 对比，第 240 行）。一个参数不同，这个 target 所有 Swift 任务的键都不同；依赖它的模块再一层层不同。加 `SWIFT_OTHER_PREFIX_MAPPINGS` / `CLANG_OTHER_PREFIX_MAPPINGS` = `<worktree 根>=/^worktree` 后，下一个提交在另一个 worktree 里 Swift 编译命中 691/867。

排除的原因：
- 关掉索引（`COMPILER_INDEX_STORE_ENABLE=NO`）没有用。键里本来就没有索引路径。
- 只映射 DerivedData 没有用。
- 同一个目录重编（删掉 DerivedData，保留缓存）命中 808/867，说明缓存本身可用，问题只在路径。

## 4. `XCODE_XCCONFIG_FILE`：把设置交给别人发起的 xcodebuild（#80，2026-11-14）

`kr lock build -- <命令>` 里的 xcodebuild 由调用方的脚本发起，kr 改不了它的参数，只能经环境变量把设置传下去。

- **文档**：`man xcodebuild`（Xcode 27.1）的 ENVIRONMENT 一节：`XCODE_XCCONFIG_FILE` 指向的文件「used when building all targets」，并且「override all other settings, including settings passed individually on the command line, and those in the file passed with the -xcconfig option」。
- **实测**（Xcode 27.1，27A9275）：一个 XcodeGen 工程，一个 macOS 命令行 target 依赖本地 SwiftPM 包 `Lib`。工程级写 `KR_P_PROJECT=project`、`KR_P_CLI=project`、`COMPILATION_CACHE_ENABLE_CACHING=NO`；target 级写 `KR_P_TARGET=target`、`SWIFT_ACTIVE_COMPILATION_CONDITIONS=DEBUG`；xcconfig 把三个 `KR_P_*` 都写成 `xcconfig`，并写 `SWIFT_ACTIVE_COMPILATION_CONDITIONS = $(inherited) KR_PROBE_XCCONFIG`；命令行再传 `KR_P_CLI=cli`。
  - `-showBuildSettings`：三个 `KR_P_*` 都是 `xcconfig`，命令行的 `cli` 被盖掉；`SWIFT_ACTIVE_COMPILATION_CONDITIONS = DEBUG KR_PROBE_XCCONFIG`，即 `$(inherited)` 取到 target 级的值。与文档一致：优先级是 环境变量 xcconfig > 命令行 > target > 工程。
  - **包 target 也生效**：包源码里写 `#if !KR_PROBE_XCCONFIG #error(…) #endif`，app 源码里写同样的检查；`xcodebuild build` 通过，说明两类 target 都拿到了这个条件。
  - **缓存设置对包 target 生效**：xcconfig 换成 ADR 0013 的全部缓存设置（含 worktree 根映射），两个目录各编一次、共用一个缓存目录，各用全新 DerivedData。第一个目录 Swift 编译 0/18 命中、模块 0/8；第二个目录 18/18、8/8，其中包括包 `Lib` 的 6 个源文件。工程级 `COMPILATION_CACHE_ENABLE_CACHING=NO` 也被盖掉。
- **推论**（没有单独验证）：调用方在命令行或 `-xcconfig` 里给同名设置时，以 kr 的值为准。缓存设置不改编出的产物（ADR 0013 第 4 条），所以盖掉它们只影响要不要走缓存；这也是 kr 不在调用方已设 `XCODE_XCCONFIG_FILE` 时去合并它的原因：那个文件里可能有任何设置，kr 无法判断哪些能盖。

结论：用 `XCODE_XCCONFIG_FILE` 就够了，不需要包一层 xcodebuild 或改调用方的命令行。

## 5. 没有查的
- Bazel rules_swift、XCRemoteCache 怎样让路径与目录无关：没有读它们的源码。
- 宏插件：swift-build 用 `-Xfrontend -load-plugin-executable <真实路径>` 传宏插件，driver 不映射它。修复见 https://github.com/swiftlang/swift-build/pull/1836（2026-10-07 合入），Xcode 27.1 可能还没有。Krill 有宏（swift-syntax），但实测命中率已经够用，没有单独查。
- 调试：映射后调试信息里的路径是 `/^worktree/…`。在 Xcode 里调试这样编出的产物，可能要给 LLDB 配源码映射。kr 编的是测试产物，没有验证。
