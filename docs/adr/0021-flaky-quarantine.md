# 0021 仓库内偶发隔离名单

状态：已采纳（2026-10-16，#35，协调者已确认）

## 决定

- 固定文件 `.kr/quarantine.json`，不存在等同空名单。字段：`schema_version: 1`、`limit`、非空 `basis`（`kr report` 的范围和统计依据）、`tests`。每项有 `item`、精确 `test` 标识、`reason`、`added`（YYYY-MM-DD），可选 `issue`（链接或 #编号）。拒绝重复项、空字段、未知字段、无效日期和超过上限的名单；错误写明文件和修改方法。
- 名单及上限变更经过 PR，由 owner 和宿主保障；kr 只读，不自动提交、不替用户修改。`limit` 是项目选定的预算，不随本机事件保留时间漂移。Krill 初始预算为 4，依据见 `docs/research/quarantine-evaluation.md`。
- 不改变枚举、执行、逐条重跑、高负载整组重跑（含 #40）和原始判定。名单里的失败保留 `failed` outcome/count，单列 `quarantined`，不计为通过。只有结果完整且无其他阻塞原因、无名单外失败时不阻塞门禁；缺结果、超时、无效重跑过滤、未验证仍阻塞。人读输出明确 QUARANTINED，JSON 另有 `blocking`。
- 加入建议：同项目、同项、同精确测试在过去 90 天按 `kr report` 的 `item_end.flaky` 口径至少偶发 2 次；整项偶发不是具体测试，不进入建议。当前不在名单才建议。
- 移出建议：名单里的测试在最近 30 天至少 10 次完整运行，且没有失败或偶发。完整运行要求该项通过且该测试明确通过；该项失败、缺失或未验证不算完整通过。失败与偶发证据不会因隔离消失。缺失旧数据不推断健康。
- 两种建议均为 `affects: efficiency` 的非阻塞判断点，按默认保持当前名单、写 `decision` 为待复核；证据包含范围、次数、名单文件和精确的增删内容。建议不改变本次结论，不能用 `kr decide` 绕过 PR 改名单。
- `kr decide <id> add|remove|keep --reason …` 可复核默认待复核建议，只记录决定，不改名单或本次结论。同项目同 worktree 后续首次观测到该项进出时写对账；PR 尚未落地、名单未变不算判错。
- 每次 run/gate 对比同项目同 worktree 前次观测的完整名单，写 `quarantine_enter` / `quarantine_exit`，首次观测记录全部加入；写 `quarantine_observed` 保存快照。事件表示“观测到”，不是 PR 的合并时间；不同 worktree 不互相制造进出。
- 统计复用 `kr report` 的批次读取和偶发统计，不改变 report 输出与口径。`kr rejudge` 使用当前项目名单重判保存的历史结果，原始失败 outcome 不变，只比较门禁阻塞结论；不写事件、不写原始证据。旧记录没有重跑过滤错误或设备失联上下文时保守标注离线误差，不能当作真实门禁验收。

## 示例

```json
{
  "schema_version": 1,
  "limit": 4,
  "basis": "kr report --project Krill --since 2026-10-14T00:00:00Z --until 2026-10-16T00:00:00Z: 4 individually flaky tests",
  "tests": [
    {"item":"modules","test":"KrillUITests/ModulesUITests/testStreamsMergeProviderOrderAndSwitches()","reason":"track repeated flakes","added":"2026-10-16","issue":"#35"}
  ]
}
```

## 依据
ADR 0003、0008、0010、0011、0015、0016；`docs/research/pitfalls-inventory.md` U1；#35。名单仅明确豁免指定测试失败，不是关闭测试或将失败改称通过。
