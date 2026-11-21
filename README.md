# MOM-MCP

面向 AgentNexus 的离散制造模拟 MCP 项目。当前提供 Rust 接入探针和浏览器验收台。行业场景按 [实施计划](docs/IMPLEMENTATION_PLAN.md) 交付。

## 启动验收台

使用仓库固定的 Rust 工具链，在仓库根目录运行：

```sh
cargo run --locked -- web --db probe.sqlite3
```

打开终端输出的本地地址，从介绍页进入接入验收台。点击「加载 MCP 工具」，先调用 `get_probe`，再选择 `set_probe` 填写测试值和幂等键，最后重新查询状态。首次验收可使用新的数据库文件路径，独立于已有的验收记录。

## 连接 AgentNexus

```sh
cargo build --locked
```

将 [AgentNexus 示例配置](config/agentnexus.p0.toml) 合入测试实例并替换绝对路径。AgentNexus 会启动 stdio 服务。用 `mom_probe.get_probe` 查看值，再用 `{"value":42,"idempotency_key":"p0-write-1"}` 调用 `mom_probe.set_probe`。重复提交原请求可取得原结果，改用其他参数复用同一键会返回 `IDEMPOTENCY_CONFLICT`。

可预期的业务拒绝位于结构化回包的 `ok` 和 `code` 字段。超时后重建连接，使用同一数据库查询或按原键和参数重试。

## 验证与签收

```sh
npm ci
npx playwright install chromium
npm run acceptance -- P0
MOM_AGENTNEXUS=../multi-agent-conv-mvp npm run acceptance -- P0 --gate
```

本地命令通过 [feature map](docs/feature-map.toml) 选择 Rust stdio 和浏览器测试。严格门槛还会对固定的 AgentNexus 版本运行实际连接、Agent 工具使用和 CNC 复跑验证。验证使用该仓库的临时源码副本。可以设置 `MOM_CHROMIUM` 使用已安装的 Chromium。浏览器报告保存在 `test-results/browser.json`。

参见 [P0 验收](docs/acceptance/P0.md)、[架构设计](docs/DESIGN.md) 与 [测试及 Mock 替换计划](docs/TEST_AND_MOCK_PLAN.md)。
