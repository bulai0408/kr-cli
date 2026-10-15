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
  - `run-localhost-lost.json`：真实失联：主机一侧 xcodebuild 已经开始时结束了这次会话的 `sshd-session` 进程，ssh 以 255 结束。主机上的 kr 收到 SIGHUP，写了 `run_aborted` 并归还了 `build` 槽位。之后主机上的命令改为在登录 shell 里启动 kr，这份素材只更新了命令行，输出是原样的。
  - `run-work-build-failed.json`：局域网另一台 Mac（`work`）上第一次远程运行的真实输出：SSH 命令的 PATH 里没有 Homebrew 的 xcodegen，`generate` 失败，什么都没跑。这份输出暴露了两个问题：本机把它显示成 `0/0 PASS`，以及主机上的 kr 没有用登录 shell 的 PATH。命令行换成了 `run-localhost.json` 的，`/Users/kami` 换成 `~`，OpenSSH 断开提示里的局域网地址换成 `work`。
- `devicectl/`、`xcrun/sdk-version-iphoneos.json`：2026-10-15 在 owner 的 iMac 上对「蔡煜恒的 iPad Mini」（iPad mini (A17 Pro)，iOS 27.2）录制，`kr doctor --device` / `kr run --device` 的回放素材（`crates/kr/tests/device.rs`）。UDID 换成 `00008130-00AA00AA00AA001C`，CoreDevice ID 换成 `11111111-2222-4333-8444-555555555555`，序列号和 ECID 清空，设备列表只留这台 iPad 和一台池模拟器。
  - `list-devices-as-recorded.json`：USB 连着、已解锁时的 `list devices`，连接状态是 `disconnected`（`list devices` 不主动连接）。`list-devices-connected.json`：紧接着 `device info details` 之后录的，显示 `connected`。
  - `list-devices-missing-prerequisites.json`：在上一份上改出来的：开发者模式关、系统 28.0。
  - `lock-state-unlocked.json` 是真实输出；`lock-state-locked.json` 按同一格式把 `passcodeRequired` 改成 `true`。
  - `list-devices-over-network.json`：2026-10-15 17:58 真机测试中拔掉 USB 线之后录的：iPad 经 Wi-Fi 连着（`connected`、`localNetwork`），隧道地址已删。
  - `lock-state-lost.json`：设备连不上时的失败，按 devicectl 的 JSON 错误格式构造（`error.userInfo.NSLocalizedDescription`、`info.outcome = failed`），等真机拔线实测后换成真实输出。
- `device/runner-not-installed/`：2026-10-15 真实的 `kr run --device "蔡煜恒的 iPad Mini" launch`（KrSample 的临时副本，去掉了 App ID 不支持的 iCloud entitlement，签名团队 K4AXH95DCF）。构建和签名回读通过；UI 测试 runner 装不上设备（`0xe8008012`，自动签名的描述文件里没有这台 iPad），枚举 JSON 的 `errors` 写明原因。改动：`list devices` 换成上面裁剪过的 `devicectl/list-devices-connected.json`；构建命令的 `-destination` 从录制时的 `generic/platform=iOS` 改成现在 kr 用的 `platform=iOS,id=<UDID>`；UDID、CoreDevice ID、证书持有人姓名、临时目录和主目录换掉。签名错误和掉线的测试在这份素材上改 codesign 或 lockState 的输出。每次 lockState 之后插入一份 `devicectl/list-devices-connected.json`（`003a-…` 这样的文件名）：现在的 kr 连上设备后还要读连接方式，录制时的 kr 还不读。
- `device/launch-passed/`：2026-10-15 真实的正常运行：`[signing] register_devices = true` 之后 `kr run --device "蔡煜恒的 iPad Mini" launch`，构建、签名回读、安装都通过，2 个测试通过。同样在每次 lockState 之后插入 `list-devices-connected.json`。
- `device/cable-pulled/`：2026-10-15 真实的 `kr run --device "蔡煜恒的 iPad Mini" slow launch`（`slow` 是只加在临时副本里、在屏幕上停留 15 分钟的 UI 测试），17:58 owner 拔掉 USB 线。当时的 kr 把 `slow` 判为失败并在高负载下重跑，`launch` 也照跑了：素材里保留了这些真实调用。和上一份一样插入 `list devices`：拔线前两次是 `list-devices-connected.json`，之后是 `list-devices-over-network.json`。
- `xcode/products/KrSample_iphoneos27.1-arm64.xctestrun`、`Debug-iphoneos/KrSample.app/Info.plist`：同一次真机构建的产物。
- `device/unplugged-between-items/`：2026-10-15 真实运行，录制时的 kr 已经比较连接方式：`slow` 运行到快结束时（19:36）owner 拔掉 USB 线，`slow` 经 Wi-Fi 跑完并通过；`launch` 开始前 kr 读到 `localNetwork`（开始时是 `wired`），记为未验证、没有开始。录制时构建命中缓存，所以插入了 `launch-passed` 的生成和构建两条录制（`006a-…`、`006b-…`，命令相同）。`list devices` 按 iPad 当时的连接方式换成裁剪过的 `list-devices-connected.json` 或 `list-devices-over-network.json`。
