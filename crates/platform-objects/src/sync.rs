//! Device sync support (ADR 0024 §4). The change log cannot carry lost
//! access, so a device sends the records it holds and the server answers
//! with those the member can no longer read.

use crate::query::limits::IN_VALUES;
use crate::related::literal;
use crate::service::ObjectService;
use crate::{ObjectDefinition, ObjectError, QueryRequest};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Record ids one revocation request may name, across all objects. A
/// device with more sends its manifest in several requests.
pub const MANIFEST_MAX: usize = 1_000;

impl ObjectService {
    /// The records of `manifest`, by object API name and public ID, that
    /// the caller can no longer read: the object is gone or no longer
    /// readable, or the record is deleted, missing or no longer shared with
    /// them. Every record is read through the query language as the caller,
    /// so sharing and object permissions apply exactly as they do to a
    /// query.
    pub async fn revocations(
        &self,
        manifest: &BTreeMap<String, Vec<String>>,
    ) -> Result<BTreeMap<String, Vec<String>>, ObjectError> {
        let total: usize = manifest.values().map(Vec::len).sum();
        if total > MANIFEST_MAX {
            return Err(ObjectError::invalid(
                "records",
                "sync.manifest_too_large",
                format!("send at most {MANIFEST_MAX} record ids per request"),
            ));
        }
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let mut revoked = BTreeMap::new();
        for (api_name, ids) in manifest {
            let held: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
            if held.is_empty() {
                continue;
            }
            let readable_object = tx
                .object_by_api_name(api_name)
                .await?
                .filter(ObjectDefinition::is_active)
                .is_some_and(|object| access.for_object(object.id).read);
            let mut readable = BTreeSet::new();
            if readable_object {
                let held: Vec<&str> = held.iter().copied().collect();
                for chunk in held.chunks(IN_VALUES) {
                    let values: Vec<String> = chunk.iter().map(|id| literal(id)).collect();
                    let request = QueryRequest {
                        q: format!(
                            "SELECT id FROM {api_name} WHERE id IN ({})",
                            values.join(", ")
                        ),
                        cursor: None,
                        page_size: Some(IN_VALUES as u16),
                        include_deleted: false,
                    };
                    let result = self.query_in(tx.as_mut(), &access, &request).await?;
                    readable.extend(result.rows.into_iter().filter_map(|mut row| {
                        match row.remove("id") {
                            Some(Value::String(id)) => Some(id),
                            _ => None,
                        }
                    }));
                }
            }
            let lost: Vec<String> = held
                .into_iter()
                .filter(|id| !readable.contains(*id))
                .map(str::to_owned)
                .collect();
            if !lost.is_empty() {
                revoked.insert(api_name.clone(), lost);
            }
        }
        tx.commit().await?;
        Ok(revoked)
    }
}
