# 三源工单首个状态转换

适用范围为单工单只读观测，映射版本为 `0.1`。来源固定版本见[能力矩阵](ADAPTER_CAPABILITIES.md)。

| 来源 | 源状态转换 | 证据与业务约束 |
|---|---|---|
| Odoo Community 19.0 | `draft → confirmed` | 本地 JSON-2 读回已确认。制造确认涉及库存移动及物料准备。[运行记录及源码](../EXTERNAL_ADAPTER_DESIGN.md#首个状态转换的对照证据) |
| OpenMES | `PENDING → ACCEPTED` | 本地 Laravel 测试内核对同一工单执行接受并读回。该入口校验更新权限和 `PENDING` 前置状态。[接受实现](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Http/Controllers/Api/V1/WorkOrderController.php#L136) |
| Carbon Community | `Draft / Planned → Ready` | 固定源码显示放行时先重新计算并尝试运行 MRP，随后按当前状态有条件地更新为 `Ready`。放行还有后续业务动作。本轮证据为源码，真实后端转换待复验。[放行实现](https://github.com/crbnos/carbon/blob/05bd067c0a72ca9f0983f6278d2a7c9bb48b716c/apps/erp/app/modules/production/production.server.ts#L316) |

当前映射逐项保留源值，`normalized_status` 为空，`mapping_issue=unmapped`。未来提议同一规范状态时，需要核对各来源的操作前置条件、库存或计划副作用、进入状态后的可执行操作及反例。运行 `cargo test --locked --test external_order preserves_source_specific_release_states` 验证实际 Rust 投影器。测试使用来源响应夹具。

## Carbon 放行规则与重读复验

在固定 Carbon 提交 `05bd067c0a72ca9f0983f6278d2a7c9bb48b716c` 上，从 `apps/erp` 运行 `node_modules/.bin/vitest run test/release-jobs.test.ts test/job-status-guard.test.ts`。两套共 28 项通过，验证放行顺序和状态守卫，使用测试依赖替身。

在 MOM-MCP 运行 `cargo test --locked --test external_http rereads_carbon_job_status_after_release`。同一个 Rust Reader 绑定相同公司和 Job ID，分别验证 REST 源从 `Draft` 或 `Planned` 变为 `Ready` 后的重读。记录身份、显示号和映射版本保持一致，规范状态为空。源状态转换由回环夹具驱动，真实部署上的完整放行工作流及前后读回仍按下表补验。

### 真实 Carbon 状态守卫与读回

固定 Carbon 提交与隔离社区后端上，`updateJobStatus` 使用真实认证会话和 PostgREST，按公司、Job ID 和前置状态更新一张新建的合成 Job。Rust Adapter 在更新前读到 `Draft`，更新后按同一 ID 读到 `Ready`。重复请求未命中前置状态。每次读回也核对另一公司已知 Job 不可见。测试替身记录到一次放行事件调用，实际事件投递待另验。

复跑时，先构建 MOM-MCP 的 `external_order_smoke`，把 [真实来源测试](../references/mcp/carbon-status-guard-live.test.ts) 复制到固定 Carbon 检出的 `apps/erp/test`，在 `apps/erp` 执行 `node_modules/.bin/vitest run test/carbon-status-guard-live.test.ts`。环境提供回环网关 `MOM_CARBON_REST_ORIGIN`、公开项目键 `MOM_CARBON_PROJECT_KEY`、具备测试公司 Job 更新权限的用户会话 `MOM_SAMPLE_TOKEN`、由另一身份确认存在的 `MOM_CARBON_OTHER_JOB_ID`，以及 Rust 示例的绝对路径 `MOM_CARBON_ADAPTER_SMOKE`。隔离服务凭据 `MOM_CARBON_TEST_SERVICE_KEY` 仅供测试建立和清理合成 Job。CLI 类型生成未启动成功，数据库、认证和 PostgREST 已独立运行并通过此测试。完整 `releaseJobs` 的 MRP、拣料及事件投递仍待复验。

## 后续验收缺口

| 缺口 | 补齐条件与证据 |
|---|---|
| 正式 Observe 接真实三源，当前 0/3 | 宿主批准身份、绑定、凭据和出站后，先在 Odoo 复验，再跑 OpenMES 与 Carbon Community。每站记录正式入口 401／200／404 和第二源身份对范围外记录的独立确认。 |
| 正式身份及故障契约 | 在正式服务层记录过期、撤权、跨世界或未绑定引用的拒绝和零来源调用。记录 502／503 的错误分类、脱敏以及其他来源和 Mock 的零回退。AgentNexus #266 已提供 502 夹具证据。 |
| Carbon 原生 MCP，授权实例 0/1 | 获得启用 MCP 权益的测试实例。确认工具调用、实际公司边界，并用受信侧记录辨明工具拒绝原因。社区版 REST 证据继续独立登记。 |
| 生命周期与能力扩展 | 真实 Carbon 状态守卫已读回，仍需复验完整放行流程及其权限、MRP、拣料和事件副作用。状态等价、列表、订阅、写入和跨版本兼容随各自切片定义验收。 |
