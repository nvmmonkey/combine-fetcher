pub mod layout;
pub mod reader;
pub mod writer;

pub use layout::{AssetSlot, ShmHeader, SignalSnapshot, DEFAULT_SHM_PATH, MAX_SLOTS, SHM_MAGIC, SHM_VERSION, TOKEN_ID_LEN};
pub use reader::SignalReader;
pub use writer::SignalWriter;

#[cfg(test)]
mod tests;
