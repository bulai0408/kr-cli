# 0029 编译缓存的大小由 Xcode 按 kr 给的上限裁剪，kr 不另做清理

状态：提议（2026-11-19，#45，待协调者在 PR 审查时确认）。修改 ADR 0013「影响和限制」里「kr 不清理，靠 Xcode 默认上限」这一条；0013、0028 的其他决定不变。

## 背景

ADR 0013 把同一项目的编译缓存放在 `<kr 状态目录>/cache/<项目名>/compilation`，kr 不清理，只靠 Xcode 默认上限。Conflux 2026-11-18 打开 `kr lock --compilation-cache`（ADR 0028），不到一天缓存长到 15 GB。这台机器可用 324 GB，按默认上限缓存会长到约 160 GB。#45 要求：doctor 显示每个项目缓存的大小和最后使用时间；缓存有上限，默认值由机器硬盘算出，机器配置可以覆盖。

## Xcode 自己怎样裁剪（读源码）

- swift-build `Sources/SWBCore/Settings/CASOptions.swift`：上限依次取 `COMPILATION_CACHE_LIMIT_SIZE`（字节，或带 K/M/G/T 后缀；`0` 表示不限）、`COMPILATION_CACHE_LIMIT_PERCENT`、用户默认值 `CompilationCachingDiskSizeLimit`，都没有时是「缓存大小 + 剩余空间」的 50%。
- swift-build `Sources/SWBTaskExecution/DynamicTaskSpecs/CompilationCachingDataPruner.swift`：每次构建对每个用到的 CAS 跑一次裁剪任务（Swift 一次、Clang 一次），低优先级，与构建并行。它设上限、调用 `prune`，超限时在活动日志里写 `cache size (…) larger than size limit (…)`。
- llvm-project `llvm/lib/CAS/UnifiedOnDiskCache.cpp`（`next` 分支）：缓存按「代」放在 `v1.<n>` 子目录里。关闭缓存时，如果最新一代超过上限的一半，就新建下一代（要拿到 `lock` 的独占锁，拿不到就留给下一个进程）；`prune` 删除最新两代以外的所有代。删的只是没有进程在用的旧代，所以和并行构建、其他 worktree 的构建不冲突。
- 推论：稳定后缓存大小约为「上限 + 一次构建写入的量」：最新一代 ≤ 上限的一半才换代，上一代同样，再加正在写的这次。

## 实测

- 机器：Mac15,5，Xcode 27.1（27A9275）。对象：Conflux main 8567e294f，`xcodegen generate --spec ConfluxMobile.yml` 后 `xcodebuild build -quiet -project ConfluxMobile.xcodeproj -scheme ConfluxMobile -destination 'generic/platform=iOS Simulator'`，DerivedData 在 worktree 内，整条经 `kr lock build`。
- 缓存：用 APFS 克隆（`cp -c`）复制 Conflux 正在用的缓存（15.3 GB，`v1.1` 8.6 GB + `v1.2` 6.7 GB）到一个临时目录，不碰原缓存。设置同 ADR 0028 的 xcconfig，另加 `COMPILATION_CACHE_LIMIT_SIZE`，经 `XCODE_XCCONFIG_FILE` 传入。
- 每次构建前后用 `du -sk` 量各代目录；从活动日志数裁剪任务和超限提示。

| 构建 | 上限 | 开始时的代（GB） | 结束时的代（GB） | 秒 | 发生了什么 |
|---|---|---|---|---|---|
| 1 | 6G | v1.1 8.6 · v1.2 6.7 | v1.1 8.6 · v1.2 7.4 · v1.3 空 | 367 | 两个裁剪任务都报超限；关闭时最新一代超过 3 GB，新建 v1.3 |
| 2 | 6G | 同上 | v1.2 7.4 · v1.3 1.7 | 257 | 开始时 `prune` 删掉 v1.1；总量 16.0 → 9.1 GB |
| 3 | 3G | v1.2 7.4 · v1.3 1.7 | v1.2 7.4 · v1.3 2.4 · v1.4 空 | 199 | v1.3 超过 1.5 GB，新建 v1.4 |
| 4 | 3G | 同上 | （构建中途被会话中断）v1.3 2.4 · v1.4 1.6 | — | 开始时删掉 v1.2；总量 9.8 → 4.0 GB |

- 每次 Swift 编译命中 1268–1448、未命中 228–232。换代后第一次构建仍大量命中：上一代还在，换代不会让下一次构建变冷。
- 4 次构建没有失败（第 4 次是会话中断，不是构建失败；它的裁剪发生在构建开始时，已经观察到）。
- 结论：Xcode 会按 `COMPILATION_CACHE_LIMIT_SIZE` 裁剪，但要两次构建才落到上限附近（第一次换代，第二次删旧代），并且总量可以超过上限约一次构建的写入量。

## 决定

1. kr 写给 xcodebuild 的缓存设置里加 `COMPILATION_CACHE_LIMIT_SIZE`：ADR 0028 的 xcconfig 和 `kr run` / `kr check` 的命令行设置都加。裁剪由 Xcode 做，kr 不删缓存里的任何文件。
2. 上限按项目算（每个缓存目录一个）。默认值 = min(20 GB, 磁盘总容量的 5%)。磁盘总容量取缓存目录所在卷的 `statvfs`。用总容量不用剩余空间：剩余空间每次构建都在变，上限也会跟着跳；Xcode 默认用剩余空间，正是它在大盘上给得太大的原因。20 GB 足够放 Conflux 两代缓存（实测一代约 7–9 GB），256 GB 的机器得到 12.8 GB。
3. 机器配置 `[compilation_cache] limit = "30G"` 覆盖默认值；格式与 Xcode 相同（字节或 K/M/G/T 后缀，1024 进位；`"0"` 不限）。这是机器键，`.kr.toml` 里写会报错。写错时报出键名和示例。
4. `kr doctor` 显示上限和来源（配置或默认公式），以及 `<状态目录>/cache/*/compilation` 每个项目缓存的大小（已分配字节）和最后使用时间（目录下最新的修改时间）；`--json` 的 `compilation_cache` 字段同样有。目录不存在时为空列表。
5. 不做 kr 自己的清理入口。#45 设想的「按上限删」由 Xcode 完成，并且 Xcode 的换代机制本身就避开了正在使用的数据，kr 再做一遍只会带来与正在构建的进程抢锁的风险。

## 影响和限制

- 只裁剪「有人在构建」的项目：裁剪发生在构建里。一个项目不再构建以后，它的缓存停在最后的大小（≤ 上限 + 一次构建），不会再缩小，也不会再长。#45 的「闲置超时删除」没有做；doctor 显示的最后使用时间让人能看到这种缓存。需要时另开票。
- 用户要清空某个项目缓存时，目前只能在没有该项目构建运行时手动删目录。没有做入口。
- 上限改小以后，要两次构建缓存才降到新上限附近。
- 只在一台机器、一个项目、Xcode 27.1 上实测。裁剪逻辑来自开源的 swift-build 与 llvm-project，没有核对 Xcode 27.1 二进制里的版本是否完全一致；实测行为与源码一致。
- 调用方在自己的 xcconfig 或命令行里设的 `COMPILATION_CACHE_LIMIT_SIZE` 被 kr 的值盖掉（ADR 0028 的环境变量 xcconfig 优先级最高）。

## 参考
ADR 0013、0028；#45；swift-build `CASOptions.swift`、`CompilationCachingDataPruner.swift`；llvm-project `llvm/lib/CAS/UnifiedOnDiskCache.cpp`。
