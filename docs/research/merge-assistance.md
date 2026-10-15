# 合并辅助调研与 #31 交付证据

2026-10-15。决定见 ADR 0017（协调者已确认）。

## 已有结论与本次核对

先读 `apple-parallel-dev.md` §6、`agentic-parallel-dev.md` §2.4 和 `pitfalls-inventory.md` M1–M4。已有结论是：按 key 合并 xcstrings，不使用 union；文本无冲突不能证明逻辑兼容；rerere 只复用相同冲突解法。

本次核对一手来源：

- Git `merge-tree` 官方手册：https://git-scm.com/docs/git-merge-tree 。本机 Git 2.54.0 的 `git help -m merge-tree` 同步核对：现代 `--write-tree` 模式不改工作区和索引，但会写 tree 对象；退出 1 是有冲突，其余非零是无法预测。`--name-only -z` 的冲突路径在 tree ID 后，以空字段分隔消息；文件名可以含换行。
- Git attributes 官方手册：https://git-scm.com/docs/gitattributes ，本机 `git help -m gitattributes` 核对 `%O/%A/%B` 契约。驱动必须更新 `%A` 且以 0 表示成功；只写 attributes 不注册配置不起作用。本实现真实分歧返回非零，保留 ours 的合法 JSON；Git 索引仍为 unmerged，工作区没有强行插入无效 JSON 标记。
- Git rerere 官方手册：https://git-scm.com/docs/git-rerere ，配置手册：https://git-scm.com/docs/git-config ，本机手册核对 `--worktree`：未开启 `extensions.worktreeConfig` 时与 `--local` 等价。无需为安装驱动迁移 worktreeConfig，也不改 global 配置。对全部已有 worktree 写入，可以覆盖已存在的 per-worktree false。
- cmux PR 13692：https://github.com/manaflow-ai/cmux/pull/13692 ，用 `gh pr view ... --json title,body` 核对原说明：保留整个条目、拒绝同 key 分歧和删除/修改。它的真实样例把 42 个文本冲突块降到 0。本实现采用相同保守边界，允许 Xcode 原格式输入，输出确定排序的两空格 JSON，并额外拒绝重复对象 key。
- Apple 工具的本机一手帮助：`xcrun xcstringstool help print`，语法为 `xcstringstool print <input-file>`。这里只用 print 做目录校验，不调用可能修改源目录的 sync。

新增 `regex` 是成熟文本扫描依赖，用于有限的 Swift 注册、SQL 和凭证模式；不增加 Swift 解析器或模型服务。构建复用已有 `BuildJob.command`、`slots::take/give_back`、项目 Xcode 选择；不另写构建配置系统。

## 前后对比（ADR 0010，第 3 层）

冻结样本为 Krill `Localizable.xcstrings` 历史上最近的 14 个可读取双亲的合并（按本次 `git log --all --merges -- <path>` 顺序）。`tests/fixtures/merge/history-sample.json` 保存准确提交、双亲和 merge-base；不用运行时的最新日志来重选样本。

| 指标 | 前：git merge-file 按行合并 | 后：kr 按 key 三方合并 |
|---|---:|---:|
| 需要人工处理的目录 | 12/14 | 2/14 |
| 文本冲突样本中自动处理 | 0/12 | 10/12（83.3%） |
| same-key 分歧仍交人工 | 不区分原因 | 2/12（16.7%） |
| 目录由 xcstringstool 校验通过 | 未自动执行 | 12/14 |
| 原始第 6 批重复 DROP 在编译前被发现 | 没有这条快检 | 1/1 |

样本回放使用真实 Git 历史输出；全部合并写入临时目录，Krill 仓库只运行 log/show/merge-base 等只读操作。`evaluation.json` 保存实际 kr 退出码；12 个成功结果在本机经过 `xcstringstool print`，两个拒绝对应人工核对的 same-key 分歧（其中一例 key 为 `Needs Settings`）。完整样本/减少后的原始条目不是合成翻译。

校准误差：这是确定性的历史判定重放，没有时长模型，不报告 makespan 或 MAPE。14/14 的接受/拒绝与预先抽取的分歧类别一致，样本内分类差异 0/14；这不是面向未来的误判率估计。样本包含相关和重复祖先，不独立；16.7% 不能推广为全部 Krill 冲突的比例。12 个成功中包括原本无文本冲突的 2 个目录，不能说“修复了 12 个冲突”。

复现（只读历史，写临时目录；`--kr` 使用本分支构建的绝对可执行文件路径）：

```sh
python3 docs/research/merge-assistance-eval.py --repo <Krill-checkout> --kr <kr-binary> --output /tmp/merge-evaluation.json
```

## 素材和测试

- `base/ours/theirs.xcstrings`：真实 Krill 词条的构造插入场景，用于基本 CLI 接缝。
- `real-*.xcstrings`：合并 `b212a09` 双亲和 merge-base 的 8 个变更 key 的原始片段。临时目录中 `git merge-file -p` 重现文本冲突，kr 合并保留 8/8 条目；`xcstringstool-print.txt` 是实际合并目录的工具输出。
- `same-key-*.xcstrings`：合并 `21b89ec` 的真实分歧 key；必须非零并保留 ours，不替 agent 选语言或值。
- `batch6-duplicate-drop.swift`：直接提取 Krill 修复 `eaa1569` 的父提交 `KrillKit/Tests/UserStateTests/LibraryGroupsTests.swift` 中完整 `v14UpgradePreservesWatchDataAndStartsWithDefaultGroups` 函数。它在同一升级 fixture 中两次 DROP moduleConfigurations。没有改写为 registerMigration；另有明确标为构造的迁移版本样例。
- 正例也验证：DROP 后 CREATE 后再 DROP、不同函数的独立清理、注释里的 SQL、幂等 DROP IF EXISTS 均不误报。
- 所有自动化测试从 `kr` 进程接缝进入；Git 行为用临时仓库，Xcode/校验器用 KR_REPLAY。增量编译的成功输出来自已有 `gate-default/004-xcodebuild.json`（仅适配 argv）；硬件数据复用 `002-sysctl.json`。超时/失败和不完整证据属于构造的反例，不冒充现场录制。
- TDD 的关键 RED→GREEN：无 merge 命令→按 key 合并；无 predict→带票号判断点；无 install→所有 worktree 启用；无 check→构造重复 DROP；原始升级函数遗漏→原始重复 DROP；未编译→单模块增量构建；无 reconciliation→同树编译测试对账；普通 CatalogTests 漏识别→按已注册类核对；注释/幂等误报→允许；重复 JSON key 覆盖→拒绝。每次先观察行为失败，再实现对应切片。

## 边界与未验证

- 没跑完整门禁、模拟器、真机或真实 App 增量构建；回放不能证明 Krill 工程实际能在 120 秒内编译。静态扫描为秒级，构建可能排队，120 秒仅是构建预算，不能作为整条命令的墙钟承诺。
- 构建按 build suite 目录选取；未覆盖的源文件保守回退 App。依赖反向闭包不在本票实现，没有声称验证全部间接依赖或全部平台。
- 场景扫描检查已注册类存在和常规 UITests 类漏注册；不是 Swift AST 或运行时测试枚举，不证明每个方法能被运行。迁移扫描识别 Swift 注册和字面 SQL，不执行数据库，也不解释任意控制流。凭证扫描只覆盖列出的模式，不证明没有秘密。
- 逻辑冲突只标同文件交集和文本冲突；跨文件的语义依赖仍由 agent 审查。建议顺序是两两启发式，不是累计合并成功保证。
- 对账要求 agent 指定一次受影响测试运行；检查同树、决定后、非零、运行结论与增量编译，不替代完整选测或真实服务验收。记录决定会退出 1（未验证），不能当作 merge PASS；缺失或旧树/零测试证据不写成功对账。
- 安装只在临时 Git 仓库验收；未修改 kr-cli 或 Krill 的真实 Git 配置，也未向真实工作区安装驱动。安装会修改当前工作区的已跟踪 `.gitattributes`，调用者必须审阅并有意提交，其他 worktree 也需要包含这次 attributes 提交。

## 交付检查（2026-10-16，Asia/Taipei）

1. 机器判定：新增 CLI 接缝 28/28 PASS；合并前 fmt/clippy/test 全通过（353/353 测试）；合入 `origin/p1p2/integration` 的 `03c0c70` 后三项再次全通过（362/362 测试）。主平台是这台 macOS 上的 Rust CLI，未启动 Xcode 构建或模拟器，所以没有 xcresult。
2. 需人判断手感：0 项。本票没有新增 App UI。
3. 未验证：真实工程编译速度、全部间接依赖、设备与服务、完整门禁；具体范围见前节。安装只作用于临时仓库，所有回归均保留，不需要无回归例外。

集成时 `main.rs`、README、skill、帮助快照自动合并成功；两侧命令均保留。`run.rs` 和 `setup.rs` 与集成分支完全一致，没有手改 #40 的重跑或 setup 实现。改动文件的凭证前缀模式扫描 0 命中；不把有限扫描当作无凭证的证明。

完整检查日志保存在 [检查产物](/tmp/kr31-evidence)，分为 before-integration 和 after-integration，不覆盖失败/先前证据。

新增测试名（`crates/kr/tests/merge.rs`）：

- `xcstrings_preserves_real_krill_entries_and_validates_before_replacing_ours`
- `predict_reports_text_and_logical_conflicts_with_tickets_and_records_a_resolution`
- `install_enables_rerere_in_all_worktrees_and_warns_about_tracked_attributes`
- `quick_check_catches_constructed_batch6_duplicate_drop_before_compilation`
- `xcstrings_same_key_delete_modify_and_metadata_disagreements_preserve_ours`
- `xcstrings_validation_failure_never_replaces_ours_or_leaves_a_temp_file`
- `real_history_xcstrings_text_conflict_merges_without_dropping_entries`
- `quick_check_catches_the_original_batch6_duplicate_drop_in_upgrade_test`
- `quick_check_allows_recreated_tables_independent_fixtures_and_commented_sql`
- `quick_check_reports_invalid_json_and_credentials_without_printing_values`
- `predict_orders_stacked_branches_ancestor_first_without_asking_about_shared_commits`
- `resolution_is_reconciled_only_against_compiled_and_tested_merged_tree`
- `resolution_rejects_tests_of_an_old_tree`
- `resolution_rejects_a_zero_test_run`
- `resolution_failed_tests_are_recorded_as_unsupported`
- `quick_check_matches_registered_test_classes_without_requiring_a_name_suffix`
- `quick_check_detects_scenario_drift_and_migration_order`
- `clean_json_and_credential_references_pass_without_a_native_build`
- `clean_text_merge_of_same_file_still_needs_a_logical_decision`
- `predict_suggests_clean_branch_before_a_conflicting_pair_and_rejects_invalid_refs`
- `git_driver_keeps_a_real_same_key_conflict_unmerged`
- `quick_check_allows_idempotent_drop_if_exists`
- `xcstrings_rejects_duplicate_keys_instead_of_losing_a_translation`
- `quick_check_incrementally_builds_the_changed_module_with_recorded_xcode_output`
- `incremental_build_failure_is_not_a_pass`
- `incremental_build_timeout_is_not_a_pass`
- `changed_native_headers_also_require_incremental_compilation`
- `real_same_key_conflict_is_refused_without_changing_ours`
