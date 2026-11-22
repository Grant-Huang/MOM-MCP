# MES MCP 目录完整性标准

“完整”指约定范围内的制造业务可以由外部 Agent 独立完成，并处理拒绝、异常、恢复和追溯。每个企业场景均由 mock 实现，工具对外只提供中性的制造业务契约。软件仓库、运维文档和评测端可说明仿真实现。

## 参考项目与工具边界

- **Carbon** 的清单用于发现候选业务，审查以服务执行路径为准。以 `getJobReleaseReadiness`、`releaseJobs`、`completeOperation`、`completeJob` 为例，预检、下达、工序完成和工单完成有独立状态、权限与副作用。批量业务必须说明逐项结果，部分成功必须可查和可补偿。 [生产服务](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/production/production.server.ts)
- **Carbon** 区分库存收货与批次绑定、拣料与撤拣、转储与库存纠正。质量也区分测量、部分接收、拒收和不合格处理。存在独立业务意图、权限、状态变化或纠正路径时，应列为独立工具。仅字段不同的同一状态编辑可合为一个受约束命令。 [库存服务](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/inventory/inventory.server.ts) · [质量服务](https://github.com/crbnos/carbon/blob/2f5b652498539be79372635666f1ccc45f5cb601/apps/erp/app/modules/quality/quality.server.ts)
- **qcadoo** 用明确的工单迁移和报工纠正检查状态覆盖。**OpenMES** 用工单快照生效点与处置事务检查版本和质量覆盖。**WebErpMesv2** 用多订单套料、实际耗料和余料回库检查钣金边界。 [qcadoo 工单状态](https://github.com/qcadoo/mes/blob/625afad7a8adaaa65b5b8b7bd7867af21aafc960/mes-plugins/mes-plugins-orders/src/main/java/com/qcadoo/mes/orders/states/constants/OrderState.java) · [OpenMES 工单快照](https://github.com/Mes-Open/OpenMes/blob/faccc52a543382922aa8b7d9d79b89e6a30b28d3/backend/app/Services/WorkOrder/WorkOrderSnapshotService.php) · [WebErpMesv2 套料](https://github.com/SMEWebify/WebErpMesv2/blob/8fa6c05452ce5a5e3d0bf68ef1592446c1858a54/app/Services/Stock/NestRecorder.php)

P8/P9 的分层规则、拆并条件与场景反证见 [MCP 分层审查](MCP_LAYERING_REVIEW.md)。逐项裁决后再确定公共目录。

## 目录怎么列

用下表建立 **业务对象 × 生命周期** 矩阵。每格列出业务意图、工具名、可用场景和验收 ID。每格状态为已验收、待实现、范围外；范围外须有理由。跨行业同义的动作以同一名称和 schema 出现，例如 CNC 与 SMT 共用 `mes.reserve_material`，具体允用批次规则由领域配置决定。

| 业务对象 | 发现、预检 | 正向状态与动作 | 拒绝、恢复和纠正 | 审计、追溯 |
|---|---|---|---|---|
| 工单/工序 | 列表、详情、就绪检查、派工队列、能力检查 | 创建、下达、派工、开工、报工、完工、关单 | 缺料/缺资质、暂停恢复、改派、取消、冲销 | 快照、实绩、逐项批量结果、命令状态 |
| 物料/库存/WIP | 查余额、批次、库位、预留、WIP | 收货、检验放行、预留、拣料、发料、转储、拆合、入库 | 待检冻结、撤拣、释放预留、退料、库存纠正 | 移动台账、实际批次、投入产出和正反向谱系 |
| 主数据/版本 | 查物料、BOM/BOP、检验规格与有效期 | 创建、验证、激活及工单快照 | 非法引用、版本冲突、工程变更影响及回退策略 | 版本历史、旧工单快照、生效依据 |
| 质量 | 查计划、测量、冻结、不合格单 | 建检、记录、完检、部分接收、处置、复检、放行 | 超限、拒收、冻结、返工、报废、授权让步 | 原始测量、处置理由、放行证据与谱系 |
| 资源 | 查班次、工作中心、设备、人员/工装能力 | 排队、派工、占用、解除占用、维修发起 | 资源竞争、停机、改派和恢复 | 资源占用历史、工时、停机和处置记录 |

行业扩展增加专有业务链，例如 SMT 的料卷拆接与召回、装配的配置 BOM 与终测、钣金的余料、线束的端点压接。每条链必须能通过公共工具和行业工具连续验证，也须登记当前范围外能力，例如 APS 优化和真实外部系统接入。

## 单条契约及签收

每一行至少记录 `name`、动作语义、业务域、适用行业、权限/副作用、前置状态、输入输出 schema、错误码、落账对象、补偿与幂等规则、验收 ID。读工具写明分页和排序。批量工具写明逐项结果。普通目录与回包不得出现模拟模式、种子、模型、夹具、未来事件或评测答案；公开错误使用业务对象和原因。

一个能力只有在**可搜索、可描述、可执行、正常流程可验证、失败无未解释的副作用、可根据规则纠正或恢复**后计为已验收。每个启用场景的 feature map 都要覆盖所属矩阵。P8–P9 拟定的 83 项契约是初始清单，按矩阵补缺、合并重叠并记录逐条验收状态，再定上线数量。 [P8–P9 候选工具](COMMON_MES_DESIGN.md) · [公共验收](acceptance/COMMON_MES_GATE.md)

## 首轮审计发现

| Carbon 语义 | P8–P9 初始清单 | 审计处置 |
|---|---|---|
| `getJobReleaseReadiness` / `releaseJobs` | 已列预检、单笔下达和批量下达 | 分别以无写预检与按工单逐项执行验收 |
| `createProductionQuantity` / `completeOperation` | 已列工序报工、完工及冲销 | 验证部分报工留下在制状态，完工与纠正有独立门禁 |
| `pickPickingListLineChecked` / `unpickStockTransferLine` / `correctStockMovement` | 已列拣料、撤拣和库存冲销 | 验证可用量、移动台账和原分录关联 |
| `partialInspection` / `rejectInspection` | 有完检及不合格处置，部分接收尚无单独候选 | 冻结 P8D/P9B 前明确分批处置、余量冻结及拒收过账契约 |
| 检验、库存移动、资源与版本查询 | 初始清单已有基础查询 | 仍须查漏不合格单与冻结列表、库位能力、班次和资源占用历史，并逐项定范围 |

这些差异属于下一次契约冻结输入。每项状态必须给出可追溯的来源、所属业务流程、工具名和验收 ID，或记录范围外的原因。

目录审计同时覆盖 [长期运营测试模拟](LONG_RUNNING_MES_VALIDATION.md) 的跨班任务、异常叠加、恢复和审计。场景能单次跑完是入口，连续状态和未决事项接续由独立验收矩阵签收。
