# 对照四个开源 MES 审查公开 MCP 目录

## 结论与证据

现有目录足以支撑 CNC 和按单装配的多段制造任务。工具粒度总体合理。预检与下达、预留与发料、报工与完工、检验与处置各有独立业务结果，纠正操作能够保留原记录。按 P10 前置门槛衡量，目录仍缺可执行的库存、在制、质量、入库与关单闭环，三种非试点工艺也尚未复用同一套公共契约。 [目录完整性标准](MCP_CATALOG_COMPLETENESS.md) · [公共验收门槛](acceptance/COMMON_MES_GATE.md#p10-前置门槛)

审查基线为 `master@53b2f6f`。已注册场景定义共 74 版，按公开业务名去重有 126 项。谱系候选 `2a4c4e7` 通过真实 stdio MCP 枚举 75 个场景版本，逐一核对 `search_tools` 和 `describe_tool`，得到 129 个业务工具名，另有 8 个公开网关方法。最新场景分别开放 CNC 86、装配 80、SMT 13、钣金 7、线束 7 项，存在共享工具。这些数字表示目录可发现及可调用的范围。候选的 P9 定向 Rust 集成测试本次运行 20 项，全数通过。各行业的流程准出按验收用例另行判断。 [阶段验收映射](feature-map.toml)

复读了 Carbon、qcadoo、OpenMES、WebErpMesv2 的固定提交源码。重点是检验处置、实绩纠正、快照生效和追溯；未在此次审查中部署上游系统。Carbon 的 1,823 个操作含 ERP 配置与辅助能力，两套 Laravel 路由含页面、会话和集成入口。覆盖比较以相同业务意图与结果为单位。 [上游版本及提取口径](references/mcp/README.md)

## 可复现发现

| 优先级 | 发现 | 建议验收 |
|---|---|---|
| 优先 | CNC@37 预检依赖旧库存状态。以 WIRE 为唯一 BOM 组件并且公共台账零库存时，`get_release_readiness` 返回 `ready=true`，下达后的 `get_material_requirements` 返回需求 20 m、可用 0 m、缺料 20 m。 [预检实现](../src/mes.rs#L1196) · [需求实现](../src/inventory.rs#L793) | 预检和下达前置规则共享同一份版本化需求与可用量计算。若业务允许缺料下达，结构化返回缺料数量及政策判定，保证发料或开工门禁可解释。 |
| 优先 | 同一 CNC@37 运行的 `mes_status.available_actions` 返回 `release_order`，但 `search_tools` 没有此名称。Agent 照状态提示无法操作。 [状态实现](../src/mes.rs#L1232) · [场景能力](../fixtures/cnc-wip-genealogy.json) | 在需要操作的每个阶段，推荐动作都应在当前 run 可搜索、可描述，并用当前契约表达。验收旧场景兼容和新版预检、下达、报工三段。 |
| 优先 | 工单完工确认可以推进到关单，当前关单分支检查完工确认和质量条件，未查询公共成品入库或平衡核对。目录尚无计划中的 `receive_work_order_output` 和 `reconcile_work_order`。 [关单实现](../src/mes.rs#L1582) · [P9D 工具候选](COMMON_MES_DESIGN.md#p9-公共主数据物料及追溯) | 单独验收入库事务和只读对账。尝试未入库、在制未结清或账差未处理时关单，并检查允许的处置和复验路径。 |

前两项使用干净 SQLite、公开 `stdio --scenario-tools` 复现。打开 `cnc@37`，启用带 WIRE 2 m／件的 BOM、路线及规格，查询 `mes_status` 和预检，未执行收货就使用有效指纹下达，再查询下达快照的需求。表中列出的返回值和推荐动作均来自真实 MCP 响应。

## 按业务流程对标

| 业务链 | 已有能力及合理边界 | 缺口与取舍 |
|---|---|---|
| 订单与工程变更 | 预检、下达和冻结快照分开，版本漂移时拒绝下达，旧工单可继续读取原定义；适合复查和重试。 | Carbon 批量下达有逐项副作用，OpenMES 支持按生产单位选生效版本。新快照场景先交付齐套判定与单笔一致性，再按需接入批量下达、工程变更审批与在制生效。 [Carbon 生产服务](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/production/production.server.ts#L498) · [OpenMES 快照服务](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Services/WorkOrder/WorkOrderSnapshotService.php#L111) |
| 工序与实绩纠正 | 工序报工、完工及报工冲销分开，CNC 部分报工与装配单件报工复用实际发料逻辑。工序耗用已阻断直接冲销来源报工。 | qcadoo 的成功、撤销和更正同时影响实绩、废品及订单累计。需要同一运行内核对报工、净发料、剩余在制和成品入库。自动投影的报工产出继续用同一标识；只有独立产出事务得到证实时才开放 `record_operation_output`。 [qcadoo 报工接受与撤销](https://github.com/qcadoo/mes/blob/625afad7a8adaaa65b5b8b7bd7867af21aafc960/mes-plugins/mes-plugins-production-counting/src/main/java/com/qcadoo/mes/productionCounting/states/listener/ProductionTrackingListenerService.java#L266) · [P9C 验收](acceptance/P9C.md) |
| 库存与来料质量 | 收货、检验放行、预留、拣撤、工序发料、退库及库存纠正已有独立记录。并发争料和多种冲销有验收。 | Carbon 的部分放行与拒收明确改变数量，拒收与库存核销绑定。当前收货放行针对整笔收货；需验证同批部分合格、剩余待检或拒收数量、并发及纠正后守恒。建议先扩展既有对象和台账，依据独立审批及过账边界决定是否新增命令。 [Carbon 拒收](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/quality/quality.server.ts#L1443) · [Carbon 部分检验](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/quality/quality.server.ts#L1543) |
| 在制与质量谱系 | 候选把 CNC 原料、在制、序列号和下游耗用关联起来，能保留冲销边；现有在制查询提供冻结状态。 | OpenMES 区分批次、子批和单件级去向及召回影响，WebErpMesv2 串联库存移动、工序任务和不合格事件。当前主要是批次级实投与源批次产出关系。需补在制拆合、多层追溯、跨工单冻结、未受影响产品继续流转，以及历史边与处置证据联查。考虑在现有入口增加查询范围和证据粒度，避免同义工具重复。 [OpenMES 使用去向](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Http/Controllers/Api/V1/MaterialLotController.php#L120) · [WebErpMesv2 序列号时间线](https://github.com/SMEWebify/WebErpMesv2/blob/8fa6c05452ce5a5e3d0bf68ef1592446c1858a54/app/Http/Controllers/ProductionTraceController.php#L19) |
| 跨工艺及权限 | CNC 和装配先验证了共同收货、发料、实绩和纠正。SMT 料卷、钣金排样及线束压接保留独立业务规则。公开入口与仿真控制已分开。 | 对照 WebErpMesv2 的实际板材消耗及余料确认，用多订单共用板材证明数量和谱系。按前置门槛先做 SMT 共享契约复验，再做跨车间合批拆批。在真实企业权限签收前，把检验、处置、让步和放行的角色授权证据单独记录。 [WebErpMesv2 排样实绩](https://github.com/SMEWebify/WebErpMesv2/blob/8fa6c05452ce5a5e3d0bf68ef1592446c1858a54/app/Services/Stock/NestRecorder.php#L74) · [分层与工艺复用](MCP_LAYERING_REVIEW.md) |

## 建议交付顺序

1. 修齐预检与下一动作。以冻结 BOM、台账和当前场景工具契约为证据，让一次检查能解释缺料和可采取的操作。
2. 交付 CNC 的在制拆合、移交纠正、成品入库与工单对账。用一个 run 验证两批投入、批次级质量冻结、复检纠正、入库与关单，观察所有平衡项及被阻断的依赖。
3. 在按单装配复验核心链路，在 SMT 验证同名公共预留、发料、冻结和追溯。用跨车间案例验证合批拆批的影响范围。依据证据决定是否进入 P10。

目前的统计可用于跟踪目录演进。阶段进度以公开 MCP 正常路径、拒绝、恢复和对账结果判断。生产级有限产能优化、完整 CAPA 和外部系统写入按现有产品边界独立排期。 [能力边界](MES_SCOPE.md) · [双主轴计划](IMPLEMENTATION_PLAN.md#双主轴路线与准出)

## 已登记的后续事项

暂缓处理下达预检与公共库存不一致、`mes_status` 推荐未开放动作，以及同批部分放行和拒收的数量守恒。本次按 P9D 顺序先交付在制移交纠正。成品入库、跨车间冻结与工单对账仍属于既定 P9D 准出，继续按公共门槛验收。
