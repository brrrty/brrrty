//! JavaScript-like runtime values shared by Brrrty's template engines.

mod number;
mod value;

pub use number::{is_js_whitespace, js_number_to_string, js_string_to_number};
pub use value::{Value, array_index};
