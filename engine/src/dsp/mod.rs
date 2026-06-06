//! Signal-processing building blocks: the state-variable filter, the ADSR envelope, and
//! the master-bus effects (gain, DC high-pass, look-ahead limiter). The per-track and send
//! effects rack arrives in Phase 3.

pub mod effects;
pub mod envelope;
pub mod filter;
pub mod fx;
