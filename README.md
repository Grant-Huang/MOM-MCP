# MOM-MCP

面向 AgentNexus 的离散制造模拟 MCP 项目。当前提供 P0 stdio 接入探针和 [项目介绍页](index.html)。行业场景和 DES 通用接口按 [实施计划](docs/IMPLEMENTATION_PLAN.md) 交付。

## 启动接入探针

需要 Python 3.12 或更新版本及 uv。进入仓库根目录执行：

```sh
uv sync --locked
.venv/bin/python mom_mcp.py --db probe.sqlite3
```

stdio 服务由 MCP 客户端启动和管理。将 [AgentNexus 示例配置](config/agentnexus.p0.toml) 合入测试实例配置，并替换其中的绝对路径。发现工具后，先调用 `mom_probe.get_probe`，再用 `{"value":42,"idempotency_key":"p0-write-1"}` 调用 `mom_probe.set_probe`。重新查询确认值与修订号。重放原请求可返回原结果，换参数复用键会返回 `IDEMPOTENCY_CONFLICT`。

预期业务拒绝放在结构化返回的 `ok` 和 `code` 字段。AgentNexus 会将 MCP 协议工具错误折叠为宿主错误。探针状态写入指定 SQLite 文件，测试时应使用单独的数据库路径。超时后先重建连接，使用同一数据库查询或用原键与参数重试。

## 本地验证

```sh
uv run --locked python -m unittest discover -s tests -v
```

测试通过真实 MCP stdio 子进程覆盖发现、写入、结构化拒绝、幂等，以及提交后响应超时并重启的恢复。阶段签收见 [P0 验收](docs/acceptance/P0.md)。
