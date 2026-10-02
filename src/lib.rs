#![forbid(unsafe_code)]

mod compaction;
mod decode;
mod encode;
mod format;
mod stream;

pub use compaction::compact;
pub use decode::Decoder;
pub use encode::Encoder;
pub use format::{CHECKSUM_FLAG, Header, NO_CHECKSUM, Page, Trailer};

#[cfg(test)]
#[path = "../tests/unit/format.rs"]
mod format_tests;
