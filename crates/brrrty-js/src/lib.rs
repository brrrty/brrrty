//! JavaScript-like runtime values shared by Brrrty's template engines.

mod number;
mod value;

pub use number::js_number_to_string;
pub use value::Value;
