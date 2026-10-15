# Krill 第一阶段实测：kr 与旧脚本对照（#9）

日期：2026-10-14。本文由脚本从证据文件和 kr 事件直接生成，没有手工拼数字。生成脚本与数据来源见末尾。

## 结论

- **测试数一致**：31 个合成场景中，31/31 个场景 kr 与旧脚本的测试数完全相同（按场景比对，见对照表）。
- **结果一致**：最终 31/31 个场景两边都通过；没有「一边过一边不过」。旧脚本的 module-watchlist 第一次失败，单独重跑后通过（证据目录里两次运行都在）。
- **kr 自己的问题**：首轮 kr 有 3 项（kit-tests、module-watchlist、shelf-share）误判为「过滤选中 0 条」。原因是 Krill 的枚举 JSON 有 77–93 KB，超过 kr 内存里保留的 64 KB 日志尾部。已修复并加回放测试；用这次留下的枚举日志离线重放，三项分别得到 571、298、24 条测试。修复后只补跑这 3 项，全部通过。这个 bug 让 kr 判「失败」，没有出现假通过。
- **偶发与高负载**：kr 最终结果里偶发 0 次；被标「高负载」的项 33/35（运行期间 1 分钟负载约 85–200，高于标注线 32）。高负载期间失败、整项重跑的有 2 次，都是上面那个枚举 bug 引起的，不是测试本身。
- **时间**：default 门禁墙钟 75.4 分钟，构建 0 次（全部复用 `kr check` 的产物）。模拟器阶段有 28.9 分钟没有任何设备在跑，都在等另一个项目持有的旧全机锁（ADR 0009 桥接）。kr 单个场景普遍比旧基线慢（例如 ratings 348s 对 234s），这次运行时机器负载远高于旧基线运行时；负载已写进每项事件，没有做负载归一化，耗时不能直接比较。
- **构建**：`kr check` 第一次真构建 6 个作业，第二次全部命中缓存（2 秒）；smoke 和 default 都没有再构建。旧流程中 e2e.sh 与 test.sh 各自构建。

## 范围与方法

- 旧脚本基线：`v0-evidence/batch6-gate`，在 `eaa1569` 上跑的 `e2e.sh`（31 个合成场景）和 `test.sh`。`eaa1569` 与本次实测的 `1adec35` 是同一棵代码树（tree `3aa0c7f`），所以没有重跑旧脚本。
- kr：Krill 分支 `p0/9-krill-kr`（`1adec35` + `.kr.toml`），依次 `kr doctor`、`kr check`、`kr gate smoke`、`kr gate`，修复后 `kr run kit-tests module-watchlist shelf-share`。全部经 kr 自己的槽位和旧全机锁排队，只用 Krill-Verify-1/2/iPad-1。
- 只跑合成场景；webdav-real、smb-real 不在范围内。

## 逐项对照

| 场景 | 旧：测试数（过/败/跳） | 旧：耗时 s | kr：测试数（过/败/偶发/跳） | kr：结果 | kr：耗时 s | kr：设备 | 测试数一致 |
|---|---|---|---|---|---|---|---|
| airing-calendar | 7（7/0/0） | 100 | 7（7/0/0/0） | PASS | 100 | Krill-Verify-1 | 是 |
| analytics | 1（1/0/0） | 24 | 1（1/0/0/0） | PASS | 31 | Krill-Verify-1 | 是 |
| detail-search | 8（8/0/0） | 169 | 8（8/0/0/0） | PASS | 189 | Krill-Verify-1 | 是 |
| favorites | 2（2/0/0） | 36 | 2（2/0/0/0） | PASS | 53 | Krill-Verify-2 | 是 |
| fnos-form | 2（2/0/0） | 22 | 2（2/0/0/0） | PASS | 36 | Krill-Verify-1 | 是 |
| icloud-sync | 3（3/0/0） | 51 | 3（3/0/0/0） | PASS | 70 | Krill-Verify-1 | 是 |
| ipad-layout | 10（10/0/0） | 372 | 10（10/0/0/0） | PASS | 446 | Krill-Verify-iPad-1 | 是 |
| legal-privacy | 6（6/0/0） | 145 | 6（6/0/0/0） | PASS | 173 | Krill-Verify-1 | 是 |
| library-groups | 8（8/0/0） | 195 | 8（8/0/0/0） | PASS | 191 | Krill-Verify-1 | 是 |
| match-correction | 8（8/0/0） | 342 | 8（8/0/0/0） | PASS | 426 | Krill-Verify-2 | 是 |
| media-markers | 2（2/0/0） | 33 | 2（2/0/0/0） | PASS | 49 | Krill-Verify-2 | 是 |
| module-watchlist | 298（298/0/0），首次失败后重跑通过 | 160 | 298（298/0/0/0） | PASS（修复后补跑） | 261 | Krill-Verify-2 | 是 |
| modules | 22（22/0/0） | 592 | 22（22/0/0/0） | PASS | 615 | Krill-Verify-2 | 是 |
| mvp-polish | 7（7/0/0） | 115 | 7（7/0/0/0） | PASS | 115 | Krill-Verify-1 | 是 |
| my-stats | 5（5/0/0） | 77 | 5（5/0/0/0） | PASS | 219 | Krill-Verify-2 | 是 |
| official-tmdb | 12（12/0/0） | 217 | 12（12/0/0/0） | PASS | 211 | Krill-Verify-1 | 是 |
| player-controls | 26（26/0/0） | 219 | 26（26/0/0/0） | PASS | 272 | Krill-Verify-1 | 是 |
| ratings | 127（127/0/0） | 234 | 127（127/0/0/0） | PASS | 348 | Krill-Verify-1 | 是 |
| search | 7（7/0/0） | 130 | 7（7/0/0/0） | PASS | 148 | Krill-Verify-1 | 是 |
| settings-about | 3（3/0/0） | 109 | 3（3/0/0/0） | PASS | 113 | Krill-Verify-1 | 是 |
| shelf-edit | 11（11/0/0） | 216 | 11（11/0/0/0） | PASS | 214 | Krill-Verify-2 | 是 |
| shelf-share | 24（24/0/0） | 87 | 24（24/0/0/0） | PASS（修复后补跑） | 189 | Krill-Verify-1 | 是 |
| shelf-share-ipad | 2（2/0/0） | 45 | 2（2/0/0/0） | PASS | 188 | Krill-Verify-iPad-1 | 是 |
| smb-form | 3（3/0/0） | 30 | 3（3/0/0/0） | PASS | 42 | Krill-Verify-1 | 是 |
| source-management | 14（14/0/0） | 230 | 14（14/0/0/0） | PASS | 258 | Krill-Verify-2 | 是 |
| source-view | 8（8/0/0） | 123 | 8（8/0/0/0） | PASS | 146 | Krill-Verify-1 | 是 |
| storage-usage | 2（2/0/0） | 77 | 2（2/0/0/0） | PASS | 94 | Krill-Verify-2 | 是 |
| subtitles | 142（142/0/0） | 156 | 142（142/0/0/0） | PASS | 178 | Krill-Verify-2 | 是 |
| trakt | 5（5/0/0） | 47 | 5（5/0/0/0） | PASS | 60 | Krill-Verify-1 | 是 |
| viewing-environment | 6（6/0/0） | 78 | 6（6/0/0/0） | PASS | 85 | Krill-Verify-1 | 是 |
| webdav-form | 3（3/0/0） | 32 | 3（3/0/0/0） | PASS | 40 | Krill-Verify-1 | 是 |

套件（对应 test.sh 的各步）：

| 套件 | 旧 test.sh | kr | 说明 |
|---|---|---|---|
| app-tests | 77（73 过 / 4 跳） | 77（73 过 / 4 跳） | 一致 |
| kit-tests | test.sh 用 `-quiet`，日志里没有测试数 | 571（567 过 / 4 跳） | 旧脚本无法核对数量，这正是 T5 类假通过的风险点 |
| kit-build | iOS / tvOS / macOS 三平台编过 | 三平台编过（`kr check`） | 一致 |
| backend | 5 个文件 139 条通过 | 退出码 0 | kr 的 command 套件只看退出码，没解析测试数 |
| source-checks | 3 个脚本通过 | 退出码 0 | 同上 |

## 时间与排队（来自事件）

| 运行 | 墙钟 分钟 | 拿槽位等待 s | 旧全机锁等待 s | 构建步骤 | 缓存命中 | 项数 |
|---|---|---|---|---|---|---|
| check1 | 20.1 | 0 | 0 | 6 | 0 | 0 |
| check2 | 0.1 | 0 | 0 | 0 | 6 | 0 |
| smoke | 4.6 | 0 | 0 | 0 | 1 | 1 |
| default | 75.4 | 2370 | 3421 | 0 | 6 | 35 |
| rerun3 | 6.5 | 0 | 0 | 0 | 2 | 3 |

「等待」按每次获取累加，多条通道同时等待时会重复计入；墙钟才是真实耗时。旧基线证据里没有排队时间，排队占比无法按第 6 批对照，留给第 7 批（kr 事件）与第 6 批总结（krill-batch-review.md）比较。

## 排程模拟与校准

用旧基线 31 个场景的实测耗时（合计 73.7 分钟）模拟模拟器阶段（脚本 `sim2.py`，按 run.rs 的通道规则）：

| 设置 | 预计分钟 |
|---|---|
| sim=1 | 74.7 |
| sim=2，先 iPhone 后 iPad（kr 现状） | 41.3 |
| sim=2，先 iPad | 37.6 |
| sim=3 | 33.9 |

校准：用本次 kr 自己记录的每项耗时（33 项）模拟，预测 49.4 分钟；实测有设备在跑的时间 46.0 分钟，误差 +7%。本次通道启动顺序由线程竞争决定（iPad 通道抢到第二个槽位），模型按固定顺序，这是误差来源之一。sim=3 的结果属于外推：并发变化会改变单项耗时（见 offline-evaluation.md 0.1），只能作为方向。

> 2026-10-15（#18）：上面的 Python 模拟已迁进 `kr simulate`（用 kr 自己的调度代码，脚本已删除，见 git 历史）。两份数据换成 kr 事件放在 `tests/fixtures/simulate/`，`crates/kr/tests/simulate.rs` 回测：按记录的通道顺序重放，kr 门禁 46.0 → 46.0 分钟（+0.0%），第 6 批 44.4 → 41.6 分钟（−6.5%），排序与实测一致。module-watchlist 第一次失败的那次按证据算 230 秒（上表按 120 秒估），所以第 6 批「先 iPad」现为 38.0 分钟、「先 iPhone」41.7 分钟。

## 对第一阶段验收的意义

- 拿锁后才发现编译失败：本次 0 次。`kr check` 在占模拟器之前完成全部构建和过滤核对。
- 假通过：0 次。kr 的问题（枚举 bug）表现为误判失败，没有误判通过；旧脚本的 kit-tests 没有测试数，无法证明没有假通过。
- 排队占比：本次受另一个项目持有旧全机锁影响（28.9 分钟），不能说明 kr 的效果；需要第 7 批的数据。
- 计时：全部来自事件，本文的表都由脚本生成。

## 没验证的

- 耗时没有做负载归一化：kr 运行时 1 分钟负载约 85–200，旧基线运行时的负载没有记录。
- backend、source-checks 只核对了退出码，没有核对测试数。
- webdav-real、smb-real 不在 kr 范围内，仍用 e2e.sh。

## 数据来源

- 旧基线：`/Users/cyh/orca/workspaces/Krill/v0-evidence/batch6-gate/`（e2e.log、test.log、各场景 summary.json）。
- kr 事件：`~/.local/state/kr/events/2026-10-14.ndjson`，运行 ID：check1 `01a1395a-0125-72db-92ee-8d50442be0ca`、check2 `01a1396e-7e35-7663-b9a9-a0bb10e5b8a8`、smoke `01a1396e-a8a2-7628-bd0c-15e4a955f915`、default `01a13972-e967-759e-b387-d49fbf89cf37`、rerun3 `01a139b8-ce63-73e3-a9ce-f8446d2936ab`。
- 生成脚本：`docs/research/krill-p0-trial/`（report.py、sim2.py），#18 已删除，见 git 历史；排程模拟改用 `kr simulate`。
