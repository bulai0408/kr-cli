# 0015 判断点骨架：第一个判断点、交回方式、最终结论、对账、`kr skill`

状态：已采纳（2026-10-15，#20 实现时写，协调者已确认）。细化 ADR 0011，不改变它的决定。

## 背景
ADR 0011 定了方向：会影响结论的判断点让 kr 不给结论、退出 4；agent 用 `kr decide` 交回；kr 记录并事后对账；`kr skill` 随版本发布。实现时还要定下几件 ADR 0011 没写的事：「没见过的失败」怎么算、选项是什么、选项怎么改变结论、存在哪里、怎么对账。

## 决定

### 1. 第一个判断点 `failure_kind`：没见过的失败
同时满足才成为判断点，否则照原规则判定（ADR 0003、0005 不变）：
- 测试失败，单独重跑仍失败（重跑通过 = 偶发，高负载整项重跑通过 = 偶发，都还是规则判定）；
- 这一项只因失败的测试而失败：没有超时、没有少跑或多跑、没有重跑过滤无效、设备没有失联。否则分类改变不了结论，问了也没用；
- 这一项的每条失败测试都没见过。「见过」= 同一条测试、同样的失败特征（ADR 0010 的 `signature`）出现在本次运行之前（运行 ID 更早）过去 90 天的 `item_end.cases` 里，结果为失败或偶发。只要有一条见过的失败，这一项按规则就是 FAIL，其余失败也不问。

键用「测试 + 特征」而不是只用特征：断言消息常常相同（例如 `XCTAssertEqual failed`），不同测试的同一消息不能互相当作见过。

### 2. 选项和对结论的影响
- `product`：代码或测试的问题，测试仍为失败，该项 FAIL。
- `flaky`：偶发，测试记为偶发，与重跑通过一样不让该项失败。
- `environment`：模拟器、机器或服务导致，测试记为「未验证」。未验证不算通过（CONTEXT「结果」），该项不通过；与 `product` 的区别是 agent 应该在环境恢复后再跑，而不是改代码。

默认建议：失败类别是 infrastructure 或 launch 时建议 `environment`，其余建议 `product`（单独重跑仍失败，在证明之前当作真失败）。证据：特征、类别、摘要、结果包和日志路径、当时负载、这条测试过去 90 天的运行数、失败数、偶发数。

### 3. 停下与交回
- 运行照常跑完，最后有会影响结论的判断点时：`--json` 的 `ok` 为 false、`concluded` 为 false、`decisions` 列出判断点；人读输出该项一行 `UNDECIDED:`，判断点单独成段，每个写明 `next: kr decide …`；结论行写「no verdict yet」，不出现 PASS/FAIL；退出 4。
- 判断点 ID 为 `<运行 ID>-<序号>`。判断点和运行时的各项结果存在运行目录的 `decisions.json`，事件里写 `decision_point`，`run_end` 加 `decisions_pending`。
- 交回：`kr decide <id> <选项> --reason …`，或在下一条运行命令上 `kr run --decide <id>=<选项> --reason …`（`kr gate` 同样）。带 `--decide` 时不运行任何东西。
- 每个决定写 `decision` 事件：判断点 ID、选择、理由、交回方式、谁（`KR_REQUESTER` / `AI_AGENT`、worktree、父进程）、当时的证据；事件用被决定的那次运行的运行 ID。
- 全部交回后写 `run_decided`（label、ok、passed、total），按已有结果打印最终报告，退出 0 或 1；还有没交回的，退出 4 并列出剩下的。
- ID 不存在、选项不对、已经决定过：退出 2，写明可选的值。决定不能改：改了会让对账失去意义。

### 4. 对账
每次 `kr run` / `kr gate` 结束时，为还没对账的 `failure_kind` 决定找答案：被决定的那次运行之后，同一项、同一构建输入（`identity.input_hash`）上最早一次跑了这条测试的 `item_end`。`product` 在测试又失败时成立；`flaky`、`environment` 在测试通过或偶发时成立；跳过不算证据。每个决定只写一次 `reconciliation` 事件。判对率由 `kr report`（第三阶段）从这些事件算出（ADR 0012）。

### 5. 只影响效率的判断点
结构里有 `affects: efficiency`：kr 按默认继续，写 `decision` 事件（`via: "default"`，`review: "pending"`）。本票没有这类判断点的来源，第一个会是选测的加跑建议（spec 第 4 项）。

### 6. 判定重放
`kr rejudge` 用同样的规则（同一份「见过」的计算）在已保存的运行上列出判断点：当时是否提出过、怎么决定的、对账结果（已记录的，或从现有事件算出、标 `recorded: false`）。它不写事件。

### 7. `--json` 与 `kr skill`
- 所有 `--json` 输出（含错误）带 `schema_version: 1` 和 `decisions`（多数命令为空）。已有字段不改。
- `kr skill` 打印随版本发布的说明（常规流程、退出码、判断点、何时问 owner、硬底线），`--reference decisions|json` 按需读；项目的 `.kr/skill.md`、个人的 `~/.config/kr/skill.md`（机器配置同目录）追加在主文本后。

## 不做
- 不在运行中途停下等判断，不交互提问。
- 不让 `flaky`、`environment` 的判断绕过「0 条测试不算通过」「未验证不算通过」。
- 本票不改 `kr deliver`：停在判断点的运行，`item_end` 仍写 `passed: false`；`kr deliver` 读到的是规则判定的结果，暂不读 `run_decided`（后续票处理）。

## 参考
ADR 0003、0005、0010、0011、0012；`docs/research/agent-tool-design.md`。
