# 真机工具：devicectl 与 codesign 回读（#36）

2026-10-15 为 `kr doctor --device`、`kr check --device`、`kr run --device` 核实。来源是本机 Xcode 27.1 自带工具的 `--help` / `man`、Apple 文档的数据接口，以及在 owner 的「蔡煜恒的 iPad Mini」（iPad mini (A17 Pro)，iOS 27.2 beta）上的实测。

## devicectl

来源：`xcrun devicectl help`、`xcrun devicectl help list devices`、`xcrun devicectl help device info`、`xcrun devicectl help device info lockState`（devicectl 651.13.4，Xcode 27.1）。

- 机器读的接口是 JSON：`--json-output <path>`，`-` 表示写到 stdout，此时人读输出改走 stderr。帮助原文：JSON 输出 "is versioned and will remain stable across releases. Standard output is meant for human consumption and is not guaranteed to be stable across releases."。kr 只读 JSON。
- 每台设备的 `hardwareProperties`、`deviceProperties`、`connectionProperties` 已标为废弃（JSON 里的 `_deprecationNotice`，帮助里也写了），改用 `properties` 字典。kr 读的字段：
  - `properties.state.name`：设备名，例如 `蔡煜恒的 iPad Mini`。
  - `properties.hardware.reality`：`physical` 是真机，`simulated` 是模拟器（`list devices` 也列出模拟器；它们的 `visibilityClass` 是 `simulators`）。
  - `properties.hardware.udid`：交给 `xcodebuild -destination id=`；`identifier` 是 CoreDevice ID。
  - `properties.hardware.platform`（`iOS`，iPad 也是）、`deviceType`（`iPad`）、`marketingName`。
  - `properties.software.osVersionNumber.stringValue`：`27.2`。
  - `properties.connection.pairingState`：`paired`。
  - `properties.state.developerModeStatus`：`{"enabled": {"mode": 1}}` 或 `{"disabled": {}}`（Apple Watch 上见到）。
- **`list devices` 的连接状态不能说明设备是否可用。** 实测：iPad 用 USB 连着、已解锁时，`list devices` 给出 `properties.connection.state = "disconnected"`、`transportType = "wired"`；紧接着的 `device info details` 建立了连接，才显示 `connected`。`list devices` 不主动连接设备。
- `device info lockState --device <udid>` 会连接设备，返回 `result.passcodeRequired`（`false` 表示已解锁）和 `unlockedSinceBoot`。kr 用它同时判断「现在能不能连上」和「是否锁屏」：命令成功且 `passcodeRequired = false` 才算可用；运行中每项开始前和失败后也用它判断设备是否掉线。
- **lockState 成功不代表测试还能跑。** 2026-10-15 17:58 实测：真机测试运行中拔掉 iPad 的 USB 线，测试失败（`Encountered a problem with the test runner after launch. … The connection was invalidated.`），下一项的 runner 也起不来（`The test runner exited with code 74 before establishing connection.`），可 lockState 每次都成功：devicectl 经 Wi-Fi 连上了 iPad，`list devices` 显示 `state = connected`、`transportType = localNetwork`（拔线前是 `wired`）。所以 kr 在 lockState 之后再读一次 `list devices`，连接方式和运行开始时（连上之后读的）不同就算掉线。
- **「启用 UI 自动化」读不到。** `list devices`、`device info details` 的完整 JSON 里都没有这一项（实测全文搜索 `automation` 无结果）。kr 不能事先检查它：`kr doctor --device` 写明这一点；真机上的测试失败且结果或日志里有 `kAXError` 时（2026-09-04 真机排障记录：开发者模式已开、UI 自动化没开时出现 kAXError），在失败项下提示去 设置 > 开发者 > 启用 UI 自动化。
- `device info ddiServices` 显示 iOS 27.2 的开发者磁盘映像在 Xcode 27.1 下 `isUsable: true`、`contentIsCompatible: true`。所以设备系统比 Xcode 的 iOS SDK 新一个小版本时，kr 只给警告；主版本更新（例如 iOS 28 对 Xcode 27.1 的 iOS 27.1 SDK）才判为前置条件缺失。SDK 版本来自 `xcrun --sdk iphoneos --show-sdk-version`（用 kr 选定的 Xcode 的 `DEVELOPER_DIR`）。

## 开发者模式

来源：Apple「Enabling Developer Mode on a device」（`https://developer.apple.com/documentation/xcode/enabling-developer-mode-on-a-device`，正文取自 `https://developer.apple.com/tutorials/data/documentation/xcode/enabling-developer-mode-on-a-device.json`）。

- 在设备的 设置 > 隐私与安全性 里打开「开发者模式」，弹窗里点「重新启动」；重启后再确认一次：iOS / iPadOS 上滑、点「启用」并输入密码。
- 只有发起过配对或曾与 Mac 配对过的设备，设置里才出现这一项。

## codesign 回读

来源：`man codesign`（`-d, --display`、`--entitlements`、`--xml`），本机实测输出格式。

- `codesign -d --verbose=2 <App>`：签名信息写在 **stderr**，其中一行 `TeamIdentifier=<团队>`；ad hoc 签名是 `TeamIdentifier=not set`。
- `codesign -d --entitlements - --xml <App>`：签名里的 entitlement 以 XML plist 写到 stdout（`Executable=…` 在 stderr）；没有 entitlement 时什么也不写，这不是错误（man 原文："If the signature has no entitlement data, nothing is written (this is not an error)"）。
- 真机构建的 entitlement 在签名里；模拟器构建在可执行文件的 `__TEXT,__entitlements` 段里（`kr check` 原有的隔离检查用 `otool` 读这一段）。所以 `--device` 时 kr 用 codesign 读，原有隔离检查只核对 bundle id。

## iCloud 容器环境

来源：Apple「com.apple.developer.icloud-container-environment」（`https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.icloud-container-environment`，数据接口同上）：类型 String，可选值只有 `Development`、`Production`，说明是 "The development or production environment to use for the iCloud containers."

- 页面没有写这个键缺省时用哪个环境。kr 不猜：App 带 iCloud entitlement（`com.apple.developer.icloud-*`）却没有这个键时，回读判为不通过，提示在 entitlements 文件里写上 `Development`。Krill 的 `Krill/Krill.entitlements` 本来就显式写了 `Development`。
- 允许的值来自 `.kr.toml` 的 `[isolation.entitlements]`（与模拟器隔离检查同一份配置），没写时只允许 `Development`：CloudKit Production 不在本阶段范围内（spec「不在范围内」）。

## 签名与描述文件

- 真机测试产物用 `xcodebuild build-for-testing -destination 'platform=iOS,id=<UDID>' -allowProvisioningUpdates` 构建：自动签名需要时可以在团队账号里创建或更新描述文件（2026-10-15 owner 同意）。签名团队不由 kr 设定，kr 只回读并与 `[signing] team` 比对。
- 为什么不用 `generic/platform=iOS`：2026-10-15 实测，用它构建能成功，签名回读也通过，但 UI 测试 runner（`<UITests 目标>-Runner.app`）装不上 iPad：`0xe8008012 (This provisioning profile cannot be installed on this device.)`。自动签名只为构建目标里的设备准备描述文件；目标是通用平台时，它不知道要包括哪台设备。`xcodebuild -help`：`-allowProvisioningDeviceRegistration` "Allow xcodebuild to register your destination device on the developer portal if necessary. This flag only takes effect if -allowProvisioningUpdates is also passed."
- 设备不支持的 entitlement 在构建时就被拒：KrSample 为模拟器隔离检查写了 `com.apple.developer.icloud-container-environment = Development`，它的 App ID 没有 iCloud 能力，真机构建失败：`Provisioning profile "iOS Team Provisioning Profile: dev.kr.sample.e2e" doesn't include the com.apple.developer.icloud-container-environment entitlement.`。xcodebuild 自己写明了原因，kr 原样显示。
- 用 `id=<UDID>` 时 xcodebuild 的输出以 `--- xcodebuild: WARNING: Using the first of multiple matching destinations:` 开头，后面几行以 `{ platform:iOS, … }` 开头；枚举还先打印一份旧式属性列表。kr 从第一个能解析成 JSON 的 `{` 读起。
- runner 装不上时，`-enumerate-tests` 以 0 结束，JSON 的 `errors` 写明原因，`values` 里仍列出测试目标（`enabledTests: [{identifier: "KrSampleUITests"}]`）。kr 以 `errors` 为准：这一项不跑、判失败，原因写 xcodebuild 的错误。
- 设备的 `.xctestrun` 名为 `<scheme>_iphoneos<版本>-arm64.xctestrun`，和模拟器的 `<scheme>_iphonesimulator…` 放在同一个 `Build/Products` 目录里，kr 按名字区分。
