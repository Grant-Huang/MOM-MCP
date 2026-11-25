//! Read-only HTTP transport for trusted, host-configured source bindings.

use std::{net::IpAddr, time::Duration};

use reqwest::{Client, Response, StatusCode, Url, header::HeaderValue, redirect::Policy};
use serde_json::{Value, json};

mod carbon;
mod odoo_relation;

pub use odoo_relation::OdooSalesRelation;

use crate::external_order::{BoundOrder, OrderObservation, ProjectionError, Source, project_order};

const MAX_RESPONSE_BYTES: usize = 256 * 1024;

pub enum HttpSourceKind<'a> {
    Odoo {
        database: &'a str,
    },
    OpenMes,
    Carbon,
    CarbonRest {
        company: &'a str,
        project_key: &'a str,
    },
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
        if let HttpSourceKind::CarbonRest {
            company,
            project_key,
        } = source.kind
            && (!valid_carbon_id(company)
                || project_key.trim().is_empty()
                || HeaderValue::from_str(project_key).is_err())
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
            HttpSourceKind::CarbonRest {
                company,
                project_key,
            } => (
                Source::Carbon,
                self.read_carbon_rest(source_id, company, project_key)
                    .await?,
            ),
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

    async fn read_carbon_rest(
        &self,
        source_id: &str,
        company: &str,
        project_key: &str,
    ) -> Result<Value, ReadError> {
        use ReadError::*;
        if !valid_carbon_id(source_id) {
            return Err(InvalidBinding);
        }
        let response = self
            .client
            .get(
                self.origin
                    .join("/rest/v1/jobs")
                    .map_err(|_| InvalidBinding)?,
            )
            .query(&[
                ("select", "id,jobId,status,companyId"),
                ("id", &format!("eq.{source_id}")),
                ("companyId", &format!("eq.{company}")),
                ("limit", "2"),
            ])
            .bearer_auth(self.source.token)
            .header("apikey", project_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| SourceUnavailable)?;
        if response.status() != StatusCode::OK {
            return Err(SourceUnavailable);
        }
        let payload = read_json(response).await?;
        match payload.as_array().map(Vec::as_slice) {
            Some([]) => Err(NotVisible),
            Some([row]) if row["companyId"].as_str() == Some(company) => Ok(row.clone()),
            Some([row]) if row["companyId"].is_string() => Err(UnexpectedRecord),
            _ => Err(InvalidResponse),
        }
    }

    fn odoo_query(
        &self,
        database: &str,
        path: &str,
        id: i64,
        fields: &[&str],
    ) -> Result<reqwest::RequestBuilder, ReadError> {
        let url = self
            .origin
            .join(path)
            .map_err(|_| ReadError::InvalidBinding)?;
        Ok(self
            .client
            .post(url)
            .header("X-Odoo-Database", database)
            .json(&json!({
                "domain": [["id", "=", id]],
                "fields": fields,
                "limit": 2,
            })))
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
                self.odoo_query(
                    database,
                    "/json/2/mrp.production/search_read",
                    numeric_id,
                    &["id", "name", "state", "write_date"],
                )?,
            ),
            HttpSourceKind::OpenMes => (
                Source::OpenMes,
                self.client.get(
                    self.origin
                        .join(&format!("/api/v1/work-orders/{numeric_id}"))
                        .map_err(|_| InvalidBinding)?,
                ),
            ),
            HttpSourceKind::Carbon | HttpSourceKind::CarbonRest { .. } => {
                return Err(InvalidBinding);
            }
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

fn valid_carbon_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
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
