# 回放素材

单元测试不调用真实的外部命令，而是回放这里录制的输出（ADR 0008）。

- 按工具分目录：`tests/fixtures/<tool>/<name>.json`，例如 `simctl/list-devices.json`。
- 每个文件是一次调用：`program`、`args`、`status`（被信号终止时为 `null`）、`stdout`、`stderr`。
- 录制只由开发者显式运行，录制时去掉主目录、用户名和凭证：

```sh
cargo run -p kr-exec --example record -- tests/fixtures/simctl/list-devices.json \
    -- xcrun simctl list devices -j
# 命令输出可能含凭证时，加 --redact-file <KEY=VALUE 文件>，文件里的每个值都会被替换成 <redacted>
```

提交前看一遍新文件，确认没有个人信息。
