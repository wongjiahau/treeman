use grammar::grammar::GrammarConfiguration;
use serde_json::Value;

pub use crate::process_command::ProcessCommand;
use crate::{
    canonicalized_path::CanonicalizedPath, formatter::Formatter,
    ts_highlight_query::get_highlight_query,
};

pub use crate::languages::LANGUAGES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// As defined by the LSP protocol.
/// See sections below https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#range
pub struct LanguageId(&'static str);

impl std::fmt::Display for LanguageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl LanguageId {
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command(pub &'static str, pub &'static [&'static str]);
impl Command {
    pub const fn default() -> Command {
        Command("", &[])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    pub extensions: &'static [&'static str],
    pub lsp_language_id: Option<LanguageId>,
    pub lsp_command: Option<LspCommand>,
    pub tree_sitter_grammar_config: Option<GrammarConfig>,
    pub highlight_query: Option<&'static str>,
    pub formatter_command: Option<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspCommand {
    pub command: Command,
    pub initialization_options: Option<&'static str>,
}
impl LspCommand {
    pub const fn default() -> LspCommand {
        LspCommand {
            command: Command::default(),
            initialization_options: None,
        }
    }
}

impl Language {
    pub const fn new() -> Self {
        Self {
            extensions: &[""],
            lsp_language_id: None,
            highlight_query: None,
            lsp_command: None,
            tree_sitter_grammar_config: None,
            formatter_command: None,
        }
    }

    pub fn tree_sitter_grammar_id(&self) -> Option<String> {
        self.tree_sitter_grammar_config()
            .map(|config| config.grammar_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrammarConfig {
    pub id: &'static str,
    pub url: &'static str,
    pub commit: &'static str,
    pub subpath: Option<&'static str>,
}

impl Language {
    fn extensions(&self) -> &'static [&'static str] {
        self.extensions
    }

    pub fn initialization_options(&self) -> Option<Value> {
        serde_json::from_str(self.lsp_command.clone()?.initialization_options?).ok()
    }

    pub fn tree_sitter_language(&self) -> Option<tree_sitter::Language> {
        grammar::grammar::get_language(&self.tree_sitter_grammar_config()?.grammar_id).ok()
    }

    pub fn tree_sitter_grammar_config(&self) -> Option<GrammarConfiguration> {
        self.tree_sitter_grammar_config.as_ref().map(|config| {
            GrammarConfiguration::remote(config.id, config.url, config.commit, config.subpath)
        })
    }

    pub fn highlight_query(&self) -> Option<String> {
        // Get highlight query from `nvim-treesitter` first
        get_highlight_query(self.tree_sitter_grammar_config.clone()?.id)
            .ok()
            .map(|result| result.query)
            .or(
                // Otherwise, get from the default highlight queries defined in the grammar repo
                grammar::grammar::load_runtime_file(
                    &self.tree_sitter_grammar_config()?.grammar_id,
                    "highlights.scm",
                )
                .ok(),
            )
    }

    pub fn locals_query(&self) -> Option<&'static str> {
        None
    }

    pub fn injection_query(&self) -> Option<&'static str> {
        None
    }

    pub fn lsp_process_command(&self) -> Option<ProcessCommand> {
        self.lsp_command
            .as_ref()
            .map(|command| ProcessCommand::new(command.command.0, command.command.1))
    }

    pub fn id(&self) -> Option<LanguageId> {
        self.lsp_language_id
    }

    fn formatter_command(&self) -> Option<(ProcessCommand, FormatterTestCase)> {
        self.formatter_command.as_ref().map(|command| {
            (
                ProcessCommand::new(command.0, command.1),
                FormatterTestCase {
                    input: "",
                    expected: "",
                },
            )
        })
    }

    pub fn formatter(&self) -> Option<Formatter> {
        self.formatter_command()
            .map(|(command, _)| Formatter::from(command))
    }
}

pub fn from_path(path: &CanonicalizedPath) -> Option<Language> {
    from_extension(path.extension()?)
}

pub fn from_extension(extension: &str) -> Option<Language> {
    LANGUAGES
        .iter()
        .find(|language| language.extensions().contains(&extension))
        .map(|language| (*language).clone())
}

pub struct FormatterTestCase {
    /// The unformatted input.
    pub input: &'static str,

    /// The formatted output.
    pub expected: &'static str,
}
