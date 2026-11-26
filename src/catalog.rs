use rmcp::model::{Tool, ToolAnnotations};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::scenario::Rejection;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    IssueMaterialToOperation,
    ReturnUnusedMaterial,
    CreateStockTransfer,
    PostStockTransfer,
    ReverseInventoryMovement,
    ReserveMaterial,
    ListReservations,
    ReleaseReservation,
    PickMaterial,
    UnpickMaterial,
    ReleaseReceiptInspection,
    GetMaterialAvailability,
    GetMaterialRequirements,
    TraceLotForward,
    TraceSerialBackward,
    ListGenealogyEdges,
    GetWip,
    ReverseWipMovement,
    ReceiveWorkOrderOutput,
    ReconcileWorkOrder,
    SplitWip,
    MergeWip,
    ConsolidateWip,
    CompleteCoating,
    MoveWip,
    ListStockLocations,
    GetStockBalance,
    ListLots,
    GetLot,
    CreateReceipt,
    AssignReceiptTracking,
    PostReceipt,
    ListInventoryMovements,
    GetOrderSnapshot,
    AssessEngineeringChange,
    CreateRouteRevision,
    GetRouteRevision,
    SetRouteOperations,
    CreateBomRevision,
    GetBomRevision,
    SetBomComponents,
    ValidateRevision,
    ListItems,
    GetItem,
    CreateItem,
    CreateItemRevision,
    GetItemRevision,
    ActivateRevision,
    OpenNonconformance,
    PlaceQualityHold,
    DispositionNonconformance,
    ReleaseQualityHold,
    RecordMeasurement,
    CompleteInspection,
    CreateInspectionPlanRevision,
    ActivateInspectionPlanRevision,
    CreateInspection,
    ListInspections,
    GetInspection,
    GetWorkOrder,
    ListWorkOrders,
    ListOperations,
    GetOperation,
    ListDispatchQueue,
    GetResourceCapability,
    SetDispatchPriority,
    CreateWorkOrder,
    UpdateDraftOrder,
    GetReleaseReadiness,
    ReleaseWorkOrder,
    ReleaseWorkOrders,
    HoldWorkOrder,
    ResumeWorkOrder,
    CancelWorkOrder,
    CompleteWorkOrder,
    CloseWorkOrder,
    GetMaterial,
    HoldMaterial,
    HoldStockLot,
    ReleaseStockLotHold,
    MesStatus,
    ReleaseOrder,
    ReviseRoute,
    SetEquipment,
    DispatchOperation,
    ReassignOperation,
    SetOperationTool,
    IssueMaterial,
    StartOperation,
    PauseOperation,
    ResumeOperation,
    ReportOperation,
    PostOperationReport,
    ListOperationReports,
    ReverseOperationReport,
    CompleteOperation,
    GetCommandResult,
    RequestMaintenance,
    InspectUnits,
    Disposition,
    CloseOrder,
    Trace,
    ReverseReport,
    SmtRelease,
    SmtLoad,
    SmtSpliceReel,
    SmtPlace,
    SmtAoi,
    SmtReviewAoi,
    SmtSplit,
    SmtFinish,
    SmtStatus,
    SmtTrace,
    SmtIsolate,
    SmtGetRecallImpact,
    SmtRework,
    SmtRetest,
    SmtScrap,
    AssemblyKit,
    AssemblySubstitute,
    AssemblyDispatch,
    AssemblyBuild,
    AssemblyPriority,
    AssemblyStartTest,
    AssemblyFinishTest,
    AssemblyRepair,
    AssemblyDeliver,
    AssemblyTrace,
    GetAssemblyAsBuilt,
    SheetIssue,
    SheetNest,
    SheetCut,
    SheetForm,
    SheetInspect,
    SheetReturn,
    SheetTrace,
    CreateSheetNest,
    GetSheetNest,
    CutSheetNest,
    CancelSheetNest,
    ReturnSheetRemnant,
    TraceSheetMaterial,
    HarnessCut,
    CutHarnessBatch,
    GetHarnessCutBatch,
    CheckHarnessCrimpQualification,
    CrimpHarnessEndpoint,
    GetHarnessCrimpStatus,
    ApplyHarnessSampleHold,
    ReworkHarnessEndpoint,
    RecordHarnessContinuityTest,
    ReleaseHarnessSerial,
    HarnessCrimp,
    HarnessSample,
    HarnessTest,
    HarnessRework,
    HarnessRelease,
    HarnessTrace,
}

pub struct Gateway {
    pub name: &'static str,
    description: &'static str,
    read_only: bool,
    pub input: Value,
    pub control: bool,
}

impl Gateway {
    pub fn input_schema(&self, control: bool) -> Value {
        let mut schema = self.input.clone();
        if !control && self.name == "open_scenario" {
            schema["properties"]
                .as_object_mut()
                .expect("gateway input properties")
                .remove("seed");
        }
        schema
    }
}

#[derive(Serialize)]
pub struct Spec {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) domain: &'static str,
    pub(crate) effect: &'static str,
    pub input: Value,
    pub(crate) output: Value,
    pub(crate) preconditions: &'static [&'static str],
    pub(crate) side_effect: &'static str,
    pub(crate) errors: &'static [&'static str],
    pub(crate) visibility: &'static str,
}

pub(crate) fn object(required: &[&str], properties: Value) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

pub(crate) fn text() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256})
}

impl Action {
    pub fn named(name: &str) -> Option<Self> {
        [
            Self::IssueMaterialToOperation,
            Self::ReturnUnusedMaterial,
            Self::CreateStockTransfer,
            Self::PostStockTransfer,
            Self::ReverseInventoryMovement,
            Self::ReserveMaterial,
            Self::ListReservations,
            Self::ReleaseReservation,
            Self::PickMaterial,
            Self::UnpickMaterial,
            Self::ReleaseReceiptInspection,
            Self::GetMaterialAvailability,
            Self::GetMaterialRequirements,
            Self::TraceLotForward,
            Self::TraceSerialBackward,
            Self::ListGenealogyEdges,
            Self::GetWip,
            Self::ReverseWipMovement,
            Self::ReceiveWorkOrderOutput,
            Self::ReconcileWorkOrder,
            Self::SplitWip,
            Self::MergeWip,
            Self::ConsolidateWip,
            Self::CompleteCoating,
            Self::MoveWip,
            Self::ListStockLocations,
            Self::GetStockBalance,
            Self::ListLots,
            Self::GetLot,
            Self::CreateReceipt,
            Self::AssignReceiptTracking,
            Self::PostReceipt,
            Self::ListInventoryMovements,
            Self::GetOrderSnapshot,
            Self::AssessEngineeringChange,
            Self::CreateRouteRevision,
            Self::GetRouteRevision,
            Self::SetRouteOperations,
            Self::CreateBomRevision,
            Self::GetBomRevision,
            Self::SetBomComponents,
            Self::ValidateRevision,
            Self::ListItems,
            Self::GetItem,
            Self::CreateItem,
            Self::CreateItemRevision,
            Self::GetItemRevision,
            Self::ActivateRevision,
            Self::OpenNonconformance,
            Self::PlaceQualityHold,
            Self::DispositionNonconformance,
            Self::ReleaseQualityHold,
            Self::RecordMeasurement,
            Self::CompleteInspection,
            Self::CreateInspectionPlanRevision,
            Self::ActivateInspectionPlanRevision,
            Self::CreateInspection,
            Self::ListInspections,
            Self::GetInspection,
            Self::GetWorkOrder,
            Self::ListWorkOrders,
            Self::GetCommandResult,
            Self::RequestMaintenance,
            Self::ListOperations,
            Self::GetOperation,
            Self::ListDispatchQueue,
            Self::GetResourceCapability,
            Self::ReleaseWorkOrders,
            Self::CreateWorkOrder,
            Self::UpdateDraftOrder,
            Self::GetMaterial,
            Self::HoldMaterial,
            Self::HoldStockLot,
            Self::ReleaseStockLotHold,
        ]
        .into_iter()
        .find(|action| action.name() == name)
        .or_else(|| {
            crate::mes::TOOLS
                .iter()
                .chain(crate::smt::TOOLS)
                .chain(crate::assembly::TOOLS)
                .chain(crate::fabrication::tools())
                .find(|tool| tool.1 == name)
                .map(|tool| tool.0)
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::IssueMaterialToOperation => "issue_material_to_operation",
            Self::CreateStockTransfer => "create_stock_transfer",
            Self::ReverseInventoryMovement => "reverse_inventory_movement",
            Self::PostStockTransfer => "post_stock_transfer",
            Self::ReturnUnusedMaterial => "return_unused_material",
            Self::ReserveMaterial => "reserve_material",
            Self::ListReservations => "list_reservations",
            Self::ReleaseReservation => "release_reservation",
            Self::PickMaterial => "pick_material",
            Self::UnpickMaterial => "unpick_material",
            Self::ReleaseReceiptInspection => "release_receipt_inspection",
            Self::GetMaterialAvailability => "get_material_availability",
            Self::ReconcileWorkOrder => "reconcile_work_order",
            Self::ReceiveWorkOrderOutput => "receive_work_order_output",
            Self::SplitWip => "split_wip",
            Self::CompleteCoating => "complete_coating",
            Self::ConsolidateWip => "consolidate_wip",
            Self::MergeWip => "merge_wip",
            Self::ReverseWipMovement => "reverse_wip_movement",
            Self::MoveWip => "move_wip",
            Self::TraceLotForward => "trace_lot_forward",
            Self::TraceSerialBackward => "trace_serial_backward",
            Self::ListGenealogyEdges => "list_genealogy_edges",
            Self::GetWip => "get_wip",
            Self::GetMaterialRequirements => "get_material_requirements",
            Self::ListStockLocations => "list_stock_locations",
            Self::GetStockBalance => "get_stock_balance",
            Self::ListLots => "list_lots",
            Self::GetLot => "get_lot",
            Self::CreateReceipt => "create_receipt",
            Self::AssignReceiptTracking => "assign_receipt_tracking",
            Self::PostReceipt => "post_receipt",
            Self::ListInventoryMovements => "list_inventory_movements",
            Self::GetOrderSnapshot => "get_order_snapshot",
            Self::AssessEngineeringChange => "assess_engineering_change",
            Self::CreateRouteRevision => "create_route_revision",
            Self::GetRouteRevision => "get_route_revision",
            Self::SetRouteOperations => "set_route_operations",
            Self::CreateBomRevision => "create_bom_revision",
            Self::GetBomRevision => "get_bom_revision",
            Self::SetBomComponents => "set_bom_components",
            Self::ValidateRevision => "validate_revision",
            Self::ListItems => "list_items",
            Self::GetItem => "get_item",
            Self::CreateItem => "create_item",
            Self::CreateItemRevision => "create_item_revision",
            Self::GetItemRevision => "get_item_revision",
            Self::ActivateRevision => "activate_revision",
            Self::OpenNonconformance => "open_nonconformance",
            Self::PlaceQualityHold => "place_quality_hold",
            Self::DispositionNonconformance => "disposition_nonconformance",
            Self::ReleaseQualityHold => "release_quality_hold",
            Self::RecordMeasurement => "record_measurement",
            Self::CompleteInspection => "complete_inspection",
            Self::CreateInspectionPlanRevision => "create_inspection_plan_revision",
            Self::ActivateInspectionPlanRevision => "activate_inspection_plan_revision",
            Self::CreateInspection => "create_inspection",
            Self::ListInspections => "list_inspections",
            Self::GetInspection => "get_inspection",
            Self::GetWorkOrder => "get_work_order",
            Self::ListWorkOrders => "list_work_orders",
            Self::GetCommandResult => "get_command_result",
            Self::RequestMaintenance => "request_maintenance",
            Self::ListOperations => "list_operations",
            Self::GetOperation => "get_operation",
            Self::ListDispatchQueue => "list_dispatch_queue",
            Self::GetResourceCapability => "get_resource_capability",
            Self::ReleaseWorkOrders => "release_work_orders",
            Self::CreateWorkOrder => "create_work_order",
            Self::UpdateDraftOrder => "update_draft_order",
            Self::GetMaterial => "get_material",
            Self::HoldMaterial => "hold_material",
            Self::HoldStockLot => "hold_stock_lot",
            Self::ReleaseStockLotHold => "release_stock_lot_hold",
            _ => {
                crate::mes::TOOLS
                    .iter()
                    .chain(crate::smt::TOOLS)
                    .chain(crate::assembly::TOOLS)
                    .chain(crate::fabrication::tools())
                    .find(|tool| tool.0 == self)
                    .unwrap()
                    .1
            }
        }
    }

    fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::ReconcileWorkOrder
            | Self::GetOrderSnapshot
            | Self::AssessEngineeringChange
            | Self::GetWorkOrder
            | Self::ListWorkOrders
            | Self::CreateWorkOrder
            | Self::UpdateDraftOrder
            | Self::GetReleaseReadiness
            | Self::HoldWorkOrder
            | Self::ResumeWorkOrder
            | Self::CancelWorkOrder
            | Self::CompleteWorkOrder
            | Self::CloseWorkOrder
            | Self::ReleaseWorkOrders
            | Self::ReleaseWorkOrder => &["工单", "work order", "order"],
            Self::ListOperations | Self::GetOperation | Self::ListDispatchQueue => {
                &["工序", "派工", "operation", "dispatch", "queue"]
            }
            Self::CreateInspectionPlanRevision
            | Self::ActivateInspectionPlanRevision
            | Self::CreateInspection
            | Self::ListInspections
            | Self::GetInspection
            | Self::RecordMeasurement
            | Self::CompleteInspection
            | Self::OpenNonconformance
            | Self::PlaceQualityHold
            | Self::DispositionNonconformance
            | Self::ReleaseQualityHold => &["检验", "质量", "inspection", "quality"],
            action if action.is_reservation_action() => &["预留", "拣料", "reservation", "picking"],
            action if action.is_genealogy_action() => &["谱系", "追溯", "genealogy", "trace"],
            action if action.is_receipt_action() => &["库存", "收货", "inventory", "receipt"],
            action if action.is_route_action() => &["工艺路线", "route", "revision"],
            action if action.is_bom_action() => &["物料清单", "bom", "revision"],
            action if action.is_item_action() => &["物料", "版本", "item", "revision"],
            Self::GetMaterial => &["物料", "material", "batch", "批次", "inventory"],
            Self::HoldMaterial => &[
                "质量冻结",
                "隔离",
                "暂扣",
                "hold",
                "quality hold",
                "quarantine",
            ],
            _ => &[],
        }
    }

    pub(crate) fn is_item_action(self) -> bool {
        matches!(
            self,
            Self::ListItems
                | Self::GetItem
                | Self::CreateItem
                | Self::CreateItemRevision
                | Self::GetItemRevision
                | Self::ActivateRevision
        )
    }

    pub(crate) fn is_bom_action(self) -> bool {
        matches!(
            self,
            Self::CreateBomRevision
                | Self::GetBomRevision
                | Self::SetBomComponents
                | Self::ValidateRevision
        )
    }

    pub(crate) fn is_route_action(self) -> bool {
        matches!(
            self,
            Self::CreateRouteRevision | Self::GetRouteRevision | Self::SetRouteOperations
        )
    }

    pub(crate) fn is_receipt_action(self) -> bool {
        matches!(
            self,
            Self::ReleaseReceiptInspection
                | Self::GetMaterialAvailability
                | Self::GetMaterialRequirements
                | Self::ListStockLocations
                | Self::GetStockBalance
                | Self::ListLots
                | Self::GetLot
                | Self::CreateReceipt
                | Self::AssignReceiptTracking
                | Self::PostReceipt
                | Self::ListInventoryMovements
        )
    }
    pub(crate) fn is_reservation_action(self) -> bool {
        matches!(
            self,
            Self::IssueMaterialToOperation
                | Self::ReturnUnusedMaterial
                | Self::ReserveMaterial
                | Self::ListReservations
                | Self::ReleaseReservation
                | Self::PickMaterial
                | Self::UnpickMaterial
        )
    }
    pub(crate) fn is_genealogy_action(self) -> bool {
        matches!(
            self,
            Self::TraceLotForward | Self::TraceSerialBackward | Self::ListGenealogyEdges
        )
    }
    pub fn spec(self) -> Spec {
        if self == Self::CompleteCoating {
            return crate::wip::coating_spec();
        }
        if self == Self::ConsolidateWip {
            return crate::wip::consolidation_spec();
        }
        if matches!(self, Self::HoldStockLot | Self::ReleaseStockLotHold) {
            return crate::lot_quality::spec(self);
        }
        if self == Self::ReconcileWorkOrder {
            return crate::reconciliation::spec();
        }
        if self == Self::ReceiveWorkOrderOutput {
            return crate::finished_output::spec();
        }
        if self.is_genealogy_action() {
            return crate::genealogy::spec(self);
        }
        if matches!(self, Self::SplitWip | Self::MergeWip) {
            return crate::wip::partition_spec(self);
        }
        if self == Self::ReverseWipMovement {
            return crate::wip::reverse_spec();
        }
        if self == Self::MoveWip {
            return crate::wip::move_spec();
        }
        if self == Self::GetWip {
            return crate::wip::spec();
        }
        if matches!(
            self,
            Self::CreateStockTransfer | Self::PostStockTransfer | Self::ReverseInventoryMovement
        ) {
            return crate::transfers::spec(self);
        }
        if self.is_reservation_action() {
            return crate::reservations::spec(self);
        }
        if self.is_receipt_action() {
            return crate::inventory::spec(self);
        }
        if matches!(self, Self::GetOrderSnapshot | Self::AssessEngineeringChange) {
            return crate::snapshots::spec(self);
        }
        if self.is_route_action() {
            return crate::routes::spec(self);
        }
        if self.is_bom_action() {
            return crate::bom::spec(self);
        }
        if self.is_item_action() {
            return crate::items::spec(self, true, true);
        }
        let (description, domain, effect, input, fields, side_effect, errors) = match self {
            Self::CreateInspectionPlanRevision
            | Self::ActivateInspectionPlanRevision
            | Self::CreateInspection
            | Self::ListInspections
            | Self::GetInspection
            | Self::RecordMeasurement
            | Self::CompleteInspection
            | Self::OpenNonconformance
            | Self::PlaceQualityHold
            | Self::DispositionNonconformance
            | Self::ReleaseQualityHold => {
                let inspection_id = json!({"type":"string","minLength":1,"maxLength":64,
                    "pattern":"^[A-Za-z0-9_-]+$"});
                let row = object(
                    &[
                        "inspection_id",
                        "work_order",
                        "specification_version",
                        "status",
                    ],
                    json!({"inspection_id":inspection_id,"work_order":text(),"specification_version":text(),
                        "status":{"enum":["pending","passed","failed"]}}),
                );
                let cursor = json!({"type":"integer","minimum":0,"maximum":i64::MAX});
                let measurement = crate::quality::measurement_schema();
                match self {
                    Self::CreateInspectionPlanRevision | Self::ActivateInspectionPlanRevision => {
                        let creating = self == Self::CreateInspectionPlanRevision;
                        let mut required =
                            vec!["plan_id", "revision", "expected_cursor", "idempotency_key"];
                        let mut properties = json!({"plan_id":inspection_id,
                            "revision":{"type":"integer","minimum":1,"maximum":u32::MAX},
                            "expected_cursor":cursor,"idempotency_key":text()});
                        if creating {
                            required.push("characteristics");
                            properties["characteristics"] =
                                crate::quality::characteristics_schema();
                        }
                        (
                            if creating {
                                "创建检验规格版本"
                            } else {
                                "启用检验规格版本"
                            },
                            "quality",
                            "write",
                            object(&required, properties),
                            json!({"plan":crate::quality::plan_schema(),"cursor":cursor}),
                            self.name(),
                            &[
                                "INVALID_ARGUMENT",
                                "PLAN_REVISION_ALREADY_EXISTS",
                                "PLAN_REVISION_NOT_FOUND",
                                "PLAN_REVISION_NOT_ACTIVATABLE",
                                "VERSION_CONFLICT",
                                "IDEMPOTENCY_CONFLICT",
                            ][..],
                        )
                    }
                    Self::OpenNonconformance
                    | Self::PlaceQualityHold
                    | Self::DispositionNonconformance
                    | Self::ReleaseQualityHold => {
                        let description = match self {
                            Self::OpenNonconformance => "登记检验不合格",
                            Self::PlaceQualityHold => "冻结不合格工单的完工及放行",
                            Self::DispositionNonconformance => "提交复检、让步或报废处置",
                            _ => "解除已处置不合格的质量冻结",
                        };
                        let mut required = vec![
                            "inspection_id",
                            "responsible",
                            "reason",
                            "evidence",
                            "expected_cursor",
                            "idempotency_key",
                        ];
                        let mut properties = json!({"inspection_id":inspection_id,"responsible":text(),"reason":text(),
                            "evidence":text(),"expected_cursor":cursor,"idempotency_key":text()});
                        if self == Self::DispositionNonconformance {
                            required.push("decision");
                            properties["decision"] = json!({"type":"string","enum":["reinspection","concession","scrap"]});
                            properties["reinspection_id"] = inspection_id;
                        }
                        (
                            description,
                            "quality",
                            "write",
                            object(&required, properties),
                            json!({"nonconformance":crate::quality::nonconformance_schema(),"cursor":cursor}),
                            self.name(),
                            &[
                                "INVALID_ARGUMENT",
                                "INSPECTION_NOT_FOUND",
                                "FAILED_INSPECTION_REQUIRED",
                                "NONCONFORMANCE_STATE_INVALID",
                                "REINSPECTION_NOT_ACCEPTABLE",
                                "VERSION_CONFLICT",
                                "IDEMPOTENCY_CONFLICT",
                            ][..],
                        )
                    }
                    Self::RecordMeasurement => (
                        "登记检验特性的实测值及规格限值",
                        "quality",
                        "write",
                        object(
                            &[
                                "inspection_id",
                                "characteristic",
                                "value",
                                "unit",
                                "lower_limit",
                                "upper_limit",
                                "expected_cursor",
                                "idempotency_key",
                            ],
                            json!({"inspection_id":inspection_id,"characteristic":text(),"unit":text(),
                                "value":{"type":"number"},"lower_limit":{"type":"number"},
                                "upper_limit":{"type":"number"},"expected_cursor":cursor,"idempotency_key":text()}),
                        ),
                        json!({"measurement":measurement,"cursor":cursor}),
                        "record_measurement",
                        &[
                            "INVALID_ARGUMENT",
                            "INSPECTION_NOT_FOUND",
                            "INSPECTION_COMPLETED",
                            "MEASUREMENT_ALREADY_EXISTS",
                            "MEASUREMENT_LIMIT_REACHED",
                            "CHARACTERISTIC_NOT_IN_PLAN",
                            "MEASUREMENT_SPECIFICATION_MISMATCH",
                            "VERSION_CONFLICT",
                            "IDEMPOTENCY_CONFLICT",
                        ][..],
                    ),
                    Self::CompleteInspection => (
                        "按测量结果完成检验并确定结论",
                        "quality",
                        "write",
                        object(
                            &["inspection_id", "expected_cursor", "idempotency_key"],
                            json!({"inspection_id":inspection_id,"expected_cursor":cursor,"idempotency_key":text()}),
                        ),
                        json!({"inspection":row,"cursor":cursor}),
                        "complete_inspection",
                        &[
                            "INVALID_ARGUMENT",
                            "INSPECTION_NOT_FOUND",
                            "INSPECTION_COMPLETED",
                            "MEASUREMENT_REQUIRED",
                            "PLAN_MEASUREMENTS_INCOMPLETE",
                            "VERSION_CONFLICT",
                            "IDEMPOTENCY_CONFLICT",
                        ][..],
                    ),
                    Self::CreateInspection => (
                        "按工单与规格版本建立检验单",
                        "quality",
                        "write",
                        object(
                            &[
                                "inspection_id",
                                "work_order",
                                "specification_version",
                                "expected_cursor",
                                "idempotency_key",
                            ],
                            json!({"inspection_id":inspection_id,"work_order":text(),"specification_version":text(),"reinspection_of":inspection_id,
                                "expected_cursor":cursor,"idempotency_key":text()}),
                        ),
                        json!({"inspection":row,"cursor":cursor}),
                        "create_inspection",
                        &[
                            "INVALID_ARGUMENT",
                            "WORK_ORDER_NOT_FOUND",
                            "INSPECTION_ALREADY_EXISTS",
                            "PLAN_REVISION_NOT_FOUND",
                            "PLAN_REVISION_NOT_ACTIVE",
                            "INSPECTION_PLAN_SNAPSHOT_MISMATCH",
                            "RECEIPT_INSPECTION_NOT_PENDING",
                            "REINSPECTION_NOT_ACCEPTABLE",
                            "VERSION_CONFLICT",
                            "IDEMPOTENCY_CONFLICT",
                        ][..],
                    ),
                    Self::ListInspections => (
                        "按检验单编号分页查询，可按工单筛选",
                        "quality",
                        "read",
                        object(
                            &[],
                            json!({"work_order":text(),"after_inspection_id":inspection_id,
                            "limit":{"type":"integer","minimum":1,"maximum":100}}),
                        ),
                        json!({"inspections":{"type":"array","items":row},
                            "next_after_inspection_id":{"type":["null","string"]}}),
                        "none",
                        &["INVALID_ARGUMENT", "WORK_ORDER_NOT_FOUND"][..],
                    ),
                    _ => (
                        "按编号查询检验单",
                        "quality",
                        "read",
                        object(&["inspection_id"], json!({"inspection_id":inspection_id})),
                        json!({"inspection":row}),
                        "none",
                        &["INVALID_ARGUMENT", "INSPECTION_NOT_FOUND"][..],
                    ),
                }
            }
            Self::GetWorkOrder => (
                "查询工单",
                "production",
                "read",
                object(&["work_order"], json!({"work_order":text()})),
                json!({"work_order":text(),"status":{"type":"string"}}),
                "none",
                &["INVALID_ARGUMENT", "WORK_ORDER_NOT_FOUND"][..],
            ),
            Self::ListWorkOrders => (
                "按工单编号分页查询工单，可按生产状态筛选",
                "production",
                "read",
                object(
                    &[],
                    json!({"status":text(),"after_work_order":text(),
                    "limit":{"type":"integer","minimum":1,"maximum":100}}),
                ),
                json!({"orders":{"type":"array","items":object(&["work_order","status"],
                    json!({"work_order":text(),"status":text()}))},
                    "next_after_work_order":{"type":["null","string"]}}),
                "none",
                &["INVALID_ARGUMENT"][..],
            ),
            Self::ListOperations | Self::GetOperation | Self::ListDispatchQueue => {
                let row = object(
                    &[
                        "work_order",
                        "operation",
                        "sequence",
                        "status",
                        "work_center",
                        "blocking",
                        "priority",
                    ],
                    json!({"work_order":text(),"operation":text(),"sequence":{"type":"integer","minimum":1},
                    "status":{"enum":["pending","ready","dispatched","running","paused","blocked","held","completed","cancelled"]},
                    "work_center":{"type":["null","string"]},"blocking":{"type":["null","string"]},
                    "priority":{"type":"integer","minimum":0}}),
                );
                let (description, input, output) = match self {
                    Self::ListOperations => (
                        "查询工单工序、执行状态及阻塞",
                        object(&["work_order"], json!({"work_order":text()})),
                        json!({"operations":{"type":"array","items":row}}),
                    ),
                    Self::GetOperation => (
                        "按工单和工序查询执行详情",
                        object(
                            &["work_order", "operation"],
                            json!({"work_order":text(),"operation":text()}),
                        ),
                        json!({"operation":row}),
                    ),
                    _ => (
                        "按优先级查询待派工工序",
                        object(&[], json!({"work_center":text()})),
                        json!({"operations":{"type":"array","items":row}}),
                    ),
                };
                (
                    description,
                    "production",
                    "read",
                    input,
                    output,
                    "none",
                    &[
                        "INVALID_ARGUMENT",
                        "WORK_ORDER_NOT_FOUND",
                        "OPERATION_NOT_FOUND",
                    ][..],
                )
            }
            Self::GetResourceCapability => (
                "查询工作中心支持的工序与当前可用性",
                "production",
                "read",
                object(&["work_center"], json!({"work_center":text()})),
                json!({"resource":object(&["work_center","operations","configurations","available","occupied_by"],
                    json!({"work_center":text(),"operations":{"type":"array","items":text()},
                        "configurations":{"type":"array","items":text()},"available":{"type":"boolean"},
                        "occupied_by":{"type":["null","string"]}}))}),
                "none",
                &["INVALID_ARGUMENT", "RESOURCE_NOT_FOUND"][..],
            ),
            Self::RequestMaintenance => (
                "为已停机设备发起维修",
                "production",
                "write",
                object(
                    &["asset", "expected_cursor", "idempotency_key"],
                    json!({"asset":text(),
                    "expected_cursor":{"type":"integer","minimum":0},"idempotency_key":text()}),
                ),
                json!({"asset":text(),"status":{"const":"maintenance_requested"},"cursor":{"type":"integer","minimum":0}}),
                "schedule_equipment_maintenance",
                &[
                    "INVALID_ARGUMENT",
                    "ASSET_NOT_FOUND",
                    "INVALID_STATE",
                    "MAINTENANCE_PENDING",
                    "VERSION_CONFLICT",
                    "IDEMPOTENCY_CONFLICT",
                ][..],
            ),
            Self::GetCommandResult => (
                "按幂等键查询已提交的业务命令结果",
                "production",
                "read",
                object(&["idempotency_key"], json!({"idempotency_key":text()})),
                json!({"receipt":{"type":"object"}}),
                "none",
                &["INVALID_ARGUMENT", "COMMAND_RESULT_NOT_FOUND"][..],
            ),
            Self::ReleaseWorkOrders => (
                "批量下达工单并逐项返回提交结果",
                "production",
                "write",
                object(
                    &["work_orders", "expected_cursor", "idempotency_key"],
                    json!({
                        "work_orders":{"type":"array","minItems":1,"maxItems":50,"items":text()},
                        "expected_cursor":{"type":"integer","minimum":0,"maximum":i64::MAX},
                        "idempotency_key":text()
                    }),
                ),
                json!({"cursor":{"type":"integer","minimum":0},"results":{"type":"array","items":{
                    "type":"object","required":["work_order","ok"],"properties":{
                        "work_order":text(),"ok":{"type":"boolean"},"code":text(),"status":{"const":"released"},
                        "cursor":{"type":"integer","minimum":0},"snapshot":{"type":"object"}
                    }
                }}}),
                "release_eligible_orders_and_freeze_execution_definitions",
                &[
                    "INVALID_ARGUMENT",
                    "VERSION_CONFLICT",
                    "IDEMPOTENCY_CONFLICT",
                    "DUPLICATE_WORK_ORDER",
                    "WORK_ORDER_NOT_FOUND",
                    "INVALID_STATE",
                    "MATERIAL_SHORTAGE",
                    "MATERIAL_HELD",
                    "CELL_NOT_CAPABLE",
                ][..],
            ),
            Self::CreateWorkOrder | Self::UpdateDraftOrder => {
                let create = self == Self::CreateWorkOrder;
                let mut required = vec![
                    "work_order",
                    "source_demand",
                    "expected_cursor",
                    "idempotency_key",
                ];
                let mut properties = json!({"work_order":text(),"source_demand":text(),
                    "expected_cursor":{"type":"integer","minimum":0,"maximum":i64::MAX},"idempotency_key":text()});
                if create {
                    required.push("template_work_order");
                    properties["template_work_order"] = text();
                    properties["serial"] = text();
                }
                (
                    if create {
                        "按参考工单定义创建工单草稿"
                    } else {
                        "更新工单草稿的来源需求"
                    },
                    "production",
                    "write",
                    object(&required, properties),
                    json!({"work_order":text(),"status":{"const":"planned"},"source_demand":text(),
                        "template_work_order":text(),"cursor":{"type":"integer","minimum":0}}),
                    if create {
                        "create_planned_work_order"
                    } else {
                        "update_draft_demand"
                    },
                    &[
                        "INVALID_ARGUMENT",
                        "WORK_ORDER_EXISTS",
                        "WORK_ORDER_NOT_FOUND",
                        "INVALID_STATE",
                        "TEMPLATE_NOT_FOUND",
                        "SERIAL_IN_USE",
                        "VERSION_CONFLICT",
                        "IDEMPOTENCY_CONFLICT",
                    ][..],
                )
            }
            Self::GetMaterial => (
                "查询物料批次及冻结状态",
                "inventory",
                "read",
                object(&["batch"], json!({"batch":text()})),
                json!({"batch":text(),"work_order":text(),"held":{"type":"boolean"},"reason":{"type":["null","string"]},"cursor":{"type":"integer"}}),
                "none",
                &["INVALID_ARGUMENT", "MATERIAL_NOT_FOUND"][..],
            ),
            Self::HoldMaterial => (
                "质量冻结物料批次",
                "quality",
                "write",
                object(
                    &["batch", "reason", "idempotency_key"],
                    json!({"batch":text(),"reason":text(),"idempotency_key":text()}),
                ),
                json!({"batch":text(),"held":{"type":"boolean"},"reason":text(),"cursor":{"type":"integer"}}),
                "freeze_batch",
                &[
                    "INVALID_ARGUMENT",
                    "MATERIAL_NOT_FOUND",
                    "ALREADY_HELD",
                    "IDEMPOTENCY_CONFLICT",
                ][..],
            ),
            _ if crate::smt::TOOLS.iter().any(|tool| tool.0 == self) => {
                return crate::smt::spec(self);
            }
            _ if crate::assembly::TOOLS.iter().any(|tool| tool.0 == self) => {
                return crate::assembly::spec(self);
            }
            _ if crate::fabrication::tools().any(|tool| tool.0 == self) => {
                return crate::fabrication::spec(self);
            }
            _ => return crate::mes::spec(self),
        };
        let mut fields = fields.as_object().unwrap().clone();
        fields.insert("ok".into(), json!({"const":true}));
        fields.insert("run_id".into(), text());
        let mut output_fields = fields.clone();
        if self == Self::GetWorkOrder {
            output_fields.insert("snapshot".into(), json!({"type":"object"}));
            output_fields.insert("hold_reason".into(), text());
            output_fields.insert("cancel_reason".into(), text());
            output_fields.insert("completion_confirmed".into(), json!({"type":"boolean"}));
            output_fields.insert("source_demand".into(), text());
            output_fields.insert("template_work_order".into(), text());
        }
        if self == Self::GetInspection {
            output_fields.insert("receipt_id".into(), text());
            output_fields.insert("inspection_plan".into(), crate::quality::plan_schema());
            output_fields.insert(
                "nonconformance".into(),
                crate::quality::nonconformance_schema(),
            );
            output_fields.insert(
                "measurements".into(),
                json!({"type":"array","items":crate::quality::measurement_schema()}),
            );
        }
        let required: Vec<_> = fields.keys().map(String::as_str).collect();
        Spec {
            name: self.name(),
            description,
            domain,
            effect,
            input,
            output: json!({"oneOf":[object(&required, Value::Object(output_fields)),
                object(&["ok","code","field"],json!({"ok":{"const":false},"code":text(),"field":text()}))]}),
            preconditions: &["run_open", "tool_enabled_in_scenario"],
            side_effect,
            errors,
            visibility: "run",
        }
    }
}

pub fn search(capabilities: &[Action], args: &Map<String, Value>) -> Vec<Value> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let mut results: Vec<_> = capabilities.iter().filter_map(|action| {
        let spec = action.spec();
        if args.get("domain").is_some_and(|v| v != spec.domain) || args.get("effect").is_some_and(|v| v != spec.effect) {
            return None;
        }
        let score = if query.is_empty() { 0 }
        else if spec.name == query || action.keywords().contains(&query.as_str()) { 2 }
        else if spec.name.contains(&query) || spec.description.contains(&query)
             || action.keywords().iter().any(|keyword| keyword.contains(&query))
             || spec.input["properties"].as_object().unwrap().keys().any(|field| field.contains(&query)) { 1 }
        else { return None; };
        Some((score,spec.name,json!({"name":spec.name,"description":spec.description,"domain":spec.domain,"effect":spec.effect})))
    }).collect();
    results.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    results.into_iter().map(|(_, _, item)| item).collect()
}

pub const NAMES: [&str; 11] = [
    "list_scenarios",
    "open_scenario",
    "get_run",
    "close_run",
    "search_tools",
    "describe_tool",
    "call_tool",
    "read_events",
    "advance_run",
    "intervene_run",
    "inspect_run",
];

pub fn gateway(name: &str) -> Option<Gateway> {
    let (name, description, read_only, input) = match name {
        "list_scenarios" => (
            "list_scenarios",
            "List available manufacturing scenarios",
            true,
            object(&[], json!({})),
        ),
        "open_scenario" => (
            "open_scenario",
            "Open an isolated scenario run",
            false,
            object(
                &["scenario_id", "version"],
                json!({
                    "scenario_id":text(),"version":{"type":"integer","minimum":1,"maximum":u32::MAX},"seed":{"type":"integer","minimum":0}
                }),
            ),
        ),
        "get_run" => (
            "get_run",
            "Read run metadata",
            true,
            object(&["run_id"], json!({"run_id":text()})),
        ),
        "close_run" => (
            "close_run",
            "Close a run",
            false,
            object(&["run_id"], json!({"run_id":text()})),
        ),
        "search_tools" => (
            "search_tools",
            "Search the current run's business tools",
            true,
            object(
                &["run_id"],
                json!({
                    "run_id":text(),"query":{"type":"string","maxLength":256},"domain":{"type":"string","enum":["production","inventory","quality"]},
                    "effect":{"type":"string","enum":["read","write"]}
                }),
            ),
        ),
        "describe_tool" => (
            "describe_tool",
            "Inspect a business tool contract",
            true,
            object(&["run_id", "name"], json!({"run_id":text(),"name":text()})),
        ),
        "call_tool" => (
            "call_tool",
            "Call a business tool within this run",
            false,
            object(
                &["run_id", "name", "arguments"],
                json!({
                    "run_id":text(),"name":text(),"arguments":{"type":"object"}
                }),
            ),
        ),
        "read_events" => (
            "read_events",
            "Read already delivered events",
            true,
            object(
                &["run_id"],
                json!({
                    "run_id":text(),"after":{"type":"integer","maximum":10000},"limit":{"type":"integer","minimum":1,"maximum":100}
                }),
            ),
        ),
        "advance_run" => (
            "advance_run",
            "Advance a trusted simulation within a work budget",
            false,
            object(
                &["run_id"],
                json!({
                    "run_id":text(),"max_events":{"type":"integer","minimum":1,"maximum":mom_des::MAX_STEP_EVENTS},
                    "max_minutes":{"type":"integer","minimum":1,"maximum":mom_des::MAX_STEP_MINUTES}
                }),
            ),
        ),
        "intervene_run" => (
            "intervene_run",
            "Schedule an intervention at the current virtual time",
            false,
            object(
                &["run_id", "action"],
                json!({"run_id":text(),"action":text()}),
            ),
        ),
        "inspect_run" => (
            "inspect_run",
            "Inspect hidden evaluation state and replay controls",
            true,
            object(&["run_id"], json!({"run_id":text()})),
        ),
        _ => return None,
    };
    Some(Gateway {
        name,
        description,
        read_only,
        input,
        control: matches!(name, "advance_run" | "intervene_run" | "inspect_run"),
    })
}

pub fn tools(control: bool) -> Vec<Tool> {
    NAMES
        .iter()
        .filter_map(|name| {
            let spec = gateway(name).filter(|spec| control || !spec.control)?;
            let mut tool = Tool::new(
                spec.name,
                spec.description,
                spec.input_schema(control).as_object().unwrap().clone(),
            );
            tool.annotations = Some(
                ToolAnnotations::new()
                    .read_only(spec.read_only)
                    .destructive(false),
            );
            Some(tool)
        })
        .collect()
}

// The registry uses a limited schema vocabulary, checked here for both gateway and business inputs.
pub fn validate(schema: &Value, args: &Map<String, Value>) -> Result<(), Rejection> {
    let properties = schema["properties"].as_object().unwrap();
    for required in schema["required"].as_array().unwrap() {
        let field = required.as_str().unwrap();
        if !args.contains_key(field) {
            return Err(Rejection::invalid(field));
        }
    }
    for (name, value) in args {
        let Some(rule) = properties.get(name) else {
            return Err(Rejection::invalid(name));
        };
        let valid = match rule["type"].as_str().unwrap() {
            "object" => value.is_object(),
            "boolean" => value.is_boolean(),
            "integer" => value.as_u64().is_some_and(|n| {
                n >= rule["minimum"].as_u64().unwrap_or(0)
                    && n <= rule["maximum"].as_u64().unwrap_or(u64::MAX)
            }),
            "number" => value.as_f64().is_some_and(f64::is_finite),
            "string" => valid_text(rule, value),
            "array" => value.as_array().is_some_and(|items| {
                let len = items.len() as u64;
                len >= rule["minItems"].as_u64().unwrap_or(0)
                    && len <= rule["maxItems"].as_u64().unwrap_or(u64::MAX)
                    && items.iter().all(|item| valid_text(&rule["items"], item))
            }),
            _ => unreachable!("unsupported registry schema"),
        };
        if !valid {
            return Err(Rejection::invalid(name));
        }
    }
    Ok(())
}

fn valid_text(rule: &Value, value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        let len = s.chars().count() as u64;
        len >= rule["minLength"].as_u64().unwrap_or(0)
            && len <= rule["maxLength"].as_u64().unwrap_or(u64::MAX)
            && rule["enum"]
                .as_array()
                .is_none_or(|values| values.contains(value))
    })
}
