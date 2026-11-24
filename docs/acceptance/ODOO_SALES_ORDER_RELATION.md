# Odoo 销售订单与制造单的直接关系候选

依据[工单观测本体约定](../WORK_ORDER_ONTOLOGY.md)。本材料供联审，关系发布依赖短期三源签收。

首个关系样本使用 Odoo Community 19.0 的 `sale_mrp`。制造单的 `sale_line_id` 指向原始销售订单行，该行的 `order_id` 指向销售订单。候选关系 `fulfills_sales_order` 从 `work_order` 指向 `sales_order`，内部保留销售订单行和读取路径作为证据。该关系表示制造需求归属，销售订单保有履约状态。订单显示号和制造单 `origin` 保留为业务文字。Odoo 还有通过 `reference_ids.sale_ids` 派生的关联，此切片只验证直接路径。 [制造单源码](https://github.com/odoo/odoo/blob/d36b9405fe5d2ede753c6d7eceb778ecef20f67b/addons/sale_mrp/models/mrp_production.py#L14) · [销售订单行源码](https://github.com/odoo/odoo/blob/d36b9405fe5d2ede753c6d7eceb778ecef20f67b/addons/sale/models/sale_order_line.py#L34)

三次按 ID 查询使用同一数据库、公司作用域和读取身份。每一跳核对响应记录及公司。两个端点保留各自对象类型及完整来源身份，工单号和销售订单号可在其他实例重复。当前只覆盖 Odoo 内部关系。跨系统关联需另有来自权威来源的匹配证据。

| 读取结果 | 离线契约断言 |
|---|---|
| 三跳各返回唯一且一致的范围内记录 | 生成带来源证据的有向关系 |
| 制造单的 `sale_line_id` 为空 | 记录直接关系缺失。即使 `origin` 与订单显示号相同，也只保留业务文字 |
| 任一记录不可见或不属于绑定公司 | 返回未解析，结果省略目标源 ID |
| 精确查询返回多条记录或返回不同 ID | 记录冲突并停止投影 |
| 关联载荷畸形或记录 ID 无法无损解析 | 记录无效源响应 |

运行 `node --test docs/references/mcp/odoo-sales-order-relation.test.mjs` 执行候选契约。用例使用三个精确查询的响应夹具。上线验收还需验证真实读身份、宿主公开关系、跨系统目标匹配，以及查询间的变化处理。

## Odoo 本地 JSON-2 复核

使用 Odoo Community 19.0 镜像 `odoo@sha256:03a47921f9eef2c225ded1167fe7828465d2b3177883f30d173bb1c3c792949b` 和 PostgreSQL 16，安装 `sale_mrp`，关闭演示数据。用合成数据在两个公司各准备一条直接关联链，再在第一家公司准备一张仅填写 `origin` 的制造单。给第一家公司专属的读取身份授予制造用户及本人销售订单的访问权限。

以同一 Bearer 凭证和 `X-Odoo-Database`，依次向 `/json/2/<model>/search_read` 发送按 `id` 精确匹配的请求。三个模型均读取 `id` 和 `company_id`，制造单额外读取 `sale_line_id`，销售订单行额外读取 `order_id`。现场记录显示，本公司三跳均返回唯一匹配记录。仅填写 `origin` 的制造单返回 `sale_line_id: false`。相同身份按已知 ID 逐个查询另一公司的三条记录，均返回空数组。证据范围为本地 Odoo JSON-2 和合成业务数据。
