# #30 场景选测：离线前后对比

日期：2026-10-16。设计：ADR 0019；评估层级：ADR 0010 第 1 层（计划），统计用第 3 层的历史事件重读。没有运行完整门禁、xcodebuild、simctl 或 UI 测试。

## 既有调研依据

没有另起一套设计。`tuist-fit.md` §4 已得出 SwiftPM + XcodeGen + UI 场景模块映射的结论；`bazel-fit.md` §4 已选定 rdeps 式反向闭包。
Tuist 原始来源：https://tuist.dev/en/docs/guides/features/projects/hashing 。Bazel 的来源与调研：`bazel-fit.md`。新增依赖 globset 仅处理项目自声明 ignore glob，避免自制 glob 解析器。

## 方法与边界

读 Krill `ae09004` 的 SwiftPM describe / XcodeGen dump；第 6 批手工表取自 `1adec35`，不是拿今天的表冒充第 6 批。取 first-parent 最近 30 个真实提交，各自父提交 → 本提交的文件差异，经 `KR_REPLAY` 喂给真实 `kr plan --changed base`。记录树、提交、原始图、映射和匿名事件均在 `tests/fixtures/selection`，可复现。

映射将第 6 批共享模块规则转换成场景覆盖。Favorites 只经 TitleDetail 间接关联 Metadata/Sources。当前工作树与历史提交没有重新构建；用当前图回放历史路径有图漂移误差，不能证明历史运行的全部真实依赖。项目 ignore 可遮蔽源文件，责任在项目声明；kr 不暗藏忽略项。

## 前后对比

| 项目 | 原行为（不按改动） | 按改动，无 ignore | 按改动，有 ignore |
|---|---:|---:|---:|
| 30 个提交 × 31 个第 6 批场景的计划项总数 | 930 | 930 | 806 |
| 未知影响 → 默认加跑全部候选 | 不适用 | 22/30 (73.3%) | 18/30 (60.0%) |
| 相对原行为少排的场景项 | 0 | 0 | 124 (13.3%) |

ignore 为项目显式 `docs/**`、`**/*.md`、`.github/**`。**即使有 ignore，仍然多数提交触发未知影响**；不应宣传成精细选测已完善。后续逐步完善页面、测试资源与工程辅助文件映射，而不是让 kr 猜测无影响。

直接用第 6 批共享模块路径做定点回放，分别断言 Metadata 和 Sources 手工表全部消费者都在所选场景内，特别断言 favorites 原因链含 TitleDetail。Metadata 实际选 23 项，手工要求 15 项，多选 8 项；Sources 实际选 31 项，手工要求 21 项，多选 10 项。另对 30 个真实提交中改到 Metadata/Sources/Domain/UserState 的 8 个提交按原始手工表逐项断言，漏选 0 项。空过滤不算通过。

## 误差和校准

- 规则层断言是集合包含：手工表的要求项漏选 0；该有限 oracle 的假阴性误差为 0，不代表全项目真实依赖完整。
- 计划项计数是确定性结果，不是耗时预测；**没有本次真跑校准误差**。不将减少 13.3% 计划项说成减少 13.3% 测试时间。
- 历史图漂移、手工覆盖不完整、资源依赖未声明会增加默认全量；默认全量不造成对已知必跑场景的删减。
- 统计只读取本项目最近 90 天 item_end，同一运行同一项最后记录计一次。共同失败按共同有最终结果的运行计；偶发按“含偶发的场景运行 / 场景运行”计。未验证不当失败，没有样本用 null。297 条真实事件仅作事实计数；另用独立 3 运行案例证明共同失败 1/3、偶发 2/3。

## 未验证

- 未在 Krill 安装此版本、替换它的 .kr.toml 或真跑所选 UI；没有改 Krill 文件。
- 没有完整门禁、设备、远程主机/真实服务验收。远程命令透传选测参数，主机仍需相同版本。
- 没有解析原生 pbxproj；无 project.yml 时保守给未知影响。资源目录用 target.path 覆盖，跨 target 的额外生成输入仍须显式映射。
- 这是输入哈希解释与依赖选测，不增加测试结果缓存，不通过历史偶发率自动删必跑项。


## 交付验证

1. 机器判定：初轮 341/341 PASS；合入 `origin/p1p2/integration` 的 `d0fda3a`（含 #31、#40）后，`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test` 全通过，378/378 Rust 测试，0 失败、0 ignored。选测新增 12 条 CLI 测试，运行接入新增 4 条 CLI 测试。独立复验 merge 接缝 28/28、skill 接缝 3/3；这些是 CLI/回放结果，不是原生设备或服务验收。
2. 需人判断手感：0 项，本票不改 Apple UI。
3. 未验证：完整原生门禁、所选 UI 的真实运行、设备与外部服务、真实远程选测、历史图漂移和耗时校准；原因是本票使用获准的离线评估，不占原生构建和模拟器。

同步是实际 git merge（合并提交 `43b81a4`），main.rs、README 和 Scenario 没有文本冲突，hash/merge/setup 命令都保留。组合编译发现 #31 新建 Selection 的入口缺本票字段；已独立复现 E0063，然后仅在 `commands/merge/quick/build.rs` 补 `selection: None`，28 条 merge 回放通过。没有修改 #31 的合并算法或 #40 的重跑规则；#41 尚未出现在本次集成基线上。

本机完整日志保留为 `/tmp/kr30-fmt-final.log`、`/tmp/kr30-clippy-final.log`、`/tmp/kr30-tests-final.log`；组合编译的原始失败与独立复现分别保留在 `/tmp/kr30-clippy-postmerge.log`、`/tmp/kr30-integration-reproduce.log`，后续检查没有覆盖它们。

### 新增 CLI 回归用例

- `shared_metadata_selects_favorites_through_detail_even_outside_the_gate`
- `unknown_changes_add_candidates_but_declared_ignores_raise_no_decision`
- `invalid_ignore_and_unknown_dependencies_name_the_config_key_and_fix`
- `additional_scenarios_include_project_scoped_cofailure_and_flaky_sample_counts`
- `hash_explains_the_exact_inputs_used_by_plan_and_changes_with_file_content`
- `an_owned_file_without_scenario_coverage_adds_candidates_instead_of_silently_skipping`
- `explicit_scenarios_can_only_add_to_the_reverse_dependency_closure`
- `real_krill_commits_cover_batch_six_indirect_consumers_and_report_unknown_frequency`
- `real_event_replay_counts_final_scenario_runs_and_flaky_runs`
- `an_unreadable_or_malformed_package_graph_defaults_to_additional_scenarios`
- `deleted_and_untracked_paths_and_resource_only_targets_remain_selectable`
- `ignore_globs_require_double_star_to_cross_directory_boundaries`
- `changed_run_and_gate_default_efficiency_decisions_continue_and_record_pending_review`
- `selection_defaults_are_recorded_even_when_the_build_fails`
- `explain_without_changed_describes_explicit_and_gate_scenarios`
- `selection_review_and_a_failure_decision_have_distinct_ids_and_independent_verdicts`
