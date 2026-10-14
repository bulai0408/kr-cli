# 0004 资源层：命名资源、槽位、跨项目先来后到

状态：已采纳（2026-10-14）

## 背景
原来只有一把全机锁，构建和模拟器测试都排一条队；4 个 agent 一批累计排队约 5 小时（krill-batch-review.md）。mkdir 锁要自己判断残留，进程崩溃后要靠超时清理。

## 决定
- 资源有名字和容量：`build`（重构建）、`sim`（模拟器）、`light`（轻量单模块测试）。容量来自机器档案。
- 每个槽位是一个文件锁（Rust 标准库 `File::lock` / `try_lock`），进程退出或崩溃时系统自动释放，不需要残留判断。
- 跨项目共享一个队列目录，先来后到；`kr queue` 能看到谁在用、谁在等、预计等多久（预估来自历史事件）。
- 一个任务构建完立即归还 `build` 槽位，再去拿 `sim` 槽位跑测试。
- 第一阶段不做常驻 daemon；需要公平调度或可视化时，再做按需启动、空闲退出的 daemon（sccache 的做法）。
- 不兼容旧的 `/tmp/native-build.lock`：kr 能用就用 kr，不能用就用旧脚本，不做两套协议互通。

## 参考
cargo-nextest test-groups（max-threads）、std `File::lock`（Rust 1.89）、sccache 与 buck2 daemon。
