# #39 机器并行度校准与旧锁迁移

## 决策边界

ADR 0022 的取锁与观察方向已由协调者确认。容量用下面的小样本校准；本票不改任何机器的真实配置、不改 Krill/Conflux，也不改全局 AGENTS.md。迁移由协调者在本票合入后执行。

## 方法（ADR 0010 第 4 层）

- 输入：本票 `tests/sample` 的临时副本，KrSample scheme，iOS 18.6，iPhone 16 Pro；只运行 `KrSampleUITests/LaunchUITests/testShowsGreeting`，每次结果必须恰好 1 PASS / 0 FAIL，0 条拒绝。build-for-testing 使用 generic iOS Simulator，禁用签名；没有访问真实服务。
- 工具：同一份本机构建的 release kr，work 不安装 Rust。脚本 `scripts/calibrate-machine.py`；work 的临时目录为 `~/kr39`，本机 `/tmp/kr39-calibration`。Xcode 和机器身份保存于证据 doctor.json。
- 隔离：先按旧协议排队并持有真实 `/tmp/native-build.lock`，单个命令覆盖整个测量窗口；内部 `KR_CONFIG` 指向空临时文件，无 `[legacy]`，`KR_STATE_DIR` 独立。每个构建和模拟器任务都经 kr 的槽位；模拟器还经 `kr pool lease`，使用 Krill-Verify-N 池，不碰其他项目的进程或设备。
- 构建：每批固定两次新 DerivedData 构建，容量 1、2 交替两轮（A/B/A/B），不清 SwiftPM/全局编译缓存。每项耗时在拿到槽位后开始；总耗时包含排队、进程与关机开销。
- 模拟器：每批固定六次同一启动断言，第一轮容量递增，第二轮递减，减少时间漂移的单向偏差。本机当前公式 2，测 2/3；work 临时配置绕过真实配置的 sim=2 覆盖，按硬件公式 5 测 2/3/4/5/6。这个临时设置不改变真实机器配置。
- 保护：每秒采样 load 1/5/15 与 `kern.memorystatus_vm_pressure_level`；warn/critical 即向本次实验的 kr 子进程发 TERM，停止新增负载。失败保留，不把基础设施失败算通过，不扩到更大容量。不跑完整门禁。
- 统计：每设置同时报告两轮总耗时、单项平均耗时、失败/样本数。同设置两轮差异是观测噪声；用配对总耗时比 bootstrap 95% 区间。只有两对，区间退化且不能证明可推广。0/n 失败的 95% 上界约 3/n。
- 第 2 层使用仓库已保存的 Krill gate trace 和 kr 自己的调度器，输出原容量校准误差与超出范围的标记；不另写调度模型。第 4 层只有 6 项，capacity=5 的尾批会放大 5→6 的理论收益，不能冒充长期吞吐或 Krill 门禁收益。

## 实测结果（2026-10-16）

同一 release 二进制 SHA-256：`3205d84779c434767eb1aa9391cddb17612db60ea0c372b265ae984af11dc345`。两机回读相同。下表每个 build 设置有 4 个构建样本，每个 sim 设置有 12 次独立启动断言（同一测试方法）。总耗时含获取槽位、boot、执行、shutdown；单项均值仅计拿到设备后 xcodebuild 子命令。

| 机器 | 资源 | 并行度 | 两轮总耗时（秒） | 单项均值（秒） | 失败/样本 | 同设置两轮差异/均值 |
|---|---|---:|---|---:|---:|---:|
| local | build | 1 | 16.63 / 16.40 | 8.03 | 0/4 | 1.4% |
| local | build | 2 | 11.55 / 12.82 | 11.85 | 0/4 | 10.4% |
| local | sim | 2 | 144.56 / 124.59 | 28.92 | 0/12 | 14.8% |
| local | sim | 3 | 169.49 / 133.95 | 49.83 | 0/12 | 23.4% |
| work | build | 1 | 16.15 / 14.73 | 7.54 | 0/4 | 9.2% |
| work | build | 2 | 8.71 / 9.35 | 8.56 | 0/4 | 7.1% |
| work | sim | 2 | 109.17 / 75.62 | 15.16 | 0/12 | 36.3% |
| work | sim | 3 | 92.22 / 61.10 | 19.19 | 0/12 | 40.6% |
| work | sim | 4 | 93.79 / 75.88 | 27.97 | 0/12 | 21.1% |
| work | sim | 5 | 82.29 / 91.21 | 32.91 | 0/12 | 10.3% |
| work | sim | 6 | 119.57 / 95.36 | 53.44 | 0/12 | 22.5% |

### 结论与公式

- build：work 两任务并行的配对总耗时比为 0.587（bootstrap 95% 区间 0.539–0.635），约节省 41.3%。本机比值 0.738（0.695–0.782），约节省 26.2%；但 KrSample 的 7–13 秒构建不能证明大型 App/FFmpeg 的内存需求。本机保留 1，work 升到 2，仍预留每构建 16 GB，最大只建议已测过的 2。两对 bootstrap 只能反映这两个观测值，不能覆盖其他工程、系统时段或机器。
- sim：本机 3/2 的配对比值 1.124（1.075–1.172），两轮均变慢，保留 2。work 6/5 的比值 1.249（1.045–1.453），两轮均变慢，不升到 6。work 3/4/5 的排序随轮次变化，不能证明谁恒定最优；保留按性能核预算得到的 5。原 sim 公式在这两机上没有被数据推翻，保持不变。
- 新 build 公式：`clamp(min(P/5, M/16), 1, 2)`；sim 保持 `clamp(min(P/2, (M-8)/4), 1, 6)`；light 保持 `clamp(P/4, 1, 4)`。P 是性能核数，M 为 GiB，整数除法，内存扣减饱和到 0。其他硬件未测，建议值不是性能保证，机器显式配置继续优先。

| 机器 | 硬件 | 旧硬件建议 build/sim/light | 新硬件建议 build/sim/light | 真实配置的生效值 |
|---|---|---|---|---|
| local | M3，4P，24 GiB | 1/2/1 | 1/2/1 | 1/2/1 |
| work | M4 Pro，10P，48 GiB | 1/5/2 | 2/5/2 | 2/2/2；sim=2 仍由真实配置覆盖 |

doctor 增加 `suggested_capacity`，原 `capacity.value/source/formula` 含义不变。覆盖值的人读行下也显示 hardware suggestion，避免把旧覆盖值误认为新公式。未修改真实配置。

最终 release 在两机的真实配置下再次运行 doctor，结果见 `local-doctor-after.json` / `work-doctor-after.json`。两机最近的非 kr owner 都是本次实验驱动 `kr39 calibration`，因此输出保守地要求确认迁移清单，未宣称桥接可以立即删除。观察只覆盖 doctor 和桥接取锁；不能据此断言 Conflux 或 Krill 的旧入口已停用。work 的 `~/kr39` 已在证据回收后删除，保留真实配置、状态和池设备。

### 负载、失败与误差

- local：610 次每秒采样，1 分钟负载 8.35–606.03；541/610 高于 32。work：925 次采样，1 分钟负载 1.86–1003.21；846/925 高于 56。两机所有采样压力级别均为 1（normal），没有触发 warn/critical 停止。load average 含排队/I/O，不等于 CPU 利用率；没有据此删改负载规则。
- 实测 build 16/16 成功，启动断言 84/84 PASS，0/84 观察到的假失败。按机器看 local 0/24、work 0/60；用 3/n 近似，未来失败率的 95% 上界仍约 12.5% 和 5%，各 sim 容量只有 12 个样本，上界约 25%。不能宣称无偶发，也不能推断负载与失败的因果关系。
- 同设置两轮的总耗时差异最高 40.6%；单项启动有长尾（本机 sim=3 首轮含 139.27 秒和 111.63 秒的 xcodebuild）。全部保留。模拟器任务每项租约都会 boot/shutdown；Krill 的长期通道通常复用设备，不能把这些耗时直接外推成完整门禁收益。
- 第 2 层使用已有 Krill 33 项 gate trace，通过 kr 自己的 `kr simulate --sim 2 --sim 3 --sim 5 --sim 6 --json`，没有另写调度模型。原容量回放 measured=2758411 ms、simulated=2759113 ms，MAPE=0.0254%；what-if 的 p50 从容量 2 的 2632300 ms 到容量 3/5/6 的 2302584 ms（方向约 -12.5%）。3/5/6 全部标注 outside_calibration；该误差只属于旧 trace 的原容量回放，不是并发新容量的校准误差。第 4 层和它是不同工作负载，不能将两者差异当作模型误差。

### 复现入口

保留桥接的机器可使用以下命令拿真实旧锁，内部只在 owner 批准的校准窗口使用独立资源状态。不能在正常开发中复制这个隔离设定绕开队列。测量脚本会验证旧锁 owner 是自己或祖先：

```sh
KR39_ROOT=/tmp/kr39-calibration-new
mkdir -p "$KR39_ROOT/bin"
cp -R tests/sample "$KR39_ROOT/sample"
cp target/release/kr "$KR39_ROOT/bin/kr"
: > "$KR39_ROOT/machine.toml"
kr lock build --task kr39-calibration -- python3 "$PWD/scripts/calibrate-machine.py" "$KR39_ROOT"
```

work 上复制同一 release、sample、脚本到 `~/kr39`，将 `/opt/homebrew/bin` 加到本次 SSH 命令的 PATH，用 `~/kr39/bin/kr lock build --task kr39-calibration -- python3 ~/kr39/calibrate.py ~/kr39`；不改全局 PATH 或真实配置。实际本票窗口由遵守旧 FIFO/mkdir 协议的父 Python 驱动持锁，owner 是 `kr39 calibration`。

[原始汇总](evidence/machine-concurrency/summary.json)、两机 batches/load/doctor、[历史 trace 模拟输出](evidence/machine-concurrency/offline-simulate.json)、[身份](evidence/machine-concurrency/identity.json) 均已脱敏保存。`driver-measured.py` 是本票实际运行的原始驱动；维护入口额外加入祖先锁检查、相对路径规范化、非零/缺失结果立即停止和保留父退出码，不改变本次全通过轨迹的计时。

## 可执行迁移顺序

1. 协调者合入本票后，在本机与 work 安装同一份新 kr；work 无须 Rust。保留 `[legacy]`。分别运行 `kr --version`、`kr doctor --json`，记录 capacity.value/source/formula 和 legacy。真实配置的覆盖项必须逐项审查，否则 work 的 sim=2 仍覆盖公式。
2. 切换脚本。Conflux 先只读搜索 `native-build.lock`、`native-light.lock`、`KR_HELD` 和手写队列，按每个命令替换为 `kr lock build|sim|light --task <stable-name> -- <command>`；不要把凭证放到 `--task` 或命令参数。模拟器脚本改用 `kr pool lease`，不要保留旧的设备租约目录。Krill 按下一节拆分入口和真实服务负载。各脚本确认原来成功/失败/信号退出码保持，确认同资源嵌套可重入、别的进程会排队。
3. 两台机器都替换全局 AGENTS.md 的 Native work lock 节为后面的英文原文。不要把临时 KR_CONFIG/KR_STATE_DIR 写进全局 shell；所有项目必须共享机器的默认资源状态目录。
4. 列清仍可能执行旧脚本的自动任务、agent、手动入口；运行 doctor，审查七天内观察到的非 kr owner。实验锁 owner `kr39 calibration` 是本票临时驱动，结束后不再使用；其他 task 必须追到入口并迁移。没有观察到不等于没有旧用户，owner 仍需确认清单。
5. 两台机器分别在确认没有旧协议入口后删除 `[legacy]` 的 machine_lock/queue/poll；这一步才解除跨 kr 进程串行化。保留其他配置和状态。再运行 doctor，核对容量来源，以及并行 kr lock 的 queue 输出。回滚时先停新入口，在所有入口都可识别旧锁前重新打开桥接，避免单边开桥接产生并发窗口。

### Native work lock 全段替换文本

用以下内容整体替换两机全局 AGENTS.md 中从 `- Native work lock:` 到下一个同级规则前的全部旧锁段落（保留后面的 E2E source of truth 规则）：

```text
- Native work resources: all agents and projects on this Mac share kr's resource queues. Read `kr skill` before the first Apple build or test. Use `kr doctor --json` to see this machine's effective capacities and their sources; do not impose a separate fixed simulator count.
  - For a configured project, use `kr check`, `kr run`, and `kr gate`. They acquire and release resources themselves. Do not wrap these commands in an extra heavy-work lock.
  - For a script that does not use kr yet, run each heavy build as `kr lock build --task <name> -- <command>`. Run a simulator task as `kr pool lease --task <name> -- <command>` in the project's configured pool. The command receives KR_SIM_UDID and KR_SIM_NAME; use that one device. kr boots, leases, shuts down, and releases it.
  - `kr lock sim --task <name> -- <command>` limits one simulator task but does not allocate a device. Use it only when the command already has a kr-managed device lease. Never start several simulators under one sim slot. Run each lane under its own lease, up to kr's effective sim capacity and the project's available pool size.
  - A macOS test of one small module may use `kr lock light --task <name> -- <command>` if it builds no app or heavy dependencies and uses no simulator or physical device. It should finish within 2 minutes. If it exceeds 5 minutes, stop it and run it again under build.
  - All invocations must use the same machine resource state directory. Do not use isolated KR_STATE_DIR or capacity overrides to bypass other tasks. Calibration beyond current capacity requires explicit owner direction and must stop if memory pressure reaches warn/critical or false failures appear.
  - Do not create or remove /tmp/native-build.lock, /tmp/native-light.lock, their queues, or another task's lock files. During migration kr's configured legacy bridge owns this protocol. Remove [legacy] only after every old entry point on that machine has migrated and the owner has confirmed the checklist.
  - A kr lock holds one slot for one command and returns the child's exit code. While waiting, do independent work; do not start a parallel unqueued build. Do not kill another project's process or shut down its device. For physical-device tests use kr run --device with the intended paired device.
```

### Krill scripts/e2e.sh 的确定替换

只读基准：`/Users/cyh/orca/workspaces/Krill/v0-integration-claude`，本票记录 source tree hash 见证据。不能只把 mkdir 换成 `kr lock sim`：旧脚本 `lease_root` 和 kr 的设备租约互不相见，而且 smoke 会在一个锁里开两台设备。

建议保持 CLI 兼容，分两部分落地：

1. 合成场景（含 smoke、batch、affected、--changed）入口改为 `kr run` / `kr gate`，让 kr 构建一次、排队并租设备；原 `e2e_affected` 可保留计算列表，逐个验证场景后把数组一次传给 kr。入口把最后的退出码原样返回。不要给 `kr run` 再套 `kr lock`。`--changed` 原含义是完整 UI 目标加四项排除，不能用 impact.json 的 synthetic 数组代替；在 Krill `.kr.toml` 新增下面的兼容场景，入口用 `kr run legacy-all-ui`：

```toml
[scenarios.legacy-all-ui]
only = ["KrillUITests"]
skip = [
    "KrillUITests/ManualScreenshotTests",
    "KrillUITests/IPadLayoutUITests",
    "KrillUITests/WebDAVUITests/testRealRcloneScanPlaybackAndSeek",
    "KrillUITests/SMBUITests/testRealSMBScanPlaybackAndSeek",
]
```

迁移时逐项对照 `e2e_selection` 与 `.kr.toml` 的 only/skip/device；缺少或不相同的配置先补齐再切入口。manual-screenshots 的 phone/iPad 分支必须各有匹配的设备配置，不能静默退为合成清单。上面的 `legacy-all-ui` 精确保留旧过滤；若要额外排除 Plex 真实场景，属于行为变化，须由 owner 另行确认。
2. `webdav-real` / `smb-real` / `plex-real` 保留现有私有配置注入与验证逻辑，单场景负载提取为 `scripts/e2e-real.sh`。新入口对每个真实场景执行下列精确命令：

```bash
# Run from the project root; one real-service scenario at a time.
# Each invocation owns distinct products, so different processes cannot overwrite them.
export KRILL_E2E_DERIVED_DATA="${KRILL_E2E_DERIVED_DATA:-build/UIE2E-real-$(date +%s)-$$}"
exec kr lock build --task "krill-e2e-${scenario}-build" -- \
    kr pool lease --task "krill-e2e-${scenario}" -- \
    bash scripts/e2e-real.sh "$scenario" --device sim
```

这条兼容路径在整个单场景期间占 1 build + 1 sim（含构建期间）；先保持共享产物和私有文件安全，真实场景进一步拆 build/test 槽位属于后续优化。不能在批次父脚本先拿 sim 再调用此命令，否则会形成资源反序；父批次无槽位，逐场景调用新入口。`KRILL_NATIVE_LOCK_HELD` 不再使用。

`e2e-real.sh` 从当前文件按下列确定的块提取/替换，除此之外保留签名、凭证、过滤和结果断言：

- 入口只允许上面三个真实场景以及 `--device sim`；缺 `KR_SIM_UDID`/`KR_SIM_NAME` 立即退出 2。设备只来自父 `kr pool lease`。
- 删除 `native_lock`、`native_queue`、`native_lock_owned`、`queue_ticket`、`release_native()` 及整个 `if [[ "${KRILL_NATIVE_LOCK_HELD:-0}" != 1 ]]` 队列块。
- 删除 `lease_root`、`leases`、`allocation_owned`、`lease_devices()`、`release_devices()` 及 cleanup 中相应调用。改为 `devices=("$KR_SIM_UDID")`；不再直接 boot/shutdown。父 kr 在 payload 完成 cleanup 后归还设备。
- 删除 `phone_indices`/`ipad_indices`/`real_indices` 分组、多通道与 smoke 特例；`scenario="$mode"`、`udid="$KR_SIM_UDID"`，只调用现有 `run_scenario 0 "$udid"`。删除写死的 `booted + count > 2`（设备分配已完全交给 kr）。
- 保留 credential 注入段（webdav/smb/plex 分支）、`validate_result()`、`run_scenario()`、每个真实场景独立签名 App、`TEST_RUNNER_*` 环境和最终 PASS/FAIL、最多三行错误、xcresult 路径。凭证仍只写到原来的 App 私有文件，并由 cleanup 删除。
- build-for-testing、工程生成继续在父 build 槽位下执行；删除 `KRILL_E2E_REUSE_BUILD` 旧的父锁签名校验分支，真实场景每次使用自己的 DerivedData 构建。不能用伪造环境变量跨运行复用私有产品。
- cleanup 先终止并 wait 本负载的测试子进程，再删除私有配置。父 kr 收到 SIGINT/SIGTERM/SIGHUP 会转发到整个进程组；payload 的 trap 保留 `INT -> 130`、`TERM -> 143`，并补 `HUP -> 129`；这些清理结束后才由父租约关机归还。
- `scripts/e2e-modes.sh:e2e_batch` 不再注入旧锁环境变量，也不设置 REUSE_BUILD；它在不持任何槽位的父进程中顺序调用入口（或把合成数组一次传给 kr run）。复用现有批次 PASS/FAIL 汇总，不吞子命令退出码 4。

协调者应在 Krill 另票为这次提取写脚本回归：旧锁目录完全不访问、真实场景的正确 App/凭证文件/测试过滤、并行脚本不租同设备、取消后清理、0 条测试拒绝。真实服务凭证和验收不在本票。
