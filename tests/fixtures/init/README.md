# init 的只读命令录制

2026-10-16，在 `tests/sample` 的临时副本生成工程后录制：

- `list.json`：`xcodebuild -list -json -project KrSample.xcodeproj`。
- `settings.json`：`xcodebuild -showBuildSettings -json -project KrSample.xcodeproj -scheme KrSample -configuration Debug build-for-testing`。
  `-showBuildSettings` 只输出设置，不构建。仅保留 target、PRODUCT_TYPE、SUPPORTED_PLATFORMS、
  PRODUCT_NAME、TARGET_NAME、CONFIGURATION、IPHONEOS_DEPLOYMENT_TARGET，删除个人路径等无关字段。
- `dump.json`：`xcodegen dump --type json --no-env`，来自同一份 sample。
- `package.json`：用不含个人信息的 SampleKit（一个普通 target、一个 test target）录制
  `swift package dump-package`，把 packageKind.root 路径替换为 `<project>`。

运行时使用已有 `simctl/list-runtimes.json` 真实录制。多个 scheme、多个 UI target、
workspace、macOS 与过高部署版本的分支，由 CLI 测试在真实录制基础上修改相应字段。
未提供的外部命令会被 ReplayRunner 拒绝，保证初始化不会偷偷构建或创建模拟器。
