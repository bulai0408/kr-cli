# #35 偶发隔离区：统计依据与离线前后对比

日期：2026-10-16。设计：ADR 0021（协调者已确认）。

## 上限依据

先将本机 `~/.local/state/kr/events` 和 `runs` 复制到任务临时目录，再运行 `kr report` / `kr rejudge`。原始目录与 Krill 仓库均未写入；没有运行 Xcode 原生构建或模拟器。提交的证据去掉个人路径。

命令（`KR_STATE_DIR` 指向副本）：

```sh
kr report --project Krill --since 2026-10-14T09:40:00Z --until 2026-10-14T10:55:00Z --json
kr report --project Krill --since 2026-10-14T10:55:00Z --until 2026-10-14T16:20:00Z --json
kr report --project Krill --since 2026-10-14T00:00:00Z --until 2026-10-16T00:00:00Z --json
```

依据沿用 ADR 0016：按运行开始时间归入批次；偶发次数取 `item_end.flaky`，分母为同项的运行次数。不能把所有 check、lock 和 build 的次数当作测试分母。数据只表明观察到了什么，不证明测试一定不稳定或已经修好。

| 范围（UTC） | 运行数 | 单测偶发记录 | 整项偶发记录 |
|---|---:|---:|---:|
| 第 7 批，10-14 09:40–10:55 | 13 | 0 | 0 |
| 第 8 批，10-14 10:55–16:20 | 88 | 3 | 1 |
| 最近窗口，10-14 00:00–10-16 00:00 | 324 | 4 | 3 |

最近窗口有 2,605 条 Krill 事件、297 条完成项事件。4 条单测各偶发 1 次：

| 测试（省略公共 KrillUITests 前缀） | 场景 | 偶发次数 / 该项运行数 |
|---|---|---:|
| `DetailSearchUITests/testDoubanIDsDiscoveredByDetailsOfferCorrection()` | detail-search | 1 / 3 |
| `ModulesUITests/testStreamsMergeProviderOrderAndSwitches()` | modules | 1 / 9 |
| `PlaybackPreloadUITests/testSourceOptOutAndSharedStorageClear()` | preload | 1 / 4 |
| `SettingsAboutUITests/testAboutShowsAttributionPinnedLicensesAndSystemLogShareSheet()` | settings-about | 1 / 4 |

整项偶发分别为 preload-ipad 2/6、modules 1/9。它们不指明哪条测试偶发，不能转换成测试级隔离条目。

**初始预算取 4**：最多容纳当前窗口观察到的 4 条独立单测偶发，不把 3 次整项偶发变成额外名额。预算是需经 PR 审阅的政策值，样本不足以证明 4 是最优值；后续用同口径 report 更新 `basis` 和 `limit`。目前没有单测达到 90 天至少 2 次的加入阈值，所以没有理由自动隔离这 4 条；kr 本身也不修改名单。该预算只针对 Krill，其他项目应使用自己的报告确定预算。

完整的 `batch/time/flaky` 原样摘录：`evidence/quarantine/krill-report-excerpts.json`。`kr report` 的现有输出和统计行为没有改变；隔离建议直接复用它的范围筛选和统计函数。

## 前后对比：ADR 0010 第 3 层

### 真实历史覆盖

复制的运行目录里只有 11 份 `verdict.json`，都属于 KrSample；没有可归属 Krill 的保存判定。唯一失败项是 slow 的枚举不一致，不属于允许豁免的完整测试失败。在同一副本上，以 Krill 当前观测到的 4 条测试做名单 what-if，`kr rejudge --json` 前后都是 11 项、0 处变化。

这不能得出“隔离改善了 Krill 门禁”的结论。Krill 保存判定的覆盖为 **0 项**；变化率的误差无法估计。不能从旧事件补造缺失的首次结果、逐条重跑结果和枚举。跨项目的名单也不能豁免 KrSample 的结果。证据：`evidence/quarantine/historical-rejudge.json`。

### 可重现的受控判定重放

使用仓库已录制的真实 KrSample `broken` 外部命令输出，通过 `KR_REPLAY` 跑命令；不是运行原生测试。名单条目是受控政策输入，**不声称这个持续失败的测试在真实项目中应当被隔离**。

1. 无名单时，`kr run broken --json`：退出 1，`passed: false`，failed=1、passed=0，门禁阻塞。
2. 仅在临时项目新增名单，`kr rejudge --json`：1 项门禁阻塞结论 FAIL → PASS，测试 outcome 不变（`tests` 变化为空）。这里 PASS 表示该项不再阻塞，原始失败没有变成通过。
3. 同一录制再次 `kr run broken --json`：退出 0，`passed: false`，failed=1、passed=0、`blocking: false`、`quarantined` 单列该测试；仍执行逐条重跑。
4. 只重判这次隔离运行：0 处变化。删除临时名单再重判同一次运行：1 项门禁阻塞结论 PASS → FAIL，原始失败仍不变。

证据：`evidence/quarantine/rejudge-demo.json`；自动化接缝为 `crates/kr/tests/run.rs` 的 `rejudge_applies_the_current_list_and_preserves_failed_test_outcomes` 和 `quarantine_is_scoped_to_the_item_and_observed_entries_and_exits_are_events`。

在这一份完整录制上，独立预期为启用时 1 处、规则不变时 0 处、移出时 1 处变化，实得 1/0/1，差异为 0。此处的 0 是确定性回放与预期的差异，不是生产错误率的统计置信区间。耗时收益不评估；判定变化不能换算成节省的构建或模拟器时间。

### 不得被隔离绕过的失败

回放同时覆盖缺结果、超时、无效重跑过滤、逐条重跑结果缺失：运行仍退出 1，重判也不改成通过。名单外或其他项的失败仍阻塞。保存判定新增隔离完整性与当时阻塞状态，用来支持移出后的反向比较；旧文件缺少无效过滤或设备失联上下文时，仅靠旧文件不能证明那部分环境状态。新文件保留这些保护的判定结果。

## 判断点和事件验证

- 加入：同项目同项至少 2 次单测偶发，输出 `quarantine_add`、90 天统计、精确追加项；整项偶发不生成测试条目。
- 移出：最近 30 天至少 10 次完整通过，且无失败、偶发，输出 `quarantine_remove`、完整次数、精确删除项（含 `added` / `issue`）。9 次、近期失败、近期偶发不建议移出。
- 两种均为非阻塞效率判断点，默认 keep / pending review；`kr decide … add|remove|keep` 复核只写事件，不改文件。同 worktree 后续首次实际观察到该测试进出时对账；PR 尚未落地不算判错。
- 每次实际观测写 `quarantine_observed`，进出分别写 `quarantine_enter` / `quarantine_exit`，包含文件、worktree、条目与原因。不同 worktree 不串名单。

## 没有验证的部分

- 没有 Xcode 或模拟器真跑、真实服务、真机验收，也没有原生完整门禁。本票只改变统计与结论，不改变执行/重跑流程。
- 没有可用于 Krill 的历史 `verdict.json`；无法量化真实 Krill 门禁减少了多少阻塞。
- PR 流程由 owner/宿主保障；kr 只读仓库名单，不通过网络验证 PR 状态。进出事件是观测时间，不是合并时间。
- 旧 `--evidence` 的 xcresult 缓存缺少稳定项目/场景身份与配对重跑，不套用项目隔离名单。
- 加入/移出建议需要足够事件；没有记录不代表没有失败。预算和阈值的长期效果需后续批次数据。

## Rust 检查

合入集成分支前：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test` 均成功；Rust harness 汇总 347 passed / 0 failed。原生集成测试开关 `KR_INTEGRATION` 未开启，这些测试体未执行，不能作为原生验证通过的证据。新增命令回放覆盖 gate/run、加入/移出建议、kr decide 复核与对账、上限报错、完整性保护、项目/场景范围、跨窗口计数、历史正反向重判和远程隔离结果展示。

合入 `origin/p1p2/integration@4610e54` 后：三项检查再全部通过，392 passed / 0 failed。新增 14 个命令回放的完整名称见 `evidence/quarantine/verification.json`。冲突仅在 run 的选择/判断汇总与输出及追加测试区：保留 #30 的选择与解释、判断点 ID 预分配和提前记录默认事件；隔离建议在结果后通过同一 record_default 接口记录，序号接在选择与失败判断之后。两侧追加测试全部保留，另加跨功能回放断言 ID 不重叠、每种判断点和默认事件各仅一条。#31 的合并辅助和入口保持集成版本，本票未改选择规则或新增 Scenario 配置字段。
