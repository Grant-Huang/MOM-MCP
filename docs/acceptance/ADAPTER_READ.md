# 三源工单读取短期验收候选

依据 [产品目标](../FEATURES.md#双主轴需求与验收目标)、[两线接口边界](../DESIGN.md#两条主线的接口边界) 和 [双主轴路线](../IMPLEMENTATION_PLAN.md#双主轴路线与准出)。此表用于短期证据复查和宿主接口联审。正式 Fxx、用例 ID 和 feature map 登记随契约冻结。

## 同一业务意图

在已授权生产世界中按一个不透明引用观察一张工单。Odoo 制造单、OpenMES 工单和 Carbon Job 使用各自的受权绑定。验收读取同范围工单、另一范围已确认存在的工单，并复查错误和来源。成功结果必须保留公开来源别名、工单引用、显示号、源状态和映射版本。使用相同显示号的样本检查来源隔离。

| 断言 | 准出证据 |
|---|---|
| 绑定与授权 | 拒绝匿名、过期或撤权身份、跨世界引用和未知引用。来源调用数为零。 |
| 可追溯读取 | 返回唯一记录，核对对象引用、来源、原始状态和观测时间。未知规范状态呈现为未映射。 |
| 来源范围 | 同一源身份查询范围外的已知记录。核对公开不可见结果及受信侧范围记录。 |
| 故障及来源隔离 | 模拟已配置源停机，返回稳定错误。记录其他外部来源及 Mock 调用数为零。 |
| 可复查记录 | 记录源版本、传输、角色范围、宿主与适配器提交和脱敏结果。真实来源、夹具和宿主桩证据分别登记。 |

## 证据进度

| 来源 | 真实来源接口 | AgentNexus 候选接线 | 正式生产接口 |
|---|---|---|---|
| Odoo JSON-2 | 本地 Community 19.0 实际应用验证范围内读取和公司隔离 | 工作区候选已通过 200／404 | 待合并链路验收 |
| OpenMES HTTP | 本地 Laravel 实际应用验证范围内读取和产线隔离 | 工作区候选已通过 200／404 | 待合并链路验收 |
| Carbon 社区版 REST | 隔离 PostgreSQL／PostgREST 执行上游视图与公司读取策略，通过真实数据库层复核。完整 Carbon 栈及真实登录会话待验证 | 工作区候选对隔离接口通过 200／404 | 待合并链路验收 |
| Carbon 原生 MCP | 回环验证固定协议与结果映射，具备权益的授权实例待验证 | 工作区候选已通过协议夹具 | 兼容性证据待补 |

适配器回环共用入口可执行 `cargo test --locked --test external_observe one_observe_entry_reads_three_authorized_worlds -- --exact`。另执行 `cargo test --locked --test external_observe` 验证逐请求授权、配置与故障隔离。工作区候选证据使用 `/api/world/production/observe`，依赖适配器提交 `ea1fcd8` 和宿主提交 `7a680bf`。用 `scripts/accept-m3-observe.py` 复跑，覆盖 Odoo、OpenMES、Carbon 原生 MCP、Carbon REST 四条回环传输及拒绝、撤权和故障隔离。Carbon 社区版数据路径作为短期取证候选，原生 MCP 兼容性证据单列。MOM-MCP `fbd798a` 的三个适配器 Rust 集成套件共 11 项通过，Carbon 原生探针与本体离线用例共 11 项通过。

Carbon 社区后端的本地复跑完成依赖准备并拉齐 API 网关、认证、PostgREST 等镜像。Docker 的 VFS 在解包固定版本的 Supabase PostgreSQL 镜像时再次返回 `no space left on device`。本次完整后端的迁移、真实登录和读取尚无通过证据。

## AgentNexus 正式接口联审输入

AgentNexus `c74b144` 已合入 M3-2，并在 `/api/world/prod` 实现 Bind、Observe、Subscribe 和生产桩水位。桩 Observe 接收绑定的 `world_id` 并返回设备观测。在该提交执行 `cargo test --locked -p an-server --test mom_world_prod`，`RT-WAPROD-01/02` 两项通过。三源工单读取接入以正式 World Adapter 接口为目标。 [M3 任务卡](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/c74b144/docs/mom/WA_M3_TASKS.md) · [M3-2 证据](https://github.com/Grant-Huang/multi-agent-conv-mvp/blob/c74b144/docs/testing/evidence/wa-m3-2-green.md)

| 联审项 | 首个切片待决内容 |
|---|---|
| 工单选择 | 正式桩请求只接受 world_id 与 request_id。联审确认单工单引用选择方式及现有绑定后观测的兼容策略。 |
| 生产身份与站点 | 宿主确认调用主体、租户、站点、权威源和工单引用。适配器从已授权绑定取得读取目标。正式源凭据及出站接线依宿主升级方案评审。 |
| 来源与时间 | 保留 `provenance`、来源及观测时间。区分已验证的源更新时间、读取时点和可续读的发布水位。单次源读取的续读能力待显式声明。 |
| 错误 | 对照正式错误包络冻结授权拒绝、不可见、源故障及响应错配。Carbon 原生工具拒绝仍需源端证据完成分类。 |
| 验收边界 | 三源实测和正式宿主串联用于短期准出。事件续读、多源延迟和浏览器准出按 AgentNexus M3 后续任务卡独立验收。 |

两条主线完成短期验证后，提交对象引用、来源、版本、能力和错误语义的共同评审。Mock 证据以同一个 CNC run 核对下达、领料、部分报工、质量冻结、纠正及台账，再用装配复验。 [Mock 公共门槛](COMMON_MES_GATE.md)
