# 外部系统只读适配设计

状态为待评审。首个切片验证 AgentNexus 对两种异构生产来源执行同一工单读取意图。需求见 [双主轴需求](FEATURES.md#双主轴需求与验收目标)，执行状态见 [外部适配器实施计划](IMPLEMENTATION_PLAN.md#外部适配器实施计划)。

## 职责与调用边界

AgentNexus 的 MOM／World Adapter 负责服务端绑定、租户与站点范围、调用身份、能力发现及用户侧错误。MOM-MCP 外部适配层负责调用站点配置选定的数据源，并把响应转换为版本化的生产观测。外部系统保有业务数据权威性。首期对接 World Adapter 的 Observe 路径，具体传输与认证方式在设计联审时冻结。已有模拟 run 和公开 MCP 工具按现有契约运行。 [World Adapter 设计](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/43014db/docs/mom/WORLD_ADAPTER.md) · [上游 M3 计划](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/43014db/docs/mom/WORLD_ADAPTER_PLAN.md)

| 绑定状态 | 处理 |
|---|---|
| 未指定外部数据源 | 由宿主按既有权限进入默认 Mock MES 测试路径。 |
| 指定且有效 | 按绑定选择已登记连接和映射版本，按租户、站点、对象、读取能力进行授权。 |
| 指定但不可用 | 返回结构化不可用错误。保留所选绑定，禁止切换到其他数据源或模拟模式。 |

连接地址与凭据从服务端站点配置解析。请求只引用授权范围内的数据源及对象。能力发现与调用均使用同一绑定策略，调用时重新校验。用户响应保留可公开的来源和错误信息，凭据与原始诊断写入受限的观测路径，记录需脱敏。

## 首期工单语义契约

待评审输入为已认证绑定与 `work_order` 对象引用。一次查询只访问一个权威数据源和一个源对象。输出与 World Adapter Observe 对齐，包含以下语义。表中名称用于联审，协议字段以冻结记录为准。

| 语义 | 约束 |
|---|---|
| 对象引用与来源 | 工单身份由站点绑定、源系统实例、对象类型及源记录 ID 共同确定。工单号用于展示和搜索，同号对象不自动合并。 |
| 业务状态 | 仅映射已证实等价的源状态。未知、缺失或歧义返回相应映射诊断，规范状态为空，保留可公开的源值和映射版本。 |
| 观测时点与版本 | 分别记录读取时点和源更新时间（若有）。源对象版本有可靠依据时才返回；映射版本独立于对象版本。 |
| 关联与数量 | 源中已有经确认的关联可保留为类型化引用。客户订单、销售订单和生产工单各有独立生命周期。数量与单位只有在语义和单位可证实时才能归一；首期工单查询可省略这些可选字段。 |
| 失败与恢复 | 授权失败、对象不存在、源不可用、源响应无效和映射不完整分别表达。暂时故障只允许有界重试读取，返回可诊断结果。严禁用空结果表达连接故障。 |

状态映射按源类型、接口版本和映射版本登记。升级前用同一组样本回归并记录兼容性，变更既有含义须升级契约版本。首期范围为单对象读取。列表、分页、事件游标、多源对账和生产写入由后续切片各自冻结。

## 样本和退出断言

| 候选样本 | 已核实 | 待补证据 |
|---|---|---|
| Carbon MCP 生产工单读取 | 固定源码中的 `production_getJob` 接受字符串 ID，声明生产查看权限。 | 固定部署与授权、脱敏响应、实际状态生命周期。 [源码](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/production/production.server.ts#L6957) |
| OpenMES HTTP 工单读取 | 固定源码注册 `work-orders` API resource。详情控制器先执行 `view` 授权，再以 `data` 包装工单和关联。 | 确认部署路由前缀、实际凭据、详情 schema、业务状态和错误响应。 [详情控制器](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Http/Controllers/Api/V1/WorkOrderController.php#L59) · [源码](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/routes/api.php#L653) |
| Odoo Community 19.0 自托管 JSON-2 | 固定源码定义 `mrp.production` 和使用 Bearer 身份的 JSON-2 路由。候选读取为 `POST /json/2/mrp.production/search_read`，服务账号按授权公司和记录 ID 读取。 | 验证自托管实例的端点、记录规则、脱敏响应与失败行为。按数据库绑定区分源记录，核对跨公司隔离。保留源状态 `to_close` 与 `done` 的区别，待核实后决定规范状态。 [制造订单源码](https://github.com/odoo/odoo/blob/d36b9405fe5d2ede753c6d7eceb778ecef20f67b/addons/mrp/models/mrp_production.py#L177) · [JSON-2 路由源码](https://github.com/odoo/odoo/blob/d36b9405fe5d2ede753c6d7eceb778ecef20f67b/addons/rpc/controllers/json2.py#L49) |

Odoo 作为第三个候选，首期仍选择两种可运行接口。Odoo 制造订单与其工序工单是不同对象，首期对齐制造订单。后续可用同一 Odoo 实例验证销售与制造关联，跨厂商关联另用独立来源验证。

两套样本均须有版本、来源和契约证据。验收用同一工单读取意图覆盖等价状态、同号不同源、未映射状态、越权、对象不存在和源不可用。已配置源失败时验证零模拟调用。批准的接口夹具可用于结构与回归验证，已部署系统的调用轨迹作为运行时接入证据。CRM／ERP 销售订单到生产工单的跨系统关系在首期结果基础上另行设计。

### Odoo 本地接口验证

使用 Odoo Community 19.0 镜像 `odoo@sha256:03a47921f9eef2c225ded1167fe7828465d2b3177883f30d173bb1c3c792949b` 和 PostgreSQL 16，初始化数据库并安装 `mrp`，关闭演示数据。为两个公司各配置制造仓库和一张合成工单。使用只属于首个公司的制造用户及临时 API 密钥，按工单 ID 过滤 `search_read`，读取 `id`、`name`、`state`、`company_id`、`write_date`。

首个公司的工单返回一条 `draft` 记录，`company_id` 为 ID 和显示名组成的数组。相同身份按第二个公司的已知工单 ID 查询返回空数组。因此这个调用的空结果只说明在当前授权范围内无可见记录，适配层不应从中推断对象是否存在于其他公司。该证据覆盖本地接口读取和公司隔离，Observe 联调及状态生命周期映射待验证。

### OpenMES 本地接口验证

在 OpenMES `faccc52a543382922aa8b7d9d79b89e6a30b28d3` 上，使用 PHP 8.5.13 与 PHPUnit 11.5.56，经 Laravel HTTP 测试内核和内存 SQLite 执行真实路由、Sanctum 认证、工单查看策略与序列化。测试为单产线 Operator 创建 Bearer token，并为两条产线分别创建一个合成工单。将 [样本测试](references/mcp/openmes-adapter-sample.php) 复制到 OpenMES 的 `backend/tests/Feature/Api/AdapterSampleSmokeTest.php`，在 `backend` 下运行 `vendor/bin/phpunit tests/Feature/Api/AdapterSampleSmokeTest.php`，1 个用例、11 条断言通过。 [查看策略源码](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Policies/WorkOrderPolicy.php#L32)

| 请求 | 结果 |
|---|---|
| 无 token 读取本产线工单 | 401 |
| Operator 读取本产线工单 | 200，`data` 含 `id`、`order_no`、`status: PENDING` 和 UTC `updated_at` |
| 同一 token 读取其他产线工单 | 403 |
| 同一 token 读取不存在的 ID | 404 |

Odoo 已观测到的空数组和 OpenMES 的 403／404 须分别解释。适配层应保留可公开的拒绝或不可见语义，并避免以跨范围查询推断存在性。Odoo `draft` 与 OpenMES `PENDING` 各自保留源值，需按生命周期证据评审是否可映射为同一规范状态。另用 [HTTP 样本脚本](references/mcp/openmes-adapter-http-smoke.sh) 在相同 OpenMES 版本、隔离 SQLite 和 PHP 8.5.13 环境启动回环 HTTP 服务。将脚本复制到已安装 Composer 依赖的 OpenMES backend 目录，运行 `sh openmes-adapter-http-smoke.sh`。真实请求复现了表中的四项结果，200 返回的四个字段一致。实际站点网络、部署身份与 AgentNexus Observe 联调仍待验证。

AgentNexus `8debd3e` 已合并 M2-5 远程 DES 浏览器验收。其 DES Observe 当前按 `world_id` 读取已发布的模拟快照。生产工单读取需要 M3 绑定与对象选择契约，联审需确认生产身份、授权范围、适配调用位置和响应错误归属。 [DES Observe 实现](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/8debd3e/crates/server/src/mom_world_des.rs#L680)

## M3 Observe 联审契约

候选版本 v1，待 AgentNexus 与 MOM-MCP 联审冻结。首期限定单对象、单已绑定来源的只读工单观测。基线为 MOM-MCP `81513b0` 与 AgentNexus `8debd3e`，分别对应两套本地 HTTP 样本与 M2-5 DES 浏览器验收。现有 DES Observe 按 `world_id` 取模拟快照，生产 Observe 需要对象选择及生产身份绑定。 [World Adapter Observe](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/8debd3e/docs/mom/WORLD_ADAPTER.md#41-observe) · [DES Observe 实现](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/8debd3e/crates/server/src/mom_world_des.rs#L680)

### 请求、授权与返回

- AgentNexus 接收已认证会话的 `Observe(world_id, selection)`。首期 `selection` 指定一个 `{type: "work_order", id: "..."}`。宿主按可信会话解析租户、站点、生产世界、授权范围和适配器绑定。调用方不能覆盖来源地址或凭据。
- `id` 为指定生产世界内的不透明对象引用，由已授权的来源引用或联调夹具提供。AgentNexus 将其解析为绑定范围内的源记录 ID。解析失败或越权在访问来源前拒绝。同一数字源 ID 在不同来源形成不同对象引用。
- MOM-MCP 适配器读取已绑定来源，校验响应对象与请求匹配，并返回公共语义和受控来源标识。宿主按公开权限组装 `protocol_version`、`request_id`、`world_id`、`objects` 与 `as_of`。对象包含 `ref`、`source`、`observed_at` 和 `state`。候选 `state` 字段为 `display_number`、`source_status`、可空的 `normalized_status`、`mapping_version` 和可选的 `mapping_issue`、`source_updated_at`。
- 有源更新时间时转为 RFC 3339 UTC。Odoo 样本时间无偏移，需核实上游 UTC 约定再转换。更新时间与观测时间分别保留。两套响应均未证明提供可复核的对象版本，首期省略 `object_version`。
- 对接 World Adapter 的 `per_source` 一致性口径。来源标记为仅支持快照重读。建议返回空的可续读游标，并且不声明 Subscribe 能力。游标字段的可空性、水位形状和 DTO 兼容策略需在联审中确认。

### 来源字段与语义映射

| 业务信息 | Odoo Community 19.0 | OpenMES |
|---|---|---|
| 读取意图 | JSON-2 `mrp.production/search_read` 按 ID 取单条 | `GET /api/v1/work-orders/{id}` 读取 `data` |
| 源身份、显示号 | 数据库实例内的 `id`，`name` 用于展示 | 实例内的 `data.id`，`data.order_no` 用于展示 |
| 状态与更新时间 | `state`、`write_date` | `data.status`、`data.updated_at` |
| 首轮映射 | `draft` 与 `to_close` 保留源值，规范状态待核实 | `PENDING`、`CHANGE_HOLD` 与 `DONE` 保留源值，规范状态待核实 |
| 待评审等价候选 | `progress` 表示生产已启动 | `IN_PROGRESS` 表示执行中，需补两端状态转换证据后才发布为同一规范状态 |

首轮读取样本为 Odoo `draft` 和 OpenMES `PENDING`。后续已验证各自的确认或接受路径，见下节。两边进入条件和动作副作用的等价性仍无充分证据，应返回空的规范状态和 `unmapped` 诊断。缺失、未知、歧义可分别标注。响应结构无效或返回记录不匹配时拒绝整条观测并记录受限诊断。来源状态仅在调用身份已获授权且值可公开时返回。

### 失败映射与验收断言

| 情况 | 候选公开语义与验证 |
|---|---|
| AgentNexus 会话越权或跨绑定 | 在来源调用前拒绝，并断言没有上游请求。 |
| 来源内不可见 | Odoo 成功空数组及 OpenMES 对象级 403／404 返回统一不可见结果。受信侧保留实际响应分类，普通调用方无法借此区分受限对象是否存在。 |
| 来源身份或配置故障 | 来源 401 或可确认的连接级拒绝返回连接不可用。不得把来源 401 解释为调用方的会话身份失败。 |
| 来源暂时故障 | 超时和上游暂时故障仅对读取有界重试。测试断言最终返回不可用，且零模拟调用。 |
| 观测成功 | 在两种绑定上验证同一读取意图、稳定来源、同号不同对象、时间语义以及待映射状态。 |

联审输出应冻结绑定字段、连接器部署和身份、对象引用签发方式、一致性字段、公开错误表示及兼容策略。冻结后登记 Fxx、执行阶段和验收用例；首先验证两种真实 HTTP 来源，再通过 AgentNexus Observe 运行同一套断言。生产站点、事件订阅、跨系统关联与写入各自保留独立签收条件。

### 联审决策与落地顺序

| 决策 | MOM-MCP 提案 | 需 AgentNexus 确认 |
|---|---|---|
| 生产绑定 | 适配器只接收由服务端解析的来源、授权范围和对象引用。绑定判定、能力发现及公开错误由 World Adapter 负责。 | 确认绑定与调用身份的事实来源、撤权后的失效机制、部署及凭据边界。 |
| 快照与游标 | 首期只读一个对象，来源标记为需要重读。不给只读样本签发事件游标或声明订阅能力。 | 确认 `per_source` 水位结构、无游标表示和既有 DTO／客户端兼容口径。 |
| 语义映射 | 源状态和映射诊断随授权后的观测保留。只有进入条件和业务后果都核实后，才新增可用规范状态。 | 确认映射版本发布责任、诊断公开范围，以及哪些规范状态为首期必需。 |
| 证据和准出 | 冻结后先登记独立只读验收，再实现两种来源，最后通过真实 Observe 验证同号、权限、来源故障和零模拟调用。 | 确认测试站点身份、集成用例入口和签收责任。 |

联审前可以继续验证来源生命周期、业务字段与 HTTP 事实。生产路由、公开 DTO 和生产写入须按各自的授权与验收推进。

### 首个状态转换的对照证据

| 来源 | 运行验证 | 映射结论 |
|---|---|---|
| Odoo Community 19.0 | 使用上文固定镜像和本地 JSON-2 读取。首个公司的草稿单返回 `draft`，对同公司另一个合成制造单执行服务器端 `action_confirm` 后读到 `confirmed`，跨公司查询仍为空。 | 确认进入制造执行准备，并可能触发库存移动确认或物料调度。保留源状态。 [确认实现](https://github.com/odoo/odoo/blob/d36b9405fe5d2ede753c6d7eceb778ecef20f67b/addons/mrp/models/mrp_production.py#L1626) |
| OpenMES | 在固定版本运行 [样本测试](references/mcp/openmes-adapter-sample.php)，用 Laravel HTTP 内核读取 `PENDING`，执行接受后再读取 `ACCEPTED`。与访问范围用例合计 2 项测试和 16 条断言通过。 | 接受接口校验授权与前置状态，并持久化状态转换。尚无与 Odoo 库存或工序副作用等价的证据。 [接受实现](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Http/Controllers/Api/V1/WorkOrderController.php#L136) |

两套试验用本地合成数据准备读取样本。公开适配器的首期能力仍为只读。候选契约保留上述四个源值，规范状态在取得等价证据前保持为空。`progress` 与 `IN_PROGRESS` 仍待运行证据和联审确认。
