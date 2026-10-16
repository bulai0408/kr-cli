# 0013 同一项目的 worktree 共用 Xcode 编译缓存，默认关闭

状态：已采纳（2026-10-15，#37，按 owner 阈值实测达到；2026-10-16 协调者确认）

## 背景
spec 第二、三阶段第 11 条：先在 Krill 上实测共用 Xcode 原生编译缓存能省多少时间、会不会出假红，再决定是否做成功能。

多个 agent 各用一个 worktree。每个 worktree 有自己的 DerivedData（`build/kr/DerivedData`），所以每个新 worktree 的第一次构建都从头编。Xcode 26 起有编译缓存，但缓存默认放在 DerivedData 里，路径也进缓存键，不在 worktree 之间共用。设置和源码依据见 `docs/research/xcode-compilation-cache.md`。

## 方法
- 对象：Krill，`xcodebuild build-for-testing`，scheme `KrillPlayerControls`，配置 `DebugUIAcceptance`，目标 `generic/platform=iOS Simulator`，参数与 `kr check` 相同。只编译，不跑测试，不用模拟器。
- 一批票：5 个真实的连续提交 2f22081 → 8c98d6c（+35 个 Swift 文件）→ 1e2030f（+21）→ b466d44（+44）→ 4ea68bc。每个提交新建一个 worktree（`git worktree add --detach`），各自的 DerivedData 从空开始。
- 每个提交编两次：关缓存一次，开缓存一次，先后顺序逐个提交交替。开缓存的 5 次共用一个缓存目录，第 1 次是冷启动（缓存为空）。
- 开缓存 = kr 加的设置：`COMPILATION_CACHE_ENABLE_CACHING`、`COMPILATION_CACHE_CAS_PATH=<共享目录>`、四个前缀映射开关，再加 `SWIFT_/CLANG_OTHER_PREFIX_MAPPINGS=<worktree 根>=/^worktree`。
- 每次构建都经 `kr lock build` 拿全机锁，同一时间只排一个。包解析（`-resolvePackageDependencies`）在同一把锁里先做、单独计时，不算构建时间（这台机器访问 GitHub 很慢）。开始前等 load1 降到 10 以下（最多 5 分钟），记录拿到锁时和结束时的 load1/load5、内存空闲比例。
- 命中数来自 DerivedData 里的活动日志（kr 用 `-quiet`，命中信息不在控制台）。
- 假红：开缓存失败、清空缓存后通过的次数。
- 阈值：第二个提交起，开缓存的总构建时间比关缓存少 20% 以上，并且 Swift 编译命中率不低于 50%。理由：早先在负载很高的本机上，同一构建的时间波动约 ±25%，少于 20% 分辨不出来；也抵不上缓存占的磁盘和失败时再编一次的代价。

## 数据（结论依据）
机器 `work`：Mac16,8（M4 Pro，14 核，48 GB），macOS 26.6.1，Xcode 27.1（27A9269）。kr 是本分支编出的 release 版（代码同 ed7fd33）。这台机器上常驻的 osqueryd 约占 1 个核，所以空闲时 load1 也有 4–10。

| 提交 | 关（秒） | 开（秒） | 变化 | Swift 编译命中 | 模块命中 | Clang 命中 | 开始 load1/5（关；开） |
|---|---|---|---|---|---|---|---|
| 2f22081 | 124.3 | 155.6（冷） | +25% | 0/1313 | 196/1363 | 0/172 | 15.1/9.2；20.5/19.5 |
| 8c98d6c | 148.2 | 86.2 | −42% | 1040/1313 | 1066/1076 | 172/172 | 18.7/19.5；7.8/19.1 |
| 1e2030f | 139.4 | 95.4 | −32% | 856/1313 | 1077/1083 | 172/172 | 12.4/21.9；19.5/23.9 |
| b466d44 | 131.9 | 103.4 | −22% | 985/1313 | 1025/1028 | 172/172 | 17.9/24.1；19.9/21.7 |
| 4ea68bc | 133.9 | 87.2 | −35% | 985/1313 | 936/936 | 172/172 | 9.9/23.3；20.1/23.7 |

- 第二个提交起合计：关 553.4 秒，开 372.2 秒，**少 32.7%**。逐个提交的变化：均值 −32.5%，标准差 8.4 个百分点，n=4，95% 置信区间 −45.9% 到 −19.1%（t 分布，自由度 3）。区间下界接近阈值。
- 含冷启动的 5 个提交合计：677.7 → 527.8 秒，少 22.1%。冷启动多 25%（写缓存的代价）。
- Swift 编译命中率 65%–79%。未命中的是本次提交改动的文件和依赖它们的文件，正是最贵的部分，所以时间降得比命中率少。
- 没有一次开始时 load1 超过 24。结束时的 load1（31–56）是构建本身造成的。内存空闲比例始终 74%–75%。
- 假红 0 次：10 次构建全部成功。日志里的 `error: the following command failed with exit code 0 but produced no further output` 开关两边都有，是 Xcode 的输出，与缓存无关。
- 5 次开缓存后，缓存目录 4.6 GB。

## 本机排查记录（不作结论依据）
本机 Mac15,5，8 核，24 GB。测量时其他 agent 的模拟器和构建在跑，load1 在 7–294 之间，同一构建时间差可达一倍。
- 第一轮（worktree 在 `/tmp` 下）：关 889.6 / 1430.5 秒，开 941.5（冷）/ 1118.9 秒，Swift 编译 0/867 命中。原因是 `/tmp` 符号链接（调研第 3 节）。
- 第二轮（worktree 在 `/Users` 下，未映射 worktree 根）：关 366.6 / 375.4 秒，开 392.2（冷）/ 471.1 秒，Swift 编译仍是 0/867。原因是 `-file-compilation-dir` 未映射。
- 加 worktree 根映射后：下一个提交 Swift 编译命中 691/867（80%），时间 366.6 秒，同一提交关缓存 375.4 秒，两次负载不同（load1 80 和 126），分辨不出时间差。
- 在 work 上重测前，本机最后一轮（batch4）在 owner 决定后作废，只完成了 3 次。

## 决定
1. **做成功能，默认关闭。** `.kr.toml` 的 `[project]` 写 `compilation_cache = true` 打开。
2. **同一项目共用一个缓存目录**：`<kr 状态目录>/cache/<项目名>/compilation`（默认 `~/.local/state/kr/cache/…`，不在符号链接路径下）。
3. **映射 worktree 根**：除了四个前缀映射开关，kr 还加 `SWIFT_OTHER_PREFIX_MAPPINGS` 和 `CLANG_OTHER_PREFIX_MAPPINGS` = `<worktree 根>=/^worktree`。没有这一项，SwiftPM 包 target 在每个 worktree 都不命中。
4. **缓存设置不进输入哈希**：开关缓存不改变编出的产物，不让已有的构建标记失效。#29 的 `item_end.identity.config_hash` 只算机器配置，不含这个开关；每次构建是否开缓存看下面第 5 条的事件字段。
5. **事件**：每个构建步骤的 `step_end` 多一个 `compilation_cache` 字段：`enabled`；开着时还有 `clean`（是否用的空缓存）、`hits`、`misses`（从活动日志数；没有日志时为 null）。字段只增不改。
6. **疑似缓存导致的失败再判一次**：开缓存的构建失败时，如果失败不是源文件某一行的编译错误（失败的编译不会被缓存，这种错误来自源码本身），就用一个空缓存（`build/kr/CleanCompilationCache`，用完删除）再编一次，由这一次决定结果。两次都写进事件，另写一条 `compilation_cache_retry`（步骤、原因、两次的结果和日志、`cache_caused`）。再编通过时，结果里提示删除共享缓存目录。
7. **远程（#32）**：远程主机读同一个 `.kr.toml`，开关不需要另外传；缓存目录在远程主机自己的状态目录里。

## 影响和限制
- worktree 放在 `/tmp` 或 `/var` 下（macOS 上是指向 `/private` 的符号链接）时不会命中。kr 不检查，写在 README。
- 缓存会变大。Xcode 默认上限是可用磁盘的 50%；kr 不清理。
- 调试信息里的源码路径变成 `/^worktree/…`。在 Xcode 里调试这些测试产物，可能要给 LLDB 配源码映射。没有验证。
- 假红的判断规则是推断：实测没有出现过缓存导致的失败，测试用的失败输出是构造的。
- 只测了一个 scheme、一台机器、一批 5 个提交。

## 参考
spec 第二、三阶段第 11 条；ADR 0006、0010；`docs/research/xcode-compilation-cache.md`、`docs/research/tuist-fit.md`。
