# #69 用 Conflux 真实构建校准 build 公式（结论：本机保持 1）

> **状态：下面的结论未证实，等待在空闲机器上重测。** 测量时机器上还有别的会话：t = 1 秒（编译开始前）CPU 空闲只有 53%，PhysMem 只剩 422 MB 空闲，build 队列里还有别的 Conflux 任务在等。采样只记录整机 CPU，分不出构建自己的 CPU 和别人的 CPU，所以“一个冷构建就占满 CPU”这一条没有证据支撑。
>
> 新的驱动脚本（#69b）先等机器安静，再测量，并且按进程归属 CPU：
>
> - 安静门：`--not-before` 之后轮询，`kr queue --json` 里 build/sim/light 连续 120 秒没有别的持有者或等待者，驱动进程树之外的 CPU 低于整机的 15%，内存压力为 normal，才执行 `kr lock build --task kr69b-calibration`。到 `--deadline` 仍不安静就以 `not-quiet` 退出（退出码 3），不测量。
> - 每个构建用 `wait4` 取自己和全部子进程的 user + sys CPU 秒，算出有效核数 = CPU 秒 / 墙钟秒。`SWBBuildService` 和 `swift-frontend` 都是 `xcodebuild` 的子进程（2026-11-14 用一个小 Swift 包实测），所以都计入。
> - 每秒一条采样：整机 CPU（`top`）、每个构建进程树和驱动进程树的 CPU（`ps` 累计 CPU 时间的差）、树外 CPU 最高的 5 个进程、内存压力、swap、空闲内存、`kr queue`。某阶段树外 CPU 超过阈值的采样多于 10% 时，分析脚本把这个阶段标为受干扰，它所在的配对不计入结论。
> - 冷构建按 A/B/A/B（串行、并行、串行、并行），测量窗口最长 60 分钟；不再测增量构建，也不再改源文件。
> - 分析脚本另写一份可以提交的 `public/`：家目录、用户名、主机名、team ID 都被替换，替换后仍有残留就拒绝写出。
>
> 离线测试与 30 秒内跑完的整套演练：`python3 -m unittest discover -s scripts -p test_calibrate_conflux_build.py`，`python3 scripts/calibrate-conflux-build.py run --dry-run --out /tmp/kr69b-dry`。

## 结论

本机（iMac，Apple M3，4 性能核 + 4 能效核，24 GiB）同时跑两个 Conflux 冷构建，总耗时与串行相同，内存压力升到 warn。数据不支持把本机 build 容量改为 2。`clamp(min(P/5, M/16), 1, 2)` 不变，不写新 ADR。

| 容量 | 两个构建的总耗时 | 单个构建耗时 | CPU 空闲均值 | 内存压力 | swap |
|---:|---:|---|---:|---|---|
| 1（串行） | 366.6 秒 | 180.9 / 185.8 秒 | 9.7% | 301/301 次采样 normal | 1176 MB，不变 |
| 2（并行） | 372.1 秒 | 372.1 / 371.6 秒 | 3.2% | 283 次 normal，15 次 warn | 1176 MB，不变 |

- 配对总耗时比 372.1 / 366.6 = 1.015：并行没有省时间。
- 每个构建的减速倍数 371.8 / 183.3 = 2.03。
- 4 个构建全部 `BUILD SUCCEEDED`。

## 为什么不是更快：瓶颈是 CPU

单个冷构建已经把 8 个核占满：串行阶段 301 次采样里 245 次 CPU 空闲低于 10%，空闲均值 9.7%。第二个构建只能分走剩下不到 10% 的空闲，所以两个构建各自变慢一倍，总耗时不变。#39 的 KrSample 构建只有 7 到 13 秒，大部分时间花在进程启动和串行步骤上，单个构建占不满 CPU，所以当时测到并行省 26%（推论：#39 没有采 CPU 空闲）。这个差别说明小样本高估了大工程的收益。

日志里没有编译缓存回放：单个构建约 3,900 个编译步骤，generic iOS Simulator 目的地同时编 arm64 与 x86_64。所以这里测的是真实编译，不是缓存命中。SwiftPM 和 CarPlay 预编译框架的用户级缓存保留（与 #39 相同），第一次解析在计时外预热。

## 内存

- warn 出现在并行阶段末尾，t = 706.8 秒到 724.0 秒，共 15 次采样（约 18 秒），没有到 critical。
- 压缩器占用从串行阶段的最高 6.42 GiB 升到 8.40 GiB；swap 全程 1176 MB 不变。
- 实验前机器空闲时 `PhysMem` 已显示约 23 GB used、5.7 GB compressor：这台机器上还有别的应用常驻，两个构建留给它们的余量很小。

按预先写下的规则（每一对并行都要比串行快，且全部采样 normal），两条都不满足。

## 方法

- 工作负载：Conflux origin/main `186bcb0f`（见 `evidence/issue-69/conflux-commit.txt`）的临时分离检出，`xcodegen generate --spec ConfluxMobile.yml` 后执行 `xcodebuild build -project ConfluxMobile.xcodeproj -scheme ConfluxMobile -destination 'generic/platform=iOS Simulator' -derivedDataPath <每个构建一个新目录>`。Xcode 版本见 `evidence/issue-69/xcode.txt`。
- 独占：整个窗口用 `kr lock build --task kr69-calibration -- python3 scripts/calibrate-conflux-build.py …` 持有一个真实 build 槽位（本机 build 容量 1，没有用独立 `KR_STATE_DIR`）。窗口内没有别的 kr 持有者；只有一个 `conflux-zero-test-guard-test` 在 build 队列里等。sim 槽位全程空闲。kr 之外的进程（桌面应用）没有控制。
- 采样：约每 1.2 秒一次，记录 `kern.memorystatus_vm_pressure_level`、`vm.swapusage`、`vm.compressor_bytes_used`、1/5/15 分钟负载、`top` 的 CPU 与 PhysMem 行、剩余磁盘、`kr queue --json`。共 626 次，见 `evidence/issue-69/samples.ndjson.gz`。
- 安全规则：压力到 warn 后不再新增负载；到 critical 终止构建。本次在第一对并行构建末尾到 warn，驱动脚本按规则停止，所以只跑了一组 A/B（不是 A/B/A/B），增量构建（改一个 ConfluxCore 文件后重编）也没有测。整个窗口 12.8 分钟。

## 回放（`kr simulate`，在 Conflux 工作区执行）

#69 时的 `--slowdown` 只让“开始时已有别的构建在跑”的那个构建按整段时长变慢，已经在跑的那个不变慢。#77 改为处理器共享：两个以上构建同时持有槽位时，每个都按记录速度的 1/slowdown 推进，只剩一个时恢复原速。两个构建全程并行时各慢 2.03 倍，与本次实测一致；slowdown 1.0 时与旧算法逐毫秒相同。

`kr simulate --build 1 --build 2 --slowdown 2.03 --since 2026-11-07T14:29:45Z`（#77 重放时的 `--since 7d`，固定下来以便复现；原文见 `evidence/issue-69/simulate-2.03-overlap.txt`，#69 当时的原文见 `evidence/issue-69/simulate-2.03.txt`）。同一窗口，1892 个 build 请求：

| 设定 | 排队总时长 | 占比 | p90 等待 |
|---|---:|---:|---:|
| build=1 | 2042.1 分钟 | 38% | 3.6 分钟 |
| build=2，slowdown 2.03，#69 的算法（只拉长后开始的） | 1200.4 分钟 | 19% | 2.0 分钟 |
| build=2，slowdown 2.03，#77 的算法（重叠期间都变慢） | 833.3 分钟 | 15% | 1.1 分钟 |

校准误差 −32.8%（目标 ≤ 10%），build=2 在校准范围外，只能看方向。

新算法的 build=2 排队比旧算法少，不是多。两个算法在这个窗口里给持有时间加的总量：旧算法 1783.0 分钟，新算法 1399.8 分钟（用 `tests/fixtures/simulate/conflux-build-queue/oracle.py` 的新规则和它 #77 之前的旧规则在同一批事件上独立算出，两者的排队总时长都与 kr 一致）。原因：旧算法把后开始的构建整段乘 2.03，哪怕旁边那个几秒后就结束；新算法只拉长真正重叠的那一段，虽然先开始的那个也变慢（被拉长的请求 880 → 946 个），总量仍更小。两个构建全程重叠时旧算法偏乐观，部分重叠时偏悲观，这批数据里部分重叠占多数。

回放仍显示 build=2 能减少排队，主要原因是：7 天的 build 请求里有大量很短的持有，例如 `conflux-zero-test-guard-test` 中位数不到 1 分钟。它们排在 3 分钟的构建后面等，容量 2 让它们插空，所以排队时间下降，但这不需要多一份 CPU。新算法让这些短任务旁边的大构建也慢 2.03 倍，而一个不到 1 分钟、不编 App 的检查大概占不满 CPU（推论，未测），所以对这类重叠 2.03 偏悲观。

## 建议

- 本机保持 build=1，公式不变。
- 本机的排队要从短任务入手：把只跑几秒、不编 App 的检查移到 light（#70 的 `kr report` 已经列出候选，Conflux 分支 `t3/research-build-verification-speedups` 的 418ed880 正在迁，尚未合入）。这比把大构建并行更省时间，也不增加内存压力。
- `kr simulate --slowdown` 已改为对重叠的构建都生效（#77），结果见上表。

## 没有验证的

- 只有 1 对样本，没有同设置的重复，无法给出区间或噪声。#39 同设置两轮的差异最高 10.4%（build），而这里的结论是“并行没有省时间”，与 10% 噪声相比，1.5% 的差异不足以说明更慢，只能说明没有收益。
- 增量构建没有测（被 warn 停止规则跳过）。增量构建的串行步骤（链接、脚本阶段）比例更高，单个构建可能占不满 CPU，并行可能有收益。kr 的容量不区分冷构建与增量构建，而新工作区的第一次构建都是冷构建，所以单靠增量数据不足以改变本结论。
- work Mac（M4 Pro，10P，48 GiB）的 build=2 仍来自 #39 的 KrSample，没有用 Conflux 重测。

## 复现

上面的数据由 #76 合入时的旧驱动脚本产生（见该提交的 `scripts/calibrate-conflux-build.py`）。用现在的驱动脚本重测：

```sh
python3 scripts/calibrate-conflux-build.py run --conflux <Conflux> --out <证据目录> \
    --not-before 03:30 --deadline 07:30
```

它自己建 `/tmp` 下的分离检出、跑 xcodegen、等安静、拿锁、测量，退出时（包括失败和收到 SIGTERM）删除检出和 DerivedData，最后运行 `analyze-conflux-build.py`，写出 `summary.json` 和 `public/`。压力到 warn 后不再开始新阶段，到 critical 终止构建。证据：[summary.json](evidence/issue-69/summary.json)、[builds.json](evidence/issue-69/builds.json)、采样、构建日志末尾、回放原文。
