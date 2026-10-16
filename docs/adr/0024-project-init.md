# 0024 用 kr init 生成最小项目配置

状态：已采纳（2026-10-16，#47，协调者授权写完直接实现）

## 决定

- `kr init` 在当前项目根目录创建 `.kr.toml`，已有文件退出 2，绝不覆盖，也没有 `--force`。
- 只读工程信息：普通 Xcode 工程和 workspace 用 `xcodebuild -list -json`、`-showBuildSettings -json`；
  XcodeGen 用 `xcodegen dump --type json --no-env` 读取解析后的 `project.yml`；
  SwiftPM 用 `swift package dump-package` 读取 `Package.swift`。
  所有命令经过 Runner，可录制、回放（ADR 0008）。init 不生成工程、不构建、不租模拟器。
- 生成 project、已安装运行时及兼容设备类型的单设备池、单元测试 suite、一个 UI smoke 场景和 default gate。
  SwiftPM 使用 macOS 命令套件运行 `swift test`；原生 macOS UI 测试用 macos 测试套件，
  不虚构 macOS 模拟器。没有测试的工程生成 build suite，不虚构测试。
  不把读取到的正式 bundle id 自动当成合成数据隔离 id；提醒调用方配置隔离再运行 UI 测试。
- 多个工程、App scheme、UI target、没有 UI 测试或缺运行时等不确定项使用稳定默认值，
  并按 ADR 0011、0015 的结构输出 `decisions`（id、kind、affects、question、item、options、evidence、default）。
  这是配置草稿，没有测试结论；成功写入并校验退出 0。判断点通过编辑 `.kr.toml` 解决，
  不交给仅接收已保存运行判断点的 `kr decide`。
- 写入前使用 kr-config 的相同 TOML 和语义校验，写入时排他创建，再以 `kr config` 的配置加载路径校验。
  缺运行时且需要模拟器时不给出虚构的运行时，退出并说明安装方法。
- `kr skill --reference config` 的说明与完整示例随二进制发布。测试从配置定义提取全部字段，
  对照说明并加载示例，新增字段缺说明即失败。说明包括项目配置和机器配置，明确两者边界。
- 不交互式提问：kr 由 agent 调用，交互提示会阻塞无人终端。结构化问题和证据交给调用方，
  保持离线可重放，沿用 ADR 0011 的计算层与判断层分工。

## 依据

ADR 0001、0008、0011、0015、0018；`docs/research/agent-tool-design.md`；
现有 `selection.rs` 的 XcodeGen 只读 dump 接口；`kr-config` 的配置定义和检查入口。
