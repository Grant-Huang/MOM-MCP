# P8–P9 公共 MES 详细设计

## 范围与交付判定

P8A 是已验收的 CNC 部分报工、幂等和冲销。P8 整体要交付工单、工序、资源派工、实绩和质量门禁的公共契约。P9 交付版本化主数据、库存及预留、工序投入/WIP、纠正和正反向谱系。CNC 为首个完整运行时消费者，SMT 提供最小的第二工艺验收。运行时试点建立在五场景及补充样例的 [工具分层、覆盖和粒度审查](MCP_LAYERING_REVIEW.md) 之上。每批以审查确定的公共契约为实现范围。P10 承接 CNC 专有参数、换刀和详细检验策略，P11 承接完整 SMT 迁移。

全部业务行为由离散制造 mock 实现。公开 MCP 仅提供中性的制造业务契约。种子、模型、夹具及评测细节限制在控制面。生产任务需可从 MCP 接收、执行、拒绝、恢复与追溯。计划排程采用约束和优先级，设备及外部系统采用仿真回执。APS 求解、真实 ERP/WMS/QMS/PLC 接入、财务过账及合规认证分别立项。

## Carbon 的粒度如何落实

- 工单就绪检查和下达分别建模。Carbon 的 `getJobReleaseReadiness`、`releaseJobs` 区分可预检条件和逐项结果。批量下达复用单工单规则，响应按项返回结果及失败原因。 [Carbon 生产服务](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/production/production.server.ts#L498)
- 分开建模开工、报工、完工与关单。Carbon 的 `completeOperation` 涉及数量和发料，`completeJob` 涉及入库和退余料。MOM-MCP 的单 run 关键写入在一个 SQLite 事务内提交。跨系统回执若无法原子提交，记录可查询的提交状态和补偿动作。 [Carbon 工序完成](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/production/production.server.ts#L3231)
- 库存收货、批次绑定、拣料、撤销拣料、转储和纠正各有独立业务语义。质量检验计划、部分接收、拒收及处置也分别建模。 [Carbon 库存服务](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/inventory/inventory.server.ts) · [Carbon 质量服务](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/quality/quality.server.ts)
- 沿用搜索、契约描述、调用和风险标记。工具边界按独立业务意图、权限、前置条件、副作用及纠正机制确定。同状态同权限的多字段修改可合并在一个受约束命令中。 [Carbon MCP 服务](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/packages/ee/src/mcp/server.ts)

## P8 公共执行与质量门禁

下表是计划契约目录，名字统一以 `mes.` 为命名空间。已有工具可由兼容适配器复用。是否生效由对应场景能力声明决定。

| 批次 | 能力族 | 候选工具 | 通过门槛 |
|---|---|---|---|
| P8A 已验收 | 报工起步 | 现有 `report_operation`、`reverse_report` | 分笔报工、累计推进、超量拒绝、冲销和序列号防复用 |
| P8B | 工单 | `list_work_orders`、`get_work_order`、`create_work_order`、`update_draft_order`、`get_release_readiness`、`release_work_order`、`release_work_orders`、`hold_work_order`、`resume_work_order`、`cancel_work_order`、`complete_work_order`、`close_work_order` | 失败的预检不改变状态，批量下达逐项回报。暂停、完工和关单分别校验。 |
| P8C | 工序、资源与实绩 | `list_operations`、`get_operation`、`list_dispatch_queue`、`set_dispatch_priority`、`get_resource_capability`、`dispatch_operation`、`reassign_operation`、`start_operation`、`pause_operation`、`resume_operation`、`post_operation_report`、`list_operation_reports`、`reverse_operation_report`、`complete_operation`、`get_command_result` | 工序依赖阻断越序，设备能力和占用竞争可验，报工与工序完工分开，冲销有下游依赖保护。 |
| P8D | 通用检验及放行 | `create_inspection`、`list_inspections`、`get_inspection`、`record_measurement`、`complete_inspection`、`open_nonconformance`、`place_quality_hold`、`disposition_nonconformance`、`release_quality_hold` | 质量冻结阻止完工或放行，处置记录包含责任、证据和处置理由，历史处置可查。 |

配置装配的工位筛选队列会列出与该工位兼容的待派工序。候选行的 `work_center` 表示本次查询的目标工位，实际派工归属由 `get_operation` 和 `get_resource_capability` 查询。已有业务阻塞优先展示，无其他阻塞时，独占工位的候选行显示 `RESOURCE_BUSY`。派工、改派和重开工序仍需在写事务内校验独占占用。 [P12 验收](acceptance/P12.md#共享装配工位)

P8D 提供外部检验规格引用与质量门禁。规格版本与收货检验的库存联动随 P9 落地。检验、库存和工序状态均有自身的生命周期；由门禁和显式业务关系决定彼此影响。

## P9 公共主数据、物料及追溯

| 批次 | 能力族 | 候选工具 | 通过门槛 |
|---|---|---|---|
| P9A | 物料与版本 | `list_items`、`get_item`、`create_item`、`create_item_revision`、`get_item_revision`、`create_bom_revision`、`get_bom_revision`、`set_bom_components`、`create_route_revision`、`get_route_revision`、`set_route_operations`、`validate_revision`、`activate_revision`、`get_order_snapshot`、`assess_engineering_change`、`create_inspection_plan_revision`、`activate_inspection_plan_revision` | 单位和引用校验，BOM/BOP 回路拒绝，生效点不冲突。同张已下达工单仍可复查原快照。 |
| P9B | 库位、收货及可用性 | `list_stock_locations`、`get_stock_balance`、`list_lots`、`get_lot`、`create_receipt`、`assign_receipt_tracking`、`post_receipt`、`get_material_availability`、`get_material_requirements`、`list_inventory_movements` | 物料、版本、库位、批次和单位可查。待检、冻结、预留和可用数量可对账。 |
| P9C | 预留、领退及转储 | `reserve_material`、`list_reservations`、`release_reservation`、`pick_material`、`unpick_material`、`issue_material_to_operation`、`return_unused_material`、`create_stock_transfer`、`post_stock_transfer`、`reverse_inventory_movement` | 两张工单争料不会超额。领料绑定实际批次和工序；部分报工按发料记录分摊实际投入。退料及冲销能恢复原库位，且保留原事务。 |
| P9D | WIP、质量联动、谱系 | `get_wip`、`move_wip`、`split_wip_lot`、`merge_wip_lots`、`record_operation_output`、`trace_lot_forward`、`trace_serial_backward`、`list_genealogy_edges`、`receive_work_order_output`、`reconcile_work_order` | 跨批部分报工后，投入、产出、废品和剩余量可对账。工序谱系保留有效记录与纠正链。 |

初始目录共 **83 项候选业务契约**，P8B–P8D 36 项，P9A–P9D 47 项。候选包含现有能力的统一迁移与新增能力。上线数量以可发现且通过契约验收的目录为准。8 个 AgentNexus 直挂入口继续复用。

P12 的 `get_assembly_as_built` 按成品序列号读取冻结 BOM、实际发料批次和报工更正历史。清单由报工及原始批次台账投影，沿用原报工标识和冲销状态。替代资格按该次报工记录读取。 [P12 验收](acceptance/P12.md#按序列号查询实装清单)

配置装配的终测尝试引用工单冻结的检验规格、快照指纹和有效装配报工。复测附带所纠正的失败尝试和返修原因。终测时点由服务端事件生成，终测通过后才能交付，序列号追溯保留各次尝试。 [P12 验收](acceptance/P12.md#冻结终测版本与返修复测)

## 实施依赖和事务顺序

1. P8B、P8C 先在 CNC 上验证公共工单和工序执行，临时读取既有夹具的 BOM、路线与库存。开工、暂停、恢复、报工和完工都进入同一执行记录。
2. P8D 以版本引用接入质量门禁。P9A 提供实际版本及快照，替换主数据夹具；当次预检的快照指纹须能在下达时重新验证，发现版本漂移则返回阻塞原因。
3. P9B、P9C 将库存临时实现替换为公共批次台账。试点依次执行收货、待检放行、预留、拣料、工序发料，再报工。发料分录负责库存扣减，报工引用已发料并记录实际消耗，两处通过稳定 ID 关联，避免同一投入被扣两次。
4. P9D 启用公共 WIP 和谱系。工序产出可独立登记或由报工原子生成，使用同一实绩标识去重。工序完工检查产出与检验条件；工单完工入库和关单结算分开。有关账前发现异常的工单沿冲销或补偿路径恢复。

## 数据与接口约束

- 主键使用 run 范围内的稳定 ID。主数据版本激活后不可覆写。下达快照固定 BOM、工艺、检验规格与生效依据。变更影响评估给出适用后续工单或需评估的 WIP。
- 数量携带规范单位，离散件使用整数，其他单位采用固定精度。预留独立于实际移动，冻结与在途量纳入可用量计算。冲销通过成对台账分录处理。任何下游耗用或序列号依赖阻止直接冲销，并返回依赖对象。
- 写命令关联 `idempotency_key`，对需要并发保护的实体检查 `expected_version`；同键同参返回提交证据，同键异参返回冲突。重试先查询幂等结果，再校验最新版本。读接口提供游标分页及明确的排序规则。
- 区分调用人身份、业务对象版本、业务写游标及 DES 事件游标。CNC 和 SMT 通过同一个服务执行公共约束，实际库存、工序和质量使用公共记录；生产、工艺和测试的行业扩展保留在行业适配层。禁止访问另一 run 的对象或诊断。
- 工具说明展示输入输出 schema、前置条件、阻塞原因和推荐可选后续动作。批量业务提供逐项结果，关键生产命令提供提交 ID。纠正使用明确的反向业务动作，保留审计和谱系。

## 验收及投入安排

先锁定每个批次的字段、拒绝代码、契约用例和 feature map，再实现。原 P8B–P9D 基线为 17–27 人周。首轮目录裁决后按新增工具、多场景验收、数据迁移和兼容成本重新估算。各批次通过当期及前序回归后发 PR。未执行或阻塞的用例保留状态与证据。

公共能力签收要求 CNC 使用共享服务走完两批次执行链，并以 SMT 的薄适配器重复同一套预留、发料、冻结和追溯契约。跨行业复用须用相同工具名和语义验证。通用工单、库存、质量及追溯能力通过后，才将 P10 的领域扩展算作公共能力之上的新增工作。

完整性判定按 [MCP 目录完整性标准](MCP_CATALOG_COMPLETENESS.md) 执行。

## 设置类 MCP 待办

生产动作与配置动作使用不同的权限和审计边界。将来由 AgentNexus 内服使用设置 MCP 自动配置模拟 MES，支持从初始配置到可下达工单的准备流程。工厂结构、工作中心、班次、编号、单位与策略默认值是待调研对象。下一次设计确认配置作用域、影响预览、生效与回滚契约，以及设置层和主数据层的分工。当前 83 项候选业务契约按生产业务统计，设置类工具单列待办。详见 [深化路线图](MES_EXPANSION_PLAN.md#待办--mes-设置-mcp)。
