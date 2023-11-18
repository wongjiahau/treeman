use crate::app::Dispatch;
use crate::components::component::Component;
use crate::components::editor::Movement;
use crate::context::Context;

use itertools::Itertools;
use std::cell::RefCell;
use std::rc::Rc;

use super::component::ComponentId;
use super::editor::Editor;
use super::suggestive_editor::Info;

pub trait DropdownItem: Clone + std::fmt::Debug + Ord {
    fn emoji(&self) -> String {
        String::new()
    }
    fn label(&self) -> String;
    fn group(&self) -> String;
    fn info(&self) -> Option<Info>;
}

impl DropdownItem for String {
    fn label(&self) -> String {
        self.clone()
    }

    fn info(&self) -> Option<Info> {
        None
    }

    fn group(&self) -> String {
        self.clone()
    }
}

pub struct Dropdown<T: DropdownItem> {
    editor: Editor,
    filter: String,
    items: Vec<T>,
    filtered_items: Vec<T>,
    current_item_index: usize,
    info_panel: Option<Rc<RefCell<Editor>>>,
}

pub struct DropdownConfig {
    pub title: String,
}

impl<T: DropdownItem> Dropdown<T> {
    pub fn new(config: DropdownConfig) -> Self {
        let mut editor = Editor::from_text(tree_sitter_md::language(), "");
        editor.set_title(config.title);
        let mut dropdown = Self {
            editor,
            filter: String::new(),
            items: vec![],
            filtered_items: vec![],
            current_item_index: 0,
            info_panel: None,
        };
        dropdown.update_editor();
        dropdown
    }

    pub fn change_index(&mut self, index: usize) -> Option<T> {
        if !(0..self.items.len()).contains(&index) {
            return self.current_item();
        }
        self.current_item_index = index;
        self.editor.select_line_at(self.current_item_index).ok()?;
        self.show_current_item()
    }

    pub fn next_item(&mut self) -> Option<T> {
        self.change_index(self.current_item_index + 1)
    }

    pub fn previous_item(&mut self) -> Option<T> {
        self.change_index(self.current_item_index.saturating_sub(1))
    }

    fn last_item(&mut self) -> Option<T> {
        self.change_index(self.items.len().saturating_sub(1))
    }

    fn first_item(&mut self) -> Option<T> {
        self.change_index(0)
    }

    fn groups(&self) -> Vec<String> {
        self.filtered_items
            .iter()
            .map(DropdownItem::group)
            .unique()
            .sorted()
            .collect_vec()
    }

    fn change_group_index(&mut self, increment: bool) -> Option<T> {
        let current_group = self.current_item()?.group();
        let groups = self.groups();
        let (current_group_index, _) = groups
            .iter()
            .find_position(|group| group == &&current_group)?;
        let new_group_index = if increment {
            current_group_index.saturating_add(1)
        } else {
            current_group_index.saturating_sub(1)
        };
        let new_group = groups.get(new_group_index)?;
        let (new_item_index, _) = self
            .filtered_items
            .iter()
            .find_position(|item| &item.group() == new_group)?;
        self.change_index(new_item_index)
    }

    fn next_group(&mut self) -> Option<T> {
        self.change_group_index(true)
    }

    fn previous_group(&mut self) -> Option<T> {
        self.change_group_index(false)
    }

    pub fn show_current_item(&mut self) -> Option<T> {
        self.filtered_items
            .get(self.current_item_index)
            .cloned()
            .map(|item| {
                let info = item.info();
                self.show_info(info);
                item
            })
    }

    pub fn current_item(&self) -> Option<T> {
        self.filtered_items.get(self.current_item_index).cloned()
    }

    pub fn set_items(&mut self, items: Vec<T>) {
        self.items = items;
        self.current_item_index = 0;
        self.compute_filtered_items();
        self.update_editor();
    }

    fn compute_filtered_items(&mut self) {
        self.filtered_items = self
            .items
            .iter()
            .filter(|item| {
                item.label()
                    .to_lowercase()
                    .contains(&self.filter.to_lowercase())
            })
            .cloned()
            .sorted_by(|a, b| {
                let ord = a.group().cmp(&b.group());
                match ord {
                    std::cmp::Ordering::Equal => a.label().cmp(&b.label()),
                    _ => ord,
                }
            })
            .collect();

        self.show_current_item();
    }

    pub fn set_filter(&mut self, filter: &str) -> anyhow::Result<()> {
        self.filter = filter.to_string();
        self.current_item_index = 0;
        self.compute_filtered_items();
        self.update_editor()
    }

    fn update_editor(&mut self) -> anyhow::Result<()> {
        self.editor.set_content(
            &self
                .filtered_items
                .iter()
                .map(|item| {
                    if item.emoji().is_empty() {
                        item.label()
                    } else {
                        format!("{} {}", item.emoji(), item.label())
                    }
                })
                .collect::<Vec<String>>()
                .join("\n"),
        )?;

        self.editor.select_line_at(0)?;
        Ok(())
    }

    fn show_info(&mut self, info: Option<Info>) -> anyhow::Result<()> {
        match info {
            Some(info) => {
                let info_panel = match self.info_panel.take() {
                    Some(info_panel) => info_panel,
                    None => Rc::new(RefCell::new(Editor::from_text(
                        tree_sitter_md::language(),
                        "INFO",
                    ))),
                };

                info_panel.borrow_mut().show_info(info);
                self.info_panel = Some(info_panel);
            }
            _ => self.info_panel = None,
        }
        Ok(())
    }

    pub fn get_item(&mut self, movement: Movement) -> Option<T> {
        match movement {
            Movement::Next => self.next_item(),
            Movement::Current => self.current_item(),
            Movement::Previous => self.previous_item(),
            Movement::Last => self.last_item(),
            Movement::First => self.first_item(),
            Movement::Up => self.previous_group(),
            Movement::Down => self.next_group(),
            _ => None,
        }
    }

    pub fn filtered_items(&self) -> &Vec<T> {
        &self.filtered_items
    }
}

impl<T: DropdownItem + 'static> Component for Dropdown<T> {
    fn editor(&self) -> &Editor {
        &self.editor
    }

    fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    fn handle_key_event(
        &mut self,
        context: &Context,
        event: event::KeyEvent,
    ) -> anyhow::Result<Vec<Dispatch>> {
        self.editor.handle_key_event(context, event)
    }

    fn children(&self) -> Vec<Option<Rc<RefCell<dyn Component>>>> {
        vec![self
            .info_panel
            .clone()
            .map(|info_panel| Some(info_panel as Rc<RefCell<dyn Component>>))
            .unwrap_or_default()]
    }

    fn remove_child(&mut self, component_id: ComponentId) {
        if matches!(self.info_panel, Some(ref info_panel) if info_panel.borrow().id() == component_id)
        {
            self.info_panel = None;
        }
    }
}

#[cfg(test)]
mod test_dropdown {
    use crate::{
        components::{
            dropdown::{Dropdown, DropdownConfig, DropdownItem},
            suggestive_editor::Info,
        },
        selection::CharIndex,
    };
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Item {
        label: String,
        info: Info,
        group: String,
    }
    impl PartialOrd for Item {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            self.label.partial_cmp(&other.label)
        }
    }

    impl Ord for Item {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.label.cmp(&other.label)
        }
    }

    impl Item {
        fn new(label: &str, info: &str, group: &str) -> Self {
            Self {
                label: label.to_string(),
                info: Info::new(info.to_string()),
                group: group.to_string(),
            }
        }
    }

    impl DropdownItem for Item {
        fn label(&self) -> String {
            self.label.to_string()
        }

        fn info(&self) -> Option<Info> {
            Some(self.info.clone())
        }

        fn group(&self) -> String {
            self.group.clone()
        }
    }

    #[test]
    fn test_next_prev_group() {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        dropdown.set_items(
            [
                Item::new("a", "", "1"),
                Item::new("d", "", "2"),
                Item::new("c", "", "2"),
                Item::new("b", "", "3"),
            ]
            .to_vec(),
        );

        // Expect the items are sorted by group first, then by label
        assert_eq!(
            dropdown.editor.buffer().rope().to_string(),
            "a\nc\nd\nb".to_string()
        );

        assert_eq!(dropdown.current_item().unwrap().label(), "a");

        dropdown.next_group();
        assert_eq!(dropdown.current_item().unwrap().label(), "c");
        dropdown.next_group();
        assert_eq!(dropdown.current_item().unwrap().label(), "b");

        dropdown.previous_group();
        assert_eq!(dropdown.current_item().unwrap().label(), "c");
        dropdown.previous_group();
        assert_eq!(dropdown.current_item().unwrap().label(), "a");
    }

    #[test]
    fn test_dropdown() -> anyhow::Result<()> {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        dropdown.set_items(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        assert_eq!(dropdown.current_item().unwrap().label(), "a");
        assert_eq!(
            dropdown.editor.buffer().rope().to_string(),
            "a\nb\nc".to_string()
        );
        assert_eq!(dropdown.editor.get_selected_texts(), vec!["a\n"]);
        assert_eq!(
            dropdown.editor.selection_set.primary.extended_range(),
            (CharIndex(0)..CharIndex(2)).into()
        );
        dropdown.next_item();
        assert_eq!(dropdown.current_item().unwrap().label(), "b");
        dropdown.next_item();
        assert_eq!(dropdown.current_item().unwrap().label(), "c");
        dropdown.next_item();
        assert_eq!(dropdown.current_item().unwrap().label(), "c");

        dropdown.previous_item();
        assert_eq!(dropdown.current_item().unwrap().label(), "b");
        dropdown.previous_item();
        assert_eq!(dropdown.current_item().unwrap().label(), "a");
        dropdown.previous_item();
        assert_eq!(dropdown.current_item().unwrap().label(), "a");

        dropdown.set_filter("b")?;
        assert_eq!(dropdown.current_item().unwrap().label(), "b");
        dropdown.set_filter("c")?;
        assert_eq!(dropdown.current_item().unwrap().label(), "c");
        dropdown.set_filter("d")?;
        assert_eq!(dropdown.current_item(), None);

        dropdown.set_filter("")?;
        assert_eq!(dropdown.current_item().unwrap().label(), "a");
        dropdown.next_item();
        assert_eq!(dropdown.current_item().unwrap().label(), "b");

        dropdown.set_items(vec![
            "lorem".to_string(),
            "ipsum".to_string(),
            "dolor".to_string(),
        ]);

        // The current item should be `dolor` because dropdown will sort the items
        assert_eq!(dropdown.current_item().unwrap().label(), "dolor");
        assert_eq!(dropdown.editor.current_line().unwrap(), "dolor");
        dropdown.next_item();
        assert_eq!(dropdown.current_item().unwrap().label(), "ipsum");
        assert_eq!(dropdown.editor.current_line().unwrap(), "ipsum");
        Ok(())
    }

    #[test]
    fn filter_should_work_regardless_of_case() -> anyhow::Result<()> {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        dropdown.set_items(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        dropdown.set_filter("A")?;
        assert_eq!(dropdown.current_item().unwrap().label(), "a");
        Ok(())
    }

    #[test]
    fn setting_filter_should_show_info_of_the_new_first_item() -> anyhow::Result<()> {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        dropdown.set_items(vec![
            Item::new("a", "info a", ""),
            Item::new("b", "info b", ""),
            Item::new("c", "info c", ""),
        ]);

        assert_eq!(dropdown.current_item().unwrap().label(), "a");
        assert_eq!(
            dropdown.info_panel.as_ref().unwrap().borrow().text(),
            "info a"
        );

        dropdown.set_filter("b")?;

        assert_eq!(dropdown.current_item().unwrap().label(), "b");
        assert_eq!(
            dropdown.info_panel.as_ref().unwrap().borrow().text(),
            "info b"
        );
        Ok(())
    }
}
