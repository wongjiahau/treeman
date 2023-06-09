#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Documentation {
    pub content: String,
}

impl From<lsp_types::Documentation> for Documentation {
    fn from(value: lsp_types::Documentation) -> Self {
        Self {
            content: match value {
                lsp_types::Documentation::String(s) => s,
                lsp_types::Documentation::MarkupContent(content) => content.value,
            },
        }
    }
}
