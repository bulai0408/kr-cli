# 0019 场景选测：依赖闭包只加不减，未知影响按默认加跑

状态：已采纳（2026-10-15，#30，协调者已确认）。细化 ADR 0006、0011、0015。

## 决定

- `run`、`gate`、`plan` 增加 `--changed <git-ref>`，比较该 ref 与当前工作树（含暂存、未暂存、未跟踪及删除路径），以及 `--also <scenario>`、`--explain`。没有 `--changed` 时保持原行为。
- 从 git 文件清单发现本地 Package.swift，执行 `swift package describe --type json`；工程若有 project.yml，执行只读 `xcodegen dump --type json --no-env`。都经过 Runner，失败进入未知影响而不声称图完整。
- `.kr.toml` 的场景增加 `modules = ["Metadata", "TitleDetail"]`。可用 `[modules.TitleDetail] paths = ["Krill/Features/TitleDetail"]`、`depends_on = ["Metadata"]` 补充包图无法表达的页面关系。显式模块和发现的模块合并；未知场景模块是配置错误，给出配置文件、键路径及修正方法。
- 改动路径映射到模块，再走反向闭包。场景覆盖闭包内任一模块即必跑，不受门禁子集或命令行目标限制；显式目标及 `--also` 只能加跑。门禁的套件保持执行。
- 未映射的改动、未声明覆盖的场景、工程或包描述失败、依赖名无法解析均显示效率类 `additional_scenarios` 判断点。默认加跑候选场景，不阻塞结论；真实运行用已有 decision 事件写 `via: default`、`review: pending`。plan 只展示，不写事件。
- `[selection] ignore` 是项目声明的 glob 数组，没有内置名单。命中路径不触发未知影响；`--explain` 写明 `ignored by <pattern>`。
- 统计读取过去 90 天本项目的 item_end，按运行和项去重，整项自动重跑合并到原项。共同失败分母为两个场景共同有最终结果的运行，分子为共同失败；偶发分母为场景运行次数，分子为有偶发的场景运行次数。缺数据时比例为 null，不推断因果；展示样本数。
- `kr hash [scenario...]` 展示现有构建哈希的文件、逐文件内容摘要、命令行、Xcode 路径和生成命令，与现有缓存哈希共用计算入口，不引入结果缓存。

## 依据

现有 `docs/research/tuist-fit.md` §4：SwiftPM + XcodeGen + 场景覆盖；`bazel-fit.md` §4：rdeps；`krill-batch-review.md`：收藏页经详情页间接依赖 Metadata/Source。未知影响默认加跑取保守边界，agent 后续可以据证据完善映射，不能删掉已算出的必跑场景。
