# #39 交付证据

## 机器已判定

- 已修复旧锁与槽位反序死锁，顺序为负载检查、旧锁、槽位；逆序释放。祖先旧锁重入仍验证 PID；保持旧 owner 四列协议。观察历史以独立文件锁串行化并原子替换，不记录进程命令行。
- 新增回放回归 `replay_outer_legacy_holder_can_enter_kr_while_another_kr_waits`、`sigterm_while_waiting_for_the_bridge_removes_its_ticket_without_starting_work`；RED/GREEN 原始日志保存在 `evidence/machine-concurrency/`。
- 新增 doctor 回归 `doctor_remembers_non_kr_legacy_owners_after_they_release_the_lock`、`doctor_excludes_kr_owned_locks_and_explains_the_observation_limit`、`doctor_calibrates_the_work_mac_to_two_builds_without_hiding_config_overrides`，覆盖近期已退出 owner、kr 自身排除和真实容量覆盖。
- 两机 KrSample 构建 16/16 PASS；启动断言 84/84 PASS（local 24，work 60）。逐个读取 84 个 xcresult 的 summary，每个都是 1 passed / 0 failed / 0 skipped。脱敏批次、负载、身份和 doctor 前后结果均已入库；完整 xcresult 保留在本机 `/tmp/kr39-calibration`，没有提交二进制结果包。
- 集成同步前 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test` 全部通过，测试合计 415 PASS / 0 FAIL；另已构建 release 并在两机真实配置下回读 doctor。最终同步后的检查证据随交付补入。
- `kr lock` 的脚本替换命令和退出码已写入 README 与 `kr skill --reference locks`；维护版校准驱动通过 Python 语法检查。未改其他仓库、机器真实配置或全局 AGENTS.md；work 临时目录 `~/kr39` 已删除。

## ADR 0010 前后对比

第 4 层两机实测：work build 从 1 到 2 时，两轮固定两任务总耗时均值从 15.44 秒降到 9.03 秒（约 -41.3%）；新公式把 work 硬件建议从 1/5/2 改为 2/5/2。本机建议仍为 1/2/1；work 的真实 sim=2 覆盖保留，实际显示 2/2/2。sim 超出原硬件建议一步后，本机 3 比 2 慢约 12.4%，work 6 比 5 慢约 24.9%，因此保留 sim 原公式。

第 2 层使用已有 33 项 Krill trace 和 kr 自己的 simulate：原容量回放 MAPE 0.0254%；容量 3/5/6 的 what-if p50 比容量 2 低约 12.5%，全部标为 outside_calibration。这不等于新容量预测误差。实测只有两轮，同设置差异最高 40.6%；观察到 0 假失败，不能证明未来无偶发。详细方法、负载及区间见 ADR 0022 和 `machine-concurrency.md`。

## 需要人判断

视觉和触感判断：0 项。本票没有 UI 改动。协调者需确认迁移清单，按安装两机新 kr、切脚本、切 AGENTS.md、删 `[legacy]` 的顺序执行；精确替换文本及 Krill 脚本提取步骤见 `machine-concurrency.md`。

## 没有验证的部分

- 未跑完整 Krill 门禁、真实服务或物理设备；没有大型 App/FFmpeg 双构建及其他硬件的性能保证。
- 本次所有内存压力采样均 normal，没有真实触发 warn/critical 的停机路径。维护驱动新增的失败保护未重新运行完整两机矩阵；实际执行版本单独保存为 `driver-measured.py`。
- 两机 doctor 只列出观察到的非 kr owner；没有进行完整进程审计，也未执行跨仓库迁移。实际配置覆盖需要 owner 逐项审核。
- Krill 真实服务脚本的新提取、取消清理及凭证验收由迁移票执行，本票仅提供确定替换步骤；旧 `--changed` 的完整过滤范围明确保留。
