use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStats {
    pub rtt_micros: Option<u64>,
    pub packets_lost: Option<u64>,
}
