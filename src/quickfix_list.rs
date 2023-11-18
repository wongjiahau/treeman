use std::{cell::RefCell, ops::Range, rc::Rc};

use itertools::Itertools;
use lsp_types::DiagnosticSeverity;

use crate::{
    components::{
        component::{Component, ComponentId},
        dropdown::{Dropdown, DropdownConfig, DropdownItem},
        editor::Movement,
        suggestive_editor::Info,
    },
    position::Position,
};
use shared::canonicalized_path::CanonicalizedPath;

pub struct QuickfixLists {
    lists: Vec<QuickfixList>,
    dropdown: Dropdown<QuickfixListItem>,
}

impl DropdownItem for QuickfixListItem {
    fn label(&self) -> String {
        self.location().display()
    }

    fn info(&self) -> Option<Info> {
        self.info.clone()
    }

    fn group(&self) -> String {
        self.location().path.display_absolute()
    }
}

impl Component for QuickfixLists {
    fn editor(&self) -> &crate::components::editor::Editor {
        self.dropdown.editor()
    }

    fn editor_mut(&mut self) -> &mut crate::components::editor::Editor {
        self.dropdown.editor_mut()
    }

    fn handle_key_event(
        &mut self,
        context: &crate::context::Context,
        event: event::KeyEvent,
    ) -> anyhow::Result<Vec<crate::app::Dispatch>> {
        self.dropdown.handle_key_event(context, event)
    }

    fn children(&self) -> Vec<Option<Rc<RefCell<dyn Component>>>> {
        self.dropdown.children()
    }

    fn remove_child(&mut self, component_id: ComponentId) {
        self.dropdown.remove_child(component_id)
    }
}

impl QuickfixLists {
    pub fn new() -> QuickfixLists {
        QuickfixLists {
            lists: vec![],
            dropdown: Dropdown::new(DropdownConfig {
                title: "Quickfix".to_string(),
            }),
        }
    }

    pub fn current(&self) -> Option<&QuickfixList> {
        self.lists.last()
    }

    pub fn current_mut(&mut self) -> Option<&mut QuickfixList> {
        self.lists.last_mut()
    }

    pub fn push(&mut self, quickfix_list: QuickfixList) {
        self.dropdown.set_items(quickfix_list.items.clone());
        self.lists.push(quickfix_list);
    }

    pub fn get_item(&mut self, movement: Movement) -> Option<QuickfixListItem> {
        self.dropdown.get_item(movement)
    }

    pub fn get_items(&self) -> Option<&Vec<QuickfixListItem>> {
        self.lists.last().map(|list| &list.items)
    }
}

#[derive(Clone)]
pub struct QuickfixList {
    current_index: usize,
    items: Vec<QuickfixListItem>,
    title: Option<String>,
}

impl QuickfixList {
    pub fn new(items: Vec<QuickfixListItem>) -> QuickfixList {
        QuickfixList {
            current_index: 0,
            items: {
                let mut items = items;

                // Sort the items by location
                items.sort_by(|a, b| a.location.cmp(&b.location));

                // Merge items of same locations
                items
                    .into_iter()
                    .group_by(|item| item.location.clone())
                    .into_iter()
                    .map(|(location, items)| QuickfixListItem {
                        location,
                        info: items
                            .into_iter()
                            .flat_map(|item| item.info)
                            .reduce(Info::join),
                    })
                    .collect_vec()
            },
            title: None,
        }
    }

    pub fn items(&self) -> &Vec<QuickfixListItem> {
        &self.items
    }

    pub fn set_title(self, title: Option<String>) -> QuickfixList {
        Self { title, ..self }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickfixListItem {
    location: Location,
    info: Option<Info>,
}

impl PartialOrd for QuickfixListItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.location.partial_cmp(&other.location)
    }
}

impl Ord for QuickfixListItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl From<Location> for QuickfixListItem {
    fn from(value: Location) -> Self {
        QuickfixListItem {
            location: value,
            info: None,
        }
    }
}

impl QuickfixListItem {
    pub fn new(location: Location, info: Option<Info>) -> QuickfixListItem {
        QuickfixListItem { location, info }
    }

    pub fn location(&self) -> &Location {
        &self.location
    }

    pub fn info(&self) -> &Option<Info> {
        &self.info
    }

    pub(crate) fn set_info(self, info: Option<Info>) -> Self {
        Self { info, ..self }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Location {
    pub path: CanonicalizedPath,
    pub range: Range<Position>,
}

impl Location {
    pub fn display(&self) -> String {
        format!(
            "{}:{}:{}-{}:{}",
            self.path
                .display_relative()
                .unwrap_or_else(|_| self.path.display_absolute()),
            self.range.start.line + 1,
            self.range.start.column + 1,
            self.range.end.line + 1,
            self.range.end.column + 1
        )
    }

    pub fn read(&self) -> anyhow::Result<String> {
        // TODO: optimize this function, should not read the whole file
        self.path
            .read()
            .map(|result| {
                // Return only the specified range
                result
                    .lines()
                    .enumerate()
                    .filter(|(line_index, _)| {
                        line_index >= &self.range.start.line && line_index <= &self.range.end.line
                    })
                    .map(|(_, line)| line)
                    .collect_vec()
                    .join("\n")
            })
            .map_err(|err| {
                anyhow::anyhow!(
                    "Failed to read file {}: {}",
                    self.path.display_absolute(),
                    err.to_string()
                )
            })
    }
}

impl TryFrom<lsp_types::Location> for Location {
    type Error = anyhow::Error;

    fn try_from(value: lsp_types::Location) -> Result<Self, Self::Error> {
        Ok(Location {
            path: value
                .uri
                .to_file_path()
                .map_err(|_| anyhow::anyhow!("Failed to convert uri to file path"))?
                .try_into()?,
            range: value.range.start.into()..value.range.end.into(),
        })
    }
}

impl PartialOrd for Location {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.path.partial_cmp(&other.path).map(|ord| match ord {
            std::cmp::Ordering::Equal => self.range.start.cmp(&other.range.start),
            _ => ord,
        })
    }
}

impl Ord for Location {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickfixListType {
    LspDiagnostic(Option<DiagnosticSeverity>),
    Items(Vec<QuickfixListItem>),
}

#[cfg(test)]
mod test_quickfix_list {
    use crate::{components::suggestive_editor::Info, position::Position};

    use super::{Location, QuickfixList, QuickfixListItem};
    use pretty_assertions::assert_eq;

    #[test]
    fn should_sort_items() {
        let foo = QuickfixListItem {
            location: Location {
                path: ".gitignore".try_into().unwrap(),
                range: Position { line: 1, column: 2 }..Position { line: 1, column: 3 },
            },
            info: None,
        };
        let bar = QuickfixListItem {
            location: Location {
                path: "readme.md".try_into().unwrap(),
                range: Position { line: 1, column: 1 }..Position { line: 1, column: 2 },
            },
            info: None,
        };
        let spam = QuickfixListItem {
            location: Location {
                path: ".gitignore".try_into().unwrap(),
                range: Position { line: 1, column: 1 }..Position { line: 1, column: 2 },
            },
            info: None,
        };
        let quickfix_list = QuickfixList::new(vec![foo.clone(), bar.clone(), spam.clone()]);

        assert_eq!(quickfix_list.items, vec![spam, foo, bar])
    }

    #[test]
    fn should_merge_items_of_same_location() {
        let items = [
            QuickfixListItem {
                location: Location {
                    path: "readme.md".try_into().unwrap(),
                    range: Position { line: 1, column: 1 }..Position { line: 1, column: 2 },
                },
                info: Some(Info::new("spongebob".to_string())),
            },
            QuickfixListItem {
                location: Location {
                    path: "readme.md".try_into().unwrap(),
                    range: Position { line: 1, column: 1 }..Position { line: 1, column: 2 },
                },
                info: Some(Info::new("squarepants".to_string())),
            },
        ]
        .to_vec();

        let quickfix_list = QuickfixList::new(items);

        assert_eq!(
            quickfix_list.items,
            vec![QuickfixListItem {
                location: Location {
                    path: "readme.md".try_into().unwrap(),
                    range: Position { line: 1, column: 1 }..Position { line: 1, column: 2 },
                },
                info: Some(Info::new(
                    ["spongebob", "squarepants"].join("\n==========\n")
                ))
            }]
        )
    }
}
