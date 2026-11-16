//! Finding included templates by name: the loader Brrrty's template
//! engines share.

/// Why a [`Loader`] could not supply a template.
///
/// `io` is `None` when no root has the file: the template is missing, and
/// `tried` lists every path looked at. `io` is `Some` when a file was found
/// but could not be used: it can't be read, isn't UTF-8, or resolves
/// outside the loader's roots.
#[derive(Debug, Default)]
pub struct LoadError {
    pub tried: Vec<String>,
    pub io: Option<std::io::Error>,
}

/// Finds included templates by name. Returns the resolved path and the
/// source.
///
/// The engine does its own name handling first (LiquidJS appends
/// `.liquid` to a name with no extension; Nunjucks resolves `./` and `../`
/// names against the including template's path) and calls `load` with the
/// result. The loader only maps a name to a file: it tries each of its
/// roots in order (11ty's are the includes folder, then the input folder
/// for Liquid or the project folder for Nunjucks), refuses names that
/// leave every root, and returns the path it found and the source.
/// Templates are told apart by that path, so it should be canonical: a
/// cycle is then found however the name is spelled.
pub trait Loader: Send + Sync {
    fn load(&self, name: &str) -> Result<(String, String), LoadError>;
}
