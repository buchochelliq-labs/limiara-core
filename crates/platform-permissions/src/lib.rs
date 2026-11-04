#![forbid(unsafe_code)]
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub user_id: String,
    pub permissions: BTreeSet<String>,
}
#[derive(Debug, Error, PartialEq)]
pub enum PermissionError {
    #[error("permission denied: {0}")]
    Denied(String),
}
pub trait PolicyEngine {
    fn require(&self, principal: &Principal, permission: &str) -> Result<(), PermissionError>;
}
#[derive(Default)]
pub struct RbacPolicyEngine;
impl PolicyEngine for RbacPolicyEngine {
    fn require(&self, principal: &Principal, permission: &str) -> Result<(), PermissionError> {
        principal
            .permissions
            .contains(permission)
            .then_some(())
            .ok_or_else(|| PermissionError::Denied(permission.to_string()))
    }
}
