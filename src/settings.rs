use std::sync::{Arc, Mutex};

use crate::settings_drafts::{self, CREATE, Drafts, PREVIEW, VALIDATE};
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

#[derive(Clone)]
pub struct Settings {
    site: Site,
    drafts: Option<Arc<Mutex<Drafts>>>,
}

impl Settings {
    pub fn new(site: Site, draft_tools: bool) -> Self {
        Self {
            site,
            drafts: draft_tools.then(|| Arc::new(Mutex::new(Drafts::default()))),
        }
    }

    pub fn can_draft(&self) -> bool {
        self.drafts.is_some()
    }

    pub fn supports(&self, name: &str) -> bool {
        name == GET_SETTINGS
            || (self.drafts.is_some() && matches!(name, CREATE | VALIDATE | PREVIEW))
    }

    pub fn invoke(&self, name: &str, args: Map<String, Value>) -> Value {
        if name == GET_SETTINGS {
            return self.site.read(&args);
        }
        if !self.supports(name) {
            return settings_drafts::error("UNKNOWN_TOOL");
        }
        let active = self.site.read(&Map::new());
        self.drafts
            .as_ref()
            .unwrap()
            .lock()
            .expect("settings draft store")
            .invoke(name, args, &active)
    }
}

impl ServerHandler for Settings {
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
        let mut tools = vec![tool];
        if self.drafts.is_some() {
            for (name, description) in [
                (CREATE, "登记站点设置草稿"),
                (VALIDATE, "校验工作中心与班次草稿"),
                (PREVIEW, "预览候选设置及生效范围"),
            ] {
                let mut tool = Tool::new(
                    name,
                    description,
                    settings_drafts::schema(name).as_object().unwrap().clone(),
                );
                tool.annotations = Some(ToolAnnotations::new().read_only(name != CREATE));
                tools.push(tool);
            }
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        Ok(CallToolResult::structured(
            self.invoke(&request.name, request.arguments.unwrap_or_default()),
        )
        .into())
    }
}
