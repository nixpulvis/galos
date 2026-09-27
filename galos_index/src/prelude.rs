//! The core API in one place: [`Tree`], [`Index`], [`Sky`], [`Galaxy`] and
//! the types a caller passes them.
//!
//! `use galos_index::prelude::*` brings in what building, serving and reading
//! the index takes. Everything else is named by its module path, and by that
//! one path only.

pub use crate::accumulate::galaxy::Galaxy;
pub use crate::accumulate::report::SystemReport;
pub use crate::build::snapshot::{BuildParams, Snapshot};
pub use crate::build::tree::Tree;
pub use crate::core::geometry::CellId;
pub use crate::core::moments::Moments;
pub use crate::core::name::SystemName;
pub use crate::core::star::StarKind;
pub use crate::format::lock::Lock;
pub use crate::format::payload::INDEX_VERSION;
pub use crate::read::sky::Sky;
pub use crate::read::source::{FsSource, Part, Source, Stamp};
pub use crate::read::walk::{Mode, Needed, View};
pub use crate::store::names::Names;
pub use crate::store::tables::{Table, TableSet};
pub use crate::system::System;
pub use crate::tree::cell::{Cell, CellSystem};
pub use crate::tree::index::Index;
