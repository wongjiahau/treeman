use event::{parse_key_event, KeyEvent};
use regex::Regex;
use unicode_width::UnicodeWidthStr;

use itertools::Itertools;
use my_proc_macros::key;

use crate::{
    app::{Dispatch, Dispatches},
    grid::StyleKey,
    rectangle::Rectangle,
};

use super::{
    component::{Component, ComponentId},
    editor::{Direction, Editor, Mode, RegexHighlightRule},
    render_editor::{HighlightSpan, HighlightSpanRange, Source},
};

pub struct KeymapLegend {
    editor: Editor,
    config: KeymapLegendConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapLegendConfig {
    pub title: String,
    pub body: KeymapLegendBody,
    pub owner_id: ComponentId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeymapLegendBody {
    SingleSection { keymaps: Keymaps },
    MultipleSections { sections: Vec<KeymapLegendSection> },
}
const BETWEEN_KEY_AND_DESCRIPTION: &'static str = " → ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymaps(Vec<Keymap>);
impl Keymaps {
    fn display(&self, indent: usize, width: usize) -> String {
        let width = width.saturating_sub(indent);
        let max_key_width = self
            .0
            .iter()
            .map(|keymap| keymap.key.len())
            .max()
            .unwrap_or(0);
        let max_description_width = self
            .0
            .iter()
            .map(|keymap| keymap.description.len())
            .max()
            .unwrap_or(0);
        let key_description_gap = UnicodeWidthStr::width(BETWEEN_KEY_AND_DESCRIPTION);
        let column_gap = key_description_gap * 2;
        let column_width = max_key_width + key_description_gap + max_description_width + column_gap;
        let column_count = width / column_width;

        // Align the keys columns and the dispatch columns
        let result = self
            .0
            .iter()
            // .sorted_by_key(|keymap| keymap.key.to_lowercase())
            .map(|keymap| {
                let formatted = format!(
                    "{: >width$}{}{}",
                    keymap.key,
                    BETWEEN_KEY_AND_DESCRIPTION,
                    keymap.description,
                    width = max_key_width
                );
                formatted
            })
            .chunks(column_count.max(1)) // At least 1, otherwise `chunks` will panic
            .into_iter()
            .map(|chunks| {
                chunks
                    .map(|chunk| {
                        let second_formatted = format!("{: <width$}", chunk, width = column_width);
                        second_formatted
                    })
                    .join("")
            })
            .join("\n");
        let result = dedent(&result);
        result
            .lines()
            .map(|line| format!("{}{}", " ".repeat(indent), line.trim_end()))
            .join("\n")
    }

    pub fn new(keymaps: &[Keymap]) -> Self {
        Self(keymaps.to_vec())
    }

    pub(crate) fn get(&self, event: &KeyEvent) -> std::option::Option<&Keymap> {
        self.0.iter().find(|key| &key.event == event)
    }
}

fn dedent(s: &str) -> String {
    // Split the input string into lines
    let lines: Vec<&str> = s.lines().collect();

    // Find the minimum indentation (number of leading spaces)
    let min_indent = lines
        .iter()
        .filter(|&&line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|&c| c == ' ').count())
        .min()
        .unwrap_or(0);

    // Remove the common indentation from each line
    let dedented_lines: Vec<String> = lines
        .iter()
        .map(|&line| {
            if line.len() >= min_indent {
                line[min_indent..].to_string()
            } else {
                line.to_string()
            }
        })
        .collect();

    // Join the dedented lines back into a single string
    dedented_lines.join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapLegendSection {
    pub title: String,
    pub keymaps: Keymaps,
}

impl KeymapLegendSection {
    fn display(&self, width: usize) -> String {
        format!("{}:\n{}", self.title, self.keymaps.display(2, width))
    }
}

impl KeymapLegendBody {
    fn display(&self, width: usize) -> String {
        match self {
            KeymapLegendBody::SingleSection { keymaps } => keymaps.display(0, width),
            KeymapLegendBody::MultipleSections { sections } => sections
                .iter()
                .map(|section| section.display(width))
                .join("\n\n"),
        }
    }

    fn keymaps(&self) -> Vec<&Keymap> {
        match self {
            KeymapLegendBody::SingleSection { keymaps } => keymaps.0.iter().collect_vec(),
            KeymapLegendBody::MultipleSections { sections } => sections
                .iter()
                .flat_map(|section| section.keymaps.0.iter())
                .collect_vec(),
        }
    }
}

impl KeymapLegendConfig {
    fn display(&self, width: usize) -> String {
        self.body.display(width)
    }

    pub fn keymaps(&self) -> Vec<&Keymap> {
        self.body.keymaps()
    }

    fn get_regex_highlight_rules(&self) -> Vec<RegexHighlightRule> {
        self.keymaps()
            .into_iter()
            .flat_map(|keymap| {
                let keymap_key = RegexHighlightRule {
                    regex: Regex::new(&format!(
                        "({})({})({})",
                        regex::escape(keymap.key),
                        BETWEEN_KEY_AND_DESCRIPTION,
                        regex::escape(&keymap.description),
                    ))
                    .unwrap(),
                    get_highlight_spans: Box::new(|captures| {
                        let get_highlight_span = |index: usize, style_key: StyleKey| {
                            let match_ = captures.get(index)?;
                            Some(HighlightSpan {
                                source: Source::StyleKey(style_key),
                                ranges: HighlightSpanRange::ByteRange(match_.range()),
                                set_symbol: None,
                                is_cursor: false,
                            })
                        };
                        [
                            get_highlight_span(1, StyleKey::KeymapKey),
                            get_highlight_span(2, StyleKey::KeymapArrow),
                            get_highlight_span(3, StyleKey::KeymapDescription),
                        ]
                        .into_iter()
                        .flatten()
                        .collect_vec()
                    }),
                };
                let keymap_hint = (|| {
                    let index = keymap
                        .description
                        .to_lowercase()
                        .find(&keymap.key.to_lowercase())?;
                    let range = index..index + 1;
                    let marked_description = {
                        let mut description = keymap.description.clone();
                        description.replace_range(
                            range.clone(),
                            &format!("___OPEN___{}___CLOSE___", keymap.description.get(range)?),
                        );
                        regex::escape(&description)
                            .replace("___OPEN___", "(")
                            .replace("___CLOSE___", ")")
                    };
                    Some(RegexHighlightRule {
                        regex: regex::Regex::new(
                            &(format!(
                                "{}{}{}",
                                keymap.key, BETWEEN_KEY_AND_DESCRIPTION, marked_description
                            )),
                        )
                        .unwrap(),
                        get_highlight_spans: Box::new(|captures| {
                            if let Some(match_) = captures.get(1) {
                                vec![HighlightSpan {
                                    source: Source::StyleKey(StyleKey::KeymapHint),
                                    ranges: HighlightSpanRange::ByteRange(match_.range()),
                                    set_symbol: None,
                                    is_cursor: false,
                                }]
                            } else {
                                Default::default()
                            }
                        }),
                    })
                })();
                vec![Some(keymap_key), keymap_hint]
            })
            .flatten()
            .collect_vec()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    key: &'static str,
    description: String,
    event: KeyEvent,
    dispatch: Dispatch,
}

impl Keymap {
    pub fn new(key: &'static str, description: String, dispatch: Dispatch) -> Keymap {
        Keymap {
            key,
            description,
            dispatch,
            event: parse_key_event(key).unwrap(),
        }
    }

    pub(crate) fn dispatch(&self) -> Dispatch {
        self.dispatch.clone()
    }
}

impl KeymapLegend {
    pub fn new(config: KeymapLegendConfig) -> KeymapLegend {
        // Check for duplicate keys
        let duplicates = config
            .keymaps()
            .into_iter()
            .duplicates_by(|keymap| keymap.key)
            .collect_vec();

        if !duplicates.is_empty() {
            let message = format!(
                "Duplicate keymap keys for {}: {:#?}",
                config.title,
                duplicates
                    .into_iter()
                    .map(|duplicate| format!("{}: {}", duplicate.key, duplicate.description))
                    .collect_vec()
            );
            log::info!("{}", message);
            // panic!("{}", message);
        }

        let mut editor = Editor::from_text(tree_sitter_md::language(), "");
        editor.set_title(config.title.clone());
        editor.enter_insert_mode(Direction::End).unwrap_or_default();
        editor.set_regex_highlight_rules(config.get_regex_highlight_rules());
        KeymapLegend { editor, config }
    }

    fn refresh(&mut self) {
        let content = self.config.display(self.editor.rectangle().width as usize);
        self.editor_mut().set_content(&content).unwrap_or_default();
    }
}

impl Component for KeymapLegend {
    fn editor(&self) -> &Editor {
        &self.editor
    }

    fn set_rectangle(&mut self, rectangle: Rectangle) {
        self.editor_mut().set_rectangle(rectangle);
        self.refresh()
    }

    fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    fn handle_key_event(
        &mut self,
        context: &crate::context::Context,
        event: event::KeyEvent,
    ) -> Result<Dispatches, anyhow::Error> {
        let close_current_window = Dispatch::CloseCurrentWindow {
            change_focused_to: Some(self.config.owner_id),
        };
        if self.editor.mode == Mode::Insert {
            match &event {
                key!("esc") => {
                    self.editor.enter_normal_mode()?;
                    Ok(Default::default())
                }
                key_event => {
                    if let Some(keymap) = self
                        .config
                        .keymaps()
                        .iter()
                        .find(|keymap| &keymap.event == key_event)
                    {
                        Ok([close_current_window]
                            .into_iter()
                            .chain(vec![keymap.dispatch.clone()])
                            .collect_vec()
                            .into())
                    } else {
                        Ok(vec![].into())
                    }
                }
            }
        } else if self.editor.mode == Mode::Normal && event == key!("esc") {
            Ok([close_current_window].to_vec().into())
        } else {
            self.editor.handle_key_event(context, event)
        }
    }

    fn children(&self) -> Vec<Option<std::rc::Rc<std::cell::RefCell<dyn Component>>>> {
        self.editor.children()
    }

    fn remove_child(&mut self, component_id: super::component::ComponentId) {
        self.editor.remove_child(component_id)
    }
}

#[cfg(test)]
mod test_keymap_legend {
    use my_proc_macros::keys;

    use super::*;

    #[test]
    fn test_display_1() {
        let keymaps = Keymaps(
            [
                Keymap::new("a", "Aloha".to_string(), Dispatch::Null),
                Keymap::new("b", "Bomb".to_string(), Dispatch::Null),
                Keymap::new("c", "Caterpillar".to_string(), Dispatch::Null),
                Keymap::new("d", "D".to_string(), Dispatch::Null),
                Keymap::new("e", "Elephant".to_string(), Dispatch::Null),
                Keymap::new("space", "Gogagg".to_string(), Dispatch::Null),
            ]
            .to_vec(),
        );
        let width = 53;
        let actual = keymaps.display(2, width).to_string();
        let expected = "
  a → Aloha                b → Bomb
  c → Caterpillar          d → D
  e → Elephant         space → Gogagg"
            .trim_matches('\n');
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_display_2() {
        let keymaps = Keymaps(
            [
                Keymap::new("a", "Aloha".to_string(), Dispatch::Null),
                Keymap::new("b", "Bomb".to_string(), Dispatch::Null),
                Keymap::new("space", "Gogagg".to_string(), Dispatch::Null),
                Keymap::new("c", "Caterpillar".to_string(), Dispatch::Null),
            ]
            .to_vec(),
        );
        let width = 53;
        let actual = keymaps.display(2, width).to_string();
        let expected = "
      a → Aloha                b → Bomb
  space → Gogagg               c → Caterpillar"
            .trim_matches('\n');
        assert_eq!(actual, expected);
    }

    #[test]
    fn should_intercept_key_event_defined_in_config() {
        let owner_id = ComponentId::new();
        let mut keymap_legend = KeymapLegend::new(KeymapLegendConfig {
            title: "Test".to_string(),
            body: KeymapLegendBody::SingleSection {
                keymaps: Keymaps::new(&[Keymap::new(
                    "s",
                    "test".to_string(),
                    Dispatch::Custom("Spongebob".to_string()),
                )]),
            },
            owner_id,
        });

        let dispatches = keymap_legend.handle_events(keys!("s")).unwrap();

        assert_eq!(
            dispatches,
            Dispatches::new(vec![
                Dispatch::CloseCurrentWindow {
                    change_focused_to: Some(owner_id)
                },
                Dispatch::Custom("Spongebob".to_string())
            ])
        )
    }

    #[test]
    fn test_regex_keymap_hint() {
        let keymaps = Keymaps(
            [
                Keymap::new("a", "Aloha".to_string(), Dispatch::Null),
                Keymap::new("b", "Cob".to_string(), Dispatch::Null),
                Keymap::new("f", "Find (Local)".to_string(), Dispatch::Null),
                Keymap::new("g", "Find (Global)".to_string(), Dispatch::Null),
            ]
            .to_vec(),
        );
        let regexes = KeymapLegendConfig {
            title: "".to_string(),
            body: KeymapLegendBody::SingleSection { keymaps },
            owner_id: ComponentId::new(),
        }
        .get_regex_highlight_rules()
        .into_iter()
        .map(|rule| rule.regex.as_str().to_string())
        .collect_vec();

        assert_eq!(
            regexes,
            vec![
                "(a) → ",
                "a → (Aloha)",
                "a → (A)loha",
                "(b) → ",
                "b → (Cob)",
                "b → Co(b)",
                "(f) → ",
                "f → (Find \\(Local\\))",
                "f → (F)ind \\(Local\\)",
                "(g) → ",
                "g → (Find \\(Global\\))",
                "g → Find \\((G)lobal\\)"
            ]
        )
    }
}
