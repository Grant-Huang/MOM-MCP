# 三源 Adapter 能力矩阵

基线为 `mom-external-adapter` 0.1.0，工单观测映射版本为 Odoo 与 Carbon 社区 REST `0.2`、OpenMES 与 Carbon 原生 MCP `0.1`。范围是按授权引用读取单张工单。矩阵供三源扩展联审，正式接线与短期准出以[验收台账](ADAPTER_READ.md)为准。

| 来源与验证版本 | Adapter 读取入口 | 已验证的源身份范围 | 工单观测映射 | 实际来源与宿主证据 |
|---|---|---|---|---|
| Odoo Community 19.0，镜像摘要 `03a47921f9eef2c225ded1167fe7828465d2b3177883f30d173bb1c3c792949b` | JSON-2 `mrp.production/search_read`，按记录 ID 读取 | 指定数据库及单公司制造用户。已知跨公司记录对该身份返回空数组 | `id`、`name`、`state`。保留源状态，`write_date` 以固定格式解析为 UTC `source_updated_at` | 本地实际应用及候选宿主通过，正式入口待首站联调 |
| OpenMES `faccc52a543382922aa8b7d9d79b89e6a30b28d3` | HTTP `GET /api/v1/work-orders/{id}` | 单产线 Operator。实际后端对跨产线记录返回 403，对缺失记录返回 404 | `data.id`、`order_no`、`status`。保留源状态，带偏移量的 `updated_at` 转成 UTC | 本地 Laravel HTTP 及候选宿主通过，正式入口待联调 |
| Carbon Community `05bd067c0a72ca9f0983f6278d2a7c9bb48b716c` | Supabase REST `GET /rest/v1/jobs`，以记录 ID 和公司筛选 | 普通用户短期会话与公开项目键。不同公司身份的双向隔离已由不带公司过滤的查询验证 | `id`、`jobId`、`status`、`updatedAt`。读取层核对 `companyId`，投影保留源状态，有效更新时间转为 UTC | 本地真实 Supabase 后端及候选宿主通过，正式入口待联调 |
| Carbon 原生 MCP，同一上游源码版本 | MCP `production_getJob`，固定单工单读取 | 声明生产查看权限，协议与权益拒绝已有夹具 | 解码 MCP 文本中的工单记录，保留源状态 | 授权实例、跨公司边界与工具拒绝分类待补，兼容性单独签收 |

三种首期来源支持重新读取以核对当前可见工单及其源状态。标准化状态当前为空。Adapter 的首期能力范围为单条只读查询，写入、回执核验、列表、订阅和事件续读待各自定义与验证。正式接口的来源探针及审批由宿主负责。[工单观测本体 v0.1](../WORK_ORDER_ONTOLOGY.md) · [真实来源和版本证据](../EXTERNAL_ADAPTER_DESIGN.md)

后续增加能力时，为每个来源登记入口及版本、要求的权限、业务可见范围、映射版本、查询／执行／核验支持和可复跑证据。状态等价须有进入条件、业务约束与反例。Odoo 销售关系候选、OpenMES 生产状态转换和 Carbon Job 生命周期各自评审，跨来源语义以各方证据为依据。
