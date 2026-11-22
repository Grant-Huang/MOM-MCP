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
