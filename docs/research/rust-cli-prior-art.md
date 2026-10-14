# 个人 Rust CLI 的先例调研（Krill 多 agent 并行开发）

日期：2026-10-14。范围：只调研，不改仓库。每条结论都附来源。
“推论”表示作者的判断，不是来源中的原话。

## 0. 背景痛点（今晚实测）

| # | 痛点 |
|---|------|
| P1 | 全机一把重锁（xcodebuild 构建 + 模拟器 UI 测试）；4 个 agent 共排队约 5h；3 次等锁 20–37 min 后编译失败，0 个测试运行 |
| P2 | 受影响测试靠手写 路径→场景 JSON + 通配符；改共享模块就选中全部 30 个 UI 场景 |
| P3 | 热点文件反复冲突：Localizable.xcstrings、场景 JSON、shell 场景列表、测试里的迁移列表、大 Swift actor |
| P4 | 协调者解冲突解错，9 min 后全量测试才发现 |
| P5 | UI 测试不稳定（flaky） |
| P6 | 耗时靠手工收集 |

---

## 1. cargo-nextest：测试运行器的设计参考

### 1.1 “构建一次，到别处运行”（archive）
- `cargo nextest archive --archive-file x.tar.zst` 打包测试二进制、元数据、动态库；`cargo nextest run --archive-file` 在另一台/另一目录运行；`--workspace-remap` 处理源码路径不同。来源：https://nexte.st/docs/ci-features/archiving/
- 归档**不含源码**，目标机需检出同一 revision；仅支持 `.tar.zst`。来源同上。
- 借鉴：对应 `xcodebuild build-for-testing` 产出的 `.xctestrun` + products。把“构建”与“跑测”拆成两个产物阶段，CLI 记录 产物 ↔ git tree SHA 的对应。（推论）

### 1.2 分片（partitioning）
- `--partition hash:m/n`：按二进制 ID + 测试名哈希，增删测试不会让其他测试换桶；`slice:m/n` 轮转分配，`count:` 已废弃。来源：https://nexte.st/docs/ci-features/partitioning/
- 借鉴：两台模拟器并行时，用稳定哈希分配场景，结果可复现。（推论）

### 1.3 重试与 flaky 识别
- `--retries N`；先失败后通过即标记 flaky，默认算成功；backoff 支持 fixed / exponential / jitter；可按测试 override。来源：https://nexte.st/docs/features/retries/
- `flaky-result = "fail"`（0.9.131 起）让“重试后才通过”也算失败。来源同上。
- 借鉴：flaky 是**一等结果**（不是 pass 也不是 fail），要写入报告和历史。

### 1.4 JUnit 输出
- `[profile.ci.junit] path = "junit.xml"`；flaky 记为 `flakyFailure`，持续失败的重跑记为 `rerunFailure`；可设 `store-success-output`。来源：https://nexte.st/docs/machine-readable/junit/
- 借鉴：每次运行落一份 JUnit/JSON（含耗时），P6 的耗时统计自动产生。（推论）

### 1.5 测试组 / 互斥（test-groups）
- `[test-groups] name = { max-threads = N }`，再用 `[[profile.X.overrides]] filter=... test-group=...` 分配；本质是“逻辑信号量/互斥锁”。来源：https://nexte.st/docs/configuration/test-groups/
- 测试进程能读到 `NEXTEST_TEST_GROUP` 和 slot 编号；官方建议能用随机端口就别用测试组（会变慢）。来源同上。
- 借鉴：把“全机一把锁”拆成**带容量的命名资源**：如 `xcodebuild = 1`、`simulator = 2`、`light = 1`，每个任务声明需要哪些资源。（推论）

### 1.6 超时
- `slow-timeout = { period = "60s", terminate-after = 5, grace-period = "30s" }`；Unix 下先 SIGTERM 进程组，再 SIGKILL。来源：https://nexte.st/docs/features/slow-tests/

### 1.7 Setup scripts（实验性）
- `[scripts.setup.<name>] command=...`，按 filter 选择；脚本向 `$NEXTEST_ENV` 追加 `KEY=value` 传给测试；脚本非零退出则终止整轮；JUnit 中显示为 `@setup-script:<name>`。来源：https://nexte.st/docs/configuration/setup-scripts/
- 借鉴：模拟器租借/清数据作为 setup 阶段，失败要早、要单独计入报告。

### 1.8 Filterset 依赖查询
- `rdeps(x)` 选中 x 及所有（传递）依赖 x 的 crate 的测试；`deps(x)` 反向。来源：https://nexte.st/docs/filtersets/reference/
- 也可以在归档时用 `-E 'rdeps(db-test-utils)'` 只打包子集。来源：https://nexte.st/docs/ci-features/archiving/

---

## 2. 合并冲突：mergiraf、rerere、自定义 merge driver、jj

### 2.1 mergiraf（Rust，语法感知 merge driver）
- 支持语言含 JSON、YAML、TOML、XML、Bash、Kotlin、Rust 等；**列表中没有 Swift**。来源：https://mergiraf.org/languages.html
- 注册方式：
  ```
  git config --global merge.mergiraf.name mergiraf
  git config --global merge.mergiraf.driver 'mergiraf merge --git %O %A %B -s %S -x %X -y %Y -p %P -l %L'
  # .gitattributes 或 ~/.config/git/attributes
  *.json merge=mergiraf
  ```
  来源：https://mergiraf.org/usage.html
- 非标准扩展名可用属性 `mergiraf.language`（或 `linguist-language`）指定语言，例如 `*.xcstrings merge=mergiraf mergiraf.language=json`（xcstrings 本身是 JSON，此组合为推论，需实测）。来源：https://mergiraf.org/usage.html
- 解不了时留下冲突标记；它解掉的冲突会打印 merge ID，可 `mergiraf review <id>` 复核；`mergiraf=0` 环境变量可关闭。来源同上。
- 新语言以声明式方式加入（`supported_langs.rs` + 测试，如 Gleam PR #756）。来源：https://zff.dev/ada4a/mergiraf （仓库镜像）、https://codeberg.org/mergiraf/mergiraf

### 2.2 git 自定义 merge driver 与 union
- 自定义 driver：`merge.<name>.driver = cmd %O %A %B %L %P`，把结果写回 `%A`，干净退出 0，有冲突非 0，崩溃 >128。来源：https://git-scm.com/docs/gitattributes
- 内置 `merge=union`：两边的行都保留，不留标记，但**顺序可能随机**，官方警告需人工核对。来源同上。
- 借鉴：对“列表型”热点文件（场景列表、迁移列表）与其用 union，不如写**专用语义 driver**（解析→按 key 三方合并→排序→校验），由个人 CLI 提供子命令 `krill merge-driver xcstrings|scenarios`。（推论）

### 2.3 git rerere
- `rerere.enabled=true` 后 merge/rebase 自动记录并复用冲突解法；`rerere.autoUpdate` 自动 `git add`；`gc.rerereResolved` 默认 60 天。来源：https://git-scm.com/docs/git-rerere
- 本机验证：在 worktree 中 `git rev-parse --git-path rr-cache` 输出 `/Users/cyh/Workspace/Krill/.git/rr-cache`，即**所有 worktree 共享**（本地实测，git 2.54.0）。
- 注意：rerere 复用的是“同一冲突”的解法；若当初解错（P4），也会被复用。`git rerere forget <path>` 可清除。来源：https://git-scm.com/docs/git-rerere

### 2.4 Jujutsu（jj）
- 一等冲突：冲突可存进 commit；rebase 遇冲突也成功；后代自动 rebase；可以稍后再解。来源：https://docs.jj-vcs.dev/latest/conflicts/
- `jj workspace add` 一个仓库多个工作副本；另一个 workspace 改写了 commit 会导致 stale，用 `jj workspace update-stale`。来源：https://docs.jj-vcs.dev/latest/working-copy/
- 不支持：git worktree、hooks、**.gitattributes**（因此 git merge driver/mergiraf 的 gitattributes 路线不适用）、LFS、submodule；默认 colocated。来源：https://docs.jj-vcs.dev/latest/git-compatibility/
- mergiraf 文档建议不要在 jj 仓库里用 `mergiraf solve`。来源：https://mergiraf.org/usage.html
- 与 agent 并行的实践：wsg 用 jj workspace + worker 池把 GitHub issue 派给 Claude Code/Codex。来源：https://github.com/jarvvski/wsg
- 一篇 2026 对比文章建议先用 git worktree，只有当历史改写与冲突处理成为主要成本时再试点 jj。来源：https://wavect.io/blog/git-worktrees-vs-jujutsu-ai-coding-agents/
- 结论（推论）：jj 的“冲突可延后”对协调者有吸引力，但会失去 gitattributes 驱动的语义合并；对 Krill 当前 git worktree + Orca 流程，先上 rerere + 语义 driver 收益更确定。

---

## 3. CLI 工程形态与 Rust 最佳实践

### 3.1 cargo-xtask vs 独立个人 CLI
- xtask：工作区内一个 `xtask` bin crate + `.cargo/config.toml` 中 `xtask = "run --package xtask --"`；只依赖 cargo；是项目本地的，不能从依赖引入，不挂进 cargo 生命周期。来源：https://github.com/matklad/cargo-xtask
- 推论：Krill 是 Swift 仓库，没有 Cargo 工作区；且锁/队列是**跨项目、全机**的。更合适的形态是独立安装的个人 CLI（`~/.cargo/bin` 或 Homebrew），项目内只放一个声明式配置文件（如 `krill-ci.toml`）。xtask 的“一个二进制 + 子命令”布局仍可借鉴。

### 3.2 clap derive
- `Parser` 顶层、`Subcommand` 枚举、`Args` + `#[command(flatten)]` 复用参数组；`global = true` 全局参数；`ValueEnum`；`#[arg(env)]` 需 `env` feature；测试里调 `Command::debug_assert`。来源：https://docs.rs/clap/latest/clap/_derive/index.html

### 3.3 配置分层（figment）
- `Figment::new().merge(Toml::file(..)).merge(Env::prefixed("APP_"))`；`merge` 覆盖，`join` 只补缺；值带来源元数据，报错能指出“哪个文件哪个 key”。来源：https://docs.rs/figment/latest/figment/
- 建议层次（推论）：内置默认 → `~/.config/<cli>/config.toml`（机器级：锁、模拟器池）→ 仓库 `krill-ci.toml`（场景图、资源声明）→ 环境变量 → 命令行。

### 3.4 结构化输出与日志
- tracing-subscriber 的 fmt 层支持 `format::Json`（换行分隔 JSON）与 `RUST_LOG` 过滤。来源：https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/index.html
- 推论：给人看的用 stderr 文本，给 agent/协调者的用 `--json`（stdout，一行一个事件：queued/acquired/build_done/test_result/flaky），耗时字段内建 → 解决 P6。

### 3.5 错误报告
- miette：`#[derive(Diagnostic)]` 带 `code`/`help`/`url`/源码 span；有给屏幕阅读器与 `NO_COLOR` 的 narratable 输出；应用层有 `IntoDiagnostic`/`WrapErr`。来源：https://docs.rs/miette/latest/miette/
- 推论：配置文件错误（场景图写错）用 miette 指到行；其余内部错误用 anyhow 即可。

### 3.6 分发（dist / cargo-dist）
- `dist init` 生成 GitHub release 工作流（plan→build→publish→announce），有 shell、Homebrew 等安装器。来源：https://axodotdev.github.io/cargo-dist/book/
- 推论：单人单机初期 `cargo install --path` 已足够；多机（iMac + 其他 Mac）再上 dist + Homebrew tap。

---

## 4. 全机资源协调：锁、信号量、守护进程

- 标准库 `File::lock / try_lock / lock_shared`，**Rust 1.89.0 稳定**；Unix 用 `flock`；文件（含继承的描述符）关闭即释放。来源：https://doc.rust-lang.org/std/fs/struct.File.html#method.lock
  - 推论：进程崩溃时内核自动释放 → 消灭当前 `mkdir` 锁的“陈旧锁判定”逻辑。注意子进程会继承 fd，启动 xcodebuild 时要设 CLOEXEC（std 默认对新开文件设 CLOEXEC）。
- fd-lock：基于文件的读写锁（advisory），guard drop 时释放。来源：https://docs.rs/fd-lock/latest/fd_lock/
- jobserver：GNU make jobserver 的 Rust 实现，`Client::new(n)` 建 n 个令牌，`acquire()` 阻塞取令牌，可传给子进程；Unix 用管道/命名管道；**进程被异常杀死时令牌不归还**。来源：https://docs.rs/jobserver/latest/jobserver/
  - 推论：计数信号量可用“N 个锁文件槽位”（`sim-1.lock`、`sim-2.lock`）+ `try_lock` 实现，崩溃安全，比 jobserver 更适合跨会话的 agent。
- 命名 POSIX 信号量 crate（sem_safe 等）生态零散，macOS 支持不一。来源：https://docs.rs/sem_safe 、https://users.rust-lang.org/t/any-existing-system-semaphore-crate/62923
- 守护进程模式：
  - sccache：首次调用自动拉起后台 server，默认监听 `127.0.0.1:4226`（`SCCACHE_SERVER_PORT`），空闲 10 分钟退出，`--stop-server`。来源：https://github.com/mozilla/sccache/blob/main/README.md ；客户端/服务端经本地 IPC 通信。来源：https://github.com/mozilla/sccache/blob/main/docs/Architecture.md
  - buck2：首次命令为项目拉起 daemon，后续命令复用；默认一个项目根一个 daemon，`--isolation-dir` 可多开。来源：https://buck2.build/docs/concepts/daemon/
  - 推论：先做“无守护进程 + flock 槽位 + 队列文件”；当需要**公平排队、可视状态、抢占、预估等待时间**时，再升级为“按需自启、空闲自退”的小 daemon（sccache 模式），用 Unix socket。

### 4.1 针对 P1 的关键想法（推论）
1. **先便宜检查再排重队**：取重锁前，在轻锁下跑 `swift build`/类型检查或 `xcodebuild build` 的增量编译；编译不过不进重队列。今晚 3 次“等 20–37 min 后编译失败”就是这一步缺失。
2. **拆资源**：build 锁（1）与 simulator 槽（2）分开；build-for-testing 完成即释放 build 锁，再去拿模拟器槽跑 test-without-building（对应 nextest archive 思路）。
3. **队列可观测**：`cli queue --json` 显示前面是谁、已等多久、历史平均耗时。

---

## 5. 受影响分析 / 测试影响分析（TIA）

- Bazel target-determinator（Go）：对比两个 commit，对前后各跑 `cquery` 并哈希，输出受影响 target；可选输出“为什么变了”；`--ignore-file` 排除无关文件。来源：https://github.com/bazel-contrib/target-determinator
- guppy 的 `determinator`（Rust）：文件变化映射到最近的包；依赖变化用 cargo 构建模拟判断；包外文件变化 → 全量；可用 TOML 规则忽略文件、强制全量、加 cargo 不知道的“虚拟依赖”；建议定期全量兜底。来源：https://docs.rs/determinator/latest/determinator/
- nextest `rdeps()`：按 crate 依赖图反查。来源：https://nexte.st/docs/filtersets/reference/
- cargo-difftests：用 LLVM 覆盖率记录每个测试触及的文件，文件变了（mtime / git diff 文件 / git diff hunk）则该测试“dirty”需重跑；需 nightly。来源：https://github.com/dnbln/cargo-difftests
- cargo-impact：Rust 工作区“爆炸半径”分析，标出受影响测试。来源：https://docs.rs/cargo-impact
- 非 Rust 但同领域：Tuist selective testing 对每个模块算哈希，给 xcodebuild 加 `-skip-testing` 跳过未变模块；FeatureA/B 依赖 Core 时改 Core 会跑三者；UI 测试需事先给出 destination。来源：https://tuist.dev/en/docs/guides/features/selective-testing

### 5.1 针对 P2 的关键想法（推论）
- 粒度问题：现在是“路径通配 → 场景”，共享模块一改就全选。应改为两层：**文件 → 模块**（由 Xcode 工程 / SwiftPM 依赖图自动得出，同 determinator）+ **场景 → 实际触及的模块/类型**（由覆盖率自动得出，同 cargo-difftests）。Xcode 的 `-enableCodeCoverage YES` + `xcrun xccov` 可以在 UI 测试下按场景导出覆盖文件。
- 保留 determinator 式规则文件：`ignore`（文档）、`force-all`（xcstrings 结构、工程文件）、`virtual-deps`（资源 → 场景）。
- 兜底：定期全量（determinator 原文建议），并记录“选择漏掉的缺陷”来修规则（与全局 AGENTS.md 的 Batched Integration 一致）。

---

## 6. 建议借鉴（痛点 → 想法 → 工具/先例）

| 痛点 | 具体想法 | 借鉴对象 |
|------|----------|----------|
| P1 重锁排队 + 等完编译失败 | (a) 进重队列前强制增量编译预检；(b) 资源拆为 `build=1`、`sim=2` 命名容量；(c) build-for-testing 产物与 git tree SHA 绑定，构建一次多处跑；(d) `File::try_lock` 槽位替代 mkdir 锁，崩溃自动释放；(e) 需要公平/可视时再做按需自启 daemon | nextest test-groups、nextest archive、std `File::lock`（1.89）、sccache/buck2 daemon |
| P2 共享模块全选 30 场景 | 文件→模块依赖图 + 场景覆盖率映射；规则文件支持 ignore/force-all/virtual-deps；定期全量兜底 | guppy determinator、target-determinator、cargo-difftests、Tuist selective testing、nextest `rdeps()` |
| P3 热点文件冲突 | `.xcstrings`/JSON → mergiraf（`mergiraf.language=json`，需实测）或自写按 key 三方合并 driver；场景列表/迁移列表 → 语义 driver（合并+排序+去重+校验）或改为“一项一文件”目录结构；开启 rerere（worktree 间共享）；大 Swift actor 无 mergiraf 支持 → 只能拆文件 | mergiraf、git 自定义 merge driver、git rerere |
| P4 冲突解错 9 min 后才知 | 合并后立即跑**秒级**校验：JSON/xcstrings 解析、场景表与 shell 列表一致性、迁移列表单调、增量编译；mergiraf `review` 复核自动解决；rerere 记录了错解要 `forget` | mergiraf review、git rerere forget、nextest setup-scripts 的“失败即终止” |
| P5 flaky | 重试 + 把 flaky 作为一等结果写入报告与历史；可配 `flaky-result=fail` 用于发布前；超时用 period × terminate-after + grace-period | nextest retries / flaky-result / slow-timeout |
| P6 手工计时 | 每次运行输出 JUnit + `--json` 事件流（排队时长、构建、每场景耗时、重试次数），追加到本地历史文件；按历史给出排队预估与分片 | nextest JUnit、tracing JSON 层、nextest hash 分片 |

CLI 骨架建议（推论）：独立二进制；clap derive 子命令 `lock`/`queue`/`build`/`test`/`affected`/`merge-driver`/`report`；figment 分层配置（机器级 + 仓库级）；tracing（人读 stderr，`--json` 给 agent）；miette 报配置错误、anyhow 处理其余；单机阶段 `cargo install`，多机后用 dist + Homebrew。

## 来源清单（主要）
- nextest：https://nexte.st/docs/ci-features/archiving/ · https://nexte.st/docs/ci-features/partitioning/ · https://nexte.st/docs/features/retries/ · https://nexte.st/docs/machine-readable/junit/ · https://nexte.st/docs/configuration/test-groups/ · https://nexte.st/docs/configuration/setup-scripts/ · https://nexte.st/docs/features/slow-tests/ · https://nexte.st/docs/filtersets/reference/
- mergiraf：https://mergiraf.org/languages.html · https://mergiraf.org/usage.html
- git：https://git-scm.com/docs/gitattributes · https://git-scm.com/docs/git-rerere
- jj：https://docs.jj-vcs.dev/latest/conflicts/ · https://docs.jj-vcs.dev/latest/working-copy/ · https://docs.jj-vcs.dev/latest/git-compatibility/
- Rust CLI：https://github.com/matklad/cargo-xtask · https://docs.rs/clap/latest/clap/_derive/index.html · https://docs.rs/figment/latest/figment/ · https://docs.rs/miette/latest/miette/ · https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/index.html · https://axodotdev.github.io/cargo-dist/book/
- 锁/守护进程：https://doc.rust-lang.org/std/fs/struct.File.html#method.lock · https://docs.rs/fd-lock/latest/fd_lock/ · https://docs.rs/jobserver/latest/jobserver/ · https://github.com/mozilla/sccache/blob/main/README.md · https://buck2.build/docs/concepts/daemon/
- 影响分析：https://github.com/bazel-contrib/target-determinator · https://docs.rs/determinator/latest/determinator/ · https://github.com/dnbln/cargo-difftests · https://docs.rs/cargo-impact · https://tuist.dev/en/docs/guides/features/selective-testing
