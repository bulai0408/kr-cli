# kr report 的回放素材

`krill-batch7-8/events/2026-10-14.ndjson`：本机真实事件（`~/.local/state/kr/events/2026-10-14.ndjson`）里 Krill 项目在 2026-10-14 09:40–17:05 UTC 的 1446 行，原样保留，只做了筛选。覆盖 Krill 第 7 批（#153）、第 8 批（#125、#133）和第 9 批（#129）开头。

检查过：这段事件里没有个人路径（`/Users/<名字>`）、名字和真机 UDID；出现的 UDID 都是 `Krill-Verify-<N>` 池里的模拟器。

`crates/kr/tests/report.rs` 用它回放 `kr report` 的人读和 `--json` 输出；第 8 批的数字另用一个独立脚本按 ADR 0016 的定义算过，两边一致（见提交说明）。
