# P6 三行业基线

此基线包含汽车零部件 CNC、电子装联 SMT 和工业设备按单装配。业务操作走 MCP stdio，场景验收台通过 Axum 的本地 HTTP 桥调用同一 Rust 程序。

## 从干净检出安装

```sh
cargo build --release --locked
./target/release/mom-mcp web --db ./acceptance.sqlite3
```

终端打印监听地址，默认绑定 `127.0.0.1:4173`。用页面查看场景和模拟状态。需要推进仿真或回放诊断时，在隔离测试数据库上追加 `--control-tools`。常规验收台仅保留查询和行业操作。

AgentNexus 使用 [stdio 配置示例](../config/agentnexus.p1.toml)。将命令改成发布二进制的绝对路径并指定数据库路径。若希望与本地验收台观察同一个 run，两端须使用同一份数据库和 `run_id`。Agent 配置保留 `--scenario-tools`，评测进程单独启用控制工具。

## 三行业入口

| 行业 | 正常入口 | 异常验收 | 关键证据 |
|---|---|---|---|
| CNC | `cnc@2`，下达 → 派工 → 报工 → 终检 → 关单 | `cnc-shortage@1` 到货，`cnc-lot@1` 来料异常，`cnc@2` 维修与复检 | 工艺快照、消耗、序列号、质量阻塞与冲销 |
| SMT | `smt@2`，工单 → 上料 → 贴装 → AOI → 分板 → 放行 | 同场景按种子生成供应商召回和 AOI 扰动 | 料卷到跨工单板卡、返工换料、复检和报废 |
| 按单装配 | `assembly@2`，齐套 → 派工 → 实装 → 终测 → 交付 | `assembly-shortage@1` 替代件、测试台竞争、失败返修 | 实装 BOM、资格依据、终测区间及交付序列号 |

具体工具调用见 [README](../README.md) 和各阶段验收记录。

## 运行与恢复

- 数据库在进程间持久化。进程异常退出后以同一数据库重新连接，先调用 `get_run` 确认运行状态。可用同一幂等键及参数重放支持幂等的写入。P6 恢复测试覆盖测试台已占用且完成事件仍在队列的情况。
- `cursor` 是业务游标，`event_cursor` 是 DES 游标。读取事件时用上页的 `event_cursor` 继续翻页。相同页可按事件游标去重。
- 控制器一次最多推进 100 个事件、10,080 个模拟分钟。每个 run 保留最多 10,000 项事件及 10,000 条控制命令，同一时刻最多处理 16 项事件。达到预算后，返回停止原因。
- 使用结构化错误码区分权限隔离、无效预算、缺料、质量阻塞或终测未完成。检查浏览器响应、AgentNexus 工具轨迹和数据库对应的 run。
- 恢复旧行为时固定已验证的二进制、场景版本和模型版本。跨版本数据库回滚须基于相应的数据库备份。重放输入应保留种子和原控制顺序。

## 签收

```sh
npm ci
npx playwright install chromium
MOM_AGENTNEXUS=../multi-agent-conv-mvp npm run acceptance -- P6 --gate
```

严格门槛同时执行所有早期阶段。普通 CI 运行项目内 Rust 与浏览器用例。带固定 AgentNexus 源码的门槛另产出相邻项目的连接与 DES 证据。查看 [P6 验收记录](acceptance/P6.md)。
