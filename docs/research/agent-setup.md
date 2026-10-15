# agent 安装入口调研（2026-10-15，#40）

## 官方目录

| agent | 全局 skill | 项目 skill | 全局规则 | 项目规则 |
|---|---|---|---|---|
| Codex | `~/.agents/skills/kr/SKILL.md` | `.agents/skills/kr/SKILL.md` | `$CODEX_HOME/AGENTS.md`，默认 `~/.codex/AGENTS.md` | `AGENTS.md` |
| Claude Code | `~/.claude/skills/kr/SKILL.md` | `.claude/skills/kr/SKILL.md` | `~/.claude/CLAUDE.md` | `CLAUDE.md` |

来源（本日读取官方网页）：
- https://developers.openai.com/codex/skills ：Codex 从 `$HOME/.agents/skills` 和项目目录链上的 `.agents/skills` 发现 skill。
- https://developers.openai.com/codex/guides/agents-md ：全局目录可由 `CODEX_HOME` 改写；`AGENTS.override.md` 优先于 `AGENTS.md`。安装时若有非空 override，规则必须写入生效文件。
- https://code.claude.com/docs/en/skills ：个人和项目 skill 的目录、SKILL.md frontmatter。页面的无查询参数 URL 本机返回 403；加查询参数 `?x=40` 后成功读取。
- https://code.claude.com/docs/en/memory ：个人规则 `~/.claude/CLAUDE.md` 和项目 `./CLAUDE.md`。
- https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview ：再次确认 Claude 的个人、项目 skill 路径。

## ctx7 的入口

实际运行 `npx ctx7@latest setup --help` 和 `npx ctx7@latest remove --help`：
setup/remove 都有 `--claude`、`--codex`，以及 `-p, --project`；
setup 默认全局，可选 CLI + Skills / MCP 模式。这里只借鉴 agent 和作用域参数，不需要 MCP、认证或交互安装。

kr 用 `kr setup --remove` 对称卸载。安装内容与 ADR 0011、0015 对齐：
规则只说明 kr 项目内 Apple 构建测试统一通过 kr，并先读 `kr skill`；
skill 入口不复制正文。全局生效仍受宿主的项目规则覆盖顺序影响。
