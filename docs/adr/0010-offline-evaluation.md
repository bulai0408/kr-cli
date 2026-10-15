# 0010 kr 的改动先离线评估，不靠跑完整门禁下结论

状态：已接受（2026-10-14 协调者确认）

## 背景
owner 要求：kr 的任何改动或设定（排程、容量、通道、缓存、判定规则、负载策略），都要能做前后评估、给出量化效果，不能靠跑完整门禁或真实测试下结论。Krill 一次完整门禁是 40–75 分钟模拟器时间，而且会占着机器挡住别人。

#9 实测已经证明可行：
- 旧脚本的对照不用重跑：第 6 批证据与实测提交是同一棵代码树。
- 枚举 bug 用留下的日志离线重放就能确认。
- 排程模拟几秒钟就能比较出多个设定的差别，用这次 kr 实测校准，误差为 +7%（`docs/research/krill-p0-trial.md`）。

依据：`docs/research/offline-evaluation.md`。

## 决定

### 四层评估，每层能回答的问题不同

| 层 | 做法 | 能下结论的改动 | 只能给方向的改动 |
|---|---|---|---|
| 1 计划 `kr plan` | 只算不跑：哪些构建命中缓存、为什么要跑、通道分配、按历史预计耗时（参考 Bazel `--nobuild`、`--explain`，Terraform `plan`） | 选择、缓存是否命中 | — |
| 2 模拟 `kr simulate` | 用 kr 自己的调度代码 + 虚拟时钟 + 假执行器（按历史耗时「执行」），重放事件（参考 Borg Fauxmaster、Omega 的高保真模拟器） | 顺序、通道顺序、批内分配、缓存 | 容量、负载阈值（并发改变耗时，trace 是开环的） |
| 3 判定重放 | 用新判定规则重判已保存的结果（kr 运行和 `v0-evidence` 的历史 xcresult），只列出结论变化（参考 Google TAP、Meta 在历史数据上的离线评估） | 判定规则、解析逻辑 | 依赖不存在的证据的新规则（例如「重跑两次」） |
| 4 小样本真跑 | 同一组场景按 A/B 交替各跑一次，按场景配对，对耗时比做 bootstrap 置信区间；噪声阈值用 kr 自测的同设置重复差异 | 第 2、3 层只能给方向的改动 | — |

### 模拟器的校准
- 按时间切分：较旧的运行用来建耗时分布，较新的运行用来回测。
- 目标：门禁 makespan 的 MAPE ≤ 10%，并且模拟与实测的「方案排序」一致（A 比 B 快）。10% 是推论，依据见调研 0.2 B。
- 用耗时分布而不是均值，输出 makespan 的 p50 / p90。
- 同时输出下界 max(最长场景, 总耗时 ÷ 模拟器数)。离下界已在 7/6 以内（LPT，m = 2）时，再改排序收益有限。
- 每次输出写明这次 what-if 是否超出校准数据覆盖的范围（例如容量从 2 改到 3）。

### 事件需要补的数据
1. 运行期间定时采样负载和内存压力（不只开始和结束两点）。
2. 每项开始时的并发上下文：在跑的重活数、租出的模拟器数、旧全机锁的持有者。
3. 每条测试的耗时、失败签名（去掉数字和地址）、失败类别（断言 / 超时 / 崩溃 / 启动 / 基础设施）。
4. 身份：输入哈希、git tree、Xcode 版本、运行时、设备类型、kr 版本、生效配置的哈希。
5. 调度决策：候选、选中项、预估耗时（用来比较预估与实际）。
6. 每次重跑的尝试序号和 xcresult 路径。
7. 请求者（agent / worktree），以及它是否在等前一个结果（评估开环误差）。

#29 的实现（字段只加不改，字段说明在 `crates/kr-events/src/lib.rs`）：

| 项 | 写在哪里 | 说明 |
|---|---|---|
| 1 | 新事件 `load_sample` | 从运行开始每 15 秒一次：`load`（1/5/15 分钟）、`pressure`。直接读内核（`getloadavg`、`sysctlbyname`），不经过命令执行器，所以不影响回放测试。 |
| 2 | `item_start.concurrency` | `build`、`sim`：所有项目当时占用的槽位数（含本运行自己）；`legacy_holder`：旧全机锁的持有者（配置了桥接时）。一个模拟器租约占一个 `sim` 槽位，所以 `sim` 就是租出的模拟器数。 |
| 3 | `item_end.cases` | 每条测试：`id`、`outcome`、`duration_s`；失败和偶发的测试另有 `signature`（数字→`N`，`0x…`→`<addr>`，UUID 等十六进制标识→`<id>`）和 `category`。类别按失败消息里的固定短语判断，顺序是超时、基础设施、启动、崩溃、断言；整次运行超时一律记超时；没有匹配的记 `unknown`，kr 不猜（ADR 0011）。 |
| 4 | `item_end.identity` | `input_hash`、`tree`（`git rev-parse HEAD^{tree}`）、`xcode`（`version`、`build`）、`runtime`、`device_type`、`kr_version`、`config_hash`（生效配置 JSON 的 blake3）。找不到的值写 null。 |
| 5 | 新事件 `schedule_decision` | 调度器每给模拟器通道分一项写一条：`lane`、`kind`、`item`、`estimate_ms`、`candidates`（分配前该类型的队列及各自预估）、`rule`（`longest_first`）。预估来自最近 14 天的历史耗时，没有历史为 null。 |
| 6 | `rerun.attempt`、`rerun.xcresult`；`item_end.xcresult` | kr 只重跑一次，所以 `attempt` 为 2（第一次运行是 1）；过滤无效、没有真正重跑时 `xcresult` 为 null。整项重跑（高负载）也写这两个字段。 |
| 7 | 新事件 `run_start` | `requester`：`agent`（环境变量 `KR_REQUESTER`，没有时用 `AI_AGENT`）、`worktree`（仓库根目录）、`parent_pid`；`previous`：同一 worktree 上一次运行的 `run_id` 和 `ended_ms_ago`（结束到这次开始的毫秒数，还没结束为 null）。kr 不推断「是否在等前一个结果」，只记事实，由分析时判断。 |

远程运行（`--remote`，#32）：本机写 `run_start`（请求来自本机），并把主机结果里每项的 `cases` 和 `xcresult`（主机上的路径）抄进本机的 `item_end`；负载采样、并发上下文、身份、调度决策、重跑和 `verdict.json` 由主机上的 kr 写在主机自己的状态目录里，本机没有副本。`kr run --json` 的每项也多了 `cases`（只加不改）。

### 负载与偶发的关系
- 小样本证明不了「不偶发」：n 次运行 0 次失败时，失败率的 95% 上限约为 3/n。
- 每个场景按「高负载 / 正常」两组累计，用 Beta-Binomial 给出失败率区间，用 Fisher 精确检验比较两组，不给是/否结论。
- 以后可以加「压力模式」：在空闲时段故意加 CPU 负载跑可疑场景（Silva 等：46.5% 的 flaky 测试受资源影响，CPU 影响最大）。

### 工作方式
- kr 的每个 PR 都附一节「前后对比」，数字来自上面四层，并写明用的是哪一层、校准误差多少。
- 完整门禁只用于合入判断，由协调者统一跑，不用作评估手段。
- 每次真实运行都开 `KR_RECORD` 的轻量版本（只保存 kr 解析过的结果和枚举，不存大日志），让第 3 层有证据可重判。

  #29 的实现：轻量版不需要开关，每次 `kr run` / `kr gate` 都写。每个场景每一轮（整项重跑记为 `<场景>-again`）写一个 `~/.local/state/kr/runs/<运行 ID>/<场景>/verdict.json`：判定函数的全部输入（真实枚举、第一次运行和每次重跑解析后的测试结果、退出码、是否超时、结果包路径）和当时的结论，几 KB。完整录制的 `KR_RECORD=<目录>` 仍只给开发 kr 时录测试素材用。
- 第 3 层的命令是 `kr rejudge`：用当前的判定规则重判，只列出结论变化（某项通过与否、某条测试的结果），理由的措辞变化不算。
  - kr 运行：读 `verdict.json`，覆盖判定规则的改动；保存的是解析后的结果，所以解析逻辑的改动只能用下面的结果包评估。
  - 旧结果包：`--evidence <目录>` 找出目录下所有 `*.xcresult`。xcresulttool 读一个结果包时会往里写 `database.sqlite3`，所以 kr 先把结果包复制到临时目录（APFS 上是克隆，几乎不占空间）再读，原目录不写任何东西。读到的原始 JSON 和当时的结论缓存在 `~/.local/state/kr/rejudge/evidence/`，以后只做计算，既覆盖判定规则也覆盖解析逻辑。结果包旁边的 `status` 当作退出码，`enabled-tests.txt` 当作枚举；没有枚举时把实际跑过的测试当作选中的测试，这时「选中了却没跑」查不出来。旧脚本的重跑结果没有和失败配对，所以历史结果包里不会出现偶发。
  - 2026-10-15 对 Krill `v0-evidence` 的 427 个结果包：首次读取 98 秒（`kr lock light` 排队），之后从缓存重判 12 秒（主要是遍历 5 GB 目录）；规则不变时 0 处变化。故意去掉「0 条测试即失败」这条规则时，列出 30 个结果包 FAIL → PASS，与事先从缓存独立算出的 30 个完全一致（见 #29 的提交说明）。


### 已由模拟得出、建议立即采用的改动
- 通道启动顺序固定，先 iPad 后 iPhone（现在由线程竞争决定）：按第 6 批实测耗时模拟，default 门禁从 41.3 分钟降到 37.6 分钟。这属于第 2 层能下结论的「顺序」类改动。

## 不做
- 不另写一个与 kr 调度代码分离的模型，以免模型与实际行为不一致。
- 不用「最好的一次」做比较；不把整次门禁当成一个样本做统计检验（需要 9 次以上重复，不现实）。

## 参考
`docs/research/offline-evaluation.md`（Borg、Omega、Google TAP、Meta Predictive Test Selection 与 Probabilistic Flakiness、Silva 等的 RAFT、Graham 的 LPT 界、Sargent 的模拟验证、Bazel / Buck2 / nextest / Terraform 的 plan 与日志工具）；`docs/research/krill-p0-trial.md`。
