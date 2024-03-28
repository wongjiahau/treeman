use std::{collections::HashMap, ops::Range, sync::mpsc::Sender};

use itertools::Itertools;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::{
    app::AppMessage,
    components::component::ComponentId,
    grid::{Style, StyleKey},
    themes::{Theme, HIGHLIGHT_NAMES},
};
use shared::language::Language;

#[derive(Clone, Debug)]
pub struct HighlighedSpan {
    pub byte_range: Range<usize>,
    pub style: Style,
    pub source: Option<StyleKey>,
}

/// In hex format, e.g. "#FF0000"
#[derive(Debug, Clone, Copy)]
pub struct Color(&'static str);

pub trait GetHighlightConfig {
    fn get_highlight_config(&self) -> anyhow::Result<Option<HighlightConfiguration>>;
}

impl GetHighlightConfig for Language {
    fn get_highlight_config(&self) -> anyhow::Result<Option<HighlightConfiguration>> {
        let tree_sitter_language = if let Some(tree_sitter_language) = self.tree_sitter_language() {
            tree_sitter_language
        } else {
            return Ok(None);
        };

        let highlights_query = &self.highlight_query().unwrap_or_default();
        let mut config = HighlightConfiguration::new(
            tree_sitter_language,
            highlights_query,
            self.injection_query().unwrap_or_default(),
            self.locals_query().unwrap_or_default(),
        )?;

        config.configure(
            &crate::themes::HIGHLIGHT_NAMES
                .into_iter()
                .map(|(name, _)| name)
                .collect_vec(),
        );

        Ok(Some(config))
    }
}

pub trait Highlight {
    fn highlight(&self, theme: Box<Theme>, source_code: &str) -> anyhow::Result<HighlighedSpans>;
}
impl Highlight for HighlightConfiguration {
    fn highlight(&self, theme: Box<Theme>, source_code: &str) -> anyhow::Result<HighlighedSpans> {
        let mut highlighter = Highlighter::new();

        let highlights = highlighter.highlight(self, source_code.as_bytes(), None, |_| None)?;

        let mut highlight = None;

        let mut highlighted_spans = vec![];

        for event in highlights {
            match event? {
                HighlightEvent::HighlightStart(s) => {
                    highlight = Some(s);
                }
                HighlightEvent::HighlightEnd => {
                    highlight = None;
                }
                HighlightEvent::Source { start, end } => {
                    if let Some(highlight) = highlight {
                        if let Some(color) = HIGHLIGHT_NAMES
                            .get(highlight.0)
                            .map(|(_, style_key)| theme.get_style(style_key))
                        {
                            highlighted_spans.push(HighlighedSpan {
                                byte_range: start..end,
                                style: color,
                                source: crate::themes::HIGHLIGHT_NAMES
                                    .get(highlight.0)
                                    .map(|(_, key)| key.clone()),
                            });
                        }
                    }
                }
            }
        }
        Ok(HighlighedSpans(highlighted_spans))
    }
}

#[derive(Clone, Default, Debug)]
pub struct HighlighedSpans(pub Vec<HighlighedSpan>);

pub struct SyntaxHighlightRequest {
    pub component_id: ComponentId,
    pub language: Language,
    pub theme: Box<Theme>,
    pub source_code: String,
}

pub struct SyntaxHighlightResponse {
    pub component_id: ComponentId,
    pub highlighted_spans: HighlighedSpans,
}

pub fn start_thread(callback: Sender<AppMessage>) -> Sender<SyntaxHighlightRequest> {
    let (sender, receiver) = std::sync::mpsc::channel::<SyntaxHighlightRequest>();
    std::thread::spawn(move || {
        let mut highlight_configs = HighlightConfigs::new();
        while let Ok(request) = receiver.recv() {
            match highlight_configs.highlight(request.theme, request.language, &request.source_code)
            {
                Ok(highlighted_spans) => {
                    let _ = callback.send(AppMessage::SyntaxHighlightResponse {
                        component_id: request.component_id,
                        highlighted_spans,
                    });
                }
                Err(error) => {
                    log::info!("syntax_highlight_error = {:#?}", error)
                }
            }
        }
    });
    sender
}
type TreeSitterGrammarId = String;
/// We have to cache the highlight configurations because they load slowly.
pub struct HighlightConfigs(
    HashMap<TreeSitterGrammarId, tree_sitter_highlight::HighlightConfiguration>,
);

impl HighlightConfigs {
    pub(crate) fn new() -> HighlightConfigs {
        HighlightConfigs(Default::default())
    }

    pub(crate) fn highlight(
        &mut self,
        theme: Box<Theme>,
        language: Language,
        source_code: &str,
    ) -> Result<HighlighedSpans, anyhow::Error> {
        let Some(grammar_id) = language.tree_sitter_grammar_id() else {
            return Ok(Default::default());
        };
        let config = match self.0.get(&grammar_id) {
            Some(config) => config,
            None => {
                if let Some(highlight_config) = language.get_highlight_config()? {
                    self.0.insert(grammar_id.clone(), highlight_config);
                    let get_error = || {
                        anyhow::anyhow!("Unreachable: should be able to obtain a value that is inserted to the HashMap")
                    };
                    self.0.get(&grammar_id).ok_or_else(get_error)?
                } else {
                    return Ok(Default::default());
                }
            }
        };
        config.highlight(theme, source_code)
    }
}
