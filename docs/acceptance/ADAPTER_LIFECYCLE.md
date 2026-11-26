# 三源工单首个状态转换

适用范围为单工单只读观测。来源及映射版本见[能力矩阵](ADAPTER_CAPABILITIES.md)。

| 来源 | 源状态转换 | 证据与业务约束 |
|---|---|---|
| Odoo Community 19.0 | `draft → confirmed` | 本地 JSON-2 读回已确认。制造确认涉及库存移动及物料准备。[运行记录及源码](../EXTERNAL_ADAPTER_DESIGN.md#首个状态转换的对照证据) |
| OpenMES | `PENDING → ACCEPTED` | 本地 Laravel 测试内核对同一工单执行接受并读回。该入口校验更新权限和 `PENDING` 前置状态。实际 HTTP 复验确认接受后源更新时间推进，另一产线工单时间保持原值。[双身份实测与复跑命令](../EXTERNAL_ADAPTER_DESIGN.md#openmes-本地接口验证) · [接受实现](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Http/Controllers/Api/V1/WorkOrderController.php#L136) |
| Carbon Community | `Draft / Planned → Ready` | 固定源码显示放行时先重新计算并尝试运行 MRP，随后按当前状态有条件地更新为 `Ready`。放行还有后续业务动作。本轮证据为源码，真实后端转换待复验。[放行实现](https://github.com/crbnos/carbon/blob/05bd067c0a72ca9f0983f6278d2a7c9bb48b716c/apps/erp/app/modules/production/production.server.ts#L316) |

当前映射逐项保留源值，`normalized_status` 为空，`mapping_issue=unmapped`。未来提议同一规范状态时，需要核对各来源的操作前置条件、库存或计划副作用、进入状态后的可执行操作及反例。运行 `cargo test --locked --test external_order preserves_source_specific_release_states` 验证实际 Rust 投影器。测试使用来源响应夹具。

## Odoo 同一制造单确认前后复验

从 MOM-MCP 根目录构建 `cargo build --locked --example external_order_smoke`，再运行 `bash docs/references/mcp/odoo-adapter-lifecycle-smoke.sh`。脚本只操作新建的 Odoo 和 PostgreSQL 测试容器，使用回环端口，并在退出时清理本次容器及临时凭据。默认使用能力矩阵所列固定 Odoo 镜像。若运行环境使用同摘要重打包的本地镜像，通过 `MOM_ODOO_TEST_IMAGE` 指定该镜像。

固定 Odoo Community 19 和 PostgreSQL 16 实测通过。同一公司身份先经 Rust Adapter 读到 `draft`，测试夹具在 Odoo 内执行 `action_confirm` 后，按同一制造单 ID 读回 `confirmed`。另一公司制造单仍为 `draft`，交换身份验证双方跨公司不可见。写动作由测试夹具执行，Adapter 只查询。此结果为来源直连证据。

## Carbon 放行规则与重读复验

在固定 Carbon 提交 `05bd067c0a72ca9f0983f6278d2a7c9bb48b716c` 上，从 `apps/erp` 运行 `node_modules/.bin/vitest run test/release-jobs.test.ts test/job-status-guard.test.ts`。两套共 28 项通过，验证放行顺序和状态守卫，使用测试依赖替身。

在 MOM-MCP 运行 `cargo test --locked --test external_http rereads_carbon_job_status_after_release`。同一个 Rust Reader 绑定相同公司和 Job ID，分别验证 REST 源从 `Draft` 或 `Planned` 变为 `Ready` 后的重读。记录身份、显示号和映射版本保持一致，规范状态为空。源状态转换由回环夹具驱动，真实部署上的完整放行工作流及前后读回仍按下表补验。

### 真实 Carbon 状态守卫与读回

固定 Carbon 提交与隔离社区后端上，`updateJobStatus` 使用真实认证会话和 PostgREST，按公司、Job ID 和前置状态更新一张新建的合成 Job。Rust Adapter 在更新前读到 `Draft`，更新后按同一 ID 读到 `Ready`。重复请求未命中前置状态，Adapter 再次读到 `Ready`，源更新时间与首次成功更新后的值相同。每次读回也核对另一公司已知 Job 不可见。测试替身记录到一次放行事件调用，实际事件投递待另验。

复跑时，先构建 MOM-MCP 的 `external_order_smoke`，把 [真实来源测试](../references/mcp/carbon-status-guard-live.test.ts) 复制到固定 Carbon 检出的 `apps/erp/test`，在 `apps/erp` 执行 `node_modules/.bin/vitest run test/carbon-status-guard-live.test.ts`。环境提供回环网关 `MOM_CARBON_REST_ORIGIN`、公开项目键 `MOM_CARBON_PROJECT_KEY`、具备测试公司 Job 更新权限的用户会话 `MOM_SAMPLE_TOKEN`、由另一身份确认存在的 `MOM_CARBON_OTHER_JOB_ID`，以及 Rust 示例的绝对路径 `MOM_CARBON_ADAPTER_SMOKE`。隔离服务凭据 `MOM_CARBON_TEST_SERVICE_KEY` 用于夹具设置、清理和 Carbon 内部服务调用。Rust 示例只接收用户会话和公开项目键。CLI 类型生成未启动成功，数据库、认证和 PostgREST 已独立运行并通过此测试。

### 真实 Carbon 最小放行流程

同一测试文件还运行上游 `releaseJobs`。测试额外需要指向隔离 PostgreSQL 的 `SUPABASE_DB_URL`。Carbon 默认的 Vitest 配置使用占位服务凭据，测试只在回环环境为该 worker 设置实际测试栈绑定。用例创建独立库位、Job 和内部工序，运行真实 PostgREST、PostgreSQL 和放行函数。检查该 Job 在成功列表中、返回已调度且无警告，放行日期已写入，Rust 按同一 ID 从 `Draft` 读回 `Ready`。再次放行按 `Ready` 跳过，跳过前后 Adapter 读到同一源更新时间。两个用例均在固定社区后端通过。

最小放行用例还在同一 Job 上验证两次拒绝和恢复。没有工序时拒绝；补上内部工序后，把生产数量改为零，再次拒绝。两次均保留 `Draft`，放行日期为空，事件替身调用数为零，并用 Rust Adapter 读回当前状态。恢复正数量后，原 Job 放行到 `Ready`。测试只修改合成 Job 及其独立库位，公司级备料设置维持原值。

严格备料复验使用合成 Job 的独立库位、启用的工作中心和启用的库位容器。测试在隔离公司临时开启 `requireBackflushStorageUnit`，并在结束时恢复原设置。物料缺少 `Pull From` 时，Carbon 拒绝放行。Rust 读回 `Draft`，放行日期为空，事件替身调用数为零。给同一物料指定有效库位后，无警告放行并读回 `Ready`，原有重复放行和跨公司不可见检查均通过。

自动拣料复验在同一隔离公司临时同时开启 `requireBackflushStorageUnit` 和 `autoPickingListOnRelease`。使用外购物料，缺少 `Pull From` 时无拣料行。补齐后产生一条草稿拣料行，匹配原 Job 的物料、工序、数量 1 和来源库位。重复放行后仍是同一行。测试通过后清理测试库位下的拣料单并恢复原公司设置。此结果验证草稿生成。

真实拣料过账复用同一草稿行。测试在独立地点的仓库库位预置 2 个非追踪物料单位，再调用 Carbon `postPicking` 过账 1 个单位。台账按该拣料行核对仓库 `−1`、工位 `+1` 两条腿，过账后两个库位各剩 1。拣料行状态为 `Picked`，生产物料转指工位库位。再次过账被拒，台账和余额保持一致。Rust Adapter 复读原 Job 仍为 `Ready`，跨公司工单保持不可见。该用例用隔离 PostgreSQL 预置库存，移动调用真实 Carbon 逻辑，测试后按公司、地点和物料精确清理合成台账。

MRP 计划结果、外协采购及实际事件投递需要各自有副作用断言的夹具。事件与遥测模块在本测试中使用替身。

## 后续验收缺口

| 缺口 | 补齐条件与证据 |
|---|---|
| 正式 Observe 接真实三源，当前 0/3 | 宿主批准身份、绑定、凭据和出站后，先在 Odoo 复验，再跑 OpenMES 与 Carbon Community。每站记录正式入口 401／200／404 和第二源身份对范围外记录的独立确认。 |
| 正式身份及故障契约 | 在正式服务层记录过期、撤权、跨世界或未绑定引用的拒绝和零来源调用。记录 502／503 的错误分类、脱敏以及其他来源和 Mock 的零回退。AgentNexus #266 已提供 502 夹具证据。 |
| Carbon 原生 MCP，授权实例 0/1 | 获得启用 MCP 权益的测试实例。确认工具调用、实际公司边界，并用受信侧记录辨明工具拒绝原因。社区版 REST 证据继续独立登记。 |
| 生命周期与能力扩展 | 真实 Carbon 状态守卫、最小放行和严格备料库位已读回。非追踪物料的草稿拣料和实际过账已验证。仍需验证追踪物料、外协放行、MRP 结果及事件副作用。状态等价、列表、订阅、写入和跨版本兼容随各自切片定义验收。 |
