# 0017 合并辅助：事实、保守合并、快检和解法对账

状态：已采纳（2026-10-15，#31，协调者已确认）。细化 ADR 0011、0015。

## 决定
- `kr merge predict --base <ref> <branch>...` 两两运行 `git merge-tree --write-tree --name-only -z`。顺序先满足祖先依赖，再按冲突数少者优先，同分按输入顺序。它是启发式建议，不保证多分支累计合并无冲突；不改索引、工作区或分支，但 Git 会写临时 tree 对象。
- 同文件双方变更是逻辑冲突候选，包括没有文本冲突的情况。判断点 `merge_resolution` 带两边提交说明（其中的 #票号）、交集文件和文本冲突文件；没有票号时明确标记缺失。退出 4，经现有 `kr decide` 记录 `combine` / `ours` / `theirs` / `manual` / `reject`。记录解法不执行 Git 合并，也不等于合并通过；仍等待构建和测试证据。
- `kr merge install` 是显式 opt-in：为当前仓库所有已有 worktree 写 `rerere.enabled=true`（`git config --worktree`；未开启 worktreeConfig 时使用 shared local 配置），注册 `kr-xcstrings` driver，在仓库 `.gitattributes` 追加规则。不开启/迁移 worktreeConfig。安装以后新 worktree 若有独立覆盖，应重跑安装；不更改任何全局配置。
- `kr merge xcstrings <base> <ours> <theirs>` 是 driver，按 strings 的完整 key 条目三方合并，元数据也必须三方一致。同 key 不同改动或删除/修改冲突时退出 1，保持 ours 字节不变。成功结果先写临时文件，以 `xcrun xcstringstool print <file>` 校验再替换 ours；工具不存在或校验失败绝不返回成功。Git 的 `%A` 临时文件会原地更新。
- `kr merge check --base <ref>` 检查相对 base 的已提交及工作区改动：JSON/xcstrings 解析、`.kr.toml` 场景注册与 Swift UI 测试类一致、迁移标识递增/不重复及同一迁移重复 DROP、凭证模式扫描（只输出文件和行号，不输出匹配内容）。删除文件不读，但构建影响仍包含删除。改动 Swift/工程/包文件时，按配置的 build suite 目录选取增量 Xcode build；无法定位时保守构建 App，无法确定构建配置时给出未验证并失败。静态检查不占构建锁；增量编译走现有 build 槽位和全机锁，时限 120 秒，超时失败，绝不 clean 或跑完整门禁。静态失败时不编译。
- 快检 `--decision <id> --verification-run <run-id>` 对账：解法已经交回，当前 tree 是决定后包含双方提交的树、工作区干净；快检编译通过，加上传入的真实 kr run/gate 同一 tree 的非零测试结果且通过，才支持解法。缺证据保留未验证，不写成功；失败结果记不支持。每个判断点只对账一次，事件复用 `decision_point`、`decision`、`reconciliation`，供 `kr report` 统计。

## 边界
- 不实现语义分析器，不自动执行合并或猜需求；跨文件逻辑冲突仍需 agent 主动审查。
- 秒级指静态扫描；增量编译受缓存与机器速度影响，硬时限不能保证成功。不以编译代替测试。
- `.xcstrings` 保持完整条目保守合并，不擅自合并同 key 下不同语言；输出使用确定的 JSON 排序和两空格缩进。
- 凭证扫描是有限模式启发式，不能证明没有秘密；凭证值不进入错误、事件和扫描输出。

## 来源
已有 `docs/research/apple-parallel-dev.md` §6、`agentic-parallel-dev.md` §2.4、`pitfalls-inventory.md` M1–M4；实现核对见 `docs/research/merge-assistance.md`。
