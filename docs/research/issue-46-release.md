# #46 发版准备与本地演练

2026-10-16。这里只交付准备能力和本地证据，没有公开发布。

## 范围和基线

- 开工时 `git fetch` 和 `git merge-base --is-ancestor origin/main HEAD` 成功，
  基线 `fd5e2e6`。
- 按协调者追加要求，提交前 fetch 并合入 #43 的 `origin/main@398585b`，
  fast-forward，无冲突。README 只新增「发版」，保留 #43 的 pool 内容。
- 演练二进制来自已提交的 `ccdd85c`，版本 `0.1.0`，target
  `aarch64-apple-darwin`；没有创建源码 tag。`--ref HEAD` 仅用于 dry-run。
- 私有仓库内保留脚本、模板、证据；公开内容只允许两个 tap 文件和两个附件。

## 一次真实失败与修复

第一次真正编译用时 56.34 秒，严格隐私检查随后拒绝 `/Users/`。
定位到 `kr-exec::Sanitizer` 本来的脱敏前缀和替换文本包含该字面量；
不是本机绝对路径残留。没有为这个字面量加白名单。

协调者明确授权最小修改 `crates/kr-exec/src/lib.rs` 的两处字符串构造：
用运行时 `Path` 组合普通前缀，再从同一前缀生成 JSON 转义版本。
脱敏行为和所有原断言保留，新增一条普通路径、转义路径、相似用户名共同出现的回归。
修复后从干净提交快照重新编译，用时 47.97 秒，所有隐私检查通过。

脚本还在运行时读取本机 HOME 和 `id -un`，明确拦截它们；姓名、拼音和已知团队 ID
由仓库外权限为 600 的词表提供。不提交或记录这些值。`strings` 与原始字节均检查，
日志只输出命中规则。裸 `[A-Z0-9]{10}` 不作通用拦截，避免随机符号误报；
已知团队 ID 由私有词表拦截，通用规则只检查明确的 team 上下文。

## 机器验证

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --check` | PASS |
| `cargo test -p kr-exec sanitizer_` | 4/4 PASS，7 条非匹配 runner 测试被过滤 |
| `cargo clippy --all-targets -- -D warnings` | PASS |
| `cargo test` | 423/423 PASS，0 failed，0 ignored |
| `python3 -m unittest discover -s scripts -p test_release.py -v` | 16/16 PASS |
| shell、Python、Ruby 语法与 `git diff --check` | PASS |
| 合成带 `/Users/` 的输入，通过真实 `release.sh --verify-only` | 正确 exit 1，仅输出规则名 |
| 真 arm64 release 的 `--verify-only` | PASS；明确列出两项 pending #47 |
| tar 文件内容和元数据 | 只含可执行 `kr`；uid/gid 为 0，用户/组名为空 |

Rust 检查通过 `kr lock build --task kr46-rust-gate` 排队；
发版构建通过 `kr lock build --task kr46-release` 排队，没有绕过共享 legacy 锁。
早期将私有词表环境变量命名为 `KR_` 前缀，导致 kr 配置加载拒绝未知字段；
已改为 `KR46_DENY_FILE`，并由最终真实构建验证。原失败日志单独保留，没有覆盖。

最终 tar 的 SHA256：

```text
287420781bb5c030c14bd966a92340880f6c0d0cf57ca1667616792dd52c5190  kr-0.1.0-aarch64-apple-darwin.tar.gz
```

## Homebrew 本地验收

运行 `python3 scripts/rehearse-release.py target/release-dist/rehearsal-final`，
临时本地 tap 为 `kr46/rehearsal-7889be1f091b`，已删除。

1. 临时 HOME、隔离的 Codex/XDG 目录，PATH 中没有 `rustc`。
2. 用公开 Formula 模板和 `file://` 地址安装已校验的本地 tar；不访问私有源码构建。
3. `kr --version` 精确等于 `kr 0.1.0`；`kr skill` 包含预期说明。
4. `brew test` 实际执行版本和 skill 断言，退出 0。
5. 临时 HOME 中 `kr setup --json` 成功，断言 Codex/Claude 规则和发现入口存在。
6. `kr doctor --json` 退出 0，JSON schema 存在；不据此声称项目 check 或测试通过。
7. `kr setup --remove --json` 后入口消失；brew 卸载、untap、临时 HOME 清理均成功。
8. 真实 agent 配置和 kr 配置共 6 个文件的前后哈希没有变化。Homebrew 没有留下
   developer 模式配置；`brew test` 首次运行自动补齐 Homebrew 自身的 Ruby gems，
   位于其 `vendor/bundle`，没有安装 Rust。

完整本地日志保留在被 git 忽略的 `target/release-evidence/`：
`selftest.log`、`fmt.log`、`rust-gate.log`、`privacy-negative.log`、
`build-first-attempt.log`、`build-remap-first.log`、`build-final.log`、
`verify-final.log`、`brew-rehearsal.log`。这些日志可能含本机路径，不能公开。
最终产物位于 `target/release-dist/rehearsal-final/`；`source-commit.txt` 不公开。

## 未验证和协调者发布清单

- `kr init` 和 `kr skill --reference config` 尚待 #47；
  真实二进制明确打印 pending；自测证明 `--publish` 在 pending 时拒绝进入公开命令，
  dry-run 可以准备产物并打印命令。本票没有运行真实的 `--publish`。
- 没有创建公开 GitHub 仓库、Release、远端 tag，也没有上传或推送 tap。
  没有验收真实公开下载、全新 macOS 用户权限隔离、朋友的 agent 仅凭 README 完成流程，
  或 #47 init decisions、项目 check、真实服务/设备。没有 Intel、Developer ID 或公证验收。
- owner 确认后，由协调者执行
  `gh repo create bulai0408/homebrew-tap --public`。
- 等 #47 合入，完成所需 main/release 全量门禁、审查和新 agent 验收，在私有仓库对
  已验收提交创建 `git tag v0.1.0 <commit>`。版本变化时统一替换下面的版本号。
- 设置仓库外的 `KR46_DENY_FILE`，先执行
  `scripts/release.sh 0.1.0 --dry-run --output target/release-preview/0.1.0`，
  审查产物后再执行
  `scripts/release.sh 0.1.0 --publish --output target/release-publish/0.1.0`。
  不能复用已存在的输出目录。gh 复用既有认证，不读或打印令牌。
- 只发布 Git 文件 `README.md`、`Formula/kr.rb`，以及 Release 附件
  `kr-0.1.0-aarch64-apple-darwin.tar.gz`、`SHA256SUMS`。
  首次发布先推 main 再建 Release，失败时 Formula 可能暂时指向缺失附件；
  协调者必须检查远端状态后恢复，不可盲目重复执行。

参考实现接口：Homebrew 官方 Formula Cookbook（`bin.install` 和 `test do`），
本机 `brew tap-new --help`；Gatekeeper 提示参考 Apple 的打开未识别开发者 App 说明。
Intel 需要再编译一套 target 并用 Intel Mac 验收，当前不承担这个成本；
是否增加 Intel、私有 CI 和公证由 owner 决定。
