pub mod builder;
pub mod encoding;
pub mod error;
pub(crate) mod external;
pub mod facts;
pub mod filetypes;
pub mod git_index;
pub mod gitignore;
pub mod graph;
pub mod history;
pub mod hybrid;
pub mod live;
pub mod meta;
pub(crate) mod ondisk;
pub mod path_index;
pub mod query;
pub mod reader;
pub mod sparse;
#[cfg(feature = "structure")]
pub mod structure;
pub mod trigram;
pub mod visibility;
pub mod walker;

pub use error::{Error, Result};
pub use ondisk::PostingEntry;
