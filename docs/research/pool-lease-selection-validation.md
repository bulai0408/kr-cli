# #43 指定池设备与 setup 换行验证

日期：2026-10-16。设计见 [ADR 0023](../adr/0023-pool-lease-selection.md)，已获协调者确认。

## 机器判定

- 首轮及同步 main 后均通过 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`。
- 两轮 Rust 测试均为 **422/422 PASS**，0 failed；受影响的 pool/setup/skill 测试合计 **19/19 PASS**。
- 开工前 `git fetch`，确认 `origin/main` 是 HEAD 的祖先。交付前再 fetch 并执行 `git merge origin/main`，结果为 Already up to date，main 为 `fd5e2e62e0ec5b6a934c440a21e822897feb6502`。
- `git diff --check` 通过，新增代码的凭证模式扫描未命中；未修改全局 agent 规则或机器配置。
- 原始完整开发日志保存在本机 `/tmp/kr43-evidence/`，首轮和最终日志分开，不覆盖。

## CLI 回归与前后对比

以下均由真实 `kr` 命令进程验证，外部 Apple 命令通过 `KR_REPLAY` 回放：

| 新增测试 | 断言 |
|---|---|
| `pool_lease_selects_a_free_device_by_name_or_udid` | 名称及 UDID 都能选中第二台，而非第一台 |
| `pool_lease_waits_for_the_selected_device_and_queue_names_it` | 两个进程竞争同一设备；容量 1 时等槽，容量 2 时等设备锁；等待期间不换空闲设备，释放后仍取得目标 |
| `pool_lease_rejects_devices_outside_the_project_pool_before_queueing` | 系统设备、其他项目名称、池外 UDID 和未知值均退出 2，提示 pool status，未取资源 |
| `pool_lease_selectors_are_mutually_exclusive` | name/udid/kind 三种选择器两两冲突，退出 2 |
| `pool_lease_json_notices_and_events_record_the_selection` | stderr JSON 和租用/释放事件携带 name/udid/kind 选择；子命令 stdout 和退出 3 透传 |
| `pool_lease_help_explains_exact_device_selection` | 保存 help 快照，说明互斥、等待和 JSON 通知位置 |
| `setup_adds_a_final_newline_once_and_leaves_unmodified_files_alone` | 安装补 LF，重复安装零写入且文件字节相同，卸载保留标记外字节，无块卸载不改文件 |

等待测试还检查名称、UDID、持有者 task，以及 `kr queue --json` 中的目标；覆盖人读与 JSON 等待提示。既有 setup 测试继续覆盖两个 agent、全局/项目临时目录、混合 CRLF、导入去重、override 和卸载；最终 LF 是逐字保留的唯一例外。

ADR 0010 第 1 层前后对比：

- 实现前，指定设备测试因 `unexpected argument '--name' found` 失败；实现后通过，命令打印所请求的第二台名称与 UDID。
- 未实现等待提示时，竞争测试等不到含 `current-owner` 的提示而失败；实现后两种容量都通过，释放后取得同一台。
- 未输出 JSON 租约通知时，测试收到的 `sim_leased` 通知数为 0；实现后为 1，且选择字段与事件一致。
- 修复前，setup 文件最后字节为 `>`（62），末尾 LF 断言失败；修复后最后字节为 LF（10），重复安装不变。

这是行为回放和真实进程锁竞争验证，无性能模型或模拟器耗时校准，不能推断原生启动时间或并发收益。

## 需要人判断手感

0 项；本票没有 UI 变化。

## 未验证

- 没有启动真实模拟器，没有 Apple 构建、真机或真实服务验证，也未启用默认关闭的原生集成路径；本票要求的三项 Rust 检查已完成。
- 等设备锁仍占 sim 槽，可能降低其他设备利用率；这是协调者接受的取锁顺序取舍。
- 未修改 Krill 的“租到别台退出 2”兼容分支，未安装新 kr 到全局；由协调者在 Krill 另行处理。
- 未合并 PR，等待协调者审查。
