#![forbid(unsafe_code)]
use thiserror::Error;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WasmRuntimeError {
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
    #[error("execution limit exceeded")]
    LimitExceeded,
}
#[derive(Debug, Clone)]
pub struct WasmLimits {
    pub fuel: u64,
    pub memory_bytes: usize,
    pub timeout_ms: u64,
}
pub struct WasmPluginRunner {
    limits: WasmLimits,
    allow_fs: bool,
    allow_env: bool,
    outbound_http: Vec<String>,
}
impl WasmPluginRunner {
    pub fn new(limits: WasmLimits) -> Self {
        Self {
            limits,
            allow_fs: false,
            allow_env: false,
            outbound_http: Vec::new(),
        }
    }
    pub fn hello(&self) -> Result<String, WasmRuntimeError> {
        let _ = &self.limits;
        Ok("hello from sandboxed wasm".into())
    }
    pub fn read_file(&self, path: &str) -> Result<String, WasmRuntimeError> {
        if self.allow_fs {
            Ok(String::new())
        } else {
            Err(WasmRuntimeError::CapabilityDenied(format!(
                "filesystem:{path}"
            )))
        }
    }
    pub fn env(&self, key: &str) -> Result<String, WasmRuntimeError> {
        if self.allow_env {
            Ok(String::new())
        } else {
            Err(WasmRuntimeError::CapabilityDenied(format!("env:{key}")))
        }
    }
    pub fn outbound_http_hosts(&self) -> &[String] {
        &self.outbound_http
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denies_ambient_file_access() {
        let r = WasmPluginRunner::new(WasmLimits {
            fuel: 10_000,
            memory_bytes: 1_048_576,
            timeout_ms: 100,
        });
        assert_eq!(
            r.read_file("/etc/passwd"),
            Err(WasmRuntimeError::CapabilityDenied(
                "filesystem:/etc/passwd".into()
            ))
        );
    }
}
