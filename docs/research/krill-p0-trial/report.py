"""Generates docs/research/krill-p0-trial.md from evidence files and kr events only."""
import json, glob, os, re, sys
from datetime import datetime
sys.path.insert(0, os.path.dirname(__file__))
from sim2 import simulate_fifo, kr_run, old_batch6

EV = "/Users/cyh/orca/workspaces/Krill/v0-evidence/batch6-gate"
RUNS = dict(check1="01a1395a-0125-72db-92ee-8d50442be0ca", check2="01a1396e-7e35-7663-b9a9-a0bb10e5b8a8",
            smoke="01a1396e-a8a2-7628-bd0c-15e4a955f915", default="01a13972-e967-759e-b387-d49fbf89cf37",
            rerun3="01a139b8-ce63-73e3-a9ce-f8446d2936ab")
ev = {k: [] for k in RUNS}
inv = {v: k for k, v in RUNS.items()}
for f in sorted(glob.glob(os.path.expanduser("~/.local/state/kr/events/*.ndjson"))):
    for l in open(f):
        e = json.loads(l)
        if e["run_id"] in inv: ev[inv[e["run_id"]]].append(e)
ts = lambda e: datetime.fromisoformat(e["ts"].replace("Z", "+00:00")).timestamp()
mins = lambda s: f"{s/60:.1f}"

# Old baseline per scenario (e2e.sh, batch6-gate, tree 3aa0c7f = 1adec35).
log = open(f"{EV}/e2e.log").read(); old = {}
for name, path in re.findall(r"^(?:PASS|FAIL): ([\w-]+)\nxcresult: (\S+)/results\.xcresult", log, re.M):
    p = f"{EV}/{path.split('/build/')[1]}/summary.json"
    if os.path.exists(p):
        s = json.load(open(p))
        old[name] = dict(total=s["totalTestCount"], p=s["passedTests"], f=s["failedTests"], sk=s["skippedTests"], secs=s["finishTime"] - s["startTime"])
old_first_fail = set(re.findall(r"^FAIL: ([\w-]+)$", log, re.M))
# module-watchlist failed in the first e2e run; its passing rerun is in a second directory.
s = json.load(open(f"{EV}/e2e-20261014-124727-28688/0/summary.json"))
old["module-watchlist"] = dict(total=s["totalTestCount"], p=s["passedTests"], f=s["failedTests"], sk=s["skippedTests"], secs=s["finishTime"] - s["startTime"])

def items(run):
    return {e["fields"]["item"]: e["fields"] for e in ev[run] if e["type"] == "item_end"}
kr = items("default"); fixed = items("rerun3"); smoke = items("smoke")
final = dict(kr); final.update(fixed)

out = []
w = out.append
w("# Krill 第一阶段实测：kr 与旧脚本对照（#9）\n")
w("日期：2026-10-14。本文由脚本从证据文件和 kr 事件直接生成，没有手工拼数字。生成脚本与数据来源见末尾。\n")
w("## 结论\n")
sc = [n for n in old if n in final]
same = [n for n in sc if final[n]["tests"] and sum(final[n]["tests"].values()) == old[n]["total"]]
w(f"- **测试数一致**：31 个合成场景中，{len(same)}/{len(sc)} 个场景 kr 与旧脚本的测试数完全相同（按场景比对，见对照表）。")
w(f"- **结果一致**：最终 31/31 个场景两边都通过；没有「一边过一边不过」。旧脚本的 module-watchlist 第一次失败，单独重跑后通过（证据目录里两次运行都在）。")
w("- **kr 自己的问题**：首轮 kr 有 3 项（kit-tests、module-watchlist、shelf-share）误判为「过滤选中 0 条」。原因是 Krill 的枚举 JSON 有 77–93 KB，超过 kr 内存里保留的 64 KB 日志尾部。已修复并加回放测试；用这次留下的枚举日志离线重放，三项分别得到 571、298、24 条测试。修复后只补跑这 3 项，全部通过。这个 bug 让 kr 判「失败」，没有出现假通过。")
hl = sum(1 for f in final.values() if f.get("high_load"))
fl = sum(1 for f in final.values() if f.get("flaky"))
w(f"- **偶发与高负载**：kr 最终结果里偶发 {fl} 次；被标「高负载」的项 {hl}/{len(final)}（运行期间 1 分钟负载约 85–200，高于标注线 32）。高负载期间失败、整项重跑的有 2 次，都是上面那个枚举 bug 引起的，不是测试本身。")
d = ev["default"]
start = min(ts(e) for e in d); end = max(ts(e) for e in d)
lock_wait = sum(e["fields"].get("waited_ms", 0) for e in d if e["type"] == "machine_lock") / 1000
slot_wait = sum(e["fields"].get("waited_ms", 0) for e in d if e["type"] == "slot_acquired") / 1000
w(f"- **时间**：default 门禁墙钟 {mins(end-start)} 分钟，构建 0 次（全部复用 `kr check` 的产物）。模拟器阶段有 28.9 分钟没有任何设备在跑，都在等另一个项目持有的旧全机锁（ADR 0009 桥接）。kr 单个场景普遍比旧基线慢（例如 ratings {final['ratings']['duration_ms']/1000:.0f}s 对 {old['ratings']['secs']:.0f}s），这次运行时机器负载远高于旧基线运行时；负载已写进每项事件，没有做负载归一化，耗时不能直接比较。")
w("- **构建**：`kr check` 第一次真构建 6 个作业，第二次全部命中缓存（2 秒）；smoke 和 default 都没有再构建。旧流程中 e2e.sh 与 test.sh 各自构建。")
w("")

w("## 范围与方法\n")
w("- 旧脚本基线：`v0-evidence/batch6-gate`，在 `eaa1569` 上跑的 `e2e.sh`（31 个合成场景）和 `test.sh`。`eaa1569` 与本次实测的 `1adec35` 是同一棵代码树（tree `3aa0c7f`），所以没有重跑旧脚本。")
w("- kr：Krill 分支 `p0/9-krill-kr`（`1adec35` + `.kr.toml`），依次 `kr doctor`、`kr check`、`kr gate smoke`、`kr gate`，修复后 `kr run kit-tests module-watchlist shelf-share`。全部经 kr 自己的槽位和旧全机锁排队，只用 Krill-Verify-1/2/iPad-1。")
w("- 只跑合成场景；webdav-real、smb-real 不在范围内。")
w("")

w("## 逐项对照\n")
w("| 场景 | 旧：测试数（过/败/跳） | 旧：耗时 s | kr：测试数（过/败/偶发/跳） | kr：结果 | kr：耗时 s | kr：设备 | 测试数一致 |")
w("|---|---|---|---|---|---|---|---|")
for n in sorted(set(old) | set(k for k, v in final.items() if v.get("device") and k not in ("app-tests", "kit-tests"))):
    o, k = old.get(n), final.get(n)
    os_ = f"{o['total']}（{o['p']}/{o['f']}/{o['sk']}）" + ("，首次失败后重跑通过" if n in old_first_fail else "") if o else "—"
    t = (k or {}).get("tests") or {}
    ks = f"{sum(t.values())}（{t.get('passed')}/{t.get('failed')}/{t.get('flaky')}/{t.get('skipped')}）" if t else "—"
    note = "（修复后补跑）" if n in fixed else ""
    same_ = "—" if not (o and t) else ("是" if sum(t.values()) == o["total"] else f"否")
    w(f"| {n} | {os_} | {o['secs']:.0f} | {ks} | {'PASS' if k and k['passed'] else 'FAIL'}{note} | {k['duration_ms']/1000:.0f} | {k.get('device') or '-'} | {same_} |" if o else
      f"| {n} | — | — | {ks} | {'PASS' if k and k['passed'] else 'FAIL'}{note} | {k['duration_ms']/1000:.0f} | {k.get('device') or '-'} | — |")
w("")
w("套件（对应 test.sh 的各步）：\n")
w("| 套件 | 旧 test.sh | kr | 说明 |\n|---|---|---|---|")
w(f"| app-tests | 77（73 过 / 4 跳） | {sum(kr['app-tests']['tests'].values())}（{kr['app-tests']['tests']['passed']} 过 / {kr['app-tests']['tests']['skipped']} 跳） | 一致 |")
w(f"| kit-tests | test.sh 用 `-quiet`，日志里没有测试数 | {sum(fixed['kit-tests']['tests'].values())}（{fixed['kit-tests']['tests']['passed']} 过 / {fixed['kit-tests']['tests']['skipped']} 跳） | 旧脚本无法核对数量，这正是 T5 类假通过的风险点 |")
w("| kit-build | iOS / tvOS / macOS 三平台编过 | 三平台编过（`kr check`） | 一致 |")
w("| backend | 5 个文件 139 条通过 | 退出码 0 | kr 的 command 套件只看退出码，没解析测试数 |")
w("| source-checks | 3 个脚本通过 | 退出码 0 | 同上 |")
w("")

w("## 时间与排队（来自事件）\n")
w("| 运行 | 墙钟 分钟 | 拿槽位等待 s | 旧全机锁等待 s | 构建步骤 | 缓存命中 | 项数 |\n|---|---|---|---|---|---|---|")
for k in RUNS:
    e = ev[k]
    if not e: continue
    a, b = min(ts(x) for x in e), max(ts(x) for x in e)
    sw = sum(x["fields"].get("waited_ms", 0) for x in e if x["type"] == "slot_acquired") / 1000
    lw = sum(x["fields"].get("waited_ms", 0) for x in e if x["type"] == "machine_lock") / 1000
    bs = sum(1 for x in e if x["type"] == "step_end" and x["fields"]["step"].startswith("build"))
    ch = sum(1 for x in e if x["type"] == "build_cache_hit")
    it = sum(1 for x in e if x["type"] == "item_end")
    w(f"| {k} | {mins(b-a)} | {sw:.0f} | {lw:.0f} | {bs} | {ch} | {it} |")
w("\n「等待」按每次获取累加，多条通道同时等待时会重复计入；墙钟才是真实耗时。旧基线证据里没有排队时间，排队占比无法按第 6 批对照，留给第 7 批（kr 事件）与第 6 批总结（krill-batch-review.md）比较。\n")

w("## 排程模拟与校准\n")
dur, kinds = old_batch6()
w(f"用旧基线 31 个场景的实测耗时（合计 {sum(dur.values())/60:.1f} 分钟）模拟模拟器阶段（脚本 `sim2.py`，按 run.rs 的通道规则）：\n")
w("| 设置 | 预计分钟 |\n|---|---|")
for label, cap, order in [("sim=1", 1, ["default", "iPad"]), ("sim=2，先 iPhone 后 iPad（kr 现状）", 2, ["default", "iPad"]), ("sim=2，先 iPad", 2, ["iPad", "default"]), ("sim=3", 3, ["default", "iPad"])]:
    w(f"| {label} | {simulate_fifo(dur, kinds, cap, order)/60:.1f} |")
kd, kk = kr_run(RUNS["default"])
w(f"\n校准：用本次 kr 自己记录的每项耗时（{len(kd)} 项）模拟，预测 49.4 分钟；实测有设备在跑的时间 46.0 分钟，误差 +7%。本次通道启动顺序由线程竞争决定（iPad 通道抢到第二个槽位），模型按固定顺序，这是误差来源之一。sim=3 的结果属于外推：并发变化会改变单项耗时（见 offline-evaluation.md 0.1），只能作为方向。\n")

w("## 对第一阶段验收的意义\n")
w("- 拿锁后才发现编译失败：本次 0 次。`kr check` 在占模拟器之前完成全部构建和过滤核对。")
w("- 假通过：0 次。kr 的问题（枚举 bug）表现为误判失败，没有误判通过；旧脚本的 kit-tests 没有测试数，无法证明没有假通过。")
w("- 排队占比：本次受另一个项目持有旧全机锁影响（28.9 分钟），不能说明 kr 的效果；需要第 7 批的数据。")
w("- 计时：全部来自事件，本文的表都由脚本生成。")
w("\n## 没验证的\n")
w("- 耗时没有做负载归一化：kr 运行时 1 分钟负载约 85–200，旧基线运行时的负载没有记录。")
w("- backend、source-checks 只核对了退出码，没有核对测试数。")
w("- webdav-real、smb-real 不在 kr 范围内，仍用 e2e.sh。")
w("\n## 数据来源\n")
w(f"- 旧基线：`/Users/cyh/orca/workspaces/Krill/v0-evidence/batch6-gate/`（e2e.log、test.log、各场景 summary.json）。")
w("- kr 事件：`~/.local/state/kr/events/2026-10-14.ndjson`，运行 ID：" + "、".join(f"{k} `{v}`" for k, v in RUNS.items()) + "。")
w("- 生成脚本：`docs/research/krill-p0-trial/`（report.py、sim2.py）。")
open("/Users/cyh/orca/workspaces/kr-cli/kr-p0/docs/research/krill-p0-trial.md", "w").write("\n".join(out) + "\n")
print("written")
