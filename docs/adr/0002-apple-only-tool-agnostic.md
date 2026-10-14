# 0002 只做 Apple 客户端项目；不依赖任何 agent 编排工具

状态：已采纳（2026-10-14）

## 决定
- 只支持 Apple 客户端项目（iOS / iPadOS / tvOS / macOS），底层调用 `xcodebuild`、`xcrun simctl`、`swift`、`git`。
- kr 是 owner 的开发流程执行层，与开发工具无关：不调用、不依赖 Orca、Codex、Claude 或任何编排工具。今天由 Orca 派出的 agent 调用 kr，明天换成别的工具也一样调用。
- 资源层和事件格式写成通用的，不绑定 Apple，方便以后扩展；但第一阶段不为扩展多写代码。
