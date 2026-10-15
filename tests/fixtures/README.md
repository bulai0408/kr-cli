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

## 现有素材

- `sysctl/m3-imac.json`、`scutil/`、`df/`、`xcode-select/`、`simctl/`：2026-10-14 在 owner 的 iMac（Apple M3，8 核 4P，24 GB，Xcode 27.1 + 26.6）上录制。
- `xcode/apps/*.app/Contents/version.plist`：从本机两个 Xcode 复制，供检测已装 Xcode 的测试用。
- 高配机（M4 Max）和 Intel 机的档案没有真机可录，在 `crates/kr-machine/tests/profile.rs` 里按同一格式构造。
- `remote/`：2026-10-15 在 owner 的 iMac 上录制，`kr run --remote` 的回放素材（`crates/kr/tests/remote.rs`）。本机是 Krill `v0/integration`（4ea68bc）的干净克隆，`origin` 指向同一台机器上的裸仓库镜像（`/tmp/kr32/origin.git`）：Krill 是私有仓库，SSH 会话里用不了钥匙串里的 HTTPS 凭证。
  - `git-pushed/`：干净、已推送的分支。其他 git 状态（没推送、有未提交改动、detached HEAD、没有 origin）由测试在这份素材上改出来。
  - `probe-vps.json`、`probe-fnos.json`：真实 `ssh vps` / `ssh fnos`（Linux）。`probe-nowhere.json`：真实 ssh 连不上的主机（本机代理把不存在的域名解析到 198.18.x 假地址）。
  - `probe-localhost.json`、`run-localhost.json`：真实 `ssh localhost`。主机一侧的 kr 克隆、拉取，跑 Krill `source-checks` 和 `kit-build`（真 Xcode，按全机锁排队）。`-tt` 下行尾是 `\r\n`，结果 JSON 后面有 OpenSSH 自己的 `Connection to localhost closed.`。
  - `run-localhost-lost.json`：真实失联：主机一侧 xcodebuild 已经开始时结束了这次会话的 `sshd-session` 进程，ssh 以 255 结束。主机上的 kr 收到 SIGHUP，写了 `run_aborted` 并归还了 `build` 槽位。
