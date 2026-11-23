# 外部系统只读适配设计

状态为待评审。首个切片验证 AgentNexus 对三种异构生产来源执行同一工单读取意图。需求见 [双主轴需求](FEATURES.md#双主轴需求与验收目标)，执行状态见 [外部适配器实施计划](IMPLEMENTATION_PLAN.md#外部适配器实施计划)。

本文定义短期只读切片。中期沿同一 Adapter 边界逐步扩大三源业务覆盖。连接器按来源版本登记，通用层维护业务意图、能力、规范对象及结构化错误。每项能力记录源入口、映射版本、查询/执行/核验支持、限制与真实调用证据。ERP/CRM 扩展从有可核验依据的订单关系开始。独立 MCP 客户端的首期工具入口在联审确认。中期末段开始 Mock 联调，后期验证新增 MES 和 Mock 两项接入。 [两线阶段与准出](IMPLEMENTATION_PLAN.md#双主轴路线与准出)

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
| Carbon MCP 生产工单读取 | 可检出版本 `05bd067` 的 `production_getJob` 接受字符串 ID，声明生产查看权限。 | 固定部署与授权、脱敏响应、实际状态生命周期。 [源码](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/production/production.server.ts#L6957) |
| OpenMES HTTP 工单读取 | 固定源码注册 `work-orders` API resource。详情控制器先执行 `view` 授权，再以 `data` 包装工单和关联。 | 确认部署路由前缀、实际凭据、详情 schema、业务状态和错误响应。 [详情控制器](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Http/Controllers/Api/V1/WorkOrderController.php#L59) · [源码](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/routes/api.php#L653) |
| Odoo Community 19.0 自托管 JSON-2 | 固定源码定义 `mrp.production` 和使用 Bearer 身份的 JSON-2 路由。候选读取为 `POST /json/2/mrp.production/search_read`，服务账号按授权公司和记录 ID 读取。 | 验证自托管实例的端点、记录规则、脱敏响应与失败行为。按数据库绑定区分源记录，核对跨公司隔离。保留源状态 `to_close` 与 `done` 的区别，待核实后决定规范状态。 [制造订单源码](https://github.com/odoo/odoo/blob/d36b9405fe5d2ede753c6d7eceb778ecef20f67b/addons/mrp/models/mrp_production.py#L177) · [JSON-2 路由源码](https://github.com/odoo/odoo/blob/d36b9405fe5d2ede753c6d7eceb778ecef20f67b/addons/rpc/controllers/json2.py#L49) |

Odoo、OpenMES 与 Carbon 均纳入首期最低验证范围。Odoo 制造订单与其工序工单是不同对象，首期对齐制造订单。后续可用同一 Odoo 实例验证销售与制造关联，跨厂商关联另用独立来源验证。

三套样本均须有版本、来源和契约证据。验收用同一工单读取意图覆盖等价状态、同号不同源、未映射状态、越权、对象不存在和源不可用。已配置源失败时验证零模拟调用。批准的接口夹具可用于结构与回归验证，已部署系统的调用轨迹作为运行时接入证据。CRM／ERP 销售订单到生产工单的跨系统关系在首期结果基础上另行设计。

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

## Carbon 样本验证

旧参考提交 `2f5b652` 当前无法从上游取得。Carbon 验证选用可检出的 `05bd067c0a72ca9f0983f6278d2a7c9bb48b716c`。运行 `node docs/references/mcp/carbon-adapter-sample.mjs <Carbon checkout>`，核实 `production_getJob` 注册为只读、要求 `production:view`，并使用调用者的 Supabase 客户端按内部 `id` 查询 `jobs`。夹具通过单记录和无可见记录两条查询断言。静态核对确认 `jobs` 为 security-invoker 视图，工单表定义公司范围行级策略。 [工具注册](https://github.com/crbnos/carbon/blob/05bd067/apps/erp/app/modules/production/production.server.ts#L6957) · [查询函数](https://github.com/crbnos/carbon/blob/05bd067/apps/erp/app/modules/production/production.service.ts#L1436)

Carbon 上游 MCP 路由要求 Business 权益，默认 ERP 镜像下载也返回未授权。Carbon 仍缺获授权实例的 MCP 工单读取及跨公司验证。夹具结果不代表 MCP 载荷、权限执行或 RLS 已实测。 [上游 MCP 权益检查](https://github.com/crbnos/carbon/blob/05bd067/apps/erp/app/routes/api%2B/mcp%2B/_index.ts#L110)

### Carbon MCP 实例验收入口

在获授权的 Business 或以上测试实例中预置两条合成工单。一条属于测试身份可访问的公司，另一条由测试管理员确认存在于该身份无权访问的公司。通过环境注入 `MOM_CARBON_MCP_URL`、`MOM_CARBON_TOKEN`、`MOM_CARBON_VISIBLE_JOB_ID` 和 `MOM_CARBON_OTHER_COMPANY_JOB_ID`，运行 `node docs/references/mcp/carbon-mcp-smoke.mjs`。探针使用 HTTPS，固定 legacy MCP JSON 协议，先描述 `production_getJob`，再对两个已知样本执行只读调用。根据固定版本源码，单条工单通过 MCP 文本块承载 JSON，读取仅投影 `id`、`jobId`、`status` 和可选 `updatedAt`。 [Carbon MCP 实现](https://github.com/crbnos/carbon/blob/05bd067/packages/ee/src/mcp/server.ts) · [结果格式](https://github.com/crbnos/carbon/blob/05bd067/packages/ee/src/mcp/format-result.ts)

探针只输出脱敏结果。HTTP 402 表示当前实例未通过 MCP 权益检查。另一公司返回工具错误时，须由实例负责人从受信侧核对确实因不可见而拒绝，并排除服务故障，才能签收公司隔离。记录部署版本、身份权限范围和复核结论。离线检查命令为 `node --test docs/references/mcp/carbon-mcp-smoke.test.mjs`，覆盖成功、权益拒绝、跨公司意外成功及返回对象不匹配。当前未配置获授权实例，未执行现场验收。

## M3 Observe 联审契约

候选版本 v1，待 AgentNexus 与 MOM-MCP 联审冻结。首期限定单对象、单已绑定来源的只读工单观测。三系统类型、来源身份、字段和映射策略见 [工单观测本体约定 v0.1](WORK_ORDER_ONTOLOGY.md)。基线为 MOM-MCP `81513b0` 与 AgentNexus `8debd3e`，分别对应两套本地 HTTP 样本与 M2-5 DES 浏览器验收。现有 DES Observe 按 `world_id` 取模拟快照，生产 Observe 需要对象选择及生产身份绑定。 [World Adapter Observe](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/8debd3e/docs/mom/WORLD_ADAPTER.md#41-observe) · [DES Observe 实现](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/8debd3e/crates/server/src/mom_world_des.rs#L680)

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
| 观测成功 | 在三种绑定上验证同一读取意图、稳定来源、同号不同对象、时间语义以及待映射状态。 |

联审输出应冻结绑定字段、连接器部署和身份、对象引用签发方式、一致性字段、公开错误表示及兼容策略。冻结后登记 Fxx、执行阶段和验收用例；首先验证三种实际来源接口，再通过 AgentNexus Observe 运行同一套断言。生产站点、事件订阅、跨系统关联与写入各自保留独立签收条件。

### 联审决策与落地顺序

| 决策 | MOM-MCP 提案 | 需 AgentNexus 确认 |
|---|---|---|
| 生产绑定 | 适配器只接收由服务端解析的来源、授权范围和对象引用。绑定判定、能力发现及公开错误由 World Adapter 负责。 | 确认绑定与调用身份的事实来源、撤权后的失效机制、部署及凭据边界。 |
| 快照与游标 | 首期只读一个对象，来源标记为需要重读。不给只读样本签发事件游标或声明订阅能力。 | 确认 `per_source` 水位结构、无游标表示和既有 DTO／客户端兼容口径。 |
| 语义映射 | 源状态和映射诊断随授权后的观测保留。只有进入条件和业务后果都核实后，才新增可用规范状态。 | 确认映射版本发布责任、诊断公开范围，以及哪些规范状态为首期必需。 |
| 证据和准出 | 冻结后先登记独立只读验收，再实现三种来源，最后通过真实 Observe 验证同号、权限、来源故障和零模拟调用。 | 确认测试站点身份、集成用例入口和签收责任。 |

联审前可以继续验证来源生命周期、业务字段与 HTTP 事实。生产路由、公开 DTO 和生产写入须按各自的授权与验收推进。

### 首个状态转换的对照证据

| 来源 | 运行验证 | 映射结论 |
|---|---|---|
| Odoo Community 19.0 | 使用上文固定镜像和本地 JSON-2 读取。首个公司的草稿单返回 `draft`，对同公司另一个合成制造单执行服务器端 `action_confirm` 后读到 `confirmed`，跨公司查询仍为空。 | 确认进入制造执行准备，并可能触发库存移动确认或物料调度。保留源状态。 [确认实现](https://github.com/odoo/odoo/blob/d36b9405fe5d2ede753c6d7eceb778ecef20f67b/addons/mrp/models/mrp_production.py#L1626) |
| OpenMES | 在固定版本运行 [样本测试](references/mcp/openmes-adapter-sample.php)，用 Laravel HTTP 内核读取 `PENDING`，执行接受后再读取 `ACCEPTED`。与访问范围用例合计 2 项测试和 16 条断言通过。 | 接受接口校验授权与前置状态，并持久化状态转换。尚无与 Odoo 库存或工序副作用等价的证据。 [接受实现](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Http/Controllers/Api/V1/WorkOrderController.php#L136) |

两套试验用本地合成数据准备读取样本。公开适配器的首期能力仍为只读。候选契约保留上述四个源值，规范状态在取得等价证据前保持为空。`progress` 与 `IN_PROGRESS` 仍待运行证据和联审确认。

### Rust HTTP 读取试实现

Odoo JSON-2 和 OpenMES HTTP 已接入 Rust 工单投影。调用方从可信配置创建一个绑定到租户、站点及源实例的读取器，向其提交已经授权的源工单 ID。读取器发出固定查询，校验返回对象与 ID 匹配，再返回内部观测。读取器无模拟回退分支。宿主仍负责生产绑定解析、调用方授权、来源实例选择和公开错误转换。

端点配置为 HTTPS 来源根地址，回环 HTTP 仅用于本地联调。读取器不跟随重定向，单次请求设有时限及响应大小上限。Odoo 成功空数组与 OpenMES 403／404 返回不可见；OpenMES 401、上游故障和异常传输返回来源不可用。源响应正文和凭据不进入错误结果。

运行 `cargo test --locked --test external_http` 复跑。测试使用本地回环 HTTP 服务验证请求方式、标头、查询条件、成功投影和失败分类。此前 Odoo 与 OpenMES 的真实应用样本用于确定接口形状。本切片的 Rust 客户端仍待两套实际应用联调，Carbon MCP 传输及 AgentNexus Observe 另行验收。

### Rust Carbon MCP 读取试实现

Carbon 使用同一个绑定读取器和工单投影。候选实现对站点配置的来源根地址固定调用 `/api/mcp`，协商 `2025-11-25` 的无会话 JSON 传输，随后只通过 `call_tool` 执行 `production_getJob`。客户端只请求已声明的工单字段，检查 RPC 关联 ID，解码单条文本 JSON，再校验源工单身份。沿用 HTTP 读取器的 HTTPS、无重定向、时限及响应大小限制。

HTTP 402 保留为权益未开通。Carbon 工具级拒绝单独返回且不附上游正文，调用方仍需获授权实例和受信侧证据，才能区分记录不可见与上游故障。读取器无模拟回退路径。运行 `cargo test --locked --test external_http` 验证回环协议夹具。本切片尚未对获授权的 Carbon 实例运行，也尚未接入 AgentNexus Observe。

### Rust 客户端的实际应用复跑

从仓库根目录执行：

```sh
cargo run --locked --example external_order_smoke -- VENDOR HTTPS_OR_LOOPBACK_ORIGIN VISIBLE_ID OUT_OF_SCOPE_ID EXPECTED_SOURCE_STATUS
```

使用获授权测试环境中的合成工单。通过环境传入 `MOM_SAMPLE_TOKEN`。Odoo 还需要 `MOM_ODOO_DATABASE`。第二条工单应已由受信侧确认存在，但超出该身份的公司或产线范围。示例复用正式候选读取器，断言首条记录的源 ID和源状态正确、规范状态为空，且第二条返回不可见。输出仅含来源和通过结果。

本地复跑沿用上文固定版本及隔离数据。OpenMES 用 Laravel 实际 HTTP 服务与 SQLite，由只属于本产线的 Operator 读取 `PENDING`，另一产线工单返回不可见。Odoo 用本地 Community 19.0 与 PostgreSQL，由单公司制造用户读取 `draft`，另一公司工单返回不可见。两套验证均直接执行本仓库 Rust 读取器。

### 生产 Observe 的宿主接线候选

MOM-MCP 提供 `ExternalObserve` 供受信宿主集成。宿主从站点配置注册来源，使用租户、站点和来源实例的完整键，来源实例选择可公开的稳定别名。每次请求把世界与不透明工单引用交给宿主解析和授权回调。只有回调返回的受权目标才能选择读取器。未知目标或被拒绝的请求不会调用任何来源，已选择来源的故障直接返回，生产读取模块没有模拟分支。

Observe 候选结果保留不透明引用、来源别名、观测时点、显示号、源状态及映射信息。宿主再构建公开响应包络。内部记录 ID、租户键、端点、凭据及上游错误正文不会序列化进候选结果。Carbon 权益或无法归类的工具拒绝暂归入来源不可用；待真实取证后评审公开分类。

运行 `cargo test --locked --test external_observe`。回环测试覆盖每次调用重新授权、撤权、未配置及故障来源不向其他来源发送请求、重复配置拒绝，以及输出中省略内部绑定字段。AgentNexus M3 的认证世界绑定、引用签发、公开 DTO 和真实来源联调仍待接线；当前组件不提供公开 HTTP 入口。

### AgentNexus M3 接线提案

核对 AgentNexus `8debd3e`。当前 World Adapter 路由挂在 MOM 模拟路径，生产世界尚无真实身份入口。已有 DES Observe 接收 `world_id` 并读取模拟快照。身份契约明确真实登录仍待决定。 [宿主路由](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/8debd3e/crates/server/src/main.rs#L135) · [身份契约](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/8debd3e/docs/contracts/identity-and-approval.md#5-待决定)

候选接线先采用已认证的受信服务身份做 M3 只读联调。联审须确认哪一服务签发凭证、AgentNexus 如何校验签发方、受众与时效、绑定存储与撤权来源，以及后续如何接企业会话。生产 Observe 入口仅在上述事项获批并有可用配置后开放。模拟角色头和客户端提交的租户、站点或来源地址均不构成生产授权。

| 接线位置 | 职责 |
|---|---|
| AgentNexus 认证入口 | 验证服务身份，从可信凭证取得主体。身份校验失败即拒绝，来源调用数应为零。 |
| AgentNexus 生产绑定解析 | 逐请求按主体、世界、工单引用和读取权限查询当前授权。配置从受信站点取得。返回受权源位置和记录 ID 至 MOM-MCP 内部组件。 |
| MOM-MCP 读取与投影 | 只调用已选择的来源。源故障返回来源不可用，禁止切换到模拟或其他来源。 |
| AgentNexus 响应映射 | 将内部观测组装为 M3 公共 DTO。联审冻结对象引用、只读快照标识、无游标表示及错误码。 |

首个跨仓验收应使用真实宿主身份入口检查合法读取、未认证、跨世界、撤权及已绑定来源失败。通过计数和脱敏报告证明拒绝请求对来源零访问，断言失效来源零回退。对 Odoo、OpenMES、Carbon 重复相同读取意图。跨仓改动进入 AgentNexus 的公开接口及权限评审流程后再实施。

MOM-MCP 侧已用 `cargo test --locked --test external_observe one_observe_entry_reads_three_authorized_worlds` 验证同一宿主 API 经受权解析回调调通三种回环协议。三个世界使用相同不透明引用和显示号，各自读回正确来源与原始状态。测试身份和数据均为夹具。这组证据覆盖宿主 API 串联，实际 AgentNexus 入口、授权实例和站点验收单独取证。
