# 场景准备、就绪检查和清理调研

日期：2026-10-15。问题：测试依赖的本地服务没有启动时，如何避免把环境原因记为测试失败。

## 官方资料

| 项目 | 观察 | kr 采用的部分 |
|---|---|---|
| fastlane | `before_each` / `after_each` 包围 lane；发生错误不会调用 `after_each` / `after_all`，应使用 `error`。 | 生命周期必须覆盖成功、错误和信号，不能只在正常返回后清理。 |
| Bazel | 测试有独立的 `TEST_TMPDIR`；退出码才是测试结论的依据。测试进程模型不提供通用服务编排。 | 每次运行隔离日志和临时状态；准备和测试分开计结果。 |
| Tuist | Scheme 的 `TestAction` 支持 `preActions` / `postActions`，执行 shell script；官方用 post-action 收集测试信息。 | 显式配置工作目录和脚本；服务就绪和进程回收由 kr 保证。 |
| GitHub Actions services | service container 属于 job 生命周期；可用端口映射连接服务，并在 job 结束后移除。 | 生命周期属于运行项；不依赖使用者记住手动关服务。 |
| Docker Compose | 启动依赖不代表服务 ready；`service_healthy` 等待 healthcheck，`service_completed_successfully` 等待一次性任务成功；停止顺序反向。 | 区分一次性准备和后台服务；有截止时间的重复就绪探测；逆序清理。 |

## 来源与检索

- fastlane 官方： https://docs.fastlane.tools/advanced/lanes/ （直接读取，重点核对错误钩子）。
- Bazel 官方： https://bazel.build/reference/test-encyclopedia （直接读取，环境和测试进程模型）。
- Tuist 官方源码文档： https://github.com/tuist/tuist/blob/main/server/priv/docs/en/guides/features/test-insights/xcode.md （Context7 `/tuist/tuist` 检索 TestAction preActions/postActions；官方站点直接访问返回 403）。
- GitHub 官方： https://docs.github.com/en/actions/concepts/use-cases/about-service-containers （job 生命周期、网络与端口；实现时不引入容器依赖）。
- Docker 官方： https://docs.docker.com/compose/how-tos/startup-order/ （直接读取，service_healthy 与 healthcheck）。

## 取舍

这些工具的生命周期做法可以复用；它们没有 kr 的「未验证」契约，因此不能直接照搬失败分类。采用声明式准备数组、显式后台模式、命令型就绪检查、逆序有界清理。就绪命令可用 curl 检查 HTTP，也可用项目脚本检查端口和实例标识，避免把旧服务当成新服务。

端口由项目服务绑定：使用临时端口或每个并行项独立的端口文件，不采用先探测空闲再绑定（存在竞争窗口）。kr 不杀占用端口的其他进程，也不自动运行 Docker。
