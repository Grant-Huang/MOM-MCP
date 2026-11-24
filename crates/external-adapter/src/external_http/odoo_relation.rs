use serde_json::Value;

use super::{HttpOrderReader, HttpSourceKind, ReadError, read_json};

const WORK_ORDER: (&str, &[&str]) = (
    "/json/2/mrp.production/search_read",
    &["id", "company_id", "sale_line_id"],
);
const SALES_LINE: (&str, &[&str]) = (
    "/json/2/sale.order.line/search_read",
    &["id", "company_id", "order_id"],
);
const SALES_ORDER: (&str, &[&str]) = ("/json/2/sale.order/search_read", &["id", "company_id"]);

/// Internal evidence for the direct Odoo manufacturing-to-sales relationship.
#[derive(Debug, Eq, PartialEq)]
pub struct OdooSalesRelation<'a> {
    pub tenant: &'a str,
    pub site: &'a str,
    pub instance: &'a str,
    pub company_id: i64,
    pub work_order_id: i64,
    pub sales_line_id: i64,
    pub sales_order_id: i64,
}

impl HttpOrderReader<'_> {
    /// Reads an already authorized manufacturing order in the bound company.
    /// Each hop uses the same database and credentials. Both links are checked
    /// again before returning a relation. The calls are independent snapshots.
    pub async fn read_odoo_sales_relation(
        &self,
        work_order_id: i64,
        company_id: i64,
    ) -> Result<Option<OdooSalesRelation<'_>>, ReadError> {
        let HttpSourceKind::Odoo { database } = self.source.kind else {
            return Err(ReadError::InvalidBinding);
        };
        if work_order_id <= 0 || company_id <= 0 {
            return Err(ReadError::InvalidBinding);
        }
        let read = |record, id| self.read_odoo_relation_row(database, record, id, company_id);
        let work_order = read(WORK_ORDER, work_order_id).await?;
        let Some(sales_line_id) = relation_id(&work_order["sale_line_id"])? else {
            return Ok(None);
        };
        let line = read(SALES_LINE, sales_line_id).await?;
        let sales_order_id = relation_id(&line["order_id"])?.ok_or(ReadError::NotVisible)?;
        read(SALES_ORDER, sales_order_id).await?;

        let checked_line = read(SALES_LINE, sales_line_id).await?;
        if relation_id(&checked_line["order_id"])? != Some(sales_order_id) {
            return Err(ReadError::UnexpectedRecord);
        }
        let checked_order = read(WORK_ORDER, work_order_id).await?;
        if relation_id(&checked_order["sale_line_id"])? != Some(sales_line_id) {
            return Err(ReadError::UnexpectedRecord);
        }

        Ok(Some(OdooSalesRelation {
            tenant: self.source.tenant,
            site: self.source.site,
            instance: self.source.instance,
            company_id,
            work_order_id,
            sales_line_id,
            sales_order_id,
        }))
    }

    async fn read_odoo_relation_row(
        &self,
        database: &str,
        (path, fields): (&str, &[&str]),
        id: i64,
        company_id: i64,
    ) -> Result<Value, ReadError> {
        let response = self
            .odoo_query(database, path, id, fields, 2)?
            .bearer_auth(self.source.token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| ReadError::SourceUnavailable)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(ReadError::SourceUnavailable);
        }
        let Value::Array(mut rows) = read_json(response).await? else {
            return Err(ReadError::InvalidResponse);
        };
        let row = match rows.len() {
            0 => return Err(ReadError::NotVisible),
            1 => rows.pop().unwrap(),
            _ => return Err(ReadError::UnexpectedRecord),
        };
        let observed_id = positive_id(&row["id"])?;
        if observed_id != id {
            return Err(ReadError::UnexpectedRecord);
        }
        if relation_id(&row["company_id"])? != Some(company_id) {
            return Err(ReadError::NotVisible);
        }
        Ok(row)
    }
}

fn positive_id(value: &Value) -> Result<i64, ReadError> {
    value
        .as_i64()
        .filter(|id| *id > 0)
        .ok_or(ReadError::InvalidResponse)
}

fn relation_id(value: &Value) -> Result<Option<i64>, ReadError> {
    match value {
        Value::Bool(false) => Ok(None),
        Value::Array(parts) if parts.len() == 2 && parts[1].is_string() => {
            positive_id(&parts[0]).map(Some)
        }
        _ => Err(ReadError::InvalidResponse),
    }
}
