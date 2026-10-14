# 离线评估调研：不跑全量门禁，怎样判断 kr 的改动和设置有没有用

日期：2026-10-14。只读调研。每条结论后附来源。
“推论”表示作者的判断，不是来源原话。

被评估的方案（四层）：
1. `kr plan`：只计划，不执行。
2. `kr simulate`：用历史耗时做 trace 驱动的 what-if，并用真实运行校准。
3. 判定重放：用新规则重新判定已记录的运行和 xcresult，对比结论差异。
4. 只在需要时做固定的小样本真实运行。

---

## 结论（先看这里）

### 0.1 四层方案总体成立，但有三处要改

- **成立。** Google 在生产中用同样的分层：Borg 用高保真模拟器 Fauxmaster 做容量规划和“改配置前的检查”，不在生产 cell 上做实验（[Borg 论文](https://research.google.com/pubs/archive/43438.pdf)）。Omega 用“快的近似模拟器 + 慢的高保真模拟器”两层（[Omega 论文](https://research.google.com/pubs/archive/41684.pdf)）。Google TAP 和 Meta 都在历史数据上离线评估测试选择策略（[Memon 2017](https://huang.isis.vanderbilt.edu/cs8395-f23/paper/google-testing-icse-seip-17.pdf)；[Machalica 2019](https://ar5iv.labs.arxiv.org/html/1810.05286)）。
- **改动 1：模拟器要跑 kr 自己的调度代码，不要另写一个模型。** Fauxmaster “包含生产 Borgmaster 代码的完整副本”，只把对 Borglet 的接口换成桩（stub）。Omega 的高保真模拟器的核心也是“Google 生产调度系统的代码”。kr 已经有可替换的命令执行器（ADR 0008），所以 `kr simulate` = 真实调度代码 + 虚拟时钟 + 假执行器（按历史耗时“睡眠”）。（推论，依据上面两篇论文）
- **改动 2：只换顺序可以离线评估；换容量、换负载策略不可以只靠 trace。** 原因有两个：
  - 并发会改变每个任务的耗时。忽略资源争用的模拟器误差很大：一篇 2026 年的论文测得现有 GPU 集群模拟器的平均 JCT 误差最高 73.64% MAPE；加入争用模型后 NIC 争用因子误差降到 8.63%（[MoSim, arXiv 2609.23278](https://arxiv.org/abs/2609.23278)）。kr 的数据大多在 sim=2 下采集，不能直接外推 sim=3 的耗时。
  - trace 是开环的：历史到达时间本身受旧调度器影响（agent 等锁时不会提交新任务）。Shmueli 和 Feitelson 指出，开环 trace 模拟会导致较差的调度器设计（[TPDS 2009](https://www.cse.huji.ac.il/~feit/papers/UserAwareSched09TPDS.pdf)）。
  - 所以：顺序、批次内分配、缓存命中这类改动 → 第 2 层可信；容量、lane、负载阈值这类改动 → 第 2 层只给方向，必须用第 4 层的小样本真实运行确认。（推论）
- **改动 3：判定重放只能回答“已发生的事实换一种判法会怎样”，不能回答反事实。** 例如新规则是“失败后重跑 2 次”，但历史只重跑了 1 次，第 2 次的结果不存在。Meta 的做法是专门做“learning test run”：对一部分改动跑全部测试，失败的重试最多 10 次，用来造真值（[Machalica 2019](https://ar5iv.labs.arxiv.org/html/1810.05286)）。kr 要定期做少量“加密采样”运行（更多重复），给第 3 层提供真值。（推论）

### 0.2 给 kr 的具体建议

**A. 指标（每次模拟和每次真实运行都输出同一组）**
- 门禁墙钟时间（makespan，从第一个 `queue_start` 到最后一个 `item_end`）。
- 每个资源的排队等待（`waited_ms`）的 p50 / p90。
- 每台模拟器的忙碌率和“尾部不均衡” = 最晚结束 − 最早结束。Grab 用的就是 max(tᵢ) − min(tᵢ)（[Grab 工程博客](https://engineering.grab.com/tackling-ui-test-execution-time-imbalance-for-xcode-parallel-testing)）。
- 判定指标：通过 / 失败 / flaky 的数量，以及“结论改变的运行数”。

**B. 模拟器怎么校准，多少误差可以接受**
- 用 Sargent 的“历史数据验证”：一部分历史数据用来建模，剩下的数据用来检查模型（[Sargent, WSC 2004](https://www.informs-sim.org/wsc04papers/004.pdf)）。kr 的做法：按时间切分，旧运行估计每个场景的耗时分布，较新的运行做回测。时间切分也是 Meta 的做法（最近一周做测试集）（[Machalica 2019](https://ar5iv.labs.arxiv.org/html/1810.05286)）。
- 误差目标：Gavel 报告模拟与物理集群的指标差异 < 5%，并称“高保真”（[Gavel, arXiv 2008.09213](https://arxiv.org/pdf/2008.09213)）。kr 建议：门禁 makespan 的 MAPE ≤ 10%，并且“策略排序一致”（A 比 B 快，模拟和真实结论相同）。10% 是推论：kr 的样本远少于 Gavel，模拟器噪声底更高；我们需要的是选对策略，不是精确到分钟。
- Sargent 强调：模型只在“预期用途的精度范围”内有效；在一些条件下准确，不保证在全部范围内都准确（[Sargent, WSC 2004](https://www.informs-sim.org/wsc04papers/004.pdf)）。所以每次模拟输出要写明：这次 what-if 是否超出了校准数据覆盖的范围（例如容量从 2 改到 3）。（推论）
- 耗时用分布，不用均值：kr 现在用最近几次的平均值（`crates/kr/src/commands/run.rs`）。建议对每个场景保存最近 N 次的耗时，模拟时重采样，输出 makespan 的 p50 / p90，而不是一个点。Omega 的图用“中位数 ± MAD”做误差条（[Omega 论文](https://research.google.com/pubs/archive/41684.pdf)）。
- 先算下界，再看模拟：makespan ≥ max(最长场景, 总耗时 ÷ 模拟器数)。LPT 调度的结果最多是最优解的 4/3 − 1/(3m) 倍；m = 2 时是 7/6 ≈ 1.17。任意顺序的贪心（list scheduling）最多 2 − 1/m 倍，m = 2 时是 1.5（[Graham 1969, SIAM J. Appl. Math.](https://epubs.siam.org/doi/10.1137/0117039)；公式转引自 [Wikipedia LPT 条目](https://en.wikipedia.org/wiki/Longest-processing-time-first_scheduling)）。如果模拟结果离下界已经 < 17%，再换排序策略收益有限。（推论）

**C. 事件里还缺的数据**（对照 `crates/kr-events/src/lib.rs` 和 `crates/kr/src/commands/run.rs`）
1. **负载只有开始和结束两个采样点。** `item_end.load` 只有 start / end。要判断负载和失败的关系，需要运行期间定时采样（例如每 10 s 一次 load、内存压力），或者至少记录 max 和均值。
2. **并发上下文。** 每个 `item_start` 写入当时正在运行的重任务数、已租出的模拟器数、其他项目的锁持有者。没有它，就不能建立“耗时 = f(并发)”的模型（见 0.1 改动 2）。
3. **单个测试的耗时和失败信息。** 现在 `tests` 只有按结果分类的计数。要保存每个测试方法的耗时、失败消息的归一化签名（去掉数字和地址）、失败来源（断言 / 超时 / 崩溃 / 启动失败 / 基础设施）。Gruber 等发现 Python flaky 测试中 28% 来自测试基础设施问题（[arXiv 2101.09077](https://arxiv.org/abs/2101.09077)），所以要能把基础设施失败分出来。
4. **可复现的身份。** 每个 `item_end` 带上：输入哈希（构建产物）、git tree SHA、Xcode 版本、模拟器 runtime、设备类型、kr 版本、生效配置的哈希。判定重放和模拟都要按这些键分组。
5. **决策本身。** 调度器每次选择下一项时，写一个事件：候选列表、选中的项、原因、当时的预估耗时。这样能比较“预估 vs 实际”，也能在重放时复现同一决策。Buck2 的 `what-ran` 也会输出每个命令的执行原因（[buck2 log](https://buck2.build/docs/users/commands/log/)）。
6. **尝试序号和 xcresult 路径。** 每次重跑写 `attempt`（1, 2, …）和对应的 xcresult 路径，第 3 层要回读原始证据。
7. **请求到达时间和请求者。** `queue_start` 已有时间戳；再加上 agent / worktree 标识，并记录“这个 agent 是否在等前一个结果”。这用来判断 trace 的开环误差有多大（见 0.1 改动 2）。

**D. 判定重放怎么做**
- 输入：事件日志 + 保存下来的 xcresult（或 kr 从中提取的每个测试的结果 JSON）。输出：每次运行在旧规则和新规则下的结论，以及差异列表（像 `terraform plan` 那样只列出变化）。
- 指标借用 Meta 的定义：测试召回率、变更召回率、选择率；并单独报告“把 flaky 当成失败”时的数字。Meta 发现两者差很多：选择率 0.15 时，含 flaky 的召回约 0.9，真实召回约 0.7（[Machalica 2019](https://ar5iv.labs.arxiv.org/html/1810.05286)）。
- 标签规则：所有尝试都失败 = 失败；既有失败又有通过 = flaky。这是 Meta 的定义，它也说明这种方法只给出 flaky 数量的下界（同上）。kr 的“重跑一次”同理：一次通过不能证明不是 flaky。
- Google 的结果说明重放的价值：在历史数据上模拟“只跑 MinDist ≤ 10 的测试”，资源节省 42%，并且“没有漏掉任何一次 breakage 或 fix”（[Memon 2017](https://huang.isis.vanderbilt.edu/cs8395-f23/paper/google-testing-icse-seip-17.pdf)）。

**E. 小数据下怎样判断负载和 flaky 的关系**
- 先接受一个事实：小样本证明不了“不 flaky”。0 次失败 / n 次运行时，失败率的 95% 上限约为 3/n（rule of three）（[Hanley & Lippman-Hand, JAMA 1983](https://jamanetwork.com/journals/jama/article-abstract/389714)）。Gruber 等估计，要 95% 确信一个通过的测试不是 flaky，平均需要 170 次重跑（[arXiv 2101.09077](https://arxiv.org/abs/2101.09077)）。FlakeFlagger 的数据集对 24 个项目各重跑 10,000 次，仍有 flaky 测试没被发现（[Zenodo 4450723](https://zenodo.org/record/4450723)）。
- 所以：用累计数据，按场景给出 flaky 概率的区间，不给是/否。Meta 的 PFS 用贝叶斯模型（Stan），输出后验分布；分布宽，表示需要更多数据；它只用日常 CI 的结果，不额外跑测试（[Meta 工程博客](https://engineering.fb.com/2020/12/10/developer-tools/probabilistic-flakiness/)）。kr 可以用最简单的 Beta-Binomial：每个场景 flaky 次数 / 运行次数，按“高负载 / 正常负载”分两组。（推论）
- 判断“负载导致 flaky”：Silva 等的方法是比较受限资源下和不受限下的失败率，用 Pearson 卡方检验（p < 0.05），再用 Benjamini-Hochberg 校正多重比较（[arXiv 2310.12132](https://ar5iv.labs.arxiv.org/html/2310.12132)）。kr 场景少、次数少时，卡方检验条件不满足，改用 Fisher 精确检验（推论）。
- 主动探测比被动等待便宜：Silva 等发现 46.5% 的 flaky 测试是资源影响型（RAFT），CPU 不足是“最显著、最普遍”的因素；而且只用少数几种资源配置、多做几次就能发现大部分 RAFT（283 个 RAFT 中只有 24 个只在一种配置下出现）（同上）。kr 可以加一个“压力模式”：在空闲时段故意加 CPU 负载跑可疑场景，代替等待自然的高负载。（推论）
- Google 数据也支持“越大越 flaky、模拟器类越 flaky”：一周内小 / 中 / 大测试的 flaky 比例为 0.5% / 1.6% / 14%；RAM 使用与 flaky 比例的 r² = 0.76；使用 Android 模拟器的测试 25.46% 是 flaky（[Google Testing Blog 2017](https://testing.googleblog.com/2017/04/where-do-our-flaky-tests-come-from.html)）。XCUITest 属于最大的那一类，flaky 比例高是正常预期。（推论）

**F. 小样本真实运行（第 4 层）的统计方法**
- 全量门禁 40–70 分钟，不可能重复 9 次以上。Google Benchmark 的 `compare.py` 用 Mann-Whitney U 检验，并警告：至少 9 次重复才有意义（[google/benchmark tools.md](https://github.com/google/benchmark/blob/main/docs/tools.md)）。
- 所以把单位从“整次门禁”换成“场景”：同一组场景在 A、B 两种设置下各跑一次，按场景配对，计算每个场景的耗时比，再对比值做 bootstrap 置信区间。30 个场景 = 30 个配对样本。（推论）
- 判定阈值参考 criterion.rs 的默认值：95% 置信水平、显著性 0.05、噪声阈值 1%、bootstrap 100,000 次（[criterion docs.rs](https://docs.rs/criterion/latest/criterion/struct.Criterion.html)）。对 UI 测试，噪声阈值要按 kr 自己测到的同设置重复差异来设，大概率远大于 1%。（推论）
- 不要用“最好的一次”。Georges 等发现，常见的“best of N”方法在最多 16% 的比较中给出过强的结论（[OOPSLA 2007](https://dri.es/files/oopsla07-georges.pdf)）。
- 交替执行 A 和 B（ABAB…），减少时间漂移（其他 agent、温度、缓存）的影响。（推论）

---

## 1. trace 驱动的模拟和校准

### 1.1 Google Borg：Fauxmaster
- Fauxmaster 读取 Borgmaster 的 checkpoint 文件，“包含生产 Borgmaster 代码的完整副本”，对 Borglet 的接口是桩。用途：调试、容量规划（“还能放下多少个这种作业？”）、改配置前检查（“这个改动会不会驱逐重要作业？”）。https://research.google.com/pubs/archive/43438.pdf
- 论文中的策略评估“不能在生产 cell 上做实验”，所以用 Fauxmaster 和真实 checkpoint 数据得到高保真结果；并说明“其他 checkpoint 给出类似结果”——即用多个时间点的数据检查结论是否稳定。同上。
- 对 kr：`kr simulate` 的输入是“状态快照 + 事件流”，执行的是真实 kr 调度代码。（推论）

### 1.2 Google Omega：两层模拟器
- 轻量模拟器：用经验分布合成负载，做了简化（同构机器、忽略约束、随机首次适配），24 小时负载约 5 分钟跑完；用来扫大范围参数。https://research.google.com/pubs/archive/41684.pdf
- 高保真模拟器：用真实 trace 和生产调度代码，24 小时约 2 小时跑完。先用轻量模拟器比较架构，再用高保真模拟器在“没有简化假设”的条件下细看。同上。
- 误差条用“中位数 ± MAD（中位数绝对偏差）”，作者称其为稳健的离散度估计。同上。
- 对 kr：可以先用一个极简模型（场景耗时 + 槽位）快速扫参数，再用真实调度代码复核少数候选。（推论）

### 1.3 开环 trace 的风险（反方证据）
- Shmueli、Feitelson：并行系统调度器常用开环 trace 模拟，这会导致较差的设计；他们提出由用户模型交互生成负载的“site-level simulation”。https://www.cse.huji.ac.il/~feit/papers/UserAwareSched09TPDS.pdf
- 相关博士论文发现：用户对响应时间（而不是 slowdown）作出反应，响应越长越可能放弃会话。https://www.cse.huji.ac.il/labs/parallel/stud/Shmueli-PhD.pdf
- 对 kr：agent 等到结果后才提交下一个任务，这就是闭环。重放时，应保留“任务 B 依赖任务 A 的结果”这种关系，而不是固定的到达时间。（推论）

### 1.4 忽略争用的误差（反方证据）
- MoSim 论文：现有模拟器忽略网络争用或用固定罚值，平均 JCT 误差最高 73.64% MAPE；建模争用后误差下降，最多降低 3.28×（平均 JCT）、7.79×（p99 JCT）、8.48×（makespan）。https://arxiv.org/abs/2609.23278
- 对 kr：两台模拟器和一次构建会争用 CPU、内存、磁盘。耗时模型至少要按并发度分桶。（推论）

### 1.5 校准方法和精度目标
- Sargent 定义模型验证为证明模型在其预期用途内“具有满意的精度范围”；方法包括历史数据验证（一部分数据建模，其余数据检验）、预测验证、trace（跟踪单个实体）、参数敏感性分析、运行时图形。可行时优先用置信区间或假设检验，但它们常因数据不独立或数据量不够而无法使用。https://www.informs-sim.org/wsc04papers/004.pdf
- Gavel：48 GPU 物理集群和模拟器的指标差异 < 5%；例如平均 JCT 3.6 h（物理）对 3.4 h（模拟），makespan 17.7 h 对 17.9 h。https://arxiv.org/pdf/2008.09213
- Shockwave：模拟器与 32 GPU 物理集群相差约 5%；两者使用同一套调度代码。https://arxiv.org/pdf/2210.00093 （转引自搜索摘要，未逐行核对）
- 对 kr：用同一个指标在“回测集”上算 MAPE；每次 kr 调度代码或机器（Xcode 版本、Mac）改变后，重新回测一次。（推论）

### 1.6 Meta / Uber / BuildBuddy / EngFlow
- 未找到 Meta、Uber 公开的 CI 调度模拟器校准方法；Uber SubmitQueue（EuroSys 2019）的评估章节未取得全文，不引用。
- BuildBuddy、EngFlow 的公开文档主要提供执行日志、时间线和关键路径，没有找到 what-if 模拟功能。kr 的第 2 层没有现成的工业先例可以照抄，只能借用上面的研究方法。（推论）

## 2. 用历史耗时做测试调度和分片

### 2.1 理论界
- Graham 1969：LPT 在相同机器上的最坏比 4/3 − 1/(3m)，且是紧的；任意顺序的 list scheduling 是 2 − 1/m。https://epubs.siam.org/doi/10.1137/0117039 （公式转引自 https://en.wikipedia.org/wiki/Longest-processing-time-first_scheduling ）

### 2.2 工具做法
- **Tuist**：用最近 30 天的 test-insight 耗时平衡分片；没有 insights 时“每个分片都当作未知套件运行”；粒度可选 module 或 suite。https://tuist.dev/en/docs/guides/features/test-sharding/xcode 。Tuist 博客称算法是贪心的最长处理时间优先（LPT），无历史的测试用已知测试的中位数耗时（来自搜索摘要，博客页面返回 403，未直接核对）：https://tuist.dev/blog/2026/03/25/test-sharding
- **cargo-nextest**：分片不用历史耗时。`hash:m/n` 按二进制 ID 和测试名哈希，增删测试不改变其他测试的桶；`slice:m/n` 轮转分配；`count:` 已废弃。https://nexte.st/docs/ci-features/partitioning/
- **Bazel**：`shard_count` 属性和 `--test_sharding_strategy=explicit|disabled|forced=k`，没有用历史耗时。https://bazel.build/reference/command-line-reference
- **Xcode 并行测试**：按测试类分发到克隆的模拟器，同一个类的测试在同一台模拟器上跑。Grab 用“每个测试一个唯一类名 token”的方法让测试逐个分发，不均衡从数分钟降到约 0.4 分钟；其项目约一半测试耗时 20–40 s。https://engineering.grab.com/tackling-ui-test-execution-time-imbalance-for-xcode-parallel-testing
- 对 kr：kr 的场景就是若干测试类的组合，相当于 Tuist 的 suite 粒度。LPT + 无历史时用中位数，是业界已用的简单方案。（推论）

## 3. Dry-run / plan / explain 的先例

| 工具 | 功能 | 对 kr 的意义 |
|---|---|---|
| Bazel `--nobuild` | 在执行动作前停止；加载和分析成功则返回 0 | `kr plan` 只做选择和排序 |
| Bazel `--check_up_to_date` / `--check_tests_up_to_date` | 不构建 / 不跑测试，只检查是否最新；有需要执行的步骤就报错 | `kr plan --check`：缓存会不会命中，用退出码表示 |
| Bazel `--explain=<file>` | 解释每个被执行步骤的原因 | 每个构建、每个场景写“为什么要跑” |
| Bazel `--execution_log_compact_file` | 记录所有执行过的 spawn（zstd 压缩）；官方推荐比较两次日志来找缓存未命中 | 比较两次运行的输入哈希来源 |
| Bazel `--generate_json_trace_profile` | Chrome trace 格式，含 Critical Path 一栏 | `kr` 事件可以导出为 Chrome trace |
| Bazel `aquery` + `aquery_differ` | 在分析后的动作图上查询；比较两次 aquery 输出 | 比较两个配置下的计划 |
| Buck2 `log what-ran` / `critical-path` / `slowest-path` / `replay` / `diff action-divergence` | 输出执行原因和复现方法；关键路径；重放事件日志；找两次构建第一个不同的动作 | `kr log` 子命令的样板 |
| nextest `list` | 构建并列出测试，应用与 `run` 相同的默认过滤器；支持 JSON | `kr plan` 的选择结果要和 `kr run` 完全一致 |
| Terraform `plan` | 只提出变更，不执行；`-out` 保存计划；`-detailed-exitcode`：0 无变化，1 错误，2 有变化 | `kr plan --json` + 退出码；“speculative plan” 要在执行前重新检查 |

来源：
- Bazel 选项：https://bazel.build/reference/command-line-reference
- Bazel 执行日志比较：https://bazel.build/remote/cache-remote
- Bazel profile：https://bazel.build/advanced/performance/json-trace-profile
- aquery：https://bazel.build/query/aquery
- Buck2：https://buck2.build/docs/users/commands/log/
- nextest list：https://nexte.st/docs/listing/
- Terraform：https://developer.hashicorp.com/terraform/cli/commands/plan

补充：Terraform 文档说，不带 `-out` 的计划是“speculative plan”，执行前目标系统可能已变化，要重新检查最终计划。对 kr：`kr plan` 的结果只在当时的队列状态下有效，输出里要写明基于哪个时刻。（推论）

## 4. 在历史数据上重放和评估测试策略

### 4.1 Meta：Predictive Test Selection（ICSE SEIP 2019）
- 按时间切分：最近一周做测试集，其余做训练集；模型用最近三个月的变更训练。作者说这“贴近模型实际使用的方式”。https://ar5iv.labs.arxiv.org/html/1810.05286
- 真值来自“learning test run”：对约四分之一的提交，在非高峰时段跑全部依赖测试。同上。
- flaky 处理：失败的测试目标最多重试 10 次；全失败 = FailedTests，有失败也有通过 = FlakedTests；FlakedTests 总数约为 FailedTests 的 4 倍。同上。
- 上线目标：测试召回 > 0.95，变更召回 > 0.999，选择率 < 0.33。同上。博客称只跑传递依赖测试的三分之一，抓到 99.9% 以上的回归。https://engineering.fb.com/developer-tools/predictive-test-selection/

### 4.2 Google TAP（ICSE SEIP 2017）
- 一个里程碑最多 4.2 million 个受影响测试；基础设施问题造成最长 9 小时的延迟。https://huang.isis.vanderbilt.edu/cs8395-f23/paper/google-testing-icse-seip-17.pdf
- 作者决定不使用逐个测试的 PASSED/FAILED 历史，因为会“主要在重跑 flaky 测试”；改为建模 breakage（PASSED→FAILED）和 fix（FAILED→PASSED）。同上。
- 在历史数据上模拟 MinDist ≤ 10 和 ≤ 6：只执行 61% 和 50% 的测试目标，资源节省 42% 和 55%，没有漏掉任何 breakage 或 fix。同上。
- 对 kr：判定重放的核心问题也是“状态转换”：一次 PASS→FAIL 是代码问题还是 flaky。（推论）

### 4.3 Apple：Modeling and Ranking Flaky Tests（ICSE SEIP 2020）
- 把 flaky 看作“可排序的值”，用两种模型：结果的随机性（entropy）和随时间的翻转频率（flipRate）；在两个大型服务上，flaky 减少 44%，故障检测损失 < 1%。https://2020.icse-conferences.org/details/icse-2020-Software-Engineering-in-Practice/2/Modeling-and-Ranking-Flaky-Tests-at-Apple
- 对 kr：每个场景保存 flipRate（相邻两次结论不同的比例），按指数加权平均，作为 flaky 排名。这是 Apple 自己在服务端测试上用过的方法。（推论）

### 4.4 Meta：Probabilistic Flakiness Score
- 每个测试两个参数：坏状态概率 pb、好状态下失败概率 pf；PFS = pf。用 Stan 做贝叶斯推断，输出后验分布；每个测试拟合不到一秒；只用日常 CI 结果。2018 年中起使用。单元测试的 flaky 底线“远低于 1%”，部分端到端框架达到 10%。https://engineering.fb.com/2020/12/10/developer-tools/probabilistic-flakiness/
- 重试固定在同一配置和同一外部服务版本上，以便假设两次尝试之间“世界状态”不变。同上。
- 对 kr：重跑必须在同一构建产物、同一模拟器 runtime 上，否则重跑结果不能用于 flaky 判断。kr 现在“重跑一次”的设计符合这一点。（推论）

### 4.5 Xcode 自带的重复执行
- Xcode 13 起有三种测试重复模式：固定次数、直到失败、失败重试；对应 `-test-iterations`、`-run-tests-until-failure`、`-retry-tests-on-failure`。Apple 警告失败重试可能掩盖产品的真实问题。https://developer.apple.com/videos/play/wwdc2021/10296/
- 对 kr：第 4 层的“加密采样”可以直接用 `-test-iterations N -run-tests-until-failure` 跑单个可疑场景。（推论）

## 5. 资源 / 负载与 flaky 的实证数据

| 来源 | 数据 |
|---|---|
| Google 2016（Micco） | 约 1.5% 的测试运行结果是 flaky；约 16% 的测试有某种程度的 flaky；观察到的 PASS→FAIL 转换中约 84% 涉及 flaky 测试；可把测试标为 flaky，只有连续失败 3 次才报失败 |
| Google 2017（Listfield） | 4.2 million 测试中一周约 63,000 个有 flaky 运行（< 2%）；小 / 中 / 大测试 0.5% / 1.6% / 14%；二进制大小、RAM 与 flaky 比例 r² = 0.82 / 0.76；Android 模拟器测试 25.46% flaky；RAM 第 95 百分位预测 flaky 概率 9.2%（全部测试） |
| Silva 等（RAFT） | 52 个项目，27 种资源配置，每种配置 300 次；608 个 flaky 测试中 283 个（46.5%）是 RAFT；CPU 是 Java / JS 的主要因素；推荐配置 0.5 CPU + 2 GiB 用来探测 |
| Gruber 等（Python） | 7,571 个 flaky 测试：59% 顺序依赖，28% 测试基础设施问题；95% 确信不 flaky 平均需 170 次重跑 |
| Lam 等（ISSRE 2020） | 每个测试套件重跑 4,000 次来识别 flaky |
| FlakeFlagger 数据集 | 24 个项目各重跑 10,000 次，仍有未发现的 flaky 测试 |
| iDFlakies | 422 个 flaky 测试，50.5% 顺序依赖 |

来源：
- https://testing.googleblog.com/2016/05/flaky-tests-at-google-and-how-we.html
- https://testing.googleblog.com/2017/04/where-do-our-flaky-tests-come-from.html
- https://ar5iv.labs.arxiv.org/html/2310.12132 （期刊版见 https://conf.researchr.org/details/fse-2025/fse-2025-journal-first/45/The-Effects-of-Computational-Resources-on-Flaky-Tests ）
- https://arxiv.org/abs/2101.09077
- https://cs.gmu.edu/~winglam/publications/2020/LamETAL20ISSRE.pdf
- https://zenodo.org/record/4450723
- https://cs.gmu.edu/~winglam/publications/2019/LamETAL19iDFlakies.pdf

缺口：没有找到 Apple 官方关于“XCUITest 在高负载下 flaky”的数字，也没有找到 Apple 官方给出的每台模拟器克隆的 CPU / 内存占用。kr 只能用自己的数据。

对 kr 的推论：
- Google 的 “连续失败 3 次才报失败” 和 kr 的 “重跑一次” 是同一类规则，可以在第 3 层用历史数据比较两者，前提是有足够的重复数据（见 0.1 改动 3）。
- “84% 的 PASS→FAIL 涉及 flaky” 说明：只看一次失败就判红，噪声很大。kr 的报告应该把“新失败”和“已知 flaky 场景的失败”分开。

## 6. 小样本前后对比的统计方法

- **Mann-Whitney U**：Google Benchmark 的 `compare.py` 使用 U 检验，原假设为两组无差异；警告“至少 9 次重复”才有意义。https://github.com/google/benchmark/blob/main/docs/tools.md
- **bootstrap + 噪声阈值**：criterion.rs 默认 100 个样本、预热 3 s、测量 5 s、bootstrap 100,000 次、噪声阈值 1%、置信水平 0.95、显著性 0.05；噪声阈值内的变化被忽略。离群点用改进的 Tukey 法（1.5×IQR 轻度，3×IQR 严重），只标记不剔除。https://docs.rs/criterion/latest/criterion/struct.Criterion.html ；https://bheisler.github.io/criterion.rs/book/analysis.html
- **hyperfine**：默认至少 10 次，目标约 3 秒；支持预热；报告“均值 ± 标准差”，以第一条命令为基准算相对变化。https://github.com/sharkdp/hyperfine
- **置信区间而不是“最好的一次”**：Georges 等：若置信区间重叠，就不能说差异不是随机波动；“best of 30” 方法在最多 16% 的比较中结论过强。https://dri.es/files/oopsla07-georges.pdf
- **重复次数怎么定**：Kalibera、Jones 提出按实验的每一层（构建、进程、迭代）的方差，算出每层需要的重复次数。https://kar.kent.ac.uk/33611/
- **零失败时的上限**：0 次失败 / n 次时，95% 上限约 3/n。https://jamanetwork.com/journals/jama/article-abstract/389714

对 kr 的推论：
- 整次门禁当一个样本，n 永远太小。用场景做配对样本，n ≈ 30。
- Kalibera-Jones 的思路对 kr 有用：先测“同一设置下重复两次的场景耗时差异”，这就是噪声底，决定噪声阈值和需要的重复次数。

## 7. 对四层方案的逐层判定

| 层 | 判定 | 主要修改 |
|---|---|---|
| 1 `kr plan` | 支持 | 选择逻辑和 `run` 共用；`--json`；退出码区分“无事可做 / 有事要做 / 错误”；写明“为什么要跑” |
| 2 `kr simulate` | 有条件支持 | 跑真实调度代码 + 虚拟时钟；耗时用分布；按并发度分桶；输出是否超出校准范围；只对顺序类改动给定论 |
| 3 判定重放 | 支持，但有边界 | 只能重判已有证据；需要定期“加密采样”造真值；报告“含 flaky”和“不含 flaky”两套数字 |
| 4 小样本真实运行 | 必需 | 容量、负载阈值、lane 的改动必须走这一层；场景配对 + bootstrap；ABAB 交替 |

---

## 来源清单

调度模拟与校准
- Borg（EuroSys 2015）：https://research.google.com/pubs/archive/43438.pdf
- Omega（EuroSys 2013）：https://research.google.com/pubs/archive/41684.pdf
- Shmueli & Feitelson（TPDS 2009）：https://www.cse.huji.ac.il/~feit/papers/UserAwareSched09TPDS.pdf
- Shmueli 博士论文：https://www.cse.huji.ac.il/labs/parallel/stud/Shmueli-PhD.pdf
- Sargent，Verification and Validation of Simulation Models（WSC 2004）：https://www.informs-sim.org/wsc04papers/004.pdf
- Gavel（OSDI 2020）：https://arxiv.org/pdf/2008.09213
- Shockwave：https://arxiv.org/pdf/2210.00093
- MoSim（arXiv 2609.23278）：https://arxiv.org/abs/2609.23278

调度与分片
- Graham 1969：https://epubs.siam.org/doi/10.1137/0117039
- LPT 条目（公式转引）：https://en.wikipedia.org/wiki/Longest-processing-time-first_scheduling
- Tuist Xcode 分片：https://tuist.dev/en/docs/guides/features/test-sharding/xcode
- Tuist 分片博客：https://tuist.dev/blog/2026/03/25/test-sharding
- nextest 分片：https://nexte.st/docs/ci-features/partitioning/
- Grab Xcode 并行测试：https://engineering.grab.com/tackling-ui-test-execution-time-imbalance-for-xcode-parallel-testing

dry-run / explain
- Bazel 命令行参考：https://bazel.build/reference/command-line-reference
- Bazel 远程缓存调试：https://bazel.build/remote/cache-remote
- Bazel JSON trace profile：https://bazel.build/advanced/performance/json-trace-profile
- Bazel aquery：https://bazel.build/query/aquery
- Buck2 log：https://buck2.build/docs/users/commands/log/
- nextest list：https://nexte.st/docs/listing/
- Terraform plan：https://developer.hashicorp.com/terraform/cli/commands/plan

重放与 flaky
- Predictive Test Selection（ICSE SEIP 2019）：https://ar5iv.labs.arxiv.org/html/1810.05286
- Meta 博客：https://engineering.fb.com/developer-tools/predictive-test-selection/
- Taming Google-Scale Continuous Testing（ICSE SEIP 2017）：https://huang.isis.vanderbilt.edu/cs8395-f23/paper/google-testing-icse-seip-17.pdf
- Modeling and Ranking Flaky Tests at Apple（ICSE SEIP 2020）：https://2020.icse-conferences.org/details/icse-2020-Software-Engineering-in-Practice/2/Modeling-and-Ranking-Flaky-Tests-at-Apple
- Meta Probabilistic Flakiness：https://engineering.fb.com/2020/12/10/developer-tools/probabilistic-flakiness/
- WWDC21 Diagnose unreliable code with test repetitions：https://developer.apple.com/videos/play/wwdc2021/10296/
- Google Testing Blog 2016：https://testing.googleblog.com/2016/05/flaky-tests-at-google-and-how-we.html
- Google Testing Blog 2017：https://testing.googleblog.com/2017/04/where-do-our-flaky-tests-come-from.html
- The Effects of Computational Resources on Flaky Tests：https://ar5iv.labs.arxiv.org/html/2310.12132
- An Empirical Study of Flaky Tests in Python：https://arxiv.org/abs/2101.09077
- Lam 等 ISSRE 2020：https://cs.gmu.edu/~winglam/publications/2020/LamETAL20ISSRE.pdf
- FlakeFlagger 数据集：https://zenodo.org/record/4450723
- iDFlakies：https://cs.gmu.edu/~winglam/publications/2019/LamETAL19iDFlakies.pdf

统计
- Google Benchmark compare.py：https://github.com/google/benchmark/blob/main/docs/tools.md
- criterion.rs 默认值：https://docs.rs/criterion/latest/criterion/struct.Criterion.html
- criterion.rs 分析方法：https://bheisler.github.io/criterion.rs/book/analysis.html
- hyperfine：https://github.com/sharkdp/hyperfine
- Georges 等（OOPSLA 2007）：https://dri.es/files/oopsla07-georges.pdf
- Kalibera & Jones（ISMM 2013）：https://kar.kent.ac.uk/33611/
- Hanley & Lippman-Hand（JAMA 1983）：https://jamanetwork.com/journals/jama/article-abstract/389714
