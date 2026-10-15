# 0018 用 kr setup 安装 agent 发现入口

状态：已采纳（2026-10-15，#40，协调者已确认）

## 决定
- `kr setup` 默认同时安装 Claude Code 和 Codex；`--claude`、`--codex` 可只选一种，也可同时指定。默认全局，`--project` 以当前目录为项目目录，不要求已有 `.kr.toml`。
- Claude 的全局入口为 `~/.claude/skills/kr/SKILL.md`、规则为 `~/.claude/CLAUDE.md`；项目入口为 `.claude/skills/kr/SKILL.md`、规则为 `CLAUDE.md`。
- Codex 的全局入口为 `~/.agents/skills/kr/SKILL.md`、规则为 `~/.codex/AGENTS.md`；项目入口为 `.agents/skills/kr/SKILL.md`、规则为 `AGENTS.md`。尊重 `CODEX_HOME`；存在有效的 `AGENTS.override.md` 时使用该文件，因为它优先于 AGENTS.md。
- 入口只保留 name、description 和运行 `kr skill` 的指令，正文由当前二进制输出（ADR 0011、0015）。
- 同时选择两种 agent 时，如果 Claude 规则文件已有 `@` 行导入生效的 Codex 规则文件，只在 Codex 文件写规则块；Claude 入口照常安装。人读和 JSON 输出说明跳过原因，移除旧的重复规则块。
- 规则块以两个 `<!-- kr -->` 包住；追加时前导换行放在块内，替换和移除只改从第一个标记到第二个标记的字节。块外内容保持原样，连续安装无变化。
- `--remove` 删除入口及标记块；空规则文件删除，只清理空的安装目录。已有不同内容的 skill 文件、未配对的标记报错，指明路径和处理方法，不覆盖用户内容。
- `--json` 列出实际写入、删除的文件和 `schema_version`、`decisions`；没有变化时两个文件列表为空。

## 来源
见 `docs/research/agent-setup.md`，包含官方目录说明与 ctx7 参数调研。
