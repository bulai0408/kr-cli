# 发版

源码仓库已公开。发布产物仍需隐私检查，不能包含本机路径、凭证或私有词表内容。

发版脚本需要 macOS Apple Silicon、Python 3.12+、Rust stable、`aarch64-apple-darwin`
target、已安装的 kr 和（仅发布时）gh。只支持稳定版 `MAJOR.MINOR.PATCH`。
不注入假版本：先把 workspace 的 Cargo 版本改成待发布版本并提交，协调者再创建
对应的 `v<version>` tag。脚本从该 tag 的已跟踪源码快照构建，要求 Cargo 版本和
`kr --version` 都等于版本参数。不会把未提交改动放进二进制。

1. 在仓库之外建立权限为 `600` 的 UTF-8 私有词表，每行一个要拦截的姓名、
   拼音变体、账户名或已知团队 ID。设置 `KR46_DENY_FILE` 指向它；不要提交词表，
   不要把词放到命令参数或日志里。脚本要求词表存在且非空，但不能自动证明名单完整。
2. 本地准备：`scripts/release.sh 0.1.0`。它要求本地 `v0.1.0` tag 已存在，
   经 `kr lock build --task kr46-release -- cargo build ...` 排队。输出目录默认为
   `target/release-dist/0.1.0`，必须不存在；失败产物保留供检查，再次执行使用新的
   `--output` 目录。
3. 无 tag 演练：`scripts/release.sh 0.1.0 --ref HEAD --dry-run`。
   它仍构建和检查本地产物，只打印公开操作，不创建仓库、推送或上传。
   非 tag ref 只允许在 `--dry-run` 下使用；`--publish` 与 `--dry-run` 不能并用。
4. 运行 `python3 -m unittest discover -s scripts -p test_release.py -v`。
   再运行 `python3 scripts/rehearse-release.py target/release-dist/0.1.0`：
   临时 HOME、无 Rust 的 PATH、临时本地 tap、`file://` 二进制安装、
   `kr --version`、`kr skill`、`brew test`、临时全局 `kr setup`、
   `kr doctor --json`、移除入口、卸载和删除 tap。已有 brew 版 kr 时停止，
   不替换用户的安装。演练会临时写 Homebrew 的 tap 和 Cellar，不更改机器配置；
   不使用真实的 agent 配置目录。它不能证明无仓库访问权限的新用户或新 agent 已验收。
5. 已有二进制可独立检查：
   `scripts/release.sh 0.1.0 --verify-only target/release-dist/0.1.0/kr`。
   README 里的每个 `kr <子命令>` 必须存在于该二进制 `--help`。
   缺 `kr init` 或 `kr skill --reference config` 明确打印 `pending #47`；
   本地准备和 dry-run 允许 pending，`--publish` 拒绝。其他缺失命令一律失败。

隐私检查先做 remap，再检查真实产物。编译时清除继承的 Rust flags，用
`--remap-path-prefix` 映射源码、HOME、Cargo home 和 Rustup home，并去掉符号。
`strings -a` 检查 `/Users/`、`/home/`、Cargo/Rustup 本机路径、邮箱和带上下文的
团队 ID；运行时读取构建机器的 HOME 和 `id -un` 用户名，明确拦截这些值；
私有词表再做大小写不敏感的 strings 检查与原始 UTF-8 字节检查。
团队 ID 不能直接用裸 `[A-Z0-9]{10}` 拦截：随机字节和符号常命中，误报太多；
已知 ID 放入词表，通用规则只拦 `TeamIdentifier=` / `DEVELOPMENT_TEAM=` 上下文。
命中时只输出规则名，不输出私有值。tar 只含 `kr`，清除 uid/gid 和用户/组名。
现有日志脱敏器通过运行时组合路径生成替换前缀，保持原有脱敏行为，避免把
用于替换的 `/Users/` 字面量编进二进制；这不是隐私检查的豁免。
这些检查不能代替人工审查；未知姓名拼写、拆开的字节或其他私密格式可能漏检。

默认只构建 Apple Silicon。不签 Developer ID，也不公证。Homebrew formula 安装通常
不带浏览器下载的隔离属性；浏览器下载可能被 Gatekeeper 阻止，tap 的 README 提示先核对
校验和，再由用户决定是否在系统设置中允许打开，不能自动关闭 Gatekeeper。
Intel 需额外安装 target、完整编译一套依赖并在 Intel Mac 上验收，构建时间和产物约为
再加一份，当前未实测。GitHub Actions 自动发布需要单独配置构建环境与跨仓库最小权限上传凭证；当前流程不依赖它。

## 发布清单

- 确认待发布版本的功能、门禁和隐私检查通过，再创建对应的 `v<version>` tag。
- `bulai0408/kr-cli` 已公开；二进制和 Homebrew formula 仍发布到
  `bulai0408/homebrew-tap`，不需要重新创建仓库。
- 先执行 `scripts/release.sh <version> --dry-run --output <新目录>`，
  检查生成的 README、formula、tar 内容与校验和。
- 再显式执行 `scripts/release.sh <version> --publish --output <另一个新目录>`。
  脚本使用既有 gh 认证，先更新 tap，再创建 Release；中途失败时先检查远端状态，
  不要盲目重复发布。
- tap 的 Git 文件只更新 `README.md` 和 `Formula/kr.rb`；Release 只上传
  `kr-<version>-aarch64-apple-darwin.tar.gz` 和 `SHA256SUMS`。
  私有词表、运行日志和本地产物不要提交或上传。
