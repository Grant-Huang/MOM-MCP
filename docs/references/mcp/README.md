# 开源 MES/MOM 工具能力参考清单

供 MOM-MCP 审查工具覆盖、业务粒度和异常处理使用。四个项目按固定源码版本归档。CSV 每条记录均有能力说明，表中的计数使用各自的提取单位。

| 项目 | 清单 | 计数单位 | 能力说明来源 |
|---|---|---|---|
| Carbon | [MCP 入口](carbon-mcp-entrypoints.csv) · [业务操作](carbon-operations.csv) | 4 个 MCP 入口、1,823 个可发现业务操作 | 注册源码；业务操作取对应服务的上游 JSDoc 首段 |
| qcadoo | [HTTP 映射](qcadoo-http-mappings.csv) | 239 条 Spring 映射声明 | 控制器和方法命名推断 |
| OpenMES | [路由](openmes-routes.csv) | 932 条路由声明，其中 API 431 条、Web 501 条 | 路由和处理函数命名推断 |
| WebErpMesv2 | [路由](weberpmesv2-routes.csv) | 1,073 条路由声明，其中 API 36 条、Web 1,020 条、其余 17 条 | 路由和处理函数命名推断 |
| 四项目 | [业务能力源码审读](reviewed-capabilities.csv) | 57 条重点服务、控制器及状态模型记录 | 按实现归纳的中文说明 |

CSV 采用 UTF-8 和表头。按 `capability` 筛选业务意图，按 `description_basis` 区分上游描述和静态推断。重点审读表的 `project` 对应下文项目，`source` 为相对仓库路径，`line` 为固定提交中的一基行号。在对应源码链接后加 `/<source>#L<line>` 即可定位。建议先用重点表找业务闭环和异常分支，再回到各项目基础清单查同域操作。

## Carbon

固定提交 `2f5b652498539be79372635666f1ccc45f5cb601`。运行时注册 `search_tools`、`describe_tool`、`call_tool`、`call_destructive_tool`。业务操作数量来自提交中的 manifest digest，16 个模块合计 1,823 个，其中生产 255、质量 111、库存 162、物料 216、资源 105，五域共 849 个。该口径包含查询、配置、写入和辅助操作。权限与风险分类按上游清单记录。 [固定源码](https://github.com/crbnos/carbon/tree/2f5b652498539be79372635666f1ccc45f5cb601) · [清单摘要](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/routes/api%2B/mcp%2B/lib/tool-manifest.digest.json) · [MCP 注册](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/packages/ee/src/mcp/server.ts)

- **工单下达**。就绪检查和下达是独立意图。批量下达按工单给出成功、跳过、失败和警告。外协采购单结果可能与下达结果不同，适合覆盖 Agent 如何确认已发生的副作用并继续处理余项。 [生产服务](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/production/production.server.ts#L498)
- **执行与入库**。工序完工能记录部分产量，后续发料可能失败。工单完工和在制余量退出也存在分步结果。公共工具应让 Agent 分别查工序实绩、材料移动及工单入库结果。 [工序完工](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/production/production.server.ts#L3231) · [工单完工](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/production/production.server.ts#L981)
- **库存与质量**。收货行绑定批次、拣选与撤拣、库存纠正、部分检验与拒收各有独立语义。应单独审查这些动作的状态条件、审计记录及重复执行行为。 [库存服务](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/inventory/inventory.server.ts) · [质量服务](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/quality/quality.server.ts)

## qcadoo

固定提交 `625afad7a8adaaa65b5b8b7bd7867af21aafc960`。业务跨插件组织。Spring 映射提供工艺、工位、制造和报工等接口线索，另需查阅插件状态机与监听器才能确认事务能力。 [固定源码](https://github.com/qcadoo/mes/tree/625afad7a8adaaa65b5b8b7bd7867af21aafc960)

- **生产订单与报工**。订单状态枚举逐状态允许迁移。报工状态服务按接受、拒绝和纠正调用不同的校验与后处理。接受会更新产量和废品等统计，并可触发订单结束。撤销与纠正涉及计数回退，适合用来验收跨班次更正后订单、报工和统计是否一致。 [状态与服务插件](https://github.com/qcadoo/mes/tree/625afad7a8adaaa65b5b8b7bd7867af21aafc960/mes-plugins)
- **库存与谱系**。发料、盘点、重包装有各自状态服务，库存单据失败可在事务回滚后补记。高级谱系插件提供上下游批次树。公共模型应保留报工关联、物料批次以及可查询的纠正记录。 [库存插件](https://github.com/qcadoo/mes/tree/625afad7a8adaaa65b5b8b7bd7867af21aafc960/mes-plugins/mes-plugins-material-flow-resources) · [谱系插件](https://github.com/qcadoo/mes/tree/625afad7a8adaaa65b5b8b7bd7867af21aafc960/mes-plugins/mes-plugins-advanced-genealogy)

## OpenMES

固定提交 `faccc52a543382922aa8b7d9d79b89e6a30b28d3`。API 覆盖工单变更、工序、物料批次、质量、停机、设备和集成等；Web 路由补充管理操作。可先筛选 `backend/routes/api.php` 作为 API 比对基线。 [固定源码](https://github.com/Mes-Open/OpenMes/tree/faccc52a543382922aa8b7d9d79b89e6a30b28d3) · [API 路由](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/routes/api.php)

- **在制工程变更**。变更请求可预查影响、提交、批准、拒绝、取消和应用。工单快照按生产单位选择生效版本，并可计算剩余需求。适合用来验收旧在制件与新开工件是否遵循各自规格及料表。 [工单快照](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Services/WorkOrder/WorkOrderSnapshotService.php) · [变更验收用例](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/tests/Feature/Api/V1/WorkOrderChangeRequestApiTest.php)
- **质量与批次**。处置服务加锁校验检验状态并更新关联批次。批次接口分别暴露上下游谱系和使用位置查询。适合用待检、处置、可用性变化和召回范围验证同一个批次。 [处置服务](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Services/Quality/DispositionService.php) · [批次控制器](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Http/Controllers/Api/V1/MaterialLotController.php)

## WebErpMesv2

固定提交 `8fa6c05452ce5a5e3d0bf68ef1592446c1858a54`。API 路由包含任务、订单和集成，车间扫码、Andon、排样、质量、维修等入口大量写在 Web 路由中。Web 路由包含页面、重定向及 Web 会话操作，须按实际制造动作筛选。 [固定源码](https://github.com/SMEWebify/WebErpMesv2/tree/8fa6c05452ce5a5e3d0bf68ef1592446c1858a54) · [Web 路由](https://github.com/SMEWebify/WebErpMesv2/blob/8fa6c05452ce5a5e3d0bf68ef1592446c1858a54/routes/web.php)

- **车间与钣金**。排样计算和计算状态查询、实际耗料记录、切件分配与待确认余料分别支持不同的 Agent 决策。库存移动还会触发预留更新。钣金场景应验收跨订单套料、余料回库和批次去向。 [套料实绩](https://github.com/SMEWebify/WebErpMesv2/blob/8fa6c05452ce5a5e3d0bf68ef1592446c1858a54/app/Services/Stock/NestRecorder.php) · [库存观察器](https://github.com/SMEWebify/WebErpMesv2/blob/8fa6c05452ce5a5e3d0bf68ef1592446c1858a54/app/Observers/StockMoveObserver.php)
- **排程与异常**。有限产能排程结合订单行工序依赖。Andon 有触发、处理中与解决三个动作。生产序列号查询把库存移动、任务和质量记录编成时间线，供长周期异常跟踪和交接复核。 [排程服务](https://github.com/SMEWebify/WebErpMesv2/blob/8fa6c05452ce5a5e3d0bf68ef1592446c1858a54/app/Services/Planning/FiniteCapacityScheduler.php) · [制造追溯](https://github.com/SMEWebify/WebErpMesv2/blob/8fa6c05452ce5a5e3d0bf68ef1592446c1858a54/app/Http/Controllers/ProductionTraceController.php)

## 提取口径与使用限制

- Carbon 以 digest 操作名为集合，使用模块及末段符号定位 `@mcp` 服务定义，逐项记录分类、权限、分页、源码定位和首段说明。入口和业务操作分别计数。
- qcadoo 提取跟踪的 Java 源文件中带 Spring 请求映射注解的声明，含类级前缀、页面、管理接口和配置相关映射。HTTP 方法为空表示未从该注解提取到显式方法。
- 两个 Laravel 清单提取跟踪的路由文件中带字面量路径的声明。路径列保留原始片段，完整路径取决于外层组前缀及框架配置。资源路由一行对应一处声明，展开后的 HTTP 路由数量取决于资源限制。清单也包含页面、认证、基础设施和集成入口。
- 对 REST 和 Web 路由的计数是静态源文件索引。运行时条件、注册器动态路由、Java 插件定义中的 UI 按钮和后台调用需要另沿实现核实。本次核对源码与静态记录，未启动四个上游系统。
- MOM-MCP 采用业务流程作为交付单位。P10 前优先用这些附件对照执行、质量、主数据、库存和谱系闭环，并将配置动作纳入后续目录。以业务意图、独立权限、状态影响和纠正路径决定拆分粒度。参照 [MES MCP 目录完整性标准](../../MCP_CATALOG_COMPLETENESS.md)。

## Carbon 社区版公司隔离取证

在有合法访问权限的 Carbon 社区版测试实例中，准备两个测试公司，每个公司各一条已知工单。为两个只属于各自公司的员工签发 Supabase 登录会话。通过受信环境注入以下变量。

| 变量 | 输入 |
|---|---|
| `MOM_CARBON_REST_ORIGIN` | Supabase 网关根地址，HTTPS 或隔离环境的回环 HTTP |
| `MOM_CARBON_PROJECT_KEY` | Supabase 公开项目键 |
| `MOM_CARBON_A_TOKEN`、`MOM_CARBON_B_TOKEN` | 两个员工各自的用户访问令牌 |
| `MOM_CARBON_A_JOB_ID`、`MOM_CARBON_B_JOB_ID` | 两个对应工单的内部记录 ID |

运行 `node docs/references/mcp/carbon-community-smoke.mjs`。它先用各自的用户会话确认两条工单存在且属于不同公司，再交叉读取。查询按工单 ID 精确匹配，省略公司过滤，因此空数组验证的是源端可见范围。任一 HTTP 错误、重复响应或跨公司记录可见均使脚本失败。通过时只输出两次本公司读取和两次跨公司读取的结果分类。

记录部署版本、测试角色范围和脱敏结果。通过后继续使用同一组工单完成 [AgentNexus 短期验收](../../acceptance/ADAPTER_READ.md)。离线执行 `node --test docs/references/mcp/carbon-community-smoke.test.mjs`。
