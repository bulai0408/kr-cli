# 多代理并行开发：2025–2026 现状调研（面向单人 + 单 Mac 场景）

调研日期：2026-10-14。每条结论后附来源。标注"厂商"的来源是厂商自述，未经独立验证。

## 0. 背景与痛点对照

当前流程：3–4 个 Codex / Claude Code 代理，每个代理一个 worktree + 分支；一个协调者代理审批设计、合并到集成分支、跑组合门禁（单元测试 + 约 30 个模拟器 UI 场景）；一台 Mac，一把全机重活锁。

| 实测痛点 | 业界对应机制 | 本文章节 |
|---|---|---|
| 锁队列占满墙钟时间 | 两段式 CI、批处理、build-once-test-many、编译缓存 | 1、3、4 |
| 拿到锁后才发现编译错误 | 便宜检查前移到每次推送；轻量预检不占重锁 | 1.5、3 |
| 同模块工单需接口协商、合并冲突 | 文件/模块所有权、契约先行、tracer bullet、按影响目标分道 | 2.3、1.4 |
| 每个心跳都唤醒协调者 | 只在"需决策"事件上通知；产物写文件只回传引用 | 2.5 |
| 代理中途耗尽周配额 | 周配额机制、token 成本随代理数线性增长 | 2.6 |
| 每批都在同一热点文件冲突 | rerere、merge-tree 预检、热点文件拆分/归属 | 2.4 |

---

## 1. 合并队列与堆叠变更

### 1.1 基本原则："Not Rocket Science Rule"
- 规则：自动维护一个"永远通过全部测试"的仓库——先在合并结果上跑测试，通过才推进主干。bors 就是这条规则的实现。来源：https://graydon2.dreamwidth.org/1597.html
- bors 解决的问题：两个 PR 各自单独通过 CI，但合并后失败（例如一个改名函数、另一个新增调用）。bors 在合并结果进入 master 前检验组合。bors-ng 公共实例已在逐步下线，转向 GitHub Merge Queue。来源：https://bors.tech/essay/2017/02/02/pitch/

### 1.2 GitHub Merge Queue（串行语义 + 并发构建）
- Build concurrency（1–100）限制同时派发的 `merge_group` 构建数；后面的组总是包含前面的变更。来源：https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue
- Merge limits（最小/最大组大小 + 等待时间）只控制"已验证的组一次落地多少个"，不合并 CI 构建。来源：同上
- 关闭 "Only merge non-failing pull requests" 时，组内某 PR 失败但组尾 PR 通过即可整体合并——尾部结果覆盖了组合变更，可缓解 flaky 误杀。来源：同上
- 组失败时：移除失败 PR，并**重建**后续组的临时分支（不含被移除的 PR）后重测。来源：同上

### 1.3 Trunk Merge Queue（乐观 + 批处理 + 二分）
- Predictive testing：每个 PR 在"main 当前头 + 队列前方所有 PR"上测试，是其他优化的基础。来源：https://docs.trunk.io/merge-queue/optimizations
- Batching：多个 PR 组成一次测试；批失败后对批做二分，隔离元凶，其余回到主队列。来源：https://docs.trunk.io/merge-queue/concepts-and-optimizations/batching
- 避免重复测试："Skip Redundant Bisection Tests"——若二分中某子集已以同一 PR 集合失败过，则复用该失败、跳过 CI；**只复用失败，从不复用通过**；flaky 原始失败可能误伤，需配合防 flaky。来源：同上
- Optimistic merging：后一个 PR 通过即同时验证了前面的变更，可把前面"失败"的一起合并；Pending failure depth：失败组先不剔除，等后继结果。两者合称 Anti-Flake Protection。来源：https://docs.trunk.io/merge-queue/optimizations
- 推荐启用顺序：先 predictive testing，再 batching，最后 optimistic merging。来源：https://docs.trunk.io/merge-queue/optimizations/optimistic-merging

### 1.4 按影响面分道（"图"而不是"线"）
- Trunk Parallel Queues：每个 PR 上报 impacted targets（Bazel 目标或目录名）；无交集的 PR 在独立车道并行测试，有交集的 PR 对相关 PR 做预测测试。例：A[frontend]、B[backend]、D[docs] 并行，C[frontend,backend] 叠在 A、B 之上测试。来源：https://docs.trunk.io/merge-queue/optimizations/parallel-queues
- Uber SubmitQueue：对待合并变更构建推测树，用概率模型 + ML 排序最可能需要的构建；用目标级冲突分析剪枝，独立变更并发构建；小变更可绕过大冲突变更（依赖可交换性）。21 周评估：CI 资源 -53%，等待 -37%；iOS P95 等待 14.86→9.91 分钟。来源：https://www.uber.com/us/en/blog/slashing-ci-costs-at-uber/

### 1.5 AI 代理时代的合并队列观点（厂商）
- 代理让 PR 量放大，单车道队列成为瓶颈；建议：推测测试、批处理、按范围分队列、**两段式 CI（每次推送跑便宜检查，昂贵套件只在队列里跑）**、由队列负责 rebase、以"变更"而非 SHA 作为身份（rebase 不应作废测试结果）、给代理可查询的测试健康分类（healthy/flaky/broken）。来源（厂商）：https://mergify.com/blog/merge-queues-and-ai-coding-agents

### 1.6 堆叠变更（Graphite）
- Graphite 批处理：把多个 stack 依次 rebase 成一个临时草稿 PR，CI 通过后 main 快进到其 HEAD；批失败的二分在文章中标为"即将推出"。代价：不保证历史中每个提交可构建，只保证批合并后主干头部通过。来源（厂商）：https://graphite.com/blog/merge-queue-batching

**小结（单人可用）**：队列语义本身可以用脚本实现，不需要 SaaS。关键是三件事：(1) 便宜检查前移，昂贵门禁只跑一次；(2) 批处理 + 失败时二分，只复用失败结果；(3) 按模块分道，无交集的工单不互相等。

---

## 2. 多代理并行的工具与做法

### 2.1 隔离方式
- Claude Code `--worktree`：在 `.claude/worktrees/<name>/` 建隔离 worktree 和分支；`.worktreeinclude` 复制被忽略文件（如 `.env`）；子代理可在 frontmatter 写 `isolation: worktree`。worktree 共享 `.git`，但是一份全新 checkout，需要重新初始化依赖。来源：https://code.claude.com/docs/en/worktrees
- Claude Code agent teams（实验特性）：lead + teammates + 共享任务列表（文件锁防抢单）+ 邮箱；建议 3–5 人、每人 5–6 个任务；**"两个 teammate 编辑同一文件会互相覆盖，按文件划分所有权"**；同文件编辑、强依赖、顺序任务用单会话或子代理更合适。来源：https://code.claude.com/docs/en/agent-teams
- Conductor（Mac 应用）：每任务一个 worktree + 分支，统一界面看 diff、检查、合并；有来源称 5–10 个并行代理（第三方/竞品页，未核实）。来源：https://www.conductor.build/workflows/run-parallel-claude-codes 、https://www.morphllm.com/conductor-ai-coding
- container-use（Dagger，MCP 服务）：每个代理一个容器 + 一个 git 分支，可看完整命令历史、可接管终端，`cu merge` 合并；项目处于早期。来源：https://container-use.com/introduction 、https://dagger.io/blog/agent-container-use
- Sculptor（Imbue）：每个 Claude 在自己的容器里；Pairing Mode 把容器状态镜像到本地 IDE。来源：https://docs.imbue.com/features/containers
- Codex cloud：从一个已发布环境启动多个任务，每个任务独立工作区，电脑睡眠时也能继续。来源：https://learn.chatgpt.com/docs/cloud
- jj workspaces：`jj workspace add` 提供多工作副本；社区观点是先用"共享对象缓存 + 每任务一个 git worktree"，只有当堆叠、改历史、冲突成本高于检出成本时再试 jj（社区来源）。来源：https://wavect.io/blog/git-worktrees-vs-jujutsu-ai-coding-agents/

### 2.2 真正的瓶颈是审查，不是生成
- Simon Willison：并行的天然瓶颈是"我审查结果的速度"；一次只能认真审一个重要变更，但调研、代码解释、小维护、**已写好详细规格的工作**可以并行而不增加太多心智负担。来源：https://simonwillison.net/2025/Oct/5/parallel-coding-agents/
- Anthropic 多代理研究系统：多代理约耗 15 倍 chat 的 token；**编码比研究更难并行**（真正可并行的子任务少）；代理之间实时协调与委派能力尚不成熟；每份委派要写清目标、输出格式、工具指引、任务边界，否则重复或遗漏。来源：https://www.anthropic.com/engineering/multi-agent-research-system

### 2.3 分工以避免冲突
- 文件/模块所有权：见 agent teams 文档"按文件集划分"。来源：https://code.claude.com/docs/en/agent-teams
- Tracer bullet / 垂直切片：先打通一条贯穿所有层的细切片并立即验证，再逐片扩展；代理倾向一次性产出整层代码，导致集成问题后置。来源：https://www.aihero.dev/tracer-bullets
- 契约先行的依据：Uber 的经验是"冲突分析"决定能否并行——独立变更才可并发且可交换顺序。来源：https://www.uber.com/us/en/blog/slashing-ci-costs-at-uber/ 。推论（非来源原话）：两个工单若共享接口，应先单独合入接口（契约工单），再并行实现两端。
- Google 数据：多于 3 名开发者近期修改过的代码更容易出故障。来源：https://research.google/pubs/taming-google-scale-continuous-testing/ 。推论：同一热点文件被多个代理同批修改，应视为高风险信号。

### 2.4 热点文件冲突的工具
- `git merge-tree --write-tree A B`：不碰工作区和索引即可完成一次合并并报告冲突文件（退出码 1 表示冲突）——可在派单前/合并前廉价预检两个分支是否冲突。来源：https://git-scm.com/docs/git-merge-tree
- `git rerere`：记录冲突解决方式，相同冲突再出现时自动复用。适合"每批同一热点文件冲突"。来源：https://git-scm.com/docs/git-rerere

### 2.5 通知设计：只在需要决策时唤醒
- Claude Code `Notification` hook 按类型匹配：`permission_prompt`、`idle_prompt`、`agent_needs_input`、`agent_completed`、配额自动恢复等；可只对"等待输入"发声。`Stop` hook 可用退出码 2 阻止结束并要求继续（可用作"未达门禁不得结束"）。来源：https://code.claude.com/docs/en/hooks
- Agent teams 的 `TeammateIdle` / `TaskCompleted` hook 可用退出码 2 退回工作，作为质量门。来源：https://code.claude.com/docs/en/agent-teams
- Codex CLI 外部 `notify` 只在 `agent-turn-complete` 触发；审批提醒走 `[tui] notifications`（社区来源，需核对官方 config 文档）。来源：https://codex.danielvaughan.com/2026/04/13/codex-cli-notification-pipeline-osc9-hooks-alerts/
- 协调者负载：Anthropic 建议子代理把产物写入外部存储，只把轻量引用回传给协调者，避免"传话游戏"与 token 浪费；同步等待会让最慢的子代理阻塞整体。来源：https://www.anthropic.com/engineering/multi-agent-research-system

### 2.6 共享资源：配额、构建缓存、模拟器
- 配额：Anthropic 2025-08-28 起对 Pro/Max 加两项每 7 天重置的周上限（总量 + Opus），5 小时窗口保留；Max 可按 API 价超额。来源：https://gigazine.net/gsc_news/en/20250729-anthropic-calude-code-rate-limits 、https://en.ain.ua/2025/07/30/anthropic-introduces-weekly-limits-for-claude-code/
- token 成本随 teammate 数线性增长；常规任务单会话更省。来源：https://code.claude.com/docs/en/agent-teams
- Xcode 26 编译缓存：opt-in，对"切分支后重编相同源文件"和 clean build 收益最大（Apple 说法）。设置键 `COMPILATION_CACHE_ENABLE_CACHING`、缓存仅本机、SPM 项目效果存疑——均为第三方报告。来源：https://developer.apple.com/documentation/xcode-release-notes/xcode-26-release-notes 、https://forums.swift.org/t/about-swift-shared-cache-across-machines/81850 、https://tuist.dev/en/docs/guides/features/cache/xcode-cache
- 多 worktree 意义：每个 worktree 是独立路径，DerivedData 默认按路径区分，切分支/多 worktree 正是编译缓存宣称收益的场景（推论）。
- Build once, test many：`xcodebuild build-for-testing` 产出 `.xctestrun`，`test-without-building -xctestrun ... -only-testing` 复用同一构建跑子集。来源：https://docs.saucelabs.com/mobile-apps/automated-testing/espresso-xcuitest/xctest/
- 模拟器并行：`-parallel-testing-worker-count` 指定 runner 数；UI 测试依赖模拟器，资源重、不可无限扩展。来源：https://tuist.dev/blog/2025/03/25/tests 。Xcode 26.2 有报告称第二次运行时找不到上次的克隆模拟器。来源：https://developer.apple.com/forums/thread/817089

---

## 3. 测试影响分析 / 预测性测试选择

- Meta Predictive Test Selection：梯度提升树，用历史变更与测试结果训练；只跑"传递依赖于修改代码的测试"中的约三分之一，仍在进入主干前捕获 >99.9% 的回归；测试基础设施效率约翻倍；训练数据中对失败测试大量重试以区分 flaky。来源：https://engineering.fb.com/2018/11/21/developer-tools/predictive-test-selection/
- Google TAP：无法对每次变更跑全部测试；极少数测试会失败，失败的测试往往离被改代码"更近"；某些高频修改代码、用户、工具更易引发故障。来源：https://research.google/pubs/taming-google-scale-continuous-testing/
- Launchable：子集目标可选 `--confidence`（如 90% 表示捕获 9/10 的失败会话）、`--time`、`--target`；置信曲线由历史会话训练/评估得出。来源：https://help.launchableinc.com/features/predictive-test-selection/requesting-and-running-a-subset-of-tests/choosing-a-subset-optimization-target/

**对单人可行的做法**（推论，基于上面"失败离被改代码近"的发现）：不训练模型，用静态映射——"改动的模块/目录 → UI 场景标签"，再加一个固定的冒烟集。合并前跑受影响场景 + 冒烟；全量 30 个场景只在发布前或合并 main 前跑。约 30 个场景、每批几张工单，历史数据量不足以训练 ML 选择器。

---

## 4. Flaky 测试隔离

- Fowler：隔离（quarantine）是急救——把非确定性测试移出主套件，让健康测试继续给出可信信号；但隔离只是买时间，应限制隔离区大小并修复根因（隔离、异步、远程服务、时间、资源泄漏）。来源：https://www.martinfowler.com/articles/nonDeterminism.html
- Trunk：被隔离的测试**仍然运行并上报**，只是失败不阻塞（CLI 把仅含隔离测试失败的 job 退出码改为 0）；自 2025-09 起自动隔离只针对 flaky，不针对 broken。来源：https://docs.trunk.io/flaky-tests/quarantining 、https://docs.trunk.io/changelog/2025-09-09-flaky-tests-auto-quarantine-no-longer-applied-to-broken-tests
- Google：重跑机制只用于被标记为 flaky 的测试或用户明确要求时；按配置分别判定 flaky。来源：https://testing.googleblog.com/2016/05/flaky-tests-at-google-and-how-we.html
- 合并队列层面的抗 flaky：GitHub 的"组尾通过即可合并"、Trunk 的 optimistic merging + pending failure depth。来源：见 1.2、1.3

---

## 5. 度量：如何衡量代理工作流效率

- DORA 五指标：变更前置时间、部署频率、失败部署恢复时间、变更失败率、部署返工率。来源：https://dora.dev/guides/dora-metrics-four-keys/
- 合并队列指标（Trunk）：Time in Queue（入队到出队，按 p50/p95 看）、出队结论计数（通过/失败/取消）、队列中 CI 时间、触发的 CI 作业数（作业越多机器竞争越大）；可按 impacted target 分组看哪个模块慢。来源：https://docs.trunk.io/merge-queue/administration/metrics
- 吞吐目标：每小时能在一致的主干上落地多少正确变更，而不是队列深度（厂商观点）。来源：https://mergify.com/blog/merge-queues-and-ai-coding-agents

**建议本地记录的字段**（推论，映射上述指标到本场景）：
1. 每张工单：派单→合入集成分支的前置时间。
2. 每次重活：等锁时间 vs 持锁运行时间（排队/运行比，Uber 与 Trunk 都把等待时间当首要指标）。
3. 返工轮数：门禁失败后回到代理的次数，按原因分类（编译错误 / 测试失败 / 合并冲突 / 审查意见 / flaky）。
4. 协调者被唤醒次数，以及其中"需要决策"的比例。
5. 每张工单 token / 配额消耗。

---

## 6. 建议借鉴（按影响从大到小）

1. **两段式检查，编译错误不进锁队列**。代理在每次提交时先跑不占重锁的便宜检查（类型检查/轻量单模块测试，走轻锁），通过后才排重锁。对应 Mergify"两段式 CI"、Trunk"先便宜后昂贵"。直接解决"等完锁才发现编译错误"。
2. **集成门禁 build-once-test-many + 影响选择**。协调者对合并后的集成分支只 `build-for-testing` 一次，再 `test-without-building -only-testing` 跑"受影响场景 + 固定冒烟集"；全量 30 场景只在合 main 前跑。依据：Meta/Google"失败测试离改动近"、Xcode xctestrun。
3. **批处理 + 失败二分，只复用失败结果**。一批 3–4 个分支一起合、一起测；失败时二分定位元凶，其余继续；绝不缓存"通过"。依据：Trunk/GitHub Merge Queue。
4. **派单前做冲突预测并按模块分道**。派单前用 `git merge-tree --write-tree` 两两预检候选分支；工单声明"拥有的模块/文件"，有交集的工单不同批或串行；共享接口先拆成独立的契约工单先合入。依据：Trunk parallel queues、Uber 冲突分析、agent teams 文件所有权。
5. **热点文件治理**。开启 `git rerere`；统计每批冲突文件，反复冲突的文件（如注册表、路由表、设置聚合）拆分成每模块一个文件或指定单一所有者。依据：git-rerere、Google">3 人修改更易坏"。
6. **通知只在决策点**。协调者只订阅"需要输入/需审批/完成/失败"事件，心跳写文件不发消息；代理产物写文件、只回传路径。依据：Claude Code Notification 匹配器、Anthropic 多代理系统。
7. **Flaky 隔离仍运行、不阻塞、有上限**。flaky UI 场景移出必需集但继续运行记录；隔离区设上限，超限必须先修一个。依据：Fowler、Trunk、Google。
8. **配额预算**。派单前估算每张工单配额；周配额剩余低于阈值时不派新单，或改用另一家代理；并行数按配额而非按机器定。依据：Anthropic 周配额、token 线性增长。
9. **并行度以审查能力为上限**。同时只让 1 个"重要变更"等待审查，其余并行槽位留给调研/小维护/规格清晰的工作。依据：Simon Willison、Anthropic"编码难并行"。
10. **记录排队/运行比与返工轮数**。先测量再调整批大小与并行数（第 5 节字段）。
