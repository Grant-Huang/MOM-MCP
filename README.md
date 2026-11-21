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

## P1 场景工具

从首页进入「场景验收台」，加载 CNC、SMT 或按单装配夹具。每次打开生成独立的 `run_id`；切换后可重新搜索并复查各自的物料状态。P2 的虚拟时间由服务端持有，按评测控制请求和模型规则推进。

AgentNexus 可使用 [P1 示例配置](config/agentnexus.p1.toml) 启用场景工具。可信调用端可用 `register-scenario` 注册定义：

```sh
cargo run --locked -- register-scenario --db scenarios.sqlite3 --file /path/to/my-scenario.json
```

以 [CNC 夹具](fixtures/cnc.json) 为起点，选用新的场景 ID 或版本。同名同版本一旦注册就保留原定义。可选的 `event_rules` 使用分布参数描述相对延迟，P1 只校验和存储规则。

```sh
MOM_AGENTNEXUS=../multi-agent-conv-mvp npm run acceptance -- P1 --gate
```

## P2 仿真时钟与评测控制

普通 Agent 可调用 `read_events`，从 `after=0` 开始，使用返回的 `event_cursor` 读取下一页。读操作不会推进时钟。业务游标 `cursor` 仍用于 P1 业务写入，DES 使用独立的 `event_cursor`。

仅在本地评测时启用控制入口：

```sh
cargo run --locked -- web --db p2-test.sqlite3 --control-tools
```

评测 stdio 进程可使用 `stdio --scenario-tools --control-tools`。控制方通过 `advance_run` 指定事件和时间预算，`intervene_run` 根据当前服务端时刻安排维修，并可通过 `inspect_run` 获取未来候选、诊断、重放记录及重新执行的校验结果。普通 Agent 配置只启动 `--scenario-tools`。

```sh
MOM_AGENTNEXUS=../multi-agent-conv-mvp npm run acceptance -- P2 --gate
```

## P3 汽车零部件 CNC

在场景验收台选择 `cnc@2` 泵体生产夹具。先打开场景，再通过工具搜索进入制造操作。夹具含两张各 10 件的工单及一个原料批次。正常流程使用 WO-1，WO-2 可用于并发争料验证。

| 流程 | 工具 |
|---|---|
| 下达与准备 | `release_order`、`dispatch_operation` 和 `issue_material` |
| 工序执行 | `start_operation` 和 `report_operation`，依次执行 `machining` 与 `assembly` |
| 质量与关单 | 派工至 `inspection`，再调用 `inspect_units`、必要时调用 `disposition`，最后 `close_order` |
| 复查与纠正 | `mes_status`、`trace`、`reverse_report` |

每次写入带 `idempotency_key`，可选 `expected_cursor` 检查当前业务版本。原料在机加工报工事务中实际扣减。终检和处置是分开的模拟动作，每次关单前必须处理质量冻结。使用 `cnc-shortage@1` 验证到货，使用 `cnc-lot@1` 验证来料偏差。

```sh
MOM_AGENTNEXUS=../multi-agent-conv-mvp npm run acceptance -- P3 --gate
```
