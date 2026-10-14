# 0009 过渡期桥接旧的全机锁

状态：已采纳（2026-10-14）

## 背景
ADR 0004 决定「不兼容旧的 `/tmp/native-build.lock`」。但在机器上所有项目都改用 kr 之前，Conflux 的脚本、Krill 的 `e2e.sh`（真实服务场景第一阶段还用它）以及按全局 AGENTS.md 手写锁的 agent 仍在用旧锁。kr 的 `build` / `sim` 槽位和旧锁互不相见，结果是 kr 和旧脚本可能同时跑重活；kr 的租约还会关机/开机 `Krill-Verify-N`，而 `e2e.sh` 可能正在用同一台。

## 决定
- 机器级配置 `[legacy] machine_lock`、`queue`（可选 `poll`，默认 30s）。默认不开；在仍有旧脚本的机器上开启，例如本机：

  ```toml
  [legacy]
  machine_lock = "/tmp/native-build.lock"
  queue = "/tmp/native-build.queue"
  ```

- 开启时，kr 每次持有 `build` 或 `sim` 槽位，都按全局规则同时拿旧锁：在队列目录建 `<epoch-ns>-<task>-<pid>`，排到最前才 `mkdir`；拿到后写 `owner`（task session pid 开始时间）；归还时删 owner、rmdir、删队列文件。只在 owner 进程不存在、或无 owner 且目录超过 1 分钟时才视为过期并清理。
- 同一个 kr 进程同时持有多个重活槽位时只拿一次旧锁（进程内计数，最后一个归还时释放）。
- 旧锁的 owner 是 kr 的祖先进程时（旧脚本持锁后调用 kr，对应旧规则的 `KRILL_NATIVE_LOCK_HELD=1`），视为已持有，不排队，否则会自己等自己。
- 等待时在 stderr 写明持锁者；拿到后记 `machine_lock` 事件（等了多久、等的是谁）。
- 崩溃：正常退出和 SIGINT/SIGTERM/SIGHUP 走 Drop 归还；kill -9 留下的锁由下一个使用者按「owner 进程不存在」清理。

## 后果
- ADR 0004 的「不兼容旧锁协议」在过渡期内不完全成立：开启桥接的机器上，kr 是旧协议的一个参与者。kr 自己的槽位仍是 `File::lock`，桥接只是额外多拿一把锁。
- 旧锁一次只允许一个重活，所以开启桥接时 kr 的 `build` / `sim` 并行度实际为 1。所有项目都改用 kr 后，删掉 `[legacy]` 配置即可移除桥接，不需要改代码。

## 参考
全局 `~/.codex/AGENTS.md`「Native work lock」；`docs/research/pitfalls-inventory.md` B2；Krill `scripts/e2e.sh` 的锁实现（只读）。
