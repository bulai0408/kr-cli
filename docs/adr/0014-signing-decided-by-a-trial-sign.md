# 0014 能不能签名按试签结果判断，不按是不是 SSH 会话

状态：提议（2026-10-15，待协调者确认）

## 背景
#32（P1-7 远程机器）加了一条规则：在 SSH 会话里，需要开发者证书签名的工作（真机构建、需要开发证书的 macOS 构建）一律在开始前拒绝；`kr run --remote` 也一律拒绝这类工作。前提是「SSH 会话里登录钥匙串锁着，签名身份不可用」。

这个前提不是对所有主机都成立：
- 本机 iMac 上成立：2026-10-11 经 SSH 签名失败，codesign 报 `errSecInternalComponent`。
- 局域网 Mac `work` 上不成立：它已经配置为 SSH 登录时解锁钥匙串。2026-10-15 经 `ssh work` 用两个 Apple Development 身份给临时文件签名，都成功，团队都是 K4AXH95DCF。

所以「看到 SSH 就拒绝」在 `work` 上拒绝了能做的工作，也让 #38 要的「真机插在远程主机上」做不了。按 ADR 0012，主机怎样配置钥匙串是用户和主机的事；kr 的工具层规则只应是「签不了就不开始，并写明怎么修」。

## 决定
1. **去掉「SSH 会话一律拒绝签名」**。需要签名的运行在开始前检查里，在实际要签名的那台机器上试签一次：签得了就照常运行，签不了就拒绝。
2. **什么时候试签**：
   - `kr run --remote` / `kr gate --remote`：选中的项目里有需要签名的，就在主机上试签（经 ssh，和主机检查一起，在拉取代码前）。主机上的 kr 自己也在 SSH 会话里，开始前会再试签一次。
   - 本机：只在 SSH 会话里（`SSH_CONNECTION`、`SSH_CLIENT`、`SSH_TTY` 之一非空）运行 `kr run` / `kr gate` / `kr check` 时试签。在本机登录会话里签名一直可用，不多花这一步；真签不了时 xcodebuild 自己会报错。
   - 哪些项目需要签名，规则不变：给真机或 macOS 构建 App、项目没有关签名（`CODE_SIGNING_ALLOWED = NO`）。模拟器构建和单独构建的包不需要。
3. **怎样试签**：一段 `sh` 脚本，本机和主机用同一段：
   - `mktemp` 建一个临时文件，`trap` 保证结束时删除；
   - `security find-identity -v -p codesigning` 列出有效的签名身份，只取 `Apple Development:` 开头的（Xcode 自动签名用的就是它）；
   - 对每个身份 `codesign --force --sign <SHA-1> <临时文件>`，成功的再用 `codesign -dv` 读回团队 ID；
   - 每个身份输出一行 `signed team=<ID>` 或 `failed <codesign 最后一行>`，最后输出身份个数。
   kr 只读这些输出。它不解锁、不修改钥匙串，不读、不存密码或私钥；`find-identity` 只给出证书名字和指纹。
4. **怎样判断**：
   - 有一个身份签成功，且团队等于 `[signing].team`（没写团队时任意团队都行）：通过。
   - 没有 Apple Development 身份：拒绝，`signing_no_identity`。
   - 有身份但都签不了：拒绝，`signing_unavailable`，原因写 codesign 的报错；报错是 `errSecInternalComponent` 时写明「这台机器在这个会话里钥匙串锁着」。
   - 签得了但团队都不是 `[signing].team`：拒绝，`signing_wrong_team`。
5. **错误信息**：和 #32 的其他开始前问题一样，每条写明在哪台机器、原因、下一步，`--json` 的 `error.problems[]` 带 `code`、`message`、`next`。例：
   - `signing_unavailable`：「work cannot sign with a developer certificate in an SSH session: errSecInternalComponent (the login keychain is locked in this session)」；下一步「on work: make the login keychain usable in SSH sessions (unlocked at SSH login), or run this in a login session on that Mac; kr does not unlock keychains」。
   - `signing_no_identity`：下一步「on work: add an Apple Development certificate of team K4AXH95DCF (Xcode > Settings > Accounts > Manage Certificates)」。
6. **远程真机**：`kr run --device <名字> --remote <主机>` 可以组合。主机上的 kr 做设备检查、签名回读、掉线记「未验证」（#36），结果按 #32 的方式传回：本机为每一项写 `item_end`，带主机给的 `device`、`tests.unverified`、`unverified_reason`。主机上设备的开始前问题传回本机时，逐条列出。真机仍是容量为 1 的资源，由主机上的 kr 管。不做远程多台真机并行。

## 理由
- 试签检查的是要做的事本身，判断准确；用来判断的条件（是不是 SSH）只是间接的线索。
- 试签不到一秒，不改变机器的状态，失败时的报错比 xcodebuild 构建到一半才失败早得多、清楚得多。
- 只在 SSH 会话里试签本机：本机登录会话里的签名问题由 xcodebuild 报告，这一类问题不是 SSH 带来的，不在本条规则范围内。

## 影响
- 替代 #32 的「SSH 会话一律拒绝签名」和「远程运行一律拒绝需要签名的工作」。错误码 `kr::signing::ssh_session` 不再使用，改为 `kr::signing::unavailable`（本机）；远程仍在 `kr::remote::preflight` 里，problem 的 `code` 由 `needs_signing` 改为上面三种。
- spec 第二、三阶段第 6 条「签名检查」和用户故事 27 改为按试签结果判断。
- 回放素材：能签名的输出是 `work` 上的真实录制；签不了的输出按 codesign 的报错格式构造（这台 iMac 的 `ssh localhost` 对当前用户没有授权，录不到），素材说明里写明。

## 参考
ADR 0012；#32、#36、#38；`man codesign`、`man security`（本机 macOS 27）。
