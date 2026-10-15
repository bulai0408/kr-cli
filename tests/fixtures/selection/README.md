# #30 选测回放素材

全部素材仅做离线 CLI 回放，不运行构建、模拟器或完整门禁。

- `krill-package.json`：2026-10-15 对 Krill `ae09004` 的 `KrillKit` 执行 `swift package --scratch-path <临时目录> describe --type json`。没有依赖下载或编译。把仓库绝对路径换成 `<repo>`。
- `krill-project.json`：同一树执行 `xcodegen dump --type json --no-env` 的输出。dump 不执行工程生成和 preGenCommand。
- `krill-batch6-impact.json`：`git show 1adec35d8d2c7ebafc9ac75f8e6d47a55f9ca343:scripts/e2e-impact.json` 的原始手工映射，19 条规则。该提交是第 6 批 #120/#144/#114/#124 合入点。
- `krill-selection.toml`：测试专用的模块覆盖声明。把上面手工表的共享模块覆盖移到 `scenario.modules`。Favorites 故意只声明 Favorites → TitleDetail → Metadata/Sources，不直接声明共享模块，用于验证间接闭包。测试标识是占位符，不用于真跑 UI。
- `krill-commits.json`：`ae09004` 起 first-parent 最近 30 个真实提交的 SHA、父提交、`git diff --name-only --no-renames <parent> <commit> --` 路径。最旧提交 `4ab5f3f`，其父 `0aa746c`。这里是提交时的路径差异，不混入当前工作树。
- `krill-item-events.ndjson`：先将本机 `~/.local/state/kr/events/*.ndjson` 复制到 `/tmp/kr30-events-copy` 再处理；只留 Krill 的 297 条 item_end、121 个运行。字段只留 item/kind/passed/flaky/tests；剔除日志、设备 ID、身份、请求者和个人路径。测试把事件时间平移到现在，以稳定测试 90 天窗口；不改结果与 run_id。

场景闭包 oracle 来自第 6 批表，不从本实现重算预期结果。真实事件的独立统计：favorites 2 次运行、0 失败、0 偶发；detail-search 3 次运行、1 次偶发。另有小型已知结果回放验证共同失败为 1/3，防止真实样本没有共同失败时形成空断言。
