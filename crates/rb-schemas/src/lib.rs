use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantId(pub Uuid);

impl TenantId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TenantId {
    fn default() -> Self {
        Self::new()
    }
}
