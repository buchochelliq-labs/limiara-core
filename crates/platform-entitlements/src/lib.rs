#![forbid(unsafe_code)]
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EntitlementValue {
    Boolean(bool),
    Limit(u64),
}
#[async_trait]
pub trait EntitlementService: Send + Sync {
    async fn has_feature(&self, tenant: &str, feature: &str) -> bool;
    async fn numeric_limit(&self, tenant: &str, limit: &str) -> Option<u64>;
}
#[derive(Default)]
pub struct CommunityEntitlements;
#[async_trait]
impl EntitlementService for CommunityEntitlements {
    async fn has_feature(&self, _tenant: &str, feature: &str) -> bool {
        feature == "contacts.basic"
    }
    async fn numeric_limit(&self, _tenant: &str, limit: &str) -> Option<u64> {
        (limit == "contacts.people").then_some(10_000)
    }
}
