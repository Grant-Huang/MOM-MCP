# 工单观测本体约定 v0.1

候选版本，待 MOM-MCP 与 AgentNexus M3 联审。覆盖可追溯的单工单读取，配合 [Observe 契约](EXTERNAL_ADAPTER_DESIGN.md#m3-observe-联审契约) 使用。

v0.1 是首期工单观测切片。中期按来源证据增补对象、关系、状态解释和能力。每个新增定义记录业务含义、适用范围、来源映射版本、反例以及兼容策略。源证据不足时保留原值和映射诊断。Mock 的跨工艺验证可以提出候选概念，跨系统等价仍需各来源证据。 [扩展与联调节点](IMPLEMENTATION_PLAN.md#双主轴路线与准出)

## 对象与关系

| 概念 | 业务含义 |
|---|---|
| `work_order` | 一个计划或执行制造工作的生产对象。首期对齐 Odoo 制造单、OpenMES 工单与 Carbon Job。三者的执行、放行和结案规则由各来源保有。 |
| `operation` | 制造对象内的工序。与上级制造单保持类型化关系，首期详情读取不展开。 |
| `sales_order`、`customer_order` | 商业订单对象。只有来源提供可核验关联及其方向时才建立关系。订单号相同不足以证明关联。跨源关系留待后续切片。 |
| 来源与身份 | 业务身份由租户、站点、来源实例、源对象类型和源记录 ID 共同确定。Observe 对外使用世界内不透明引用。显示编号可重复。源记录 ID 以无损字符串存储，权限由绑定与运行时授权共同确定。 |

## 字段与状态

| 字段 | Odoo | OpenMES | Carbon |
|---|---|---|---|
| 源记录 ID | `id` | `data.id` | `id` |
| 显示号 | `name` | `data.order_no` | `jobId` |
| 源作用域 | 数据库与公司绑定 | 站点与产线绑定 | 来源实例与公司绑定 |
| 源状态 | `state` | `data.status` | `status` |
| 更新时间候选 | `write_date`，JSON-2 实测为不带偏移的 UTC | `data.updated_at` | 社区 REST 的 `jobs.updatedAt` 可空，原生 MCP 载荷待实测 |

- 每个观测携带来源引用、观测时点、源状态及映射版本。仅在来源提供可验证版本时返回对象版本。时间统一用 RFC 3339 UTC，来源时间不明确时省略该字段。
- 首轮 `draft`／`confirmed`、`PENDING`／`ACCEPTED`、`Draft`／`Ready` 保留各自源值。确认、接受、就绪对应的条件及副作用需要各自证据，当前 `normalized_status` 为空并给出 `unmapped`。Carbon 的状态样例来自迁移与夹具，待获授权实例读回。
- 规范状态发布须记录来源版本、进入条件、对后续操作的约束及反例。改变现有解释时升级映射版本。数量和单位需要源侧量纲及换算依据，首期单工单观测可省略。
- 不可见、未映射、响应无效和来源故障分别表示。普通调用方的不可见结果不暴露跨范围记录是否存在。来源故障不回退到模拟系统。

## 三系统最低验收

同一个生产 Observe 工单意图须在 Odoo JSON-2、OpenMES HTTP 和 Carbon 社区版 REST 上各执行一次成功读取，并验证来源追溯和源状态、来源内不可见、绑定或凭据失败，以及明确配置来源失败时零模拟调用。保留各来源版本、授权身份的范围和脱敏结果。接口夹具用于离线回归，运行时签收要求各自的实际接口和一次 AgentNexus 串联调用。Carbon 原生 MCP 的授权实例兼容性单独登记。[短期验收台账](acceptance/ADAPTER_READ.md)

### Odoo 源更新时间

固定 Odoo Community 19 与 PostgreSQL 16 的隔离复验将同一合成制造单的 `write_date` 设为已知 UTC 值。在默认上下文、`America/Vancouver` 和 `Asia/Tokyo` 下，用同一身份按同一 ID 查询 JSON-2 `search_read`，三次均返回相同的无偏移时间字符串，并与数据库基准吻合。可用 [Odoo 生命周期脚本](references/mcp/odoo-adapter-lifecycle-smoke.sh) 复跑。Odoo 映射 v0.2 将固定格式 `YYYY-MM-DD HH:MM:SS` 转成 RFC 3339 UTC 并输出为 `source_updated_at`。缺失或不符合格式时省略该字段，仍使用 v0.2。Observe 的 `observed_at` 记录 Adapter 读取时刻。 同一脚本将确认前后两次 JSON-2 源时间交给 Rust Adapter 示例，断言 `source_updated_at` 为 UTC 且表示相同时刻。确认后源时间推进。交换公司身份后，另一张仍为草稿的工单也核对其自身源时间。 [Odoo ORM 时间存储约定](https://www.odoo.com/documentation/19.0/developer/reference/backend/orm.html#date-time-fields)

### 快照时间与事件续读

`source_updated_at` 是来源报告的工单更新时间，`observed_at` 是 Adapter 读取时点。来源时间可以缺失，也可以在两次不同快照间相同或回退。每次授权读取都返回本次取得的快照，保留来源时间和状态。候选 Adapter 输出只承载快照观测。事件续读需要独立的来源排序、续读水位、重复投递和过期恢复契约。

运行 `cargo test --locked --test external_observe rereads_snapshots_when_source_time_is_equal_or_older -- --exact`。回环用例针对同一引用连续读取三次，检查状态变化和来源时间相同或回退时的返回值、每次一次来源调用，以及候选输出中省略游标及水位。

### 离线契约回归

运行 `cargo test --locked --test external_order`。用例调用实际 Adapter 的投影器，覆盖三源字段、映射版本、同号及跨作用域身份、记录 ID 精确匹配、缺失或畸形状态及单对象响应形状。数值 ID 在 Rust 中按整数保存并与绑定中的字符串精确匹配。OpenMES HTTP 工单路径接受正的无符号 64 位整数，Odoo JSON-2 查询使用正的有符号 64 位整数。两者的绑定均使用无前导零的十进制字符串。OpenMES 带偏移量的源时间统一为 UTC，Odoo 使用已验证的 UTC 规则，Carbon 社区 REST 将 `jobs.updatedAt` 中带时区的有效时间转为 UTC（映射 `0.2`），空值或无效时间省略。Carbon 原生 MCP 沿用映射 `0.1`。

输入从宿主已解析的绑定及传输层已分类的响应开始。身份元组用于内部断言，产品 Observe 对外使用不透明引用。Carbon 投影用合成记录验证。来源权限、传输包络、故障隔离和宿主接线按[短期验收台账](acceptance/ADAPTER_READ.md)复跑。
