use clap::ValueEnum;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use serde_json::{Map, Value, json};

pub const GET_SETTINGS: &str = "get_mes_settings";

#[derive(Clone, Copy, ValueEnum)]
pub enum Site {
    #[value(name = "SITE-A")]
    SiteA,
    #[value(name = "SITE-B")]
    SiteB,
}

impl Site {
    pub fn read(self, args: &Map<String, Value>) -> Value {
        if !args.is_empty() {
            return json!({"ok":false,"code":"INVALID_ARGUMENT"});
        }
        let (site, timezone, center, start, end) = match self {
            Self::SiteA => ("SITE-A", "America/Vancouver", "MACH-01", "08:00", "16:00"),
            Self::SiteB => ("SITE-B", "Asia/Shanghai", "CELL-A", "22:00", "06:00"),
        };
        json!({"ok":true,"site_id":site,"active_version":1,"timezone":timezone,
            "work_centers":[{"work_center":center,"active":true}],
            "shifts":[{"shift_id":"SHIFT-1","work_center":center,"start_local":start,"end_local":end}]})
    }
}

impl ServerHandler for Site {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("mom-mes-settings", env!("CARGO_PKG_VERSION")),
        )
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tool = Tool::new(
            GET_SETTINGS,
            "查询当前站点的设置版本、工作中心与班次",
            json!({"type":"object","additionalProperties":false})
                .as_object()
                .unwrap()
                .clone(),
        );
        tool.annotations = Some(ToolAnnotations::new().read_only(true));
        Ok(ListToolsResult::with_all_items(vec![tool]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name != GET_SETTINGS {
            return Ok(
                CallToolResult::structured(json!({"ok":false,"code":"UNKNOWN_TOOL"})).into(),
            );
        }
        Ok(CallToolResult::structured(self.read(&request.arguments.unwrap_or_default())).into())
    }
}
