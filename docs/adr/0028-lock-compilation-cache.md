# 0028 `kr lock --compilation-cache`：调用方逐次选择共享编译缓存

状态：提议（2026-11-14，#80 由 owner 提出，待协调者在 PR 审查时确认）。扩展 ADR 0013，不改它的决定。

## 背景

ADR 0013 的共享编译缓存只作用于 `kr run` / `kr check` 自己发起的构建。Conflux 的构建脚本都经 `kr lock build --task <t> -- xcodebuild …`，用不上。Conflux 两周的 kr 记录里，每个 (task, worktree) 的第一次 xcodebuild 中位数 3.0 分钟，合计持有 build 槽位 45.4 小时；之后的构建中位数 0.4 分钟，合计 14.0 小时。冷构建是这台机器上 build 槽位最大的消耗。

`kr lock` 不认识它运行的命令，改不了 xcodebuild 的参数。xcodebuild 会读环境变量 `XCODE_XCCONFIG_FILE`：文件里的设置用于所有 target，并盖过其他一切设置，包括命令行和 `-xcconfig`。实测它对 SwiftPM 包 target 同样生效，缓存在两个目录之间对包 target 也命中（`docs/research/xcode-compilation-cache.md` 第 4 节）。

## 决定

1. `kr lock` 加 `--compilation-cache`，默认关闭，由调用方逐次选择。只有 `.kr.toml` 的 `[project]` 写了 `compilation_cache = true` 才生效。
2. 生效时，kr 把 ADR 0013 的全部设置（缓存目录、诊断输出、四个前缀映射开关、`SWIFT_/CLANG_OTHER_PREFIX_MAPPINGS = <worktree 根>=/^worktree`）写进本次运行目录里的 `compilation-cache.xcconfig`，经 `XCODE_XCCONFIG_FILE` 传给子进程。缓存目录与 `kr run` 共用：`<kr 状态目录>/cache/<项目名>/compilation`。
3. 下列情况不开缓存，照常运行命令，并在 stderr 写一行原因（`--json` 时是 `{"compilation_cache_ignored": …}`）：
   - 找不到 `.kr.toml`，或它没有打开缓存；
   - worktree 在 `/tmp` 或 `/var` 下（macOS 上指向 `/private` 的符号链接，缓存永远不命中，ADR 0013）；
   - 调用方已经设了 `XCODE_XCCONFIG_FILE`。kr 不覆盖也不合并：那个文件里可能有任何设置，环境变量文件又盖过一切，kr 判断不了合并后会改变什么；
   - 命令的某个参数是 `archive`。前缀映射会改写调试信息里的源码路径，发版产物不能这样编。命令藏在脚本里时 kr 看不到，由调用方负责不在归档时传这个参数。
   不开缓存不让命令失败：这是性能选项，不该把构建变红。
4. `slot_released` 事件多一个 `compilation_cache` 字段，只在调用方传了这个参数时出现：`{enabled: true, dir, xcconfig}` 或 `{enabled: false, ignored: <原因>}`。没有这个字段就是没选。
5. 不做 ADR 0013 第 6 条的「空缓存再编一次」。`kr lock` 不知道命令是不是构建、失败输出在哪里，也不该替调用方重跑命令。缓存导致的失败由调用方去掉参数重跑来判断。
6. 不统计命中数。命令的 DerivedData 由调用方决定，kr 找不到可靠的活动日志。

## 实测

### 方法
- 机器：Conflux 的日常构建机，Mac15,5（M3，8 核，24 GB），Xcode 27.1（27A9275）。kr 是本分支编出的 release 版。测量期间其他 agent 照常工作，没有让机器空闲。
- 对象：Conflux main 7dcc4b7（2026-11-14），`xcodegen generate --spec ConfluxMobile.yml` 后 `xcodebuild build -project ConfluxMobile.xcodeproj -scheme ConfluxMobile -destination 'generic/platform=iOS Simulator' -derivedDataPath <全新目录>`，整条经 `kr lock build [--compilation-cache]`。
- 每次构建用一个新的 detached worktree（放在 `/Users` 下）、一个全新的 DerivedData。开缓存的 worktree 只在自己的 `.kr.toml` 里加 `compilation_cache = true`。包解析（`-resolvePackageDependencies`）在同一把锁里先做、单独计时（13–17 秒），不算构建时间。
- 预热：先用一天前的 main（3feba56，之后 23 个提交、183 个文件）开缓存编一次，填缓存。这模拟「别的 worktree 昨天从 main 切出来编过」。
- 两对（开关交替，ABBA 顺序：关、开、开、关），DerivedData 在 worktree 外。再一组三次，DerivedData 在 worktree 内（`build/Verify/…`，与 Conflux 的 `verify-changed.sh`、`test.sh` 一致）：先开缓存编一次填这种路径的键，再关、开各一次。
- 记录：墙钟与 CPU 秒（`/usr/bin/time -l` 的 user+sys，包括编译器子进程），kr 事件里的持有时长，活动日志里的命中数（与 `kr run` 同样的计数规则，分 Swift 编译、模块、Clang），开始时的 load1 和其他进程的 CPU 占用（`ps` 合计，扣除本次构建的进程树），构建期间每 10 秒再采一次。

### 数据

| 构建 | DerivedData | 墙钟（秒） | CPU 秒 | 持有槽位（秒） | Swift 编译命中 | 模块命中 | Clang 命中 | 开始 load1 | 其他进程 CPU：开始 / 期间均值 |
|---|---|---|---|---|---|---|---|---|---|
| 预热（开，冷） | 外 | 237.0 | 1128 | 255.1 | 0/226 | 0/746 | 0/138 | 38.5 | 489% / 不可用¹ |
| 第 1 对 关 | 外 | 217.4 | 1097 | 231.9 | — | — | — | 44.1 | 408% / 132% |
| 第 1 对 开 | 外 | 138.4 | 691 | 153.7 | 122/228 | 503/522 | 138/138 | 47.8 | 335% / 200% |
| 第 2 对 开 | 外 | 123.7 | 660 | 137.5 | 122/228 | 533/541 | 138/138 | 35.6 | 220% / 172% |
| 第 2 对 关 | 外 | 194.9 | 1098 | 209.3 | — | — | — | 33.4 | 381% / 110% |
| 填内路径（开） | 内 | 109.2 | 598 | 122.8 | 122/228 | 557/565 | 138/138 | 7.6 | 49% / 171% |
| 第 3 组 关 | 内 | 164.1 | 1002 | 177.5 | — | — | — | 31.6 | 215% / 118% |
| 第 3 组 开 | 内 | 37.4 | 104 | 51.7 | 228/228 | 567/567 | 138/138 | 58.0 | 483% / 320% |

¹ 预热时的期间采样把本次构建的 `SWBBuildService` 和 `swift-frontend` 也算进去了（它们各自另开进程组），之后改为按进程树扣除。

- **DerivedData 在 worktree 外（两对）**：CPU 秒少 37.0% 和 39.9%，墙钟少 36.3% 和 36.5%，持有槽位少 33.7% 和 34.3%。关缓存的两次 CPU 秒 1097 与 1098，相差 0.1%，所以负载高时 CPU 秒仍是稳的量；墙钟受负载影响更大。
- **DerivedData 在 worktree 内（第 3 组）**：CPU 秒少 89.6%，墙钟少 77.2%，持有槽位少 70.9%。缓存里已有同一提交的结果，所以这是上限，不是日常值。
- 没有假红：8 次构建全部成功。
- 负载：每次开始时 load1 都在 7.6–58 之间（8 核），机器一直很忙。开缓存的构建期间其他进程的 CPU 均值（171%–320%）都高于同组关缓存的（110%–132%），负载偏向不利于开缓存一侧，所以没有一对因负载而夸大收益；但绝对时间不能和空闲机器比。
- 缓存目录 8 次构建后 3.8 GB。

### 为什么不是更多：DerivedData 在 worktree 外时，每次都有 106 个 Swift 编译任务不命中
- 两对里开缓存的构建都是 122/228，并且第 2 对开之前，缓存里已经有第 1 对开留下的同一提交的结果。所以这 106 个未命中不是提交改动造成的。
- 按 target 分：远程包（RevenueCat、SwiftProtobuf、MPVKit 的 Swift 部分）和 `ConfluxLiveActivity` 全部命中；本地包 `ConfluxCore`、`ConfluxPurchases`、`MPVSurfaceSupport` 系列、`SherpaOnnx`，以及依赖它们的 `ConfluxMobile` 不命中。
- DerivedData 换到 worktree 内以后：第一次仍是 122/228（这种路径下的键第一次出现），第二次 228/228。所以原因是每次构建各不相同、又在 worktree 根映射之外的某个 DerivedData 路径进了这些任务的缓存键。
- 推断（没有用 `llvm-cas` 逐项比对）：不命中的 target 都导入 Clang 模块（SocketRocket、MPV 与 sherpa-onnx 的 C 模块），预编译模块的路径在 DerivedData 里；纯 Swift 的 target 不受影响。宏插件路径已排除：构建日志里没有指向 DerivedData 的插件路径。
- ADR 0013 没有遇到这个问题，因为 `kr run` / `kr check` 的 DerivedData 在 worktree 内（`build/kr/DerivedData`）。

## 用法上的结论
- 开缓存时，让 DerivedData 在 worktree 内（`-derivedDataPath <worktree 内的目录>`）。Conflux 的门禁脚本已经是这样；用 Xcode 默认 DerivedData 的调用收益会小一截，相当于上面的两对。

## 影响和限制

- 生效后，调用方在命令行或 `-xcconfig` 里给出的同名设置被 kr 的值盖掉。这些设置只决定走不走缓存，不改编出的产物（ADR 0013 第 4 条）。
- 调试信息里的源码路径变成 `/^worktree/…`，同 ADR 0013。
- 嵌套调用（祖先已持有槽位）同样传 xcconfig，但不写事件，与现有的重入路径一致。

## 参考
ADR 0013；`docs/research/xcode-compilation-cache.md` 第 4 节；`man xcodebuild` 的 ENVIRONMENT 一节；#80。
