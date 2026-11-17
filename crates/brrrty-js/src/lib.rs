//! JavaScript-like runtime values shared by Brrrty's template engines.

pub mod clock;
mod eleventy;
mod eleventy_tables;
mod loader;
mod number;
pub mod traps;
pub mod tz;
mod value;
mod zones;

pub use eleventy::{
    FilterEnv, FilterFn, FilterImpl, REFUSAL_PREFIX, console_format, eleventy_filters,
};
pub use loader::{LoadError, Loader};
pub use number::{
    is_js_whitespace, js_number_to_string, js_string_to_number, js_to_fixed, js_to_locale_string,
};
pub use tz::Tz;
pub use value::{Value, array_index};
