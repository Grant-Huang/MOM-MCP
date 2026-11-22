//! Service-side routing for a production work-order observation.
//! The host supplies the per-request authorization and opaque-reference resolver.

use std::{collections::HashMap, future::Future, time::SystemTime};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

use crate::{
    external_http::{HttpOrderReader, HttpSource, ReadError},
    external_order::MappingIssue,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceScope<'a> {
    pub tenant: &'a str,
    pub site: &'a str,
    pub instance: &'a str,
}

#[derive(Clone, Copy)]
pub struct WorkOrderSelection<'a> {
    pub world_id: &'a str,
    pub object_ref: &'a str,
}

/// Produced by the trusted host after checking the current caller against both
/// the selected world and the requested work-order reference.
pub struct AuthorizedWorkOrder<'a> {
    pub scope: SourceScope<'a>,
    pub source_id: &'a str,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ConfigurationError {
    DuplicateSource,
    InvalidSource(ReadError),
}

#[derive(Debug, Eq, PartialEq)]
pub enum ObserveError {
    AccessDenied,
    NotVisible,
    SourceUnavailable,
    InvalidSourceResponse,
}

/// Candidate internal output. The host is responsible for the public Observe envelope.
#[derive(Debug, Serialize)]
pub struct ObservedWorkOrder<'a> {
    pub object_ref: &'a str,
    pub source_instance: &'a str,
    pub observed_at: String,
    pub display_number: String,
    pub source_status: Option<String>,
    pub normalized_status: Option<String>,
    pub mapping_issue: MappingIssue,
    pub mapping_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_updated_at: Option<String>,
}

pub struct ExternalObserve<'a> {
    sources: HashMap<SourceScope<'a>, HttpOrderReader<'a>>,
}

impl<'a> ExternalObserve<'a> {
    pub fn new(
        sources: impl IntoIterator<Item = HttpSource<'a>>,
    ) -> Result<Self, ConfigurationError> {
        let mut readers = HashMap::new();
        for source in sources {
            let scope = SourceScope {
                tenant: source.tenant,
                site: source.site,
                instance: source.instance,
            };
            let std::collections::hash_map::Entry::Vacant(entry) = readers.entry(scope) else {
                return Err(ConfigurationError::DuplicateSource);
            };
            entry.insert(HttpOrderReader::new(source).map_err(ConfigurationError::InvalidSource)?);
        }
        Ok(Self { sources: readers })
    }

    pub async fn observe<'request, Resolve, Resolution>(
        &'request self,
        selection: WorkOrderSelection<'request>,
        resolve: Resolve,
    ) -> Result<ObservedWorkOrder<'request>, ObserveError>
    where
        Resolve: FnOnce(WorkOrderSelection<'request>) -> Resolution,
        Resolution: Future<Output = Option<AuthorizedWorkOrder<'request>>>,
    {
        if selection.world_id.trim().is_empty() || selection.object_ref.trim().is_empty() {
            return Err(ObserveError::AccessDenied);
        }
        let authorized = resolve(selection).await.ok_or(ObserveError::AccessDenied)?;
        let reader = self
            .sources
            .get(&authorized.scope)
            .ok_or(ObserveError::SourceUnavailable)?;
        let order = reader
            .read_order(authorized.source_id)
            .await
            .map_err(|error| match error {
                ReadError::NotVisible => ObserveError::NotVisible,
                ReadError::InvalidResponse | ReadError::UnexpectedRecord => {
                    ObserveError::InvalidSourceResponse
                }
                ReadError::InvalidBinding
                | ReadError::SourceUnavailable
                | ReadError::EntitlementRequired
                | ReadError::UpstreamToolRefused => ObserveError::SourceUnavailable,
            })?;
        Ok(ObservedWorkOrder {
            object_ref: selection.object_ref,
            source_instance: authorized.scope.instance,
            observed_at: DateTime::<Utc>::from(SystemTime::now())
                .to_rfc3339_opts(SecondsFormat::Millis, true),
            display_number: order.display_number,
            source_status: order.source_status,
            normalized_status: order.normalized_status,
            mapping_issue: order.mapping_issue,
            mapping_version: order.mapping_version,
            source_updated_at: order.source_updated_at,
        })
    }
}
