pub mod config;
pub mod format;
pub mod handlers;
pub mod inference;
pub mod memory;
pub mod outbox;
pub mod pairing;
pub mod queue;
pub mod telegram;
pub mod update_check;
pub mod vecstore;
pub mod watchdog;

// Re-export key dependencies for use by sophia-rescue binary
pub use grammers_client;
pub use grammers_mtsender;
pub use grammers_session;
pub use serde_json;
