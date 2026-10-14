# 0001 独立的个人 Rust CLI，而不是脚本

状态：已采纳（2026-10-14）

## 背景
流程规则原来散在 shell 脚本（`e2e.sh`、`test.sh`、各 worker 自写的锁）和 AGENTS.md 的文字里。Krill 一个仓库就有 3 份各自实现的全机锁；Conflux 的 `test.sh` 不加锁（见 `docs/research/pitfalls-inventory.md` B1、B2、T4）。

## 决定
做一个独立的 Rust 二进制 `kr`，跨项目共用，配置分机器级和项目级。不用 cargo-xtask：锁和模拟器池是整台机器的，不属于某个仓库（`docs/research/rust-cli-prior-art.md` §CLI 骨架）。

- clap derive 定义子命令；所有命令支持 `--json`。
- figment 分层配置：机器级 `~/.config/kr/config.toml` → 项目级 `.kr.toml` → 环境变量 → 命令行参数。
- tracing 记日志；miette 报配置错误，anyhow 处理其余错误。
- 先用 `cargo install --git` 安装；多机后再考虑 dist / Homebrew。

## 参考
cargo-nextest、sccache、clap、figment、miette 的官方文档（链接见 rust-cli-prior-art.md）。
