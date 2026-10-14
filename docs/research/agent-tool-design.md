# 给 agent 用的 CLI：业界怎么设计（kr 第二、三阶段的总体方向调研）

2026-10-14，协调者亲自调研（未派子 agent）。问题：kr 本身不懂判断（没有模型），自动选测、合并辅助、失败分类、自动报告、信任策略这些需要「智能」的事，应该怎么做？

## 结论先说
1. **业界一致的做法：CLI 只做确定的事，智能留在调用它的 agent 里，中间靠 skill 连接。** CLI 不内置模型。
2. CLI 要做的是三件事：**把事实算准、把需要判断的地方明确交出去、把 agent 的决定记下来并验证**。
3. skill 由 CLI 自己输出（版本永远一致），agent 的说明文件（AGENTS.md）只放一句引导。
4. 这条路线对 kr 是对的，但「交出判断」和「记录并验证判断」这两件事，业界的现成工具大多只做了一半。kr 可以做完整：判断 → 验证 → 记录 → 学习 → 按证据放手。

## 看了哪些、各自的关键做法

### Orca（我们每天在用）
- CLI 只提供原语：Run、Task、Dispatch、消息、`ask`/`reply`、`gate-create`/`gate-resolve`。**编排的智慧不在 CLI 里，在 skill 里。**
- skill 分两层：装在 agent 里的只是一个「发现入口」（`~/.claude/skills/orchestration/SKILL.md`，3.6K，写明「完整说明由 `orca skills get orchestration` 输出」）；正文由程序本身输出，**版本和程序永远一致，不会过时**。原话：完整参考 "is served by the orca binary itself — kept out of this file on purpose so it can never drift from the binary"。
- 正文再分层：主文件讲常规流程，少见情况放 `--reference references/<名字>.md`，按需加载。
- **判断交给 agent，但 CLI 把判断变成有记录的事实**：`ask` 留下一条持久问题，`gate-create` 记下「这里要做决定、有哪些选项」，`gate-resolve` 记下选了什么。CLI 不判断，但每个判断都有记录、可追溯。
- 安全底线写在 CLI 里而不是 skill 里：「只有确凿的退出证据才允许停止、放弃或重试」，看不清时报告 `unverifiable`，不猜。

### Anthropic《Writing effective tools for agents》
- 工具是「确定的系统」和「不确定的 agent」之间的合同。
- 少而精：把多步操作合成一个调用（例如「查日志」直接返回相关的几行，不是整份日志）。
- 返回高信号的信息：用人能读的名字，不用一串 ID；提供「简要 / 详细」两档输出。
- 错误信息要写清楚「怎么改」，并带一个正确的例子，让 agent 能自己纠正。
- **用评估来改工具**：记录 agent 调用了几次、出了几次参数错、绕了哪些弯，据此改工具和说明。说明文字的小改动能带来很大提升。

### Anthropic Agent Skills
- 三层按需加载：名字和一句描述常驻 → 需要时读正文 → 更细的放单独文件。
- **确定的操作用代码（可重复、一致），需要判断的用文字说明。** 这正是「kr 做确定的事、skill 教 agent 判断」的分工。

### Beads（Steve Yegge 给 agent 用的任务追踪 CLI）
- `bd prime` 输出「给 AI 看的工作流说明」，是唯一来源；各家 agent 的说明文件只放一个指向 `bd prime` 的引导。能挂启动钩子的 agent（Claude、Gemini）在会话开始时自动注入；不能挂的才把完整说明写进 AGENTS.md。
- 允许按项目或全局覆盖这段说明（`.beads/PRIME.md`、`~/.config/beads/PRIME.md`）。
- 命令面是「看 agent 实际尝试调用什么，就补什么」长出来的。

### Arcjet CLI（专门为 agent 设计）
- 退出码分类：0 成功、2 认证、3 输入错误、4 **需要确认**。agent 靠退出码分支，不去 grep 错误文字。
- **需要确认的操作不直接执行**：返回一个「建议」结构（要改什么 + 确认用的完整命令），agent 拿给人看，人同意后 agent 带 `--confirm` 重跑。不是交互式的「确定吗？」提示。
- 详细的使用指导放在 skill 里，CLI 有一个 `skills` 命令指过去。
- 1.0 之后只加不改：可以加新参数，不改已有参数的含义。

### 「Agentic CLI Design」七原则（社区总结）
- 机器可读（`--json`、结果走 stdout、日志走 stderr、带 schema 版本）；默认不交互；可重复执行不出错；危险操作默认不执行；可观察（trace id、结构化日志）；省上下文（只返回需要的字段、先摘要后细节）；**能自我描述**（`--help --json`、命令清单可机器读）。

### Playwright CLI + skill（微软）
- 微软给编程 agent 推荐「CLI + skill」而不是 MCP：不必把一大堆工具定义预先塞进上下文；大块结果（页面快照）写到文件，agent 按需读。
- 反面意见：Checkly 认为差距在缩小；Ranger 的测试里 CLI 更省上下文但更慢。结论：对「一条命令跑很久」的 kr，CLI + skill 明显合适。

### 12-Factor Agents（HumanLayer）
- 好的 agent 产品「大部分是普通软件，在恰当的位置插几步模型判断」。
- 「工具调用就是结构化输出」「找人也用工具调用」「把错误压缩后放回上下文」。
- 这里有个和 kr 相反的视角：12-Factor 主张**程序掌握控制流**、模型只在节点上给结构化答案。kr 的场景是**agent 掌握控制流**、kr 当工具。两者不矛盾：kr 内部的流程（排队、构建、核对）由 kr 掌握，需要判断的节点以结构化方式交给 agent。

### 老牌工具里的同一个思路
- **Terraform plan / apply**：先算出「要做什么」，人（或 agent）确认后才执行。判断和执行分开，判断有记录。
- **git 的 plumbing / porcelain**：底层命令只做确定的事、输出稳定；上层体验另做。
- **Kubernetes 控制器**：人写「想要的状态」，控制器反复对账让现实趋近。判断（想要什么）和执行（怎么做到）分开。

## 落到 kr：建议的设计（待 owner 拍板）

### 1. kr 不内置模型（包括决策模型和大模型）
理由：调用 kr 的 agent 本身就是模型；再内置一个会多出凭证、网络、费用、不可复现，还把 kr 绑在某个模型上。所有来源都没有在 CLI 里内置模型的做法。

### 2. 「判断点」成为 kr 输出的一等公民
- 命令输出（`--json`）里加一个固定字段，例如 `decisions`：每项有 id、问题、选项、证据（文件路径、统计数字）、kr 的默认建议和理由。参考 Orca 的 gate 和 Arcjet 的「需要确认」结构。
- 有未决判断时用专门的退出码（参考 Arcjet 的 4），agent 不用解析文字就知道「该我判断了」。
- 例子：
  - `kr run`：遇到没见过的失败 → 判断点「这次失败是什么类型」，证据是失败特征、日志尾部路径、当时负载、这条测试的历史偶发率。
  - `kr plan`（自动选测）：依赖关系算出必跑场景；另给判断点「这些改动影响不明确，是否加跑以下场景」，附历史共同失败统计。只能加，不能删必跑场景。
  - 合并辅助：机器能合的直接合；逻辑冲突 → 判断点，附两边的需求票和提交说明。

### 3. agent 用命令把决定交回，kr 记录并验证
- 例如 `kr decide <id> <选项> --reason "..."`，或在下一次命令上带参数（`kr run --also ratings --reason ...`）。
- kr 把每个决定写进事件：谁、什么时候、依据什么证据、选了什么、理由。
- kr 事后对账：判成偶发的，重跑结果是否支持；加跑的场景，是否真的发现了问题；合并的解法，编译和测试是否通过。对账结果也写进事件。

### 4. 学习层用这些记录变准
- 历史共同失败、偶发率、失败特征库、每台机器的速度，都从事件里算，不靠模型。
- 每类判断的「agent 判对率」也从这里算 → 第三阶段的信任策略据此建议「这类判断以后不用问 owner」，owner 确认后生效。

### 5. skill 由 kr 自己输出
- `kr skill`（或 `kr prime`）打印当前版本的使用说明：常规流程（体检 → 预检 → 运行）、每类判断点怎么判、什么时候必须问人、硬底线。
- 主文件短；少见情况用 `kr skill --reference <名字>` 按需读（照 Orca）。
- 允许项目覆盖或追加（照 Beads 的 `PRIME.md`）：Krill 的特殊规则放在 Krill 仓库里，不进 kr。
- agent 侧只装一个很小的发现入口；各项目 AGENTS.md 只写一句「构建和测试用 kr，先读 `kr skill`」。

### 6. 用评估来改 kr 和 skill（照 Anthropic）
- 每批结束后看：agent 调了哪些 kr 命令、出了几次参数错、哪些判断点被判错、哪里绕了弯。据此改输出和 skill。
- 这和 ADR 0010 的离线评估合在一起：判定重放可以拿新的 skill 规则重判历史判断点。

## 当时待 owner 决定的问题（已决定）
当时列出的 3 个问题都已由 ADR 0011 决定：会影响结论的判断点停下、只影响效率的按默认继续；`kr skill` 默认随版本发布，项目和个人可以追加；作为第二阶段第一张票先做骨架。

上面第 4 节「学习层用这些记录变准」里由 kr 提出放宽建议的部分，后来由 ADR 0012 改为：信任和权限由 owner 和 agent 宿主管理，判对率只放在 `kr report` 里。

## 来源
- Orca：`orca skills get orchestration`、`--reference references/messaging-and-gates.md`、`~/.claude/skills/orchestration/SKILL.md`（本机 2026-10-14）
- https://www.anthropic.com/engineering/writing-tools-for-agents
- https://www.anthropic.com/engineering/equipping-agents-for-the-real-world-with-agent-skills
- https://github.com/steveyegge/beads （docs/SETUP.md、cmd/bd/prime.go）；https://simonwillison.net/2026/Jan/30/steve-yegge/
- https://arcjet.com/blog/designing-a-cli-for-ai-agents
- https://dev.to/tumf/agentic-cli-design-7-principles-for-designing-cli-as-a-protocol-for-ai-agents-2c10
- https://tester.army/blog/inside-playwright-cli-browser-automation-for-coding-agents ；https://www.checklyhq.com/blog/mcp-vs-cli-token-efficiency/ ；https://outpost.ranger.net/post/the-hidden-cost-of-fewer-tokens/
- https://github.com/humanlayer/12-factor-agents
