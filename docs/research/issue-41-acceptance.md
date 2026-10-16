# #41 场景准备步骤：交付与离线前后对比

日期：2026-10-16。分支：`bulai0408/kr-p1-41`。

## 交付范围

- 调研提交 `34c03df`；ADR 0020 在 Orca 问题 `msg_2323d962b420` 中得到协调者确认，加入 owner 选择的共享实例与环境变量后实现。
- 实现提交 `1330082`：场景级 prepare、命令/目录/超时验证、后台就绪、共享生命周期、环境原因未验证、日志/事件/JSON 和文档。
- 开始实现前合入 `03c0c70`（#40）。交付前在 `740decc` 合入 `4610e54`（#30/#31）；最终元数据修正为 `5a3ef83`。README 的两个新增章节取并集；Scenario 同时保留 prepare 和 modules；run 测试以共同基线加双方新增测试合并。没有修改 setup、merge 或选测算法，也没有修改重跑规则。

## 机器判定

合入最新集成后：`cargo fmt --check` PASS、`cargo clippy --all-targets -- -D warnings` PASS、`cargo test` **395/395 PASS**。合入前同样三项通过，Rust 测试 **351/351 PASS**。零测试的 doc-test 目标没有计入通过数。主平台为本机 macOS。

#41 新增 17 个测试：14 个 kr CLI 回放测试和 3 个真实进程执行器测试；另扩充现有 stop 测试，验证收到 SIGTERM 后清理命令仍执行。完整名称：

```text
preparation_success_is_reported_and_cleaned_before_the_item_ends
preparation_failure_is_unverified_and_still_runs_cleanup
preparation_config_errors_name_the_file_and_key_path
preparation_background_waits_until_ready_and_cleans_up
preparation_shared_instance_is_started_once_for_two_scenarios
preparation_shared_failure_marks_all_users_unverified_without_retry
preparation_shared_mismatch_names_both_scenarios
preparation_readiness_timeout_is_unverified_and_cleaned
preparation_interruption_cleans_up_and_returns_unverified
preparation_cleanup_failure_keeps_test_evidence_but_is_unverified
preparation_invalid_fields_have_actionable_config_errors
preparation_context_variables_are_not_machine_config_keys
preparation_offline_comparison_separates_environment_from_product_failure
preparation_unused_shared_dependency_does_not_delay_cleanup_past_slot_release
dropping_a_background_service_stops_its_descendants_even_after_the_leader_exits
a_background_service_ignoring_term_is_killed_after_the_grace_period
a_cleanup_command_has_one_hard_deadline_even_if_it_ignores_term
```

两通道用例实际回放 iPad/iPhone 两条模拟器通道，只有一份服务 start/ready/cleanup 录制；重复启动会耗尽录制并失败。断言设备不同、仅一次共享清理、使用场景列表完整，且清理结束后才写最终 item_end。前一步失败、未使用共享服务的场景不会出现在实际使用者列表。中断用例分别发送 SIGINT 和 SIGTERM，退出码 130/143，断言未验证与清理结果、槽位已归还。

配置错误覆盖：缺少 ready、空命令、未知字段、非法/溢出时长、绝对或不存在的目录、非后台 ready、空共享名、同名共享定义不一致。错误为 miette 配置诊断，含文件、键路径和修复提示。保留现有断言；未放宽 skill 主文本少于 70 行的检查。

真实进程测试只启动自己的小进程组；组长先退出仍杀子进程，忽略 TERM 时在 5s 宽限后 KILL，清理命令超时没有第二个宽限期。没有借这些测试调用 Xcode。

## ADR 0010 前后对比

方法：第 3 层的判定规则离线回放，入口为 assert_cmd 启动真实 kr + KR_REPLAY；基于录制的 KrSample Xcode 输出，注入可控准备故障。不是调度模拟、不是完整门禁，也不是对历史业务故障的因果归类。

可复现命令：

```sh
cargo test -p kr --test run preparation_offline_comparison -- --nocapture
cargo test -p kr --test run preparation_ -- --nocapture
```

主对照用例在同一临时仓库执行两次，使用相同的已录制失败测试与既有历史特征。第一次不声明准备步骤，代表旧接口；第二次显式声明准备步骤并注入退出 17 的准备故障。

| 量 | 之前：没有准备声明 | 之后：准备先失败 |
|---|---:|---:|
| tests.failed | 1 | 0 |
| tests.unverified | 0 | 1 |
| 测试结果包 | 存在 | 未执行测试，null |
| 命令退出码 | 1 | 1 |
| 环境原因与准备日志 | 无 | 有 |
| 清理结果 | 无 | passed |

这不是声称旧录制里的断言真实由后端引起；环境故障是本次回放注入的。对照证明：有明确准备失败证据后，kr 不再运行并错误归类后续测试，同时仍保持非零退出，不能出现假绿。

其他精确断言：共享失败使两个场景都未验证且原因为同一字符串；就绪超时产生 unverified=1、failed=0；SIGINT/SIGTERM 保留各自退出码并清理；成功准备后通过；清理失败保留原始 cases，但最终未验证。期望与实测的计数偏差为 **0**。这是确定性回放的计数误差，不是耗时模型的校准误差；没有估计 MAPE，也没有声称性能提升或真实服务失败率。

RED 证据已观察：旧配置拒绝 prepare；普通失败重复标记未验证时计数归零；后台路径缺就绪结果；共享步骤被重复启动；成功测试后的清理失败被输出为 FAIL；子命令继承 KR_* 上下文时被误认成机器配置；未到达共享步骤的场景延迟清理至槽位释放后，以及被错误列入实际使用者。对应回归全部 GREEN。

## 第 4 层：小样本真实运行

本工作区 `target/kr-41-real` 从 `tests/sample` 复制，使用同样的 KrSample scheme、bundle ID 和 UI 测试。临时配置仅把 pool 的 project.name 改为 Krill，使其租已有的 `Krill-Verify-1/2`，不创建或擦除设备。命令由 kr 自己的资源层拿 build/sim 槽位，并通过机器已配置的 legacy 桥接排队拿 `/tmp/native-build.lock`；事件中两个阶段均有 machine_lock。没有直接运行 xcodebuild 或 simctl。

```toml
[[scenarios.settings.prepare]]
shared = "sample-backend"
run = "python3 service.py start"
background = true
ready = "python3 service.py ready"
timeout = "10s"
cleanup = "python3 service.py cleanup"
```

在临时样例根目录执行 `KR_RECORD=<工作区>/target/kr-41-record <工作区>/target/debug/kr --json run settings`。服务绑定 127.0.0.1:0，并在 KR_RUN_DIR 写端口/PID/run_id；ready 通过 HTTP 响应核对 run_id；cleanup 检查 PID 消失、端口关闭并删除状态文件。真实 start/ready/cleanup 录制已去除个人路径并保存到 `tests/fixtures/prepare`，用于 gate smoke 回放。

- Run ID：`01a14051-c48d-7384-84e1-0adce28da645`。
- Xcode 27.1 (27A9269)，iOS 18.6，已有池设备 `Krill-Verify-1`。
- `KrSampleUITests/SettingsUITests/testLaunchesTwice()`：**1/1 PASS**，用例耗时 7.989s；整次 kr 运行 59.316s。这是观测值，不是 A/B 性能结论。
- ready_attempts=1，preparation.status=ready，cleanup=passed。
- 清理日志：`service process reaped; port closed; state removed`。
- `sample-service-cleanup.json`：pid_gone=true、port_closed=true；状态文件已删除。
- 原始 JSON：`target/kr-41-real-result.json`；原始事件摘录：`target/kr-41-real-events.json`。
- xcresult：`target/kr-41-real/build/kr/results/settings/run.xcresult`。
- 准备/就绪/清理日志：`~/.local/state/kr/runs/01a14051-c48d-7384-84e1-0adce28da645/settings/`。
- 事件顺序：slot_acquired → machine_lock → sim_leased → preparation_start → preparation_end → sim_released → slot_released → item_end → run_end。

这次真跑发生在合入 #30/#31 前；合入后用包含选测与合并辅助的最终状态通过 395 项 Rust 检查，没有重复占用模拟器。

## 需要人判断手感

0 项。本票不改 App UI，不需要视觉或动画验收。

## 没有验证的部分

- 没有运行完整 Krill 门禁、真正业务后端、付费服务或生产凭证。
- 没有在真机或 SSH 远程主机上真实运行准备步骤；共享两设备通道由回放验证，真实小样本只用一个模拟器场景。
- 不捕获 kr 自身收到的 SIGKILL；后台服务必须不自行 daemonize/脱离进程组。
- 当前接口按已确认 ADR 只支持场景级 prepare；没有加入套件级准备或跨 run 的常驻共享服务。
- 本次没有估计耗时分布、并行服务的机器容量或实际业务环境故障率。

最终凭证模式扫描未命中；新增录制不含个人目录。扫描是有限模式检查，不宣称证明不存在所有种类的秘密。
