# OpenViking（OV）与 kr-cli 集成调研

日期：2026-10-14。只读调研：没有调用 remember / write / edit / forget / add_*，没有改任何记忆。

## 1. OpenViking 是什么

- 字节跳动火山引擎 Viking 团队开源的「AI Agent 上下文数据库」。上游：https://github.com/volcengine/OpenViking ，PyPI：https://pypi.org/project/openviking 。
- 许可证：本机源码副本 `~/.openviking/openviking-repo/LICENSE` 与 `pyproject.toml` 写的是 **AGPL-3.0**。网上二手资料（landscape.jimmysong.io 等）写 Apache 2.0，可能是旧版本。以源码为准。kr-cli 只通过 HTTP/CLI 调用它，不链接它的代码，所以 AGPL 不传染到 kr-cli。
- 数据模型（`~/.openviking/openviking-repo/docs/zh/concepts/`）：
  - 一切是虚拟文件系统，用 `viking://` URI 寻址（04-viking-uri.md）。顶层有 `resources/`、`user/<id>/memories/`、`user/<id>/peers/<peer>/memories/`、`user/<id>/sessions/`、技能等。
  - 三层（03-context-layers.md）：L0 摘要 `.abstract.md`（默认 ≤256 字符，用于向量召回）、L1 概览 `.overview.md`（≤4000 字符，用于 rerank 和导航）、L2 原文。L0/L1 是目录级 sidecar，由服务器异步生成。
  - 内置记忆类型（api/16-memory.md）：profile、preferences、entities、events、identity、soul、cases、trajectories、experiences、tools、skills。
  - 会话（concepts/08-session.md、api/05-sessions.md）：建会话 → `add_message` → `commit`。commit 同步归档消息，异步做摘要与「记忆抽取」（LLM 从对话里抽出上面那些类型的记忆）。返回 `task_id`，可查进度。
  - 检索（concepts/07-retrieval.md、api/06-retrieval.md）：`find`（纯语义）、`search`（带会话上下文；`mode=context` 返回按 token 预算拼好的注入块）、`grep`、`glob`、`tree`。

## 2. 本机怎么接的

- 服务器：远程自建，`~/.openviking/ovcli.conf` 中 `url` 为 `https://openviking.kami.fit:444`，`actor_peer_id: agent`，另有 API key（本文不记录）。MCP `health` 返回 healthy，存储 VikingFS。
- CLI：`/Users/cyh/.local/bin/ov` → npm 包 `@openviking/cli` 的 `bin/ov.mjs`，它按平台转调 **Rust 二进制**（源码 `openviking-repo/crates/ov_cli`，crate 名 `ov_cli`）。版本 v0.4.14。全部命令支持 `-o json`。可用：`find / search / read / ls / tree / grep / glob / add-memory（实验性）/ add-resource / add-skill / session new|add-message|add-messages|commit / task / wait`。
- Claude Code 插件：`openviking-memory@openviking` v0.4.5，装在 `~/.claude/plugins/cache/openviking/openviking-memory/0.4.5/`，在 `~/.claude/settings.json` 的 `enabledPlugins` 中启用。hooks（`hooks/hooks.json`）：
  - `SessionStart` → `session-start.mjs`：注入 profile 等启动上下文（样例见 `~/.openviking/last_inject.md`）。
  - `UserPromptSubmit` → `auto-recall.mjs`：调 `POST /api/v1/search/find`，把高分条目拼成 `<openviking-context>` 块，作为 `additionalContext` 注入。
  - `Stop` → `auto-capture.mjs`：读 transcript，把**增量**新回合（含 assistant 文字与工具调用；工具输出默认丢弃）发到 `POST /api/v1/sessions/{id}/messages`；pending token 超过 `OPENVIKING_COMMIT_TOKEN_THRESHOLD`（默认 20000）就 `commit` 触发抽取。
  - `PreCompact` / `SessionEnd` / `SubagentStop`：补 commit；`SubagentStart`：给子 agent 注入上下文；`PreToolUse`（Read/Glob/Grep）`uri-guard`；`PostToolUse`（Read）`skill-experience`。
  - 开关：`OPENVIKING_AUTO_RECALL`、`OPENVIKING_AUTO_CAPTURE`、`OPENVIKING_CAPTURE_MODE` 等（插件 README.md「Capture tuning」）。
- peer 维度：插件默认从 git remote 推出 workspace peer。所以库里有 `peers/github.com-bulai0408-krill/`、`peers/github.com-bulai0408-conflux/`、`peers/agent/`、`peers/telegram.650865160/` 等（MCP `tree viking://user/default`）。
- Codex：`~/.codex/config.toml` 注册了 `[marketplaces.openviking]` 与 `[plugins."openviking-memory@openviking"]`，同一套 hooks（session_start / user_prompt_submit / stop / pre_compact / session_end / pre_tool_use / post_tool_use）。
- MCP 工具：`mcp__plugin_openviking-memory_openviking__*`（find、search、read、list、tree、grep、glob、health、remember、add_resource、add_skill、write、edit、forget、watch、acl 等），由插件 `servers/mcp-proxy.mjs`（stdio 代理）转发到服务器 `/mcp`。

## 3. 记忆怎么产生、怎么取回

- 产生：主路径是**自动**。Claude/Codex 每轮 Stop 把对话推进 OV 会话，满阈值 commit，服务器 LLM 抽取成 events/entities/preferences 等。显式路径：MCP `remember`、`ov add-memory`（kami-agent 分身用这个，见下）。
- 取回：每次用户输入由 `auto-recall` 注入 `<openviking-context>`（本会话开头即可见）；agent 再按需用 MCP `find/search/read` 展开 URI。
- 程序化接口（Rust 可直接用）：
  - HTTP：`/api/v1/search/find`、`/api/v1/search/search`（`mode=context`）、`/api/v1/content/read?uri=`、`/api/v1/fs/ls?uri=`、`/api/v1/sessions`、`/api/v1/sessions/{id}/messages`、`/api/v1/sessions/{id}/commit`、`/api/v1/system/status`。来源：插件脚本中的 fetch 调用与 `docs/zh/api/*.md`。
  - 认证头：`Authorization: Bearer <key>`（推荐）或 `X-API-Key`；多租户另加 `X-OpenViking-Account`、`X-OpenViking-User`、`X-OpenViking-Actor-Peer`（api/01-overview.md）。
  - SDK：Python（`openviking`）、Go、JS/TS；没有发布的 Rust SDK crate，但 `crates/ov_cli/src/client.rs` 是现成的 Rust HTTP 客户端参考。
  - 最省事：shell 调 `ov find "<q>" -o json`，复用 `ovcli.conf`，kr-cli 不用自己管 key。

## 4. 分身（kami-agent）设计中与开发流程相关的部分

来源：OV 记忆 `viking://user/default/peers/agent/memories/entities/项目/kami_agent_work_twin.md`、`.../kami_agent_approval_channel.md`、`.../kami_agent_self_correction.md`、`.../kami_agent_ai_judgment_redesign.md`、`.../entities/工具/clef_omni.md`、`.../events/2026/10/13/开发流程设计共识确认.md`、`.../分身开发流程与边界确认.md`、`.../events/2026/10/12/数字分身设计树根节点与放权规则确认.md`。分身代码目录 `~/Workspace/kami-agent` 在本机不存在（推测在另一台机器上），所以没有核对代码。

- 总原则：人格、决策方式、记忆放 OV；本机只放运行时。「判断交给模型（clef 决策模型、大模型），代码只搬运数据、记账、守不可逆动作的硬护栏」。分身不直接改 OV 文件，学到的东西走 `ov add-memory`，由 OV 抽取。
- 决策模型 clef（Cloudflare `@cf/cloudflare/clef*`）：输入状态 + 带类型问题，输出概率，不生成文字，每次判断留记录（`state/judge.log`）。用于「醒不醒」、发送前闸门、对话节奏、一致率打分。
- 开发流程共识（2026-10-13，Kami 回复「确认」）：
  - 接活：Kami 直接派的直接做；CC 需求、PR review 意见、Tasks 先问；CI 失败只作参考。
  - 实现：派 Orca worker（`implement` 技能），分身只派发和验收、不写代码；同一时间最多一个开发任务；存量问题只报告。
  - 对外或不可逆动作：由 clef 判断要不要问 Kami，**判断时用 `ov find` 附上他过去的类似决定**。硬底线：发预发、发线上、合进 main/pre 必须问。合并要他确认。
  - 有取舍的方案写一页选项给他挑；PR/提交不得有 AI 痕迹；收尾把经验写进 OV。
- 放权规则：扩大自身权限或放宽红线，不论 clef 怎么判，都必须问 Kami；无争议的自我修正可直接改，但每次改动都 git 提交并写进每日总结。
- 评估：约 40 道题库、一致率 ≥85% 才进下一阶段；「草稿 vs 实际回复」打分。低分对照被认为是最值得交给 OV 学的样本。
- 已提出、未落地的第 6 项：`add-memory` 前先用 clef 筛「有没有体现 Kami 的判断」，只写有信息量的内容（防噪声）。

## 5. kr-cli 的集成选项

前提：kr-cli 不拥有记忆，不实现抽取、去重、人格。OV 已经通过 agent 运行时的 hooks 自动收集对话。

**(a) 把结构化事件或决策推给 OV**
- 做法：在关键节点（gate 结果、owner 拍板、合并、发布）调 `ov add-memory '<json>'`，或建一个专用会话 `ov session add-messages` + `commit`，让 OV 抽取。
- 好处：决策带结构（任务号、选项、结论、理由、来源 URL），比从聊天里抽更准。
- 风险：与 hooks 自动捕获**重复**（同一决定在对话里已经出现）；`add-memory` 标为实验性；抽取结果不可控，可能写进 preferences 并影响所有项目；kr-cli 产生的流水事件（gate 通过、锁获取）是噪声。
- 缓解：只推 owner 的拍板（人的决策），不推机器事件；固定 peer（如 `github.com-bulai0408-krill`），不污染全局 preferences；给每条带稳定 id 以便以后识别重复。

**(b) 问 owner 前先回查 OV（recall）**
- 做法：`kr ask` 之类的命令先调 `ov find "<问题>" -o json`（或 `POST /api/v1/search/search`，`mode=context`、`target_uri` 限定 Krill peer），把命中的旧决定附在问题里；可再交给 clef 判断「已有决定是否覆盖本问题」。这正是分身共识里「用 `ov find` 附上他过去类似决定」的做法。
- 好处：只读，不产生重复与噪声；与分身设计一致。
- 风险：召回可能过时或错配（例如已被新 ADR 取代的决定）；必须把来源 URI 与日期一起展示，最终由 owner 或 clef 判定，不能让 kr-cli 自己据此放权；服务器不可达时要降级为「直接问」。

**(c) 不碰 OV，只写 OV 能收的文件**
- 做法：kr-cli 把决策记录写进仓库（ADR、`docs/`、PR 正文）或本地 jsonl；agent 运行时读到这些内容时，由 hooks 自动捕获；必要时由人或 agent 用 `add_resource` 把目录作为资源导入。
- 好处：最简单，kr-cli 零依赖、零密钥；仓库仍是事实源。
- 风险：进入 OV 的时机与内容不可控（取决于 agent 是否读过、是否被抽取）；没有召回能力。

**建议组合**：(c) 作为事实源 + (b) 只读召回。(a) 暂缓，等分身的「写入前 clef 筛选」落地后，再只推 owner 拍板这一类事件。

## 6. 通用风险

- 隐私：OV 里有大量个人信息（profile、家庭、薪资等，见 `last_inject.md`）。kr-cli 召回时必须用 `target_uri` 限定到项目 peer 或特定目录，不能把全局记忆打到日志、PR、issue 里。API key 只从 `ovcli.conf` 读，不打印。
- 噪声与重复：hooks 已经捕获全部对话；kr-cli 再写会形成双份。
- 可用性：远程服务器在外网；hooks 超时 45–120 秒。kr-cli 要设短超时并可降级。
- 版本漂移：插件 0.4.5，CLI 0.4.14；`/search/recall` 已弃用，新接入用 `/search/search` 的 `mode=context`。
