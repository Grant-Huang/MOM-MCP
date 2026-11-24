use reqwest::StatusCode;
use serde_json::{Value, json};

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
        let database = self.relation_database(work_order_id, company_id)?;
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

        Ok(Some(self.sales_relation(
            company_id,
            work_order_id,
            sales_line_id,
            sales_order_id,
        )))
    }

    /// Reads a relationship in one Odoo transaction using the fixed web_read specification.
    /// The caller must authorize the work order and company before invoking this method.
    pub async fn read_odoo_sales_relation_atomic(
        &self,
        work_order_id: i64,
        company_id: i64,
    ) -> Result<Option<OdooSalesRelation<'_>>, ReadError> {
        let database = self.relation_database(work_order_id, company_id)?;
        let url = self
            .origin
            .join("/json/2/mrp.production/web_read")
            .map_err(|_| ReadError::InvalidBinding)?;
        let response = self
            .client
            .post(url)
            .header("X-Odoo-Database", database)
            .header(reqwest::header::ACCEPT, "application/json")
            .bearer_auth(self.source.token)
            .json(&json!({
                "ids": [work_order_id],
                "specification": {
                    "id": {}, "company_id": {},
                    "sale_line_id": {"fields": {
                        "id": {}, "company_id": {},
                        "order_id": {"fields": {"id": {}, "company_id": {}}}
                    }}
                }
            }))
            .send()
            .await
            .map_err(|_| ReadError::SourceUnavailable)?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::FORBIDDEN => {
                let error = read_json(response)
                    .await
                    .map_err(|_| ReadError::SourceUnavailable)?;
                return Err(if error["name"] == "odoo.exceptions.AccessError" {
                    ReadError::NotVisible
                } else {
                    ReadError::SourceUnavailable
                });
            }
            _ => return Err(ReadError::SourceUnavailable),
        }
        let work_order = single_row(read_json(response).await?)?;
        if positive_id(&work_order["id"])? != work_order_id {
            return Err(ReadError::UnexpectedRecord);
        }
        let read_id = |row: &Value| {
            let id = positive_id(&row["id"])?;
            if positive_id(&row["company_id"])? != company_id {
                return Err(ReadError::NotVisible);
            }
            Ok(id)
        };
        read_id(&work_order)?;
        let sales_line = &work_order["sale_line_id"];
        if *sales_line == Value::Bool(false) {
            return Ok(None);
        }
        let sales_line_id = read_id(sales_line)?;
        let sales_order_id = read_id(&sales_line["order_id"])?;
        Ok(Some(self.sales_relation(
            company_id,
            work_order_id,
            sales_line_id,
            sales_order_id,
        )))
    }

    fn relation_database(&self, work_order_id: i64, company_id: i64) -> Result<&str, ReadError> {
        match self.source.kind {
            HttpSourceKind::Odoo { database } if work_order_id > 0 && company_id > 0 => {
                Ok(database)
            }
            _ => Err(ReadError::InvalidBinding),
        }
    }

    fn sales_relation(
        &self,
        company_id: i64,
        work_order_id: i64,
        sales_line_id: i64,
        sales_order_id: i64,
    ) -> OdooSalesRelation<'_> {
        OdooSalesRelation {
            tenant: self.source.tenant,
            site: self.source.site,
            instance: self.source.instance,
            company_id,
            work_order_id,
            sales_line_id,
            sales_order_id,
        }
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
        let row = single_row(read_json(response).await?)?;
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

fn single_row(payload: Value) -> Result<Value, ReadError> {
    let Value::Array(rows) = payload else {
        return Err(ReadError::InvalidResponse);
    };
    let mut rows = rows.into_iter();
    let row = rows.next().ok_or(ReadError::NotVisible)?;
    if rows.next().is_some() {
        return Err(ReadError::UnexpectedRecord);
    }
    Ok(row)
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
