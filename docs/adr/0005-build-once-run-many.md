# 0005 构建一次、多次运行；结果必须可信

状态：已采纳（2026-10-14）

## 背景
旧做法每个场景各自构建；出现过 4 种「假通过」：过滤一条都没匹配、退出码被吞、smoke 只跑了 5/10、单例过滤被忽略（pitfalls T5、T6、R2）。失败后 Xcode 收集诊断最多占锁 10 分钟。

## 决定
- `build-for-testing` 产出测试产物，按输入哈希（源码、工程配置、Xcode 版本、目标平台）缓存；同一份代码只构建一次。
- 运行用 `test-without-building -only-testing`，按历史耗时把场景分到可用的模拟器上。
- 运行前用 `-enumerate-tests` 确认会跑到测试；运行后从 xcresult 核对实际测试数并读取真实退出码。0 条测试一律判失败。
- 默认加 `-collect-test-diagnostics never`、`-skipMacroValidation`。
- 每个测试的结果只有四种：通过、失败、偶发、跳过。失败的测试自动单独重跑一次：重跑通过记为「偶发」，仍然失败记为「失败」。
- 只缓存「通过」，不缓存失败；依赖外部服务的场景不缓存。
- 人读输出保持现有约定：`PASS:` / `FAIL:`、最多 3 行错误摘要、xcresult 路径；`--json` 给 agent。

## 参考
cargo-nextest archive / retries / flaky-result、Bazel 测试缓存（只缓存通过）、Tuist selective testing 的哈希输入、Apple `xcodebuild` 文档（apple-parallel-dev.md）。

## 补充（2026-10-14）：预检用符号核对，运行时用真实枚举

在样例工程（`tests/sample`，Xcode 27.1）上实测：
- `test-without-building -enumerate-tests` 配 `generic/platform=iOS Simulator` 不列测试，只报「Tests must be run on a concrete device」。
- 配具体模拟器能列出（JSON，enabled/disabled 分开），但 xcodebuild 会自己把模拟器开机再关机，约 44 秒。
- 过滤写错（例如 `-only-testing:KrSampleUITests/NoSuchTests`）时 xcodebuild 不报错，直接忽略。

所以：
- `kr check` 不占模拟器（spec §5），用构建好的测试 bundle 的符号（`nm -U | swift-demangle`，XCTest 和 Swift Testing 都可见）逐条核对 `only` / `skip`，不命中就指到 `.kr.toml` 的行列。
- `kr run` 在已租到、已开机的模拟器上做真实枚举，以它为准；与符号核对不一致时记事件并提示，不判失败。
- 无论哪种方式，都由 kr 逐条核对过滤是否命中，不依赖 xcodebuild 报错。
