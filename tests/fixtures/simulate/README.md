# `kr simulate` 的测试素材

#9（Krill 第一阶段实测）的两份数据，都是 kr 事件格式（NDJSON），日期 2026-10-14。

## krill-p0-kr-gate.ndjson

kr 在 Krill 上跑的 default 门禁，运行 ID `01a13972-e967-759e-b387-d49fbf89cf37`，取自当时的
`~/.local/state/kr/events/2026-10-14.ndjson`。只保留模拟器阶段 `kr simulate` 读的事件和字段：

| 事件 | 字段 |
|---|---|
| `queue_start`、`slot_acquired`、`slot_released`（只留 `resource = sim`） | `resource`、`task`、`capacity` / `waited_ms` / `held_ms` |
| `sim_leased`、`sim_released` | `device`、`task`、`boot_ms` / `held_ms` |
| `item_start`、`item_end`（只留有 `device` 的项） | `item`、`kind`、`device`、`passed`、`duration_ms` |

实测：第一个模拟器项开始（08:35:54）到最后一个结束（09:21:52）共 46.0 分钟。通道按
default、iPad、default 拿到槽位（当时由线程竞争决定）。

## krill-batch6-e2e.ndjson

旧脚本 `e2e.sh` 跑的第 6 批（`v0-evidence/batch6-gate/e2e-20261014-115441-23458`，与 kr
实测是同一棵代码树），换算成 kr 事件，运行 ID 是编的 `01a13900-0000-7000-8000-00000000b606`：

- 每个场景的 `item_start` / `item_end` 时间取自它的 `summary.json`（`startTime`、`finishTime`），
  设备取自 `devicesAndConfigurations[0].device.deviceName`。
- module-watchlist 第一次失败、没有 `summary.json`：开始时间取整次运行的开始，结束时间取
  `1/status` 文件的修改时间（230 秒），设备记为另一台 iPhone 模拟器 Krill-Verify-2。
- 旧脚本用两台 iPhone 模拟器并行，iPad 场景放在最后：记为 `sim` 容量 2，通道按 default、
  default、iPad 拿到槽位，iPad 通道在第一个 iPad 场景开始时拿到槽位。

实测：44.4 分钟。素材里没有个人路径和凭证。
