# 三源工单读取短期验收候选

依据 [产品目标](../FEATURES.md#双主轴需求与验收目标)、[两线接口边界](../DESIGN.md#两条主线的接口边界) 和 [双主轴路线](../IMPLEMENTATION_PLAN.md#双主轴路线与准出)。此表用于短期证据复查和宿主接口联审。正式 Fxx、用例 ID 和 feature map 登记随契约冻结。

## 同一业务意图

在已授权生产世界中按一个不透明引用观察一张工单。Odoo 制造单、OpenMES 工单和 Carbon Job 使用各自的受权绑定。验收读取同范围工单、另一范围已确认存在的工单，并复查错误和来源。成功结果必须保留公开来源别名、工单引用、显示号、源状态和映射版本。使用相同显示号的样本检查来源隔离。

| 断言 | 准出证据 |
|---|---|
| 绑定与授权 | 拒绝匿名、过期或撤权身份、跨世界引用和未知引用。来源调用数为零。 |
| 可追溯读取 | 返回唯一记录，核对对象引用、来源、原始状态和观测时间。Odoo 和 Carbon Community 的精确查询最多取两条，以检测多记录响应。未知规范状态呈现为未映射。 |
| 来源范围 | 同一源身份查询范围外的已知记录。核对公开不可见结果及受信侧范围记录。 |
| 故障及来源隔离 | 模拟已配置源停机，返回稳定错误。记录其他外部来源及 Mock 调用数为零。 |
| 可复查记录 | 记录源版本、传输、角色范围、宿主与适配器提交和脱敏结果。真实来源、夹具和宿主桩证据分别登记。 |

## 证据进度

[三源 Adapter 能力矩阵](ADAPTER_CAPABILITIES.md) 汇总源版本、权限、映射和正式入口证据。 [三源工单首个状态转换](ADAPTER_LIFECYCLE.md) 记录状态对照与审查后待补的验收项。

| 来源 | 真实来源接口 | AgentNexus 候选接线 | 正式接口真实来源 |
|---|---|---|---|
| Odoo JSON-2 | 本地 Community 19.0 实际应用验证范围内读取和公司隔离 | 工作区候选已通过 200／404 | 待获批联调 |
| OpenMES HTTP | 本地 Laravel 实际应用验证范围内读取和产线隔离 | 工作区候选已通过 200／404 | 待获批联调 |
| Carbon 社区版 REST | 固定版本 Carbon 社区后端完成迁移，真实 Supabase 登录双向验证公司隔离 | 工作区候选经同一真实会话通过 200／404 | 待获批联调 |
| Carbon 原生 MCP | 回环验证固定协议与结果映射，具备权益的授权实例待验证 | 工作区候选已通过协议夹具 | 兼容性证据待补 |

适配器回环共用入口可执行 `cargo test --locked --test external_observe one_observe_entry_preserves_source_time_across_authorized_worlds -- --exact`。另执行 `cargo test --locked --test external_observe` 验证逐请求授权、配置与故障隔离。 共用入口的序列化验收覆盖 Odoo、OpenMES 和 Carbon 社区 REST，核对 `source_updated_at` 统一为 UTC、与读取时点 `observed_at` 独立，并按来源发布映射版本。无源时间的响应及 Carbon 原生 MCP 路径省略该字段。此证据来自 MOM-MCP 回环测试。工作区候选证据使用 `/api/world/production/observe`，依赖适配器提交 `ea1fcd8` 和宿主提交 `7a680bf`。用 `scripts/accept-m3-observe.py` 复跑，覆盖 Odoo、OpenMES、Carbon 原生 MCP、Carbon REST 四条回环传输及拒绝、撤权和故障隔离。Carbon 社区版数据路径作为短期取证候选，原生 MCP 兼容性证据单列。MOM-MCP `fbd798a` 的三个适配器 Rust 集成套件共 11 项通过，Carbon 原生探针与本体离线用例共 11 项通过。

执行 `cargo test --locked --test external_observe source_failures_keep_their_classification_without_fallback -- --exact` 复跑三源失败矩阵。Odoo 空结果、OpenMES 对象级 403 和 Carbon REST 空结果均返回不可见。OpenMES 无效正文或记录错配、Carbon REST 跨公司记录错配均返回响应无效。Odoo、OpenMES 和 Carbon REST 的上游 401 均返回来源不可用，Carbon REST 的上游 503 同样归为来源不可用。十种失败各触发一次已绑定来源请求，备用站点请求数为零。 同一用例还验证宿主绑定中的三种无效源工单编号，分别是 OpenMES 前导零、Odoo 超出整数范围和 Carbon REST 含逗号。候选入口报告来源不可用，已绑定来源与备用站点均零调用。此为 MOM-MCP 候选 Observe 回环证据。

运行 `cargo test --locked --test external_observe interrupted_source_response_is_unavailable_without_fallback -- --exact` 复跑响应中断。绑定来源发送 HTTP 200 和一段可解析为工单的 JSON，但在达到声明的正文长度前关闭连接。候选 Observe 返回来源不可用，备用站点零调用，且未发布工单快照。

运行 `cargo test --locked --test external_http reads_and_classifies_odoo_and_openmes_http_responses -- --exact` 复跑数值工单 ID 边界。OpenMES 回环接口往返验证 `18446744073709551615`（`u64::MAX`）的完整路径和响应身份，Odoo 拒绝同一超出有符号范围的绑定。OpenMES 拒绝溢出、零、显式加号和前导零。此证据验证 Adapter 传输，现场数据库可分配的 ID 范围由来源决定。

运行 `cargo test --locked --test external_observe carbon_native_rejections_do_not_read_community_rest -- --exact` 复跑 Carbon 原生 MCP 失败隔离。初始化返回 402 时只调用一次初始化。工具返回 `isError: true` 时完成初始化并调用一次固定读取工具。两条路径在候选 Observe 均返回来源不可用，已配置的 Carbon 社区 REST 零调用。上游错误正文不进入 Observe 结果。回环覆盖协议响应和隔离，授权实例的真实工具拒绝语义仍待核对。

运行 `cargo test --locked --test external_observe reads_again_after_source_recovery_but_stops_after_revocation -- --exact`。同一工单第一次读取遇到来源故障。来源恢复后，再次请求读取原绑定并返回当前源状态。撤销宿主授权后，再次请求被拒绝，来源调用数保持不变。全过程备用站点零调用。该项是候选入口回环证据。

缺少世界标识或工单引用时，工单详情和 Odoo 销售单关系查询都应拒绝请求。两条入口均在调用授权回调与访问来源前停止。运行 `cargo test --locked --test external_observe incomplete_order_selection_stops_before_authorization_and_source_access -- --exact` 复跑。

同一个工单引用和源记录编号可在不同租户、站点和来源实例出现。候选读取入口按这三个绑定字段共同选择来源。已配置的作用域只访问自己绑定的来源，未配置的组合报告来源不可用，所有来源调用数保持原值。运行 `cargo test --locked --test external_observe routes_by_tenant_site_and_instance_when_work_order_ids_are_equal -- --exact` 复跑。

同一租户、站点和来源实例只允许一条来源配置。给 Carbon 原生接口和社区 REST 使用相同绑定时，候选入口初始化拒绝配置，两种登记顺序结果一致。运行 `cargo test --locked --test external_observe rejects_duplicate_configuration -- --exact` 复跑。

若多站点配置中混入无效来源地址，候选入口拒绝整组配置。回归覆盖远程明文地址、含路径的地址及含查询参数的地址，无论无效项先登记还是后登记都失败。运行 `cargo test --locked --test external_observe invalid_source_configuration_rejects_entire_registry -- --exact` 复跑。

两个站点可共用同一个来源地址。候选入口仍按授权绑定使用各自凭据。回环样本以相同查询引用和同一源记录编号交替读取两个站点，服务端按收到的凭据返回不同状态，三次读回均匹配相应站点。运行 `cargo test --locked --test external_observe shared_origin_reads_with_each_sites_bound_credential -- --exact` 复跑。

### PR 自动回归

GitHub Actions 的 `adapter-contracts` 作业在推送与 PR 上运行 Rust 传输、Observe、本体投影及 Odoo 关联测试，同时复跑 Carbon REST、Carbon MCP 和 Odoo 关联的离线探针用例。默认使用本地回环和合成数据。真实来源和正式 Observe 验收使用各自的获批环境，继续按本台账登记。

### Odoo 双身份来源复验

在固定镜像 `odoo@sha256:03a47921f9eef2c225ded1167fe7828465d2b3177883f30d173bb1c3c792949b` 和 PostgreSQL 16 上初始化 `mrp`，关闭演示数据。为两家公司分别建立一条合成 `draft` 制造单和仅属于该公司的制造用户。运行 MOM-MCP `edeebf4` 的 `external_order_smoke`，分别使用两套临时 API 凭据，并交换主身份和第二身份复跑。

两个方向均通过。主身份读到自己的工单并投影为源状态 `draft`，读取另一公司工单返回 `NotVisible`。第二身份在同一数据库按相同 ID 独立读取成功，确认工单存在。此记录是实际来源直连证据。正式 AgentNexus Observe 仍待首站获批后联调。

Odoo 生命周期脚本还撤销主读取身份的临时 API 密钥。原密钥再次读取原本可见的制造单时，JSON-2 返回 401，Rust Adapter 报告来源不可用，未返回工单。另一公司的独立会话仍可经 Adapter 读取自己的工单。此项是来源凭据撤销证据，正式 AgentNexus 服务身份的撤权仍在 M3 验收范围。 [复跑脚本](../references/mcp/odoo-adapter-lifecycle-smoke.sh)

### Odoo 首站联调交接

依据 [AgentNexus 首站审批](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/2432e4b8c4bf1175c78ebee3effc4e15ce4a497f/docs/mom/WA_M3_ODOO_PILOT_REVIEW.md)，首站使用 Odoo Community 19 的 JSON-2 单工单查询。Adapter 按绑定读取 `mrp.production/search_read`，保留 Odoo 源状态和映射版本。该审批使用的宿主固定依赖为 `5aa312939df2110ff3dda401b9021a0c1ad5a67f`。后续候选提交的兼容复跑可供宿主评审更新依赖时参考。该宿主提交的正式路由仍通过测试注入读取工单，返回 `provenance=fixture`。真实服务接线获批并部署后，单站探针要求成功响应为 `provenance=production`。

| 证据 | 采集方式 |
|---|---|
| 范围内工单 | 选定联调身份可见的测试制造单，记录预期源状态。由宿主为该单配置不透明引用 `visible`。 |
| 范围外工单 | 由另一公司身份确认第二张测试制造单存在，再用联调 Odoo 身份核对该单不可见。由宿主为该单配置探针可选择的引用 `hidden`，以便验收源端 404。另用未绑定引用验收宿主 403 和零来源调用。 |
| 正式入口 | 部署方批准并配置服务身份、世界绑定、Odoo 只读凭据引用和出站后，在宿主运行 `node scripts/accept-m3-live-observe.mjs --single-site <bindings.json>`。绑定文件包含一条 `vendor=odoo` 的记录，填入宿主的 `world`、`visible`、`hidden`、公开来源别名 `source` 和预期 `sourceStatus`。 |
| 可复查记录 | 登记宿主和 Adapter 的完整提交、Odoo 版本、脱敏的 401／200／404、请求关联记录及第二身份的样本确认。密钥通过部署方的密钥设施交付。 |

MOM-MCP `adfeffeef694f5dbbc21124820f1af15b98e7fda` 在固定镜像的 Odoo `19.0-20261112` 和 PostgreSQL 16 上通过 [生命周期脚本](../references/mcp/odoo-adapter-lifecycle-smoke.sh)。两家公司分别读到自己的制造单，并以独立身份确认另一方不可见工单存在。同一测试工单从 `draft` 确认到 `confirmed` 后可读回，源更新时间推进并按 UTC 投影。撤销主账号 API 密钥后，Odoo 返回 401，Adapter 返回脱敏的来源不可用，另一公司仍可读取自己的工单。此结果验证独立 Odoo 来源和 Adapter。

在 MOM-MCP `adfeffeef694f5dbbc21124820f1af15b98e7fda` 与 AgentNexus `2432e4b8c4bf1175c78ebee3effc4e15ce4a497f` 上，用 [宿主依赖复跑脚本](../../scripts/check-external-adapter-agentnexus.sh) 通过远端 Git 引入候选 Adapter。临时宿主锁文件仅替换 Adapter 版本。正式路由夹具 9 项、Adapter 回归 33 项通过，另 1 项真实 Odoo 销售单关联用例按配置跳过。宿主单站及三站探针的 6 项测试通过。该结果用于宿主兼容评审，正式入口真实来源仍为 0/3。

### OpenMES 来源令牌撤销

OpenMES 生命周期脚本撤销单产线 Operator 的临时读取令牌。该令牌再读原本可见的工单时返回 401，Rust Adapter 报告来源不可用。另一产线的见证凭据可继续读取自己的工单，且原工单对其仍不可见。证据范围为来源凭据。 [复跑脚本](../references/mcp/openmes-adapter-http-smoke.sh)

### Carbon 社区后端实测

Carbon 固定提交为 `05bd067c0a72ca9f0983f6278d2a7c9bb48b716c`。用官方社区开发栈的隔离后端启动认证、网关、PostgREST 和 PostgreSQL，执行上游迁移。Docker VFS 的多层解包耗尽空间。本地复跑从原固定摘要重打包 PostgreSQL 17.6.1.166 为单层镜像，沿用文件系统、入口及运行环境，启动时使用该本地镜像。

用上游 `seed-dev.ts` 的 satellite 数据集为两个独立公司准备合成工单。两个开发用户分别经 Supabase `/auth/v1/token?grant_type=password` 登录。探针验证两次本公司记录及两次跨公司空结果。Rust 读取器随后通过范围内记录投影与范围外不可见断言。同一用户和工单经候选 AgentNexus Observe 复验，范围内为 200，范围外为 `404 order_not_visible`。正式 `/api/world/prod` 的真实来源接线待验收。

Carbon 社区版补充 [双会话隔离探针](../references/mcp/README.md#carbon-社区版公司隔离取证)。两个会话先分别读到自己的工单，再交叉读取。查询省略公司过滤，用源端响应验证双向隔离。已在 PostgreSQL 16／PostgREST 12.2.12 中复跑双会话。夹具沿用 Carbon 的视图与工单读取规则，用本地签发的测试 JWT 验证两次范围内读取和两次跨公司空结果。候选宿主仍返回范围内 200 和范围外 404。

Carbon 社区 REST 的双身份探针可额外接收 `MOM_CARBON_EXPIRED_A_TOKEN`。使用公司 A 工单已通过有效会话确认可见后，探针用其过期会话读取同一 ID，要求返回 HTTP 401。复验在回环隔离后端完成，测试凭据保留原会话身份，用隔离实例的签名配置将过期时间置为过去。Rust `external_order_smoke` 用同一绑定分别传入有效、过期和有效令牌，结果依次为成功、脱敏的来源不可用、成功。双向跨公司查询仍返回空结果。正式宿主服务身份撤权另行验收。[探针](../references/mcp/carbon-community-smoke.mjs)

### 三源凭据失效对照

| 来源 | 真实接口验收 | Adapter 结果 |
|---|---|---|
| Odoo JSON-2 | 删除隔离实例的主读取密钥，读取原可见制造单返回 401。另一公司见证密钥继续可用。 [生命周期脚本](../references/mcp/odoo-adapter-lifecycle-smoke.sh) | 来源不可用 |
| OpenMES HTTP | 删除隔离实例的主 Operator 令牌，读取原可见工单返回 401。另一产线见证身份继续可用。 [生命周期脚本](../references/mcp/openmes-adapter-http-smoke.sh) | 来源不可用 |
| Carbon 社区 REST | 用过期会话读取已证实可见的工单返回 401。有效会话仍可读取。 [双会话探针](../references/mcp/carbon-community-smoke.mjs) | 来源不可用 |

候选 Observe 回环对三条上游 401 路径均验证来源不可用和备用站点零调用。正式宿主收到失效调用身份时的拒绝及零来源调用，按 M3 宿主验收单独取证。

## AgentNexus 正式接口联审输入

AgentNexus `175e07b` 已包含 PR #228 的 M3 桩路径准出。正式接入需要从可访问的 Git 提交解析独立 adapter crate。先完成 [宿主 Git 依赖复跑](../EXTERNAL_ADAPTER_DESIGN.md#宿主依赖复跑)，再评审真实身份、凭据及出站配置，并经正式 Observe 运行三源验收。另备 [AgentNexus 整仓依赖复跑](../EXTERNAL_ADAPTER_DESIGN.md#agentnexus-整仓依赖复跑)，从干净的宿主提交读取正式 M3 合同和 adapter 夹具测试。 [M3-5 准出证据](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/175e07b/docs/testing/evidence/wa-m3-5.md)

AgentNexus `c74b144` 已合入 M3-2，并在 `/api/world/prod` 实现 Bind、Observe、Subscribe 和生产桩水位。桩 Observe 接收绑定的 `world_id` 并返回设备观测。在该提交执行 `cargo test --locked -p an-server --test mom_world_prod`，`RT-WAPROD-01/02` 两项通过。三源工单读取接入以正式 World Adapter 接口为目标。 [M3 任务卡](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/c74b144/docs/mom/WA_M3_TASKS.md) · [M3-2 证据](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/c74b144/docs/testing/evidence/wa-m3-2-green.md)

AgentNexus `47d8557` 已经由 PR #220 合入 M3-3。正式生产桩增加跨范围拒绝、过期游标恢复、分源延迟以及关闭写动作。用 `cargo test --locked -p an-server --test mom_world_prod` 本地复验，6 项通过。正式宿主的真来源身份、工单选择和出站方案仍按 M3 升级边界联审。 [M3-3 证据](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/47d8557/docs/testing/evidence/wa-m3-3-green.md)

AgentNexus #248 已合并正式 `/api/world/prod/observe` 路由夹具。其测试以本仓库 #63 合并提交 `5aa312939df2110ff3dda401b9021a0c1ad5a67f` 固定 `mom-external-adapter` 0.1.0。8 项宿主契约覆盖原 M3 桩合同、显式夹具门控、工单授权拒绝零调用、三源读取、范围外不可见及单源故障。夹具响应标为 `provenance=fixture`。[AgentNexus #248](https://github.com/Grant-Huang/multi-agent-conv-mvp/pull/248)

AgentNexus #251 已合并单站点探针，默认模式仍要求三源。候选入口实际来源 3/3，正式入口实际来源 0/3。建议首站采用已有 Community 19 JSON-2 实测的 Odoo。MOM-MCP 提供固定依赖、来源映射及范围内和范围外样本的证据。正式身份、工单绑定、凭据和出站由宿主负责审批与配置。首站通过后记录宿主与 Adapter 提交、源端版本、脱敏的 401／200／404 结果及范围外样本的独立确认。OpenMES 和 Carbon 社区版分别完成同等联调后再执行三源联合验收。[AgentNexus #251](https://github.com/Grant-Huang/multi-agent-conv-mvp/pull/251)

| 联审项 | 当前证据及待决内容 |
|---|---|
| 工单选择 | 正式路由夹具已验证可选 `selection`、单工单引用以及既有桩合同。真实世界绑定仍由宿主联调确认。 |
| 生产身份与站点 | 宿主确认调用主体、租户、站点、权威源和工单引用。适配器从已授权绑定取得读取目标。正式源凭据及出站接线依宿主升级方案评审。 |
| 来源与时间 | 保留 `provenance`、来源及观测时间。区分已验证的源更新时间、读取时点和可续读的发布水位。单次源读取的续读能力待显式声明。 |
| 错误 | 对照正式错误包络冻结授权拒绝、不可见、源故障及响应错配。Carbon 原生工具拒绝仍需源端证据完成分类。 |
| 验收边界 | 三源实测和正式宿主串联用于短期准出。事件续读、多源延迟和浏览器准出按 AgentNexus M3 后续任务卡独立验收。 |

两条主线完成短期验证后，提交对象引用、来源、版本、能力和错误语义的共同评审。Mock 证据以同一个 CNC run 核对下达、领料、部分报工、质量冻结、纠正及台账，再用装配复验。 [Mock 公共门槛](COMMON_MES_GATE.md)


## Adapter 与 Mock 工单读取意图预验收

运行 `cargo test --locked --test stdio adapter_and_mock_read_the_same_work_order_intent -- --exact`。 GitHub Actions 的 Acceptance 工作流在 `adapter-contracts` 中运行同一条命令。在同一验收用例中，Mock 走实际 stdio MCP 工具发现及 `get_work_order`，Adapter 走授权路由以及 OpenMES、Odoo 和 Carbon 社区 REST 回环接口。各路径读取相同显示号 `WO-1`，保留 Mock 的 `planned`、OpenMES 的 `ACCEPTED`、Odoo 的 `confirmed` 和 Carbon 的 `Ready`。Adapter 输出来源实例及世界内不透明引用，规范状态为空。Mock 缺失工单、来源对象不可见及来源不可用分别校验各自结果。正式联调仍需由共同宿主验证跨世界授权、来源标识、能力声明和公开错误包络。
