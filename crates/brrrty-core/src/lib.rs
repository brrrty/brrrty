//! Core build pipeline for Brrrty.

// `Problem` is the crate-wide diagnostic type; boxing it would burden every caller.
#![allow(clippy::result_large_err)]

pub mod build;
mod collections;
pub mod contain;
mod content;
mod data;
pub mod discover;
pub mod filters;
pub mod frontmatter;
pub mod helpers;
pub mod ignore_files;
#[doc(hidden)]
pub mod io_threads;
pub mod js;
mod layout;
mod loader;
pub mod options;
mod order;
pub mod page;
mod paginate;
pub mod passthrough;
pub mod permalink;
pub mod plugins;
pub mod problem;
mod render;
pub mod serve_lock;
mod templates;
mod yaml;

pub use build::{BuildReport, WrittenPage, build, content_hash};
pub use discover::{Discovery, discover};
pub use filters::register_nunjucks_filters;
pub use frontmatter::split_front_matter;
pub use js::js_truthy;
pub use options::{
    BuildOptions, CollectionError, CollectionFn, CollectionInput, CollectionPage, ConfigCollection,
    ConfigFilter, ConfigOptions, DataFn, DataRequest, DataRunner, PassthroughEntry, RunMode,
    SERVE_STAGING_DIR, TemplateEngines, TemplateLanguage, UndatedPage,
};
pub use page::{Page, file_path_stem, file_slug, resolve_date};
pub use passthrough::copy_passthrough;
pub use permalink::{Permalink, resolve_permalink};
pub use problem::{Problem, Severity, codes};
pub use serve_lock::{SERVE_LOCK_FILE, ServeLock, serve_lock_holder};
