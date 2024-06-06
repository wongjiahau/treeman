use std::cmp::Reverse;

use crate::{
    app::Dispatches, components::editor::Movement, lsp::completion::CompletionSource,
    position::Position,
};

use itertools::Itertools;

use nucleo_matcher::Utf32Str;
use shared::{canonicalized_path::CanonicalizedPath, icons::get_icon_config};

use super::suggestive_editor::{Decoration, Info};

#[derive(Clone, Debug, PartialEq, Eq)]
/// Note: filtering will be done on the combination of `display` and `group` (if applicable)
pub(crate) struct DropdownItem {
    pub(crate) dispatches: Dispatches,
    source: CompletionSource,
    display: String,
    group: Option<String>,
    info: Option<Info>,
    /// Sorting will be based on `rank` if defined, otherwise sorting will be based on `display`
    rank: Option<Box<[usize]>>,
}

impl DropdownItem {
    pub(crate) fn display(&self) -> String {
        self.display.clone()
    }

    pub(crate) fn new(display: String) -> Self {
        Self {
            dispatches: Default::default(),
            display,
            group: Default::default(),
            info: Default::default(),
            rank: None,
            source: CompletionSource::Null,
        }
    }

    pub(crate) fn set_info(self, info: Option<Info>) -> Self {
        Self { info, ..self }
    }

    pub(crate) fn set_dispatches(self, dispatches: Dispatches) -> DropdownItem {
        Self { dispatches, ..self }
    }

    pub(crate) fn set_group(self, group: Option<String>) -> Self {
        Self { group, ..self }
    }

    pub(crate) fn set_rank(self, rank: Option<Box<[usize]>>) -> DropdownItem {
        Self { rank, ..self }
    }

    pub(crate) fn set_source(self, source: CompletionSource) -> DropdownItem {
        Self { source, ..self }
    }
}

impl From<CanonicalizedPath> for DropdownItem {
    fn from(value: CanonicalizedPath) -> Self {
        DropdownItem::new({
            let name = value
                .to_path_buf()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let icon = value.icon();
            format!("{icon} {name}")
        })
        .set_group(value.parent().ok().flatten().map(|parent| {
            format!(
                "{} {}",
                get_icon_config().folder,
                parent.try_display_relative()
            )
        }))
        .set_dispatches(Dispatches::one(crate::app::Dispatch::OpenFile(value)))
    }
}

impl From<String> for DropdownItem {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

pub(crate) struct Dropdown {
    title: String,
    filter: String,
    items: Vec<DropdownItem>,
    filtered_item_groups: Vec<FilteredDropdownItemGroup>,
    current_item_index: usize,
}

pub(crate) struct DropdownConfig {
    pub(crate) title: String,
}

impl Dropdown {
    pub(crate) fn new(config: DropdownConfig) -> Self {
        Self {
            filter: String::new(),
            items: vec![],
            filtered_item_groups: vec![],
            current_item_index: 0,
            title: config.title,
        }
    }

    pub(crate) fn change_index(&mut self, index: usize) {
        if !self.filtered_item_groups.iter().any(|group| {
            group
                .items
                .iter()
                .any(|item| item.item_index as usize == index)
        }) {
            return;
        }
        self.current_item_index = index;
    }

    fn current_item_line_index(&self) -> usize {
        self.item_line_index(self.current_item_index)
    }

    fn item_line_index(&self, item_index: usize) -> usize {
        let group_title_size = self
            .filtered_item_groups
            .first()
            .and_then(|group| group.items.first())
            .and_then(|item| item.item.group.as_ref().map(|_| 1))
            .unwrap_or(0);
        let gap_size = 1;
        let group_index = self.get_item_group_index(item_index).unwrap_or(0);
        let group_gap = group_index * gap_size;

        item_index + group_index * group_title_size + group_gap + group_title_size
    }

    pub(crate) fn next_item(&mut self) {
        self.change_index(self.current_item_index + 1)
    }

    pub(crate) fn previous_item(&mut self) {
        self.change_index(self.current_item_index.saturating_sub(1))
    }

    fn last_item(&mut self) {
        if let Some(index) = self
            .filtered_item_groups
            .last()
            .and_then(|item| item.items.last())
            .map(|item| item.item_index)
        {
            self.change_index(index as usize)
        }
    }

    fn first_item(&mut self) {
        self.change_index(0)
    }

    fn groups(&self) -> Option<Vec<String>> {
        let groups = self
            .filtered_item_groups
            .iter()
            .filter_map(|group| group.group_key.clone())
            .collect_vec();
        if groups.is_empty() {
            None
        } else {
            Some(groups)
        }
    }

    fn get_current_item_group_index(&self) -> Option<usize> {
        self.get_item_group_index(self.current_item_index)
    }

    fn get_item_group_index(&self, item_index: usize) -> Option<usize> {
        self.filtered_item_groups
            .iter()
            .enumerate()
            .find(|(_, group)| {
                group
                    .items
                    .iter()
                    .any(|item| item.item_index as usize == item_index)
            })
            .map(|(index, _)| index)
    }

    fn change_group_index(&mut self, increment: bool) -> Option<()> {
        let groups = self.groups()?;
        let current_group_index = self.get_current_item_group_index()?;
        let new_group_index = if increment {
            current_group_index.saturating_add(1)
        } else {
            current_group_index.saturating_sub(1)
        };
        let new_group = groups.get(new_group_index)?;
        let (new_item_index, _) = self
            .filtered_item_groups
            .iter()
            .flat_map(|group| &group.items)
            .find_position(|item| item.item.group.as_ref() == Some(new_group))?;
        self.change_index(new_item_index);
        Some(())
    }

    fn next_group(&mut self) {
        self.change_group_index(true).unwrap_or_default()
    }

    fn previous_group(&mut self) {
        self.change_group_index(false).unwrap_or_default()
    }

    pub(crate) fn current_item(&self) -> Option<DropdownItem> {
        self.get_item_by_index(self.current_item_index)
    }

    fn get_item_by_index(&self, item_index: usize) -> Option<DropdownItem> {
        self.filtered_item_groups
            .iter()
            .flat_map(|group| &group.items)
            .nth(item_index)
            .map(|item| item.item.clone())
    }

    pub(crate) fn set_items(&mut self, items: Vec<DropdownItem>) {
        self.items = items;
        self.current_item_index = 0;
        self.compute_filtered_items();
    }

    fn compute_filtered_items(&mut self) {
        use nucleo_matcher::{
            pattern::{CaseMatching, Normalization, Pattern},
            Config, Matcher,
        };
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(&self.filter, CaseMatching::Ignore, Normalization::Smart);
        let mut haystack = Vec::new();
        let matches = self.items.iter().filter_map(|item| {
            let score = pattern
                .atoms
                .iter()
                .map(|atom| {
                    let score_group = item.group.as_ref().and_then(|group| {
                        haystack.clear();
                        atom.score(Utf32Str::new(group, &mut haystack), &mut matcher)
                    });
                    let score_display = {
                        haystack.clear();
                        atom.score(Utf32Str::new(&item.display, &mut haystack), &mut matcher)
                    };
                    match (score_group, score_display) {
                        (None, None) => None,
                        (None, Some(score)) | (Some(score), None) => Some(score),
                        (Some(a), Some(b)) => Some(a + b),
                    }
                })
                .try_fold(0, |total_score, score| Some(total_score + score?))?;
            Some((item, score as u32))
        });
        let mut haystack_buf = Vec::new();
        let mut matched_char_indices = Vec::new();
        let mut item_index = 0;
        /// This struct is necessary because these item can only be indexed after sorting
        struct FilteredDropdownItemWithoutIndex {
            item: DropdownItem,
            fuzzy_score: u32,
            fuzzy_matched_char_indices: Vec<u32>,
        }
        struct FilteredDropdownItemGroupWithoutIndex {
            group_key: Option<String>,
            items: Vec<FilteredDropdownItemWithoutIndex>,
            fuzzy_matched_char_indices: Vec<u32>,
        }
        self.filtered_item_groups = matches
            .into_iter()
            .sorted_by_key(|(item, _)| item.group.clone())
            // Sort by group first
            .group_by(|(item, _)| item.group.clone())
            .into_iter()
            .map(|(group_key, items)| {
                let items = items.collect_vec();
                let group_matched_char_indices = group_key
                    .as_ref()
                    .map(|group| {
                        haystack_buf.clear();
                        let haystack = Utf32Str::new(group, &mut haystack_buf);
                        matched_char_indices.clear();
                        pattern.atoms.iter().for_each(|atom| {
                            let _ = atom.indices(haystack, &mut matcher, &mut matched_char_indices);
                        });
                        matched_char_indices.clone()
                    })
                    .unwrap_or_default();
                let items = items
                    .into_iter()
                    .map(|(item, fuzzy_score)| {
                        haystack_buf.clear();
                        let haystack = Utf32Str::new(&item.display, &mut haystack_buf);
                        matched_char_indices.clear();
                        pattern.atoms.iter().for_each(|atom| {
                            let _ = atom.indices(haystack, &mut matcher, &mut matched_char_indices);
                        });

                        FilteredDropdownItemWithoutIndex {
                            item: item.clone(),
                            fuzzy_score,
                            fuzzy_matched_char_indices: matched_char_indices.clone(),
                        }
                    })
                    .sorted_by_key(|item| {
                        (
                            // Sort by source first
                            item.item.source.clone(),
                            // Sort by fuzzy score first
                            Reverse(item.fuzzy_score),
                            // Then sort by rank
                            item.item.rank.clone(),
                            // Then, shortest display should come first (for better UX of autocomplete)
                            item.item.display.len(),
                            // Then only sort lexicographically
                            item.item.display.clone(),
                        )
                    })
                    .collect_vec();

                FilteredDropdownItemGroupWithoutIndex {
                    group_key,
                    items,
                    fuzzy_matched_char_indices: group_matched_char_indices,
                }
            })
            // Then sort the group by the best fuzzy score of each group
            .sorted_by_key(|group| {
                (
                    Reverse(
                        group
                            .items
                            .iter()
                            .map(|item| item.fuzzy_score)
                            .max()
                            .unwrap_or_default(),
                    ),
                    group.group_key.clone(),
                )
            })
            .map(
                |FilteredDropdownItemGroupWithoutIndex {
                     group_key,
                     items,
                     fuzzy_matched_char_indices,
                 }| {
                    FilteredDropdownItemGroup {
                        group_key,
                        items: items
                            .into_iter()
                            .map(
                                |FilteredDropdownItemWithoutIndex {
                                     item,
                                     fuzzy_score,
                                     fuzzy_matched_char_indices,
                                 }| FilteredDropdownItem {
                                    item,
                                    // Remember that the index can only be assigned after all sorting
                                    // No sorting should be done after indexing
                                    item_index: {
                                        let result = item_index;
                                        item_index += 1;
                                        result
                                    },
                                    fuzzy_score,
                                    fuzzy_matched_char_indices,
                                },
                            )
                            .collect(),
                        fuzzy_matched_char_indices,
                    }
                },
            )
            .collect_vec();
    }

    pub(crate) fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.current_item_index = 0;
        self.compute_filtered_items();
    }

    pub(crate) fn render(&self) -> DropdownRender {
        DropdownRender {
            title: self.title.clone(),
            content: self.content(),
            decorations: self.decorations(),
            highlight_line_index: self.current_item_line_index(),
            info: self.current_item().and_then(|item| item.info),
        }
    }

    fn content(&self) -> String {
        self.filtered_item_groups
            .iter()
            .map(|group| {
                if let Some(group_key) = group.group_key.as_ref() {
                    let items_len = group.items.len();
                    let items = group
                        .items
                        .iter()
                        .enumerate()
                        .map(|(index, item)| {
                            let content = item.item.display();
                            let indicator = if index == items_len.saturating_sub(1) {
                                "└─"
                            } else {
                                "├─"
                            };
                            format!(" {} {}", indicator, content)
                        })
                        .join("\n");
                    format!("■┬ {}\n{}", group_key, items)
                } else {
                    group
                        .items
                        .iter()
                        .map(|item| item.item.display())
                        .join("\n")
                }
            })
            .collect::<Vec<String>>()
            .join("\n\n")
    }

    pub(crate) fn apply_movement(&mut self, movement: Movement) {
        match movement {
            Movement::Next => self.next_item(),
            Movement::Current => {}
            Movement::Previous => self.previous_item(),
            Movement::Last => self.last_item(),
            Movement::First => self.first_item(),
            Movement::Up => self.previous_group(),
            Movement::Down => self.next_group(),
            _ => {}
        }
    }

    #[cfg(test)]
    fn assert_highlighted_content(&self, label: &str) {
        let render = self.render();
        let index = render.highlight_line_index;
        let highlighed_content = render.content.lines().collect_vec()[index];
        assert_eq!(highlighed_content, label);
    }

    pub(crate) fn items(&self) -> Vec<DropdownItem> {
        self.items.clone()
    }

    pub(crate) fn set_current_item_index(&mut self, current_item_index: usize) {
        self.current_item_index = current_item_index
    }

    pub(crate) fn current_item_index(&self) -> usize {
        self.current_item_index
    }

    fn decorations(&self) -> Vec<Decoration> {
        self.filtered_item_groups
            .iter()
            .flat_map(|group| {
                let group_decorations = {
                    let line_index = group
                        .items
                        .first()
                        .map(|item| {
                            self.item_line_index(item.item_index as usize)
                                .saturating_sub(1)
                        })
                        .unwrap_or_default();
                    let pad_left = 3;
                    group
                        .fuzzy_matched_char_indices
                        .iter()
                        .map(move |matched_char_index| {
                            let column_index = (matched_char_index + pad_left) as usize;
                            Decoration::new(
                                crate::selection_range::SelectionRange::Position(
                                    Position {
                                        line: line_index,
                                        column: column_index,
                                    }..Position {
                                        line: line_index,
                                        column: column_index + 1,
                                    },
                                ),
                                crate::grid::StyleKey::UiFuzzyMatchedChar,
                            )
                        })
                };
                let display_decorations = group.items.iter().flat_map(|item| {
                    let line_index = self.item_line_index(item.item_index as usize);
                    let pad_left = if item.item.group.is_some() { 4 } else { 0 };
                    item.fuzzy_matched_char_indices
                        .iter()
                        .map(move |matched_char_index| {
                            let column_index = (matched_char_index + pad_left) as usize;
                            Decoration::new(
                                crate::selection_range::SelectionRange::Position(
                                    Position {
                                        line: line_index,
                                        column: column_index,
                                    }..Position {
                                        line: line_index,
                                        column: column_index + 1,
                                    },
                                ),
                                crate::grid::StyleKey::UiFuzzyMatchedChar,
                            )
                        })
                });
                group_decorations.chain(display_decorations)
            })
            .collect_vec()
    }

    pub(crate) fn no_matching_candidates(&self) -> bool {
        self.filtered_item_groups.is_empty()
    }
}

#[cfg(test)]
mod test_dropdown {
    use itertools::Itertools as _;
    use quickcheck_macros::quickcheck;

    use crate::{
        components::{
            dropdown::{Dropdown, DropdownConfig, DropdownItem},
            suggestive_editor::{Decoration, Info},
        },
        position::Position,
        selection_range::SelectionRange,
    };
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Item {
        label: String,
        info: Info,
        group: String,
    }
    impl PartialOrd for Item {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
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
                info: Info::new("".to_string(), info.to_string()),
                group: group.to_string(),
            }
        }
    }
    impl From<Item> for DropdownItem {
        fn from(value: Item) -> Self {
            Self::new(value.label.to_string())
                .set_info(Some(value.info.clone()))
                .set_group(Some(value.group.clone()))
        }
    }

    #[test]
    fn fuzzy_match_chars_decorations_more_than_one_items() {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        dropdown.set_items(
            ["bytes_offset".to_string(), "len_bytes".to_string()]
                .into_iter()
                .map(|s| s.into())
                .collect(),
        );
        dropdown.set_filter("byts");
        assert_eq!(
            dropdown.decorations(),
            [
                (0, 0),
                (0, 1),
                (0, 2),
                (0, 4),
                (1, 4),
                (1, 5),
                (1, 6),
                (1, 8),
            ]
            .into_iter()
            .map(|(line, column)| {
                Decoration::new(
                    SelectionRange::Position(
                        Position::new(line, column)..Position::new(line, column + 1),
                    ),
                    crate::grid::StyleKey::UiFuzzyMatchedChar,
                )
            })
            .collect_vec(),
        )
    }

    #[test]
    fn fuzzy_match_chars_decorations_multiple_search_terms() {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        dropdown.set_items(
            ["bytes_offset".to_string()]
                .into_iter()
                .map(|s| s.into())
                .collect(),
        );
        dropdown.set_filter("byt off");
        assert_eq!(
            dropdown.decorations(),
            [(0, 0), (0, 1), (0, 2), (0, 6), (0, 7), (0, 8),]
                .into_iter()
                .map(|(line, column)| {
                    Decoration::new(
                        SelectionRange::Position(
                            Position::new(line, column)..Position::new(line, column + 1),
                        ),
                        crate::grid::StyleKey::UiFuzzyMatchedChar,
                    )
                })
                .collect_vec(),
        )
    }

    #[test]
    fn fuzzy_match_chars_decorations_highlight_group() {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        dropdown.set_items(
            [Item::new("my_item", "", "group")]
                .into_iter()
                .map(|s| s.into())
                .collect(),
        );
        dropdown.set_filter("my gro");
        assert_eq!(
            dropdown.decorations(),
            [
                // g r o
                (0, 3),
                (0, 4),
                (0, 5),
                // m y
                (1, 4),
                (1, 5)
            ]
            .into_iter()
            .map(|(line, column)| {
                Decoration::new(
                    SelectionRange::Position(
                        Position::new(line, column)..Position::new(line, column + 1),
                    ),
                    crate::grid::StyleKey::UiFuzzyMatchedChar,
                )
            })
            .collect_vec(),
        )
    }

    #[test]
    fn test_next_prev_group() {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        let item_a = Item::new("a", "", "1");
        dropdown.set_items(
            [
                item_a.clone(),
                Item::new("d", "", "2"),
                Item::new("c", "", "2"),
                Item::new("b", "", "3"),
            ]
            .into_iter()
            .map(|item| item.into())
            .collect(),
        );

        // Expect the items are sorted by group first, then by label

        assert_eq!(
            dropdown.render().content.trim(),
            "
■┬ 1
 └─ a

■┬ 2
 ├─ c
 └─ d

■┬ 3
 └─ b
"
            .trim()
        );
        dropdown.assert_highlighted_content(" └─ a");

        dropdown.next_group();
        dropdown.assert_highlighted_content(" ├─ c");
        dropdown.next_group();
        dropdown.assert_highlighted_content(" └─ b");

        dropdown.previous_group();
        dropdown.assert_highlighted_content(" ├─ c");
        dropdown.previous_group();
        dropdown.assert_highlighted_content(" └─ a");
    }

    #[test]
    fn test_dropdown_without_group() -> anyhow::Result<()> {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        dropdown.set_items(
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
                .into_iter()
                .map(|s| s.into())
                .collect(),
        );
        assert_eq!(dropdown.render().content, "a\nb\nc");
        dropdown.assert_highlighted_content("a");
        dropdown.next_item();
        dropdown.assert_highlighted_content("b");
        dropdown.next_item();
        dropdown.assert_highlighted_content("c");
        dropdown.next_item();
        dropdown.assert_highlighted_content("c");

        dropdown.previous_item();
        dropdown.assert_highlighted_content("b");
        dropdown.previous_item();
        dropdown.assert_highlighted_content("a");
        dropdown.previous_item();
        dropdown.assert_highlighted_content("a");

        dropdown.set_filter("b");
        dropdown.assert_highlighted_content("b");
        dropdown.set_filter("c");
        dropdown.assert_highlighted_content("c");
        dropdown.set_filter("d");
        assert_eq!(dropdown.current_item(), None);

        dropdown.set_filter("");
        dropdown.assert_highlighted_content("a");
        dropdown.next_item();
        dropdown.assert_highlighted_content("b");

        dropdown.set_items(
            vec![
                "lorem".to_string(),
                "ipsum".to_string(),
                "dolor".to_string(),
            ]
            .into_iter()
            .map(|s| s.into())
            .collect(),
        );

        // The current item should be `dolor` because dropdown will sort the items
        dropdown.assert_highlighted_content("dolor");
        dropdown.next_item();
        dropdown.assert_highlighted_content("ipsum");

        Ok(())
    }

    #[test]
    fn filter_should_work_regardless_of_case() -> anyhow::Result<()> {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        dropdown.set_items(
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
                .into_iter()
                .map(|s| s.into())
                .collect(),
        );
        dropdown.set_filter("A");
        assert_eq!(dropdown.current_item().unwrap().display, "a");
        Ok(())
    }

    #[test]
    fn setting_filter_should_show_info_of_the_new_first_item() -> anyhow::Result<()> {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        dropdown.set_items(
            vec![
                Item::new("a", "info a", ""),
                Item::new("b", "info b", ""),
                Item::new("c", "info c", ""),
            ]
            .into_iter()
            .map(|s| s.into())
            .collect(),
        );

        assert_eq!(dropdown.current_item().unwrap().display, "a");
        assert_eq!(dropdown.render().info.unwrap().content(), "info a");

        dropdown.set_filter("b");

        assert_eq!(dropdown.current_item().unwrap().display, "b");
        assert_eq!(dropdown.render().info.unwrap().content(), "info b");
        Ok(())
    }

    #[test]
    /// 1. Group items by their group
    /// 2. Sort the items of each group, by fuzzy score desc, followed by rank asc, display asc
    /// 3. Rank each group by their highest fuzzy score item desc, followed by group name asc
    fn items_sorting() -> anyhow::Result<()> {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "test".to_string(),
        });
        let items = [
            Item::new("test_redditor", "", "z"),
            Item::new("test_reddit", "", "c"),
            Item::new("test_editor", "", "z"),
            Item::new("patrick", "", "e"),
            Item::new("crab", "", "c"),
            Item::new("patrick", "", "d"),
        ];
        dropdown.set_items(items.clone().into_iter().map(|s| s.into()).collect());
        dropdown.set_filter("test edit");
        assert_eq!(
            dropdown
                .filtered_item_groups
                .clone()
                .into_iter()
                .flat_map(|group| group.items)
                .map(|item| item.item.display)
                .collect_vec(),
            &[
                "test_editor",
                "test_redditor",
                // "test_reddit" is ranked lower than "test_redditor" although it's fuzzy score is higher,
                // because "test_reddit" score is lowest that the fuzzy score of the highest fuzzy score of "test_redditor"'s group
                "test_reddit"
            ]
        );

        dropdown.set_filter("");

        // When fuzzy rank is the same across all items, sort by their group name, then by their display
        let expected = items
            .into_iter()
            .map(|item| -> DropdownItem { item.into() })
            .sorted_by_key(|item| (item.group.clone(), item.display.clone()))
            .collect_vec();
        assert_eq!(
            dropdown
                .filtered_item_groups
                .into_iter()
                .flat_map(|item| item.items)
                .map(|item| item.item)
                .collect_vec(),
            expected
        );
        Ok(())
    }

    #[derive(Debug, Clone)]
    struct DropdownItems(Vec<DropdownItem>);

    impl quickcheck::Arbitrary for DropdownItems {
        fn arbitrary(g: &mut quickcheck::Gen) -> Self {
            fn random_range(g: &mut quickcheck::Gen) -> std::ops::Range<i32> {
                1..*(g.choose((1..100).collect_vec().as_slice()).unwrap())
            }
            Self(
                random_range(g)
                    .map(|_| {
                        Item::new(
                            &String::arbitrary(g),
                            &String::arbitrary(g),
                            &String::arbitrary(g),
                        )
                        .into()
                    })
                    .collect_vec(),
            )
        }
    }

    #[quickcheck]
    fn filtered_item_index_should_tally_with_their_order(items: DropdownItems) -> bool {
        let mut dropdown = Dropdown::new(DropdownConfig {
            title: "hello".to_string(),
        });
        dropdown.set_items(items.0);
        let indices = dropdown
            .filtered_item_groups
            .iter()
            .flat_map(|group| &group.items)
            .map(|item| item.item_index as usize)
            .collect_vec();
        let order = dropdown
            .filtered_item_groups
            .iter()
            .flat_map(|group| &group.items)
            .enumerate()
            .map(|(index, _)| index)
            .collect_vec();
        indices == order
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DropdownRender {
    pub(crate) content: String,
    pub(crate) decorations: Vec<Decoration>,
    pub(crate) title: String,
    pub(crate) highlight_line_index: usize,
    pub(crate) info: Option<Info>,
}
#[derive(Debug, Clone, PartialEq)]
struct FilteredDropdownItem {
    item_index: u32,
    item: DropdownItem,
    fuzzy_score: u32,
    fuzzy_matched_char_indices: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
struct FilteredDropdownItemGroup {
    group_key: Option<String>,
    items: Vec<FilteredDropdownItem>,
    fuzzy_matched_char_indices: Vec<u32>,
}
