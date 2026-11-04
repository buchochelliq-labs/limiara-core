//! The workspace API as a host of the object engine's routes (ADR 0015,
//! ADR 0024 §5): the routes live in `platform-objects-http`; this module
//! supplies the engine over PostgreSQL for the verified caller, and the
//! member directory that names users, roles and grantees.

use crate::AppState;
use async_trait::async_trait;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use platform_objects::store::PostgresObjectStore;
use platform_objects::{ObjectCaller, ObjectService};
use platform_objects_http::{Caller, ObjectsHost};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

fn unavailable() -> Response {
    crate::problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "platform.unavailable",
        "the service is temporarily unavailable",
    )
    .into_response()
}

#[async_trait]
impl ObjectsHost for AppState {
    async fn engine(&self, caller: &Caller) -> Result<ObjectService, Response> {
        let pool = self.pool.as_ref().ok_or_else(unavailable)?;
        let roles = platform_db::user_role_ids(pool, caller.tenant_id, caller.user_id)
            .await
            .map_err(|_| unavailable())?;
        // The reporting line grants managers their reports' records (ADR 0016).
        let reports = platform_db::reporting_reports(pool, caller.tenant_id, caller.user_id)
            .await
            .map_err(|_| unavailable())?;
        let engine_caller = ObjectCaller::new(
            caller.tenant_id,
            caller.user_id,
            caller.permissions.clone(),
            roles,
        )
        .with_reports(reports);
        Ok(ObjectService::new(
            Arc::new(PostgresObjectStore::new(pool.clone())),
            engine_caller,
        ))
    }

    async fn name_users(&self, tenant_id: Uuid, value: &mut Value) {
        crate::users::name_record_users(self, tenant_id, value).await;
    }

    async fn workspace_roles(&self, tenant_id: Uuid) -> Result<BTreeMap<Uuid, String>, Response> {
        let pool = self.pool.as_ref().ok_or_else(unavailable)?;
        platform_db::workspace_roles(pool, tenant_id)
            .await
            .map_err(|_| unavailable())
    }

    async fn member_names(&self, tenant_id: Uuid, users: &[Uuid]) -> BTreeMap<Uuid, String> {
        let Some(pool) = self.pool.as_ref() else {
            return BTreeMap::new();
        };
        platform_db::member_display_names(pool, tenant_id, users)
            .await
            .unwrap_or_default()
    }

    async fn is_member(&self, tenant_id: Uuid, user_id: Uuid) -> Result<bool, Response> {
        let pool = self.pool.as_ref().ok_or_else(unavailable)?;
        platform_db::workspace_member(pool, tenant_id, user_id)
            .await
            .map(|member| member.is_some())
            .map_err(|_| unavailable())
    }
}
