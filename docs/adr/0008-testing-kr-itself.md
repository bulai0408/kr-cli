# 0008 kr 自己怎么测

状态：已采纳（2026-10-14）

## 背景
kr 调用的 xcodebuild、simctl 很慢且依赖本机状态。如果 kr 的测试都真跑 Xcode，测一次几十分钟还经常因环境失败，开发 kr 本身会被它要解决的排队问题拖住。

## 决定
- 所有外部命令经过一个可替换的命令执行器。单元测试用假执行器，回放仓库里录制的真实输出（xcodebuild、simctl、xcresulttool 的 JSON），毫秒级、结果稳定。
- CLI 整体行为用 `assert_cmd` + `insta` 快照测试。
- 锁和队列用多个真实进程互相争抢来测（包括进程被杀后槽位自动释放）。
- 真正调用 Xcode 的集成测试默认不跑，用环境变量显式打开。
- CI 只跑 `cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo test`。
