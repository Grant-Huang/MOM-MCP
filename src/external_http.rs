//! Read-only HTTP transport for trusted, host-configured source bindings.

use std::{net::IpAddr, time::Duration};

use reqwest::{Client, Response, StatusCode, Url, header::HeaderValue, redirect::Policy};
use serde_json::{Value, json};

mod carbon;

use crate::external_order::{BoundOrder, OrderObservation, ProjectionError, Source, project_order};

const MAX_RESPONSE_BYTES: usize = 256 * 1024;

pub enum HttpSourceKind<'a> {
    Odoo { database: &'a str },
    OpenMes,
    Carbon,
}

pub struct HttpSource<'a> {
    pub kind: HttpSourceKind<'a>,
    pub base_url: &'a str,
    pub token: &'a str,
    pub tenant: &'a str,
    pub site: &'a str,
    pub instance: &'a str,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ReadError {
    InvalidBinding,
    SourceUnavailable,
    EntitlementRequired,
    UpstreamToolRefused,
    NotVisible,
    InvalidResponse,
    UnexpectedRecord,
}

pub struct HttpOrderReader<'a> {
    client: Client,
    origin: Url,
    source: HttpSource<'a>,
}

impl<'a> HttpOrderReader<'a> {
    pub fn new(source: HttpSource<'a>) -> Result<Self, ReadError> {
        use ReadError::InvalidBinding;
        let origin = Url::parse(source.base_url).map_err(|_| InvalidBinding)?;
        let loopback = origin
            .host_str()
            .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if !(origin.scheme() == "https" || origin.scheme() == "http" && loopback)
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
            || [source.tenant, source.site, source.instance, source.token]
                .iter()
                .any(|value| value.trim().is_empty())
            || HeaderValue::from_str(source.token).is_err()
        {
            return Err(InvalidBinding);
        }
        if let HttpSourceKind::Odoo { database } = source.kind
            && (database.trim().is_empty() || HeaderValue::from_str(database).is_err())
        {
            return Err(InvalidBinding);
        }
        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| ReadError::SourceUnavailable)?;
        Ok(Self {
            client,
            origin,
            source,
        })
    }

    pub async fn read_order<'request>(
        &'request self,
        source_id: &'request str,
    ) -> Result<OrderObservation<'request>, ReadError> {
        use ReadError::*;
        let (source, payload) = match self.source.kind {
            HttpSourceKind::Carbon => {
                if source_id.trim().is_empty() {
                    return Err(InvalidBinding);
                }
                (Source::Carbon, self.read_carbon(source_id).await?)
            }
            HttpSourceKind::Odoo { .. } | HttpSourceKind::OpenMes => {
                self.read_http(source_id).await?
            }
        };
        project_order(
            &BoundOrder {
                source,
                tenant: self.source.tenant,
                site: self.source.site,
                instance: self.source.instance,
                source_id,
            },
            payload,
        )
        .map_err(|error| match error {
            ProjectionError::InvalidBinding => InvalidBinding,
            ProjectionError::InvalidResponse => InvalidResponse,
            ProjectionError::UnexpectedRecord => UnexpectedRecord,
            ProjectionError::NotVisible => NotVisible,
        })
    }

    async fn read_http(&self, source_id: &str) -> Result<(Source, Value), ReadError> {
        use ReadError::*;
        let numeric_id = source_id
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0 && id.to_string() == source_id)
            .ok_or(InvalidBinding)?;
        let (source, request) = match self.source.kind {
            HttpSourceKind::Odoo { database } => (
                Source::Odoo,
                self.client
                    .post(
                        self.origin
                            .join("/json/2/mrp.production/search_read")
                            .map_err(|_| InvalidBinding)?,
                    )
                    .header("X-Odoo-Database", database)
                    .json(&json!({
                        "domain": [["id", "=", numeric_id]],
                        "fields": ["id", "name", "state", "write_date"],
                        "limit": 1
                    })),
            ),
            HttpSourceKind::OpenMes => (
                Source::OpenMes,
                self.client.get(
                    self.origin
                        .join(&format!("/api/v1/work-orders/{numeric_id}"))
                        .map_err(|_| InvalidBinding)?,
                ),
            ),
            HttpSourceKind::Carbon => return Err(InvalidBinding),
        };
        let response = request
            .bearer_auth(self.source.token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| SourceUnavailable)?;
        match (source, response.status()) {
            (Source::OpenMes, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) => {
                return Err(NotVisible);
            }
            (_, StatusCode::OK) => {}
            _ => return Err(SourceUnavailable),
        }
        Ok((source, read_json(response).await?))
    }
}

async fn read_json(mut response: Response) -> Result<Value, ReadError> {
    use ReadError::*;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| SourceUnavailable)? {
        if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
            return Err(InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| InvalidResponse)
}
