# Krill #125 运行的离线重放（kr-cli #25）

日期：2026-10-15。按 ADR 0010，用第 3 层「判定重放」检查 #25 的四项修复，不跑模拟器。本文第 2 节以后由 `krill-125-replay.py` 生成，没有手工改数字。

## 结论

- **结论不丢**：这次运行的 9 项在事件里都有结果，旧 kr 一项都没打印。根因是 default 的第二条通道排了 `sim` 队却没拿到槽位（第 1 节）：同一进程两条通道在同一微秒排队，票据文件重名，第二张票 rename 报 `No such file or directory (os error 2)`；这个错误等所有通道结束后才抛出，丢掉了全部结论。修复后票据不再重名；即使某条通道或某一项出错，其余各项照常给结论，总结行是 `run: 7/9 PASS`。今天重判时结果包已随工作区删除，每项都写明缺的文件路径（第 2 节）。
- **Swift Testing 重跑**：`testLocalBackendKeepsPackageEncryptedAndReceiptCanWithdraw()` 是名字以 `test` 开头的 `@Test` 函数。旧 kr 按名字当成 XCTest 去掉了 `()`，两次重跑都只用了 1–2 秒、命中 0 条（#24 的证据：totalTestCount=0）。修复后按 xcresult 的 `nodeIdentifierURL` 判断框架，保留 `()`；重跑前先枚举，命中 0 条时报 `rerun filter invalid`，不再空跑（第 3 节）。
- **SIGTERM、产物中途被删**：这两项没有保存的运行目录（#23 只有现场记录，#22 只有 `batch7-gate/postmerge.log`），由回放测试验证（见下）。

## 方法和误差

- 层级：第 3 层判定重放。输入是 #34 保存的这次运行的事件和运行目录（`tests/fixtures/deliver/krill-125`，已去掉个人路径），以及本机完整运行目录里的重跑日志和 Krill 源码（用来确认测试框架）。
- 第 2 节的「运行时结论」直接取 `item_end` 事件；修复后的 `kr run` 用的就是这些结果，只是不再被通道错误丢掉。「今天重判」按修复后的规则：结果包读不出时判失败并写明路径。
- 第 3 节的过滤由同一条规则算出（Swift Testing 保留括号和参数标签，XCTest 去掉 `()`）；框架从 Krill 源码的 `@Test` 读出，真实运行时 kr 从 xcresult 的 URL 读出，两者对这几个测试一致。
- 误差：判定重放是确定的计算，同一份证据结论不变，校准误差为 0。能下结论的范围只到「结论和过滤怎么算」；修复后的重跑是否真的通过、多花多少时间（每次重跑多一次枚举，约几秒）没有实测。
- 复现：`python3 -I docs/research/krill-125-replay.py . [<本机运行目录>] [<Krill 源码目录>]`。不给运行目录时用仓库里的副本（没有重跑日志，第 3 节没有耗时）。

## 回放测试（修复前失败、修复后通过）

| 问题 | 测试 |
|---|---|
| 结论不丢 | `kr-resource` `lanes_of_one_process_joining_the_queue_at_once_each_get_a_ticket`（修复前报同样的 os error 2）；`an_item_that_errors_fails_alone_and_the_others_still_get_a_verdict`；`a_missing_result_bundle_fails_the_item_and_names_the_bundle`；`every_run_ends_with_a_run_end_event_even_when_kr_itself_fails` |
| Swift Testing 重跑 | `a_failed_swift_testing_test_named_test_is_rerun_with_its_parentheses`；`a_rerun_filter_that_selects_no_test_is_reported_and_not_run`；`kr-xcode` `a_swift_testing_function_named_test_keeps_its_parentheses` 等 |
| SIGTERM | `a_sigterm_stops_the_run_like_sigint_and_starts_no_new_item`（回放带 10 秒耗时）；`kr-exec` `a_sigterm_stops_a_replayed_command_and_every_later_one` |
| 产物中途被删 | `test_products_deleted_while_waiting_for_a_simulator_are_rebuilt_once`；`test_products_still_missing_after_the_rebuild_fail_the_item_with_the_next_step` |

## 1 通道：谁排了 sim 队却没拿到槽位

| 通道 | 排队次数 | 拿到槽位次数 |
|---|---|---|
| run default lane | 2 | 1 |
| run iPad lane | 1 | 1 |

## 2 每项结论

修复前：kr 打印 `error: No such file or directory (os error 2)` 后退出，0 项结论。

修复后：每项照常给结论（来自运行时保存的结果）；今天重判时结果包已随工作区删除，
该项判失败并写明缺的文件。

| 项 | 设备 | 运行时结论（测试数 过/败/偶发/跳） | 今天重判 |
|---|---|---|---|
| backend | 本机 | PASS（0/0/0/0） | 命令套件，没有结果包 |
| shelf-code-ipad | Krill-Verify-iPad-1 | FAIL（1/1/0/0） | FAIL：no test results: `~/orca/workspaces/Krill/v0-125-shelf-code/build/kr/results/shelf-code-ipad/run.xcresult` is missing or unreadable |
| shelf-share-ipad | Krill-Verify-iPad-1 | PASS（2/0/0/0） | FAIL：no test results: `~/orca/workspaces/Krill/v0-125-shelf-code/build/kr/results/shelf-share-ipad/run.xcresult` is missing or unreadable |
| shelf-code | Krill-Verify-1 | FAIL（4/3/0/0） | FAIL：no test results: `~/orca/workspaces/Krill/v0-125-shelf-code/build/kr/results/shelf-code/run.xcresult` is missing or unreadable |
| modules | Krill-Verify-1 | PASS（22/0/0/0） | FAIL：no test results: `~/orca/workspaces/Krill/v0-125-shelf-code/build/kr/results/modules/run.xcresult` is missing or unreadable |
| shelf-edit | Krill-Verify-1 | PASS（11/0/0/0） | FAIL：no test results: `~/orca/workspaces/Krill/v0-125-shelf-code/build/kr/results/shelf-edit/run.xcresult` is missing or unreadable |
| source-view | Krill-Verify-1 | PASS（8/0/0/0） | FAIL：no test results: `~/orca/workspaces/Krill/v0-125-shelf-code/build/kr/results/source-view/run.xcresult` is missing or unreadable |
| shelf-share | Krill-Verify-1 | PASS（24/0/0/0） | FAIL：no test results: `~/orca/workspaces/Krill/v0-125-shelf-code/build/kr/results/shelf-share/run.xcresult` is missing or unreadable |
| viewing-environment | Krill-Verify-1 | PASS（6/0/0/0） | FAIL：no test results: `~/orca/workspaces/Krill/v0-125-shelf-code/build/kr/results/viewing-environment/run.xcresult` is missing or unreadable |

修复后的总结行：`run: 7/9 PASS`

## 3 失败重跑的过滤

| 项 | 测试 | 框架 | 修复前过滤 | 修复后过滤 | 修复前这次重跑 |
|---|---|---|---|---|---|
| shelf-code-ipad | `testPublishImportRelaunchAndWithdrawAgainstLocalBackend()` | XCTest | `testPublishImportRelaunchAndWithdrawAgainstLocalBackend` | `testPublishImportRelaunchAndWithdrawAgainstLocalBackend` | 仍失败 33 s |
| shelf-code | `testLocalBackendKeepsPackageEncryptedAndReceiptCanWithdraw()` | Swift Testing | `testLocalBackendKeepsPackageEncryptedAndReceiptCanWithdraw` | `testLocalBackendKeepsPackageEncryptedAndReceiptCanWithdraw()`（过滤不同） | 仍失败 2 s |
| shelf-code | `accountExpiryIsAssignedByServerAndReceiptPersistsAcrossStoreReopen()` | Swift Testing | `accountExpiryIsAssignedByServerAndReceiptPersistsAcrossStoreReopen()` | `accountExpiryIsAssignedByServerAndReceiptPersistsAcrossStoreReopen()` | 仍失败 32 s |
| shelf-code-ipad-again | `testPublishImportRelaunchAndWithdrawAgainstLocalBackend()` | XCTest | `testPublishImportRelaunchAndWithdrawAgainstLocalBackend` | `testPublishImportRelaunchAndWithdrawAgainstLocalBackend` | 仍失败 32 s |
| shelf-code | `testPublishImportRelaunchAndWithdrawAgainstLocalBackend()` | XCTest | `testPublishImportRelaunchAndWithdrawAgainstLocalBackend` | `testPublishImportRelaunchAndWithdrawAgainstLocalBackend` | 仍失败 33 s |
| shelf-code-again | `testLocalBackendKeepsPackageEncryptedAndReceiptCanWithdraw()` | Swift Testing | `testLocalBackendKeepsPackageEncryptedAndReceiptCanWithdraw` | `testLocalBackendKeepsPackageEncryptedAndReceiptCanWithdraw()`（过滤不同） | 仍失败 1 s |
| shelf-code-again | `accountExpiryIsAssignedByServerAndReceiptPersistsAcrossStoreReopen()` | Swift Testing | `accountExpiryIsAssignedByServerAndReceiptPersistsAcrossStoreReopen()` | `accountExpiryIsAssignedByServerAndReceiptPersistsAcrossStoreReopen()` | 通过 1 s |

