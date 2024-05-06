use crate::{
    app::{Dispatches, RequestParams, Scope, SelectionSetHistoryKind},
    buffer::Line,
    char_index_range::CharIndexRange,
    context::{Context, GlobalMode, LocalSearchConfigMode, Search},
    lsp::process::ResponseContext,
    selection::{Filter, Filters},
    selection_mode::{self, inside::InsideKind},
    transformation::Transformation,
};

use shared::{canonicalized_path::CanonicalizedPath, language::Language};
use std::{
    cell::{Ref, RefCell, RefMut},
    ops::Range,
    rc::Rc,
};

use crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use event::KeyEvent;
use itertools::{Either, Itertools};
use my_proc_macros::key;
use ropey::Rope;

use crate::{
    app::{Dimension, Dispatch},
    buffer::Buffer,
    components::component::Component,
    edit::{Action, ActionGroup, Edit, EditTransaction},
    lsp::completion::PositionalEdit,
    position::Position,
    rectangle::Rectangle,
    selection::{CharIndex, Selection, SelectionMode, SelectionSet},
};

use DispatchEditor::*;

use super::{
    component::ComponentId,
    dropdown::DropdownRender,
    render_editor::Source,
    suggestive_editor::{Decoration, Info},
};

#[derive(PartialEq, Clone, Debug, Eq)]
pub enum Mode {
    Normal,
    Insert,
    MultiCursor,
    FindOneChar,
    Exchange,
    UndoTree,
    Replace,
}

#[derive(PartialEq, Clone, Debug)]
pub struct Jump {
    pub character: char,
    pub selection: Selection,
}
const WINDOW_TITLE_HEIGHT: usize = 1;

impl Component for Editor {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn editor(&self) -> &Editor {
        self
    }

    fn editor_mut(&mut self) -> &mut Editor {
        self
    }

    fn set_content(&mut self, str: &str) -> Result<(), anyhow::Error> {
        self.update_buffer(str);
        self.clamp()
    }

    fn title(&self, context: &Context) -> String {
        context.current_working_directory();
        let title = self.title.clone();
        title
            .or_else(|| {
                let path = self.buffer().path()?;
                let current_working_directory = context.current_working_directory()?;
                let string = path
                    .display_relative_to(current_working_directory)
                    .unwrap_or_else(|_| path.display_absolute());
                let icon = path.icon();
                Some(format!(" {} {}", icon, string))
            })
            .unwrap_or_else(|| "[No title]".to_string())
    }

    fn set_title(&mut self, title: String) {
        self.title = Some(title);
    }

    fn handle_paste_event(&mut self, content: String) -> anyhow::Result<Dispatches> {
        self.insert(&content)
    }

    fn get_cursor_position(&self) -> anyhow::Result<Position> {
        self.buffer
            .borrow()
            .char_to_position(self.get_cursor_char_index())
    }

    fn scroll_offset(&self) -> u16 {
        self.scroll_offset
    }

    fn set_rectangle(&mut self, rectangle: Rectangle) {
        self.rectangle = rectangle;
        self.recalculate_scroll_offset();
    }

    fn rectangle(&self) -> &Rectangle {
        &self.rectangle
    }

    fn children(&self) -> Vec<Option<Rc<RefCell<dyn Component>>>> {
        vec![]
    }

    fn handle_key_event(
        &mut self,
        context: &Context,
        event: event::KeyEvent,
    ) -> anyhow::Result<Dispatches> {
        self.handle_key_event(context, event)
    }

    fn handle_mouse_event(
        &mut self,
        mouse_event: crossterm::event::MouseEvent,
    ) -> anyhow::Result<Dispatches> {
        const SCROLL_HEIGHT: usize = 1;
        match mouse_event.kind {
            MouseEventKind::ScrollUp => {
                self.apply_scroll(Direction::Start, SCROLL_HEIGHT);
                Ok(Default::default())
            }
            MouseEventKind::ScrollDown => {
                self.apply_scroll(Direction::End, SCROLL_HEIGHT);
                Ok(Default::default())
            }
            MouseEventKind::Down(MouseButton::Left) => Ok(Default::default()),
            _ => Ok(Default::default()),
        }
    }

    #[cfg(test)]
    fn handle_events(&mut self, events: &[event::KeyEvent]) -> anyhow::Result<Dispatches> {
        let context = Context::default();
        Ok(events
            .iter()
            .map(|event| -> anyhow::Result<_> {
                Ok(self.handle_key_event(&context, event.clone())?.into_vec())
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .into())
    }

    fn handle_event(
        &mut self,
        context: &Context,
        event: event::event::Event,
    ) -> anyhow::Result<Dispatches> {
        match event {
            event::event::Event::Key(event) => self.handle_key_event(context, event),
            event::event::Event::Paste(content) => self.handle_paste_event(content),
            event::event::Event::Mouse(event) => self.handle_mouse_event(event),
            _ => Ok(Default::default()),
        }
    }

    fn descendants(&self) -> Vec<Rc<RefCell<dyn Component>>> {
        self.children()
            .into_iter()
            .flatten()
            .flat_map(|component| {
                std::iter::once(component.clone()).chain(component.borrow().descendants())
            })
            .collect::<Vec<_>>()
    }
}

impl Clone for Editor {
    fn clone(&self) -> Self {
        Editor {
            mode: self.mode.clone(),
            selection_set: self.selection_set.clone(),
            jumps: None,
            cursor_direction: self.cursor_direction.clone(),
            scroll_offset: self.scroll_offset,
            rectangle: self.rectangle.clone(),
            buffer: self.buffer.clone(),
            title: self.title.clone(),
            id: self.id,
            current_view_alignment: None,
            regex_highlight_rules: Vec::new(),
        }
    }
}

pub struct Editor {
    pub mode: Mode,
    pub regex_highlight_rules: Vec<RegexHighlightRule>,

    pub selection_set: SelectionSet,

    pub jumps: Option<Vec<Jump>>,
    pub cursor_direction: Direction,

    /// This means the number of lines to be skipped from the top during rendering.
    /// 2 means the first line to be rendered on the screen if the 3rd line of the text.
    scroll_offset: u16,
    rectangle: Rectangle,

    buffer: Rc<RefCell<Buffer>>,
    title: Option<String>,
    id: ComponentId,
    pub current_view_alignment: Option<ViewAlignment>,
}

pub struct RegexHighlightRule {
    pub regex: regex::Regex,
    pub capture_styles: Vec<RegexHighlightRuleCaptureStyle>,
}

pub struct RegexHighlightRuleCaptureStyle {
    /// 0 means the entire match.
    /// Refer https://docs.rs/regex/latest/regex/struct.Regex.html#method.captures
    pub capture_name: &'static str,
    pub source: Source,
}
impl RegexHighlightRuleCaptureStyle {
    pub(crate) fn new(capture_name: &'static str, source: Source) -> Self {
        Self {
            capture_name,
            source,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    Start,
    End,
}

impl Direction {
    pub fn reverse(&self) -> Self {
        match self {
            Direction::Start => Direction::End,
            Direction::End => Direction::Start,
        }
    }

    #[cfg(test)]
    pub(crate) fn default() -> Direction {
        Direction::Start
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Movement {
    Next,
    Previous,
    Last,
    Current,
    Up,
    Down,
    First,
    /// 0-based
    Index(usize),
    Jump(CharIndexRange),
    ToParentLine,
    Parent,
    FirstChild,
}

impl Editor {
    /// Returns (hidden_parent_lines, visible_parent_lines)
    pub fn get_parent_lines(&self) -> anyhow::Result<(Vec<Line>, Vec<Line>)> {
        let position = self.get_cursor_position()?;

        let parent_lines = self.buffer().get_parent_lines(position.line)?;
        Ok(parent_lines
            .into_iter()
            .partition(|line| line.line < self.scroll_offset as usize))
    }

    pub fn show_info(&mut self, info: Info) -> Result<(), anyhow::Error> {
        self.set_title(info.title());
        self.set_decorations(info.decorations());
        self.set_content(info.content())
    }

    pub fn render_dropdown(
        &mut self,
        context: &mut Context,
        render: &DropdownRender,
    ) -> anyhow::Result<Dispatches> {
        self.apply_dispatches(
            context,
            [
                SetContent(render.content.clone()),
                SetDecorations(render.decorations.clone()),
                SelectLineAt(render.highlight_line_index),
            ]
            .to_vec(),
        )
    }

    pub fn from_text(language: tree_sitter::Language, text: &str) -> Self {
        Self {
            selection_set: SelectionSet {
                primary: Selection::default(),
                secondary: vec![],
                mode: SelectionMode::Custom,
                filters: Filters::default(),
            },
            jumps: None,
            mode: Mode::Normal,
            cursor_direction: Direction::Start,
            scroll_offset: 0,
            rectangle: Rectangle::default(),
            buffer: Rc::new(RefCell::new(Buffer::new(language, text))),
            title: None,
            id: ComponentId::new(),
            current_view_alignment: None,
            regex_highlight_rules: Vec::new(),
        }
    }

    pub fn from_buffer(buffer: Rc<RefCell<Buffer>>) -> Self {
        Self {
            selection_set: SelectionSet {
                primary: Selection::default(),
                secondary: vec![],
                mode: SelectionMode::LineTrimmed,
                filters: Filters::default(),
            },
            jumps: None,
            mode: Mode::Normal,
            cursor_direction: Direction::Start,
            scroll_offset: 0,
            rectangle: Rectangle::default(),
            buffer,
            title: None,
            id: ComponentId::new(),
            current_view_alignment: None,
            regex_highlight_rules: Vec::new(),
        }
    }

    pub fn current_line(&self) -> anyhow::Result<String> {
        let cursor = self.get_cursor_char_index();
        Ok(self
            .buffer
            .borrow()
            .get_line_by_char_index(cursor)?
            .to_string()
            .trim()
            .into())
    }

    pub fn get_current_word(&self) -> anyhow::Result<String> {
        let cursor = self.get_cursor_char_index();
        self.buffer.borrow().get_word_before_char_index(cursor)
    }

    pub fn select_kids(&mut self) -> anyhow::Result<Dispatches> {
        let buffer = self.buffer.borrow().clone();

        Ok(self.update_selection_set(
            self.selection_set
                .select_kids(&buffer, &self.cursor_direction)?,
            true,
        ))
    }

    pub fn select_line(&mut self, movement: Movement) -> anyhow::Result<Dispatches> {
        self.select(SelectionMode::LineTrimmed, movement)
    }

    pub fn select_line_at(&mut self, line: usize) -> anyhow::Result<Dispatches> {
        let start = self.buffer.borrow().line_to_char(line)?;
        let selection_set = SelectionSet {
            primary: Selection::new(
                (start
                    ..start
                        + self
                            .buffer
                            .borrow()
                            .get_line_by_char_index(start)?
                            .len_chars())
                    .into(),
            ),
            secondary: vec![],
            mode: SelectionMode::LineTrimmed,
            filters: Filters::default(),
        };

        Ok(self.update_selection_set(selection_set, false))
    }

    pub fn reset(&mut self) {
        self.selection_set.escape_highlight_mode();
    }

    /// WARNING: this method should only be called by `app.rs`, not by this file    
    pub fn __update_selection_set_for_real(&mut self, selection_set: SelectionSet) {
        self.selection_set = selection_set;
        self.cursor_direction = Direction::Start;
        self.recalculate_scroll_offset()
    }

    pub fn update_selection_set(
        &self,
        selection_set: SelectionSet,
        store_history: bool,
    ) -> Dispatches {
        let show_info = selection_set
            .map(|selection| selection.info())
            .into_iter()
            .flatten()
            .reduce(Info::join)
            .map(Dispatch::ShowEditorInfo);
        Dispatches::new(
            [Dispatch::UpdateSelectionSet {
                selection_set,
                kind: self
                    .buffer()
                    .path()
                    .map(SelectionSetHistoryKind::Path)
                    .unwrap_or_else(|| SelectionSetHistoryKind::ComponentId(Some(self.id()))),
                store_history,
            }]
            .to_vec(),
        )
        .append_some(show_info)
    }

    pub fn position_range_to_selection_set(
        &self,
        range: Range<Position>,
    ) -> anyhow::Result<SelectionSet> {
        let range = (self.buffer().position_to_char(range.start)?
            ..self.buffer().position_to_char(range.end)?)
            .into();

        let mode = if self.buffer().given_range_is_node(&range) {
            SelectionMode::SyntaxTree
        } else {
            SelectionMode::Custom
        };
        let primary = self.selection_set.primary.clone().set_range(range);
        Ok(SelectionSet {
            primary,
            secondary: vec![],
            mode,
            filters: Filters::default(),
        })
    }

    fn cursor_row(&self) -> u16 {
        self.get_cursor_char_index()
            .to_position(&self.buffer.borrow())
            .line as u16
    }

    fn recalculate_scroll_offset(&mut self) {
        // Update scroll_offset if primary selection is out of view.
        let cursor_row = self.cursor_row();
        let render_area = self.render_area();
        if cursor_row.saturating_sub(self.scroll_offset) > render_area.height.saturating_sub(1)
            || cursor_row < self.scroll_offset
        {
            self.align_cursor_to_center();
            self.current_view_alignment = None;
        }
    }

    pub fn align_cursor_to_bottom(&mut self) {
        self.scroll_offset = self.cursor_row().saturating_sub(
            self.rectangle
                .height
                .saturating_sub(1)
                .saturating_sub(WINDOW_TITLE_HEIGHT as u16),
        );
    }

    pub fn align_cursor_to_top(&mut self) {
        self.scroll_offset = self.cursor_row();
    }

    fn align_cursor_to_center(&mut self) {
        self.scroll_offset = self
            .cursor_row()
            .saturating_sub((self.rectangle.height as f64 / 2.0).ceil() as u16);
    }

    pub fn select(
        &mut self,
        selection_mode: SelectionMode,
        movement: Movement,
    ) -> anyhow::Result<Dispatches> {
        //  There are a few selection modes where Current make sense.
        let direction = if self.selection_set.mode != selection_mode {
            Movement::Current
        } else {
            movement
        };

        let selection_set = self.get_selection_set(&selection_mode, direction)?;

        Ok(self.update_selection_set(selection_set, true))
    }

    fn jump_characters() -> Vec<char> {
        ('a'..='z').chain('A'..='Z').chain('0'..='9').collect_vec()
    }

    pub(crate) fn get_selection_mode_trait_object(
        &self,
        selection: &Selection,
    ) -> anyhow::Result<Box<dyn selection_mode::SelectionMode>> {
        self.selection_set.mode.to_selection_mode_trait_object(
            &self.buffer(),
            selection,
            &self.cursor_direction,
            &self.selection_set.filters,
        )
    }

    fn jump_from_selection(&mut self, selection: &Selection) -> anyhow::Result<()> {
        let chars = Self::jump_characters();

        let object = self.get_selection_mode_trait_object(selection)?;

        let line_range = self.visible_line_range();
        let jumps = object.jumps(
            selection_mode::SelectionModeParams {
                buffer: &self.buffer(),
                current_selection: selection,
                cursor_direction: &self.cursor_direction,
                filters: &self.selection_set.filters,
            },
            chars,
            line_range,
        )?;
        self.jumps = Some(jumps);

        Ok(())
    }

    pub fn show_jumps(&mut self) -> anyhow::Result<()> {
        self.jump_from_selection(&self.selection_set.primary.clone())
    }

    pub fn cut(&mut self) -> anyhow::Result<Dispatches> {
        let edit_transaction = EditTransaction::from_action_groups({
            self.selection_set
                .map(|selection| -> anyhow::Result<_> {
                    let current_range = selection.extended_range();
                    let copied_text = Some(self.buffer.borrow().slice(&current_range)?);
                    Ok(ActionGroup::new(
                        [
                            Action::Edit(Edit {
                                range: current_range,
                                new: Rope::new(),
                            }),
                            Action::Select(
                                Selection::new((current_range.start..current_range.start).into())
                                    .set_copied_text(copied_text),
                            ),
                        ]
                        .to_vec(),
                    ))
                })
                .into_iter()
                .flatten()
                .collect()
        });
        // Set the clipboard content to the current selection
        // if there is only one cursor.
        let dispatch = if self.selection_set.secondary.is_empty() {
            Some(Dispatch::SetClipboardContent(
                self.buffer
                    .borrow()
                    .slice(&self.selection_set.primary.extended_range())?
                    .into(),
            ))
        } else {
            None
        };
        let dispatches = self
            .apply_edit_transaction(edit_transaction)
            .map(|dispatches| {
                dispatches
                    .into_vec()
                    .into_iter()
                    .chain(dispatch)
                    .collect_vec()
                    .into()
            });
        self.enter_insert_mode(Direction::Start)?;
        dispatches
    }

    pub fn delete(&mut self, cut: bool, context: &Context) -> anyhow::Result<Dispatches> {
        let dispatches = if cut {
            self.copy(context)?
        } else {
            Default::default()
        };
        let edit_transaction = EditTransaction::from_action_groups({
            let buffer = self.buffer();
            self.selection_set
                .map(|selection| -> anyhow::Result<_> {
                    let current_range = selection.extended_range();
                    // If the gap between the next selection and the current selection are only whitespaces, perform a "kill next" instead
                    let next_selection = Selection::get_selection_(
                        &buffer,
                        selection,
                        &self.selection_set.mode,
                        &Movement::Next,
                        &self.cursor_direction,
                        &self.selection_set.filters,
                    )?
                    .selection;

                    let next_range = next_selection.extended_range();

                    let (delete_range, select_range) = {
                        let default = {
                            let start = current_range.start;
                            (current_range, (start..start + 1).into())
                        };
                        if current_range.end > next_range.start {
                            default
                        } else {
                            let inbetween_range: CharIndexRange =
                                (current_range.end..next_range.start).into();

                            let inbetween_text = buffer.slice(&inbetween_range)?.to_string();
                            if !inbetween_text.trim().is_empty()
                                && !self.selection_set.mode.is_contiguous()
                            {
                                default
                            } else {
                                let delete_range: CharIndexRange = (current_range.start
                                    ..next_selection.extended_range().start)
                                    .into();
                                (delete_range, {
                                    next_selection
                                        .extended_range()
                                        .shift_left(delete_range.len())
                                })
                            }
                        }
                    };

                    Ok(ActionGroup::new(
                        [
                            Action::Edit(Edit {
                                range: delete_range,
                                new: Rope::new(),
                            }),
                            Action::Select(
                                selection
                                    .clone()
                                    .set_range(select_range)
                                    .set_initial_range(None),
                            ),
                        ]
                        .to_vec(),
                    ))
                })
                .into_iter()
                .flatten()
                .collect()
        });
        Ok(dispatches.chain(self.apply_edit_transaction(edit_transaction)?))
    }

    pub fn copy(&mut self, context: &Context) -> anyhow::Result<Dispatches> {
        self.selection_set.copy(&self.buffer.borrow(), context)
    }

    fn replace_current_selection_with<F>(&mut self, f: F) -> anyhow::Result<Dispatches>
    where
        F: Fn(&Selection) -> Option<Rope>,
    {
        let edit_transactions = self.selection_set.map(|selection| {
            if let Some(copied_text) = f(selection) {
                let range = selection.extended_range();
                let start = range.start;
                EditTransaction::from_action_groups(
                    [ActionGroup::new(
                        [
                            Action::Edit(Edit {
                                range,
                                new: copied_text.clone(),
                            }),
                            Action::Select(
                                Selection::new({
                                    let start = start + copied_text.len_chars();
                                    (start..start).into()
                                })
                                .set_copied_text(Some(copied_text)),
                            ),
                        ]
                        .to_vec(),
                    )]
                    .to_vec(),
                )
            } else {
                EditTransaction::from_action_groups(vec![])
            }
        });
        let edit_transaction = EditTransaction::merge(edit_transactions);
        self.apply_edit_transaction(edit_transaction)
    }

    pub fn replace_with_clipboard(&mut self, context: &Context) -> anyhow::Result<Dispatches> {
        self.replace_current_selection_with(|selection| selection.copied_text(context))
    }

    pub fn paste(&mut self, direction: Direction, context: &Context) -> anyhow::Result<Dispatches> {
        let edit_transaction = EditTransaction::from_action_groups({
            self.selection_set
                .map(|selection| -> anyhow::Result<_> {
                    let current_range = selection.extended_range();
                    let insertion_range_start = match direction {
                        Direction::Start => current_range.start,
                        Direction::End => current_range.end,
                    };
                    let insertion_range = insertion_range_start..insertion_range_start;
                    let copied_text = selection.copied_text(context).unwrap_or_default();
                    let copied_text_len = copied_text.len_chars();

                    let selection_range = if self.mode == Mode::Normal {
                        insertion_range_start..insertion_range_start + copied_text_len
                    } else {
                        let start = insertion_range_start + copied_text_len;
                        start..start
                    };
                    Ok(ActionGroup::new(
                        [
                            Action::Edit(Edit {
                                range: insertion_range.into(),
                                new: copied_text,
                            }),
                            Action::Select(Selection::new(selection_range.into())),
                        ]
                        .to_vec(),
                    ))
                })
                .into_iter()
                .flatten()
                .collect()
        });
        self.apply_edit_transaction(edit_transaction)
    }

    /// If `cut` if true, the replaced text will override the clipboard
    pub fn replace_with_copied_text(
        &mut self,
        context: &Context,
        cut: bool,
    ) -> anyhow::Result<Dispatches> {
        let edit_transaction = EditTransaction::merge(
            self.selection_set
                .map(|selection| -> anyhow::Result<_> {
                    if let Some(replacement) = &selection.copied_text(context) {
                        let replacement_text_len = replacement.len_chars();
                        let range = selection.extended_range();
                        let replaced_text = self.buffer.borrow().slice(&range)?;
                        Ok(EditTransaction::from_action_groups(
                            [ActionGroup::new(
                                [
                                    Action::Edit(Edit {
                                        range,
                                        new: replacement.clone(),
                                    }),
                                    Action::Select({
                                        let selection = Selection::new(
                                            (range.start..range.start + replacement_text_len)
                                                .into(),
                                        );
                                        if cut {
                                            selection.set_copied_text(Some(replaced_text))
                                        } else {
                                            selection
                                        }
                                    }),
                                ]
                                .to_vec(),
                            )]
                            .to_vec(),
                        ))
                    } else {
                        Ok(EditTransaction::from_action_groups(vec![]))
                    }
                })
                .into_iter()
                .flatten()
                .collect(),
        );

        self.apply_edit_transaction(edit_transaction)
    }

    fn apply_edit_transaction(
        &mut self,
        edit_transaction: EditTransaction,
    ) -> anyhow::Result<Dispatches> {
        let new_selection_set = self.buffer.borrow_mut().apply_edit_transaction(
            &edit_transaction,
            self.selection_set.clone(),
            self.mode != Mode::Insert,
        )?;

        self.selection_set = new_selection_set;

        self.recalculate_scroll_offset();

        Ok(self.get_document_did_change_dispatch())
    }

    pub fn get_document_did_change_dispatch(&mut self) -> Dispatches {
        [Dispatch::DocumentDidChange {
            component_id: self.id(),
            path: self.buffer().path(),
            content: self.buffer().rope().to_string(),
            language: self.buffer().language(),
        }]
        .into_iter()
        .chain(if self.mode == Mode::UndoTree {
            Some(self.show_undo_tree_dispatch())
        } else {
            None
        })
        .collect_vec()
        .into()
    }

    pub fn enter_undo_tree_mode(&mut self) -> Dispatches {
        self.mode = Mode::UndoTree;
        [self.show_undo_tree_dispatch()].to_vec().into()
    }

    pub fn show_undo_tree_dispatch(&self) -> Dispatch {
        Dispatch::ShowGlobalInfo(Info::new(
            "Undo Tree History".to_string(),
            self.buffer().display_history(),
        ))
    }

    pub fn undo(&mut self) -> anyhow::Result<Dispatches> {
        self.navigate_undo_tree(Movement::Previous)
    }

    pub fn redo(&mut self) -> anyhow::Result<Dispatches> {
        self.navigate_undo_tree(Movement::Next)
    }

    pub fn swap_cursor_with_anchor(&mut self) {
        self.cursor_direction = match self.cursor_direction {
            Direction::Start => Direction::End,
            Direction::End => Direction::Start,
        };
        self.recalculate_scroll_offset()
    }

    fn get_selection_set(
        &self,
        mode: &SelectionMode,
        movement: Movement,
    ) -> anyhow::Result<SelectionSet> {
        self.selection_set.generate(
            &self.buffer.borrow(),
            mode,
            &movement,
            &self.cursor_direction,
        )
    }

    pub fn get_cursor_char_index(&self) -> CharIndex {
        self.selection_set
            .primary
            .to_char_index(&self.cursor_direction)
    }

    pub fn toggle_highlight_mode(&mut self) {
        self.selection_set.toggle_highlight_mode();
        self.recalculate_scroll_offset()
    }

    pub fn apply_dispatch(
        &mut self,
        context: &mut Context,
        dispatch: DispatchEditor,
    ) -> anyhow::Result<Dispatches> {
        match dispatch {
            AlignViewTop => self.align_cursor_to_top(),
            AlignViewCenter => self.align_cursor_to_center(),
            AlignViewBottom => self.align_cursor_to_bottom(),
            Transform(transformation) => return self.transform_selection(transformation),
            SetSelectionMode(selection_mode) => {
                return self.set_selection_mode(selection_mode);
            }

            FindOneChar => self.enter_single_character_mode(),

            MoveSelection(direction) => return self.handle_movement(context, direction),
            Copy => return self.copy(context),
            ReplaceWithCopiedText => return self.replace_with_copied_text(context, false),
            SelectAll => return Ok(self.select_all()),
            SetContent(content) => self.update_buffer(&content),
            ReplaceCut => return self.replace_with_copied_text(context, true),
            ToggleHighlightMode => self.toggle_highlight_mode(),
            EnterUndoTreeMode => return Ok(self.enter_undo_tree_mode()),
            EnterInsertMode(direction) => self.enter_insert_mode(direction)?,
            Delete { cut } => return self.delete(cut, context),
            Insert(string) => return self.insert(&string),
            MatchLiteral(literal) => return self.match_literal(&literal),
            ToggleBookmark => self.toggle_bookmarks(),
            EnterInsideMode(kind) => return self.set_selection_mode(SelectionMode::Inside(kind)),
            EnterNormalMode => self.enter_normal_mode()?,
            FilterPush(filter) => return Ok(self.filters_push(context, filter)),
            CursorAddToAllSelections => self.add_cursor_to_all_selections()?,
            FilterClear => return Ok(self.filters_clear()),
            CursorKeepPrimaryOnly => self.cursor_keep_primary_only(),
            Raise => return self.replace_with_movement(&Movement::Parent),
            Exchange(movement) => return self.exchange(movement),
            EnterExchangeMode => self.enter_exchange_mode(),
            ReplacePattern { config } => {
                let selection_set = self.selection_set.clone();
                let (_, selection_set) = self.buffer_mut().replace(config, selection_set)?;
                return Ok(self
                    .update_selection_set(selection_set, false)
                    .chain(self.get_document_did_change_dispatch()));
            }
            Undo => return self.undo(),
            KillLine(direction) => return self.kill_line(direction),
            Reset => self.reset(),
            DeleteWordBackward { short } => return self.delete_word_backward(short),
            Backspace => return self.backspace(),
            MoveToLineStart => return self.move_to_line_start(),
            MoveToLineEnd => return self.move_to_line_end(),
            SelectLine(movement) => return self.select_line(movement),
            SelectKids => return self.select_kids(),
            Redo => return self.redo(),
            OpenNewLine => return self.open_new_line(),
            Change { cut } => return self.change(cut, context),
            SetRectangle(rectangle) => self.set_rectangle(rectangle),
            ScrollPageDown => return self.scroll_page_down(),
            ScrollPageUp => return self.scroll_page_up(),
            ShowJumps => self.show_jumps()?,
            SwitchViewAlignment => self.switch_view_alignment(),
            SetScrollOffset(n) => self.set_scroll_offset(n),
            SetLanguage(language) => self.set_language(language)?,
            ApplySyntaxHighlight => {
                self.apply_syntax_highlighting(context)?;
            }
            Save => return self.save(),
            ReplaceCurrentSelectionWith(string) => {
                return self.replace_current_selection_with(|_| Some(Rope::from_str(&string)))
            }
            ReplacePreviousWord(word) => return self.replace_previous_word(&word),
            ApplyPositionalEdit(edit) => return self.apply_positional_edit(edit),
            SelectLineAt(index) => return Ok(self.select_line_at(index)?.into_vec().into()),
            EnterMultiCursorMode => self.enter_multicursor_mode(),
            Surround(open, close) => return self.enclose(open, close),
            ShowKeymapLegendInsertMode => {
                return Ok([Dispatch::ShowKeymapLegend(
                    self.insert_mode_keymap_legend_config(),
                )]
                .to_vec()
                .into())
            }
            ShowKeymapLegendHelp => {
                return Ok(
                    [Dispatch::ShowKeymapLegend(self.help_keymap_legend_config())]
                        .to_vec()
                        .into(),
                )
            }
            ShowKeymapLegendNormalMode => {
                return Ok([Dispatch::ShowKeymapLegend(
                    self.normal_mode_keymap_legend_config(context),
                )]
                .to_vec()
                .into())
            }
            EnterReplaceMode => self.enter_replace_mode(),
            Paste(direction) => return self.paste(direction, context),
            SwapCursorWithAnchor => self.swap_cursor_with_anchor(),
            SetDecorations(decorations) => self.buffer_mut().set_decorations(&decorations),
            MoveCharacterBack => self.selection_set.move_left(&self.cursor_direction),
            MoveCharacterForward => self.selection_set.move_right(&self.cursor_direction),
            ReplaceWithMovement(movement) => return self.replace_with_movement(&movement),
            EnterExchangeModeJump => {
                self.enter_exchange_mode();
                self.show_jumps()?;
            }
            EnterReplaceModeJump => {
                self.enter_replace_mode();
                self.show_jumps()?;
            }
            AddCursor(movement) => self.add_cursor(&movement)?,
            Open(direction) => return self.open(direction),
        }
        Ok(Default::default())
    }

    pub fn handle_key_event(
        &mut self,
        context: &Context,
        key_event: KeyEvent,
    ) -> anyhow::Result<Dispatches> {
        match self.handle_universal_key(key_event)? {
            HandleEventResult::Ignored(key_event) => {
                if let Some(jumps) = self.jumps.take() {
                    self.handle_jump_mode(context, key_event, jumps)
                } else {
                    match &self.mode {
                        Mode::Normal => self.handle_normal_mode(context, key_event),
                        Mode::Insert => self.handle_insert_mode(key_event),
                        Mode::MultiCursor => self.handle_multi_cursor_mode(context, key_event),
                        Mode::FindOneChar => self.handle_find_one_char_mode(key_event),
                        Mode::Exchange => self.handle_exchange_mode(context, key_event),
                        Mode::UndoTree => self.handle_undo_tree_mode(context, key_event),
                        Mode::Replace => self.handle_replace_mode(context, key_event),
                    }
                }
            }
            HandleEventResult::Handled(dispatches) => Ok(dispatches),
        }
    }

    fn handle_jump_mode(
        &mut self,
        context: &Context,
        key_event: KeyEvent,
        jumps: Vec<Jump>,
    ) -> anyhow::Result<Dispatches> {
        match key_event {
            key!("esc") => {
                self.jumps = None;
                Ok(Default::default())
            }
            key => {
                let KeyCode::Char(c) = key.code else {
                    return Ok(Default::default());
                };
                let matching_jumps = jumps
                    .iter()
                    .filter(|jump| c == jump.character)
                    .collect_vec();
                match matching_jumps.split_first() {
                    None => Ok(Default::default()),
                    Some((jump, [])) => Ok(self
                        .handle_movement(context, Movement::Jump(jump.selection.extended_range()))?
                        .append(Dispatch::ToEditor(EnterNormalMode))),
                    Some(_) => {
                        self.jumps = Some(
                            matching_jumps
                                .into_iter()
                                .zip(Self::jump_characters().into_iter().cycle())
                                .map(|(jump, character)| Jump {
                                    character,
                                    ..jump.clone()
                                })
                                .collect_vec(),
                        );
                        Ok(Default::default())
                    }
                }
            }
        }
    }

    /// Similar to Change in Vim, but does not copy the current selection
    pub fn change(&mut self, cut: bool, context: &Context) -> anyhow::Result<Dispatches> {
        let dispatches = if cut {
            self.copy(context)?
        } else {
            Default::default()
        };
        let edit_transaction = EditTransaction::from_action_groups(
            self.selection_set
                .map(|selection| -> anyhow::Result<_> {
                    let range = selection.extended_range();
                    Ok(ActionGroup::new(
                        [
                            Action::Edit(Edit {
                                range,
                                new: Rope::new(),
                            }),
                            Action::Select(
                                selection
                                    .clone()
                                    .set_range((range.start..range.start).into())
                                    .set_initial_range(None),
                            ),
                        ]
                        .to_vec(),
                    ))
                })
                .into_iter()
                .flatten()
                .collect(),
        );

        let dispatches = dispatches.chain(self.apply_edit_transaction(edit_transaction)?);
        self.enter_insert_mode(Direction::Start)?;
        Ok(dispatches)
    }

    pub fn insert(&mut self, s: &str) -> anyhow::Result<Dispatches> {
        let edit_transaction =
            EditTransaction::from_action_groups(self.selection_set.map(|selection| {
                let range = selection.extended_range();
                ActionGroup::new(
                    [
                        Action::Edit(Edit {
                            range: {
                                let start = selection.to_char_index(&Direction::End);
                                (start..start).into()
                            },
                            new: Rope::from_str(s),
                        }),
                        Action::Select(
                            selection
                                .clone()
                                .set_range((range.start + s.len()..range.start + s.len()).into()),
                        ),
                    ]
                    .to_vec(),
                )
            }));

        self.apply_edit_transaction(edit_transaction)
    }

    pub fn get_request_params(&self) -> Option<RequestParams> {
        let component_id = self.id();
        let position = self.get_cursor_position().ok()?;
        self.path().map(|path| RequestParams {
            path,
            position,
            context: ResponseContext {
                component_id,
                scope: None,
                description: None,
            },
        })
    }

    pub fn set_selection_mode(
        &mut self,
        selection_mode: SelectionMode,
    ) -> anyhow::Result<Dispatches> {
        self.move_selection_with_selection_mode_without_global_mode(
            Movement::Current,
            selection_mode,
        )
        .map(|dispatches| {
            Some(Dispatch::SetGlobalMode(None))
                .into_iter()
                .chain(dispatches.into_vec())
                .collect::<Vec<_>>()
                .into()
        })
    }

    fn move_selection_with_selection_mode(
        &mut self,
        context: &Context,
        movement: Movement,
        selection_mode: SelectionMode,
    ) -> anyhow::Result<Dispatches> {
        if let Some(global_mode) = &context.mode() {
            match global_mode {
                GlobalMode::QuickfixListItem => {
                    Ok(vec![Dispatch::GotoQuickfixListItem(movement)].into())
                }

                GlobalMode::SelectionHistoryContiguous => {
                    Ok([Dispatch::GotoSelectionHistoryContiguous(movement)]
                        .to_vec()
                        .into())
                }
            }
        } else {
            self.move_selection_with_selection_mode_without_global_mode(movement, selection_mode)
        }
    }

    pub fn handle_movement(
        &mut self,
        context: &Context,
        movement: Movement,
    ) -> anyhow::Result<Dispatches> {
        match movement {
            Movement::Last => {
                let mut current_content = self.content();
                let mut dispactches = Dispatches::default();
                loop {
                    dispactches = dispactches.chain(self.handle_movement(context, Movement::Next)?);
                    let new_content = self.content();
                    if current_content == new_content {
                        return Ok(dispactches);
                    }
                    current_content = new_content
                }
            }
            Movement::First => {
                let mut current_content = self.content();
                let mut dispactches = Dispatches::default();
                loop {
                    dispactches =
                        dispactches.chain(self.handle_movement(context, Movement::Previous)?);
                    let new_content = self.content();
                    if current_content == new_content {
                        return Ok(dispactches);
                    }
                    current_content = new_content
                }
            }
            _ => {}
        }
        match self.mode {
            Mode::Normal => self.move_selection_with_selection_mode(
                context,
                movement,
                self.selection_set.mode.clone(),
            ),
            Mode::Exchange => self.exchange(movement),
            Mode::Replace => self.replace_with_movement(&movement),
            Mode::UndoTree => self.navigate_undo_tree(movement),
            Mode::MultiCursor => self.add_cursor(&movement).map(|_| Default::default()),
            _ => Ok(Default::default()),
        }
    }

    pub fn toggle_bookmarks(&mut self) {
        let selections = self
            .selection_set
            .map(|selection| selection.extended_range());
        self.buffer_mut().save_bookmarks(selections)
    }

    pub fn path(&self) -> Option<CanonicalizedPath> {
        self.editor().buffer().path()
    }

    pub fn enter_insert_mode(&mut self, direction: Direction) -> anyhow::Result<()> {
        self.selection_set =
            self.selection_set
                .apply(self.selection_set.mode.clone(), |selection| {
                    let range = selection.extended_range();
                    let char_index = match direction {
                        Direction::Start => range.start,
                        Direction::End => range.end,
                    };
                    Ok(selection.clone().set_range((char_index..char_index).into()))
                })?;
        self.mode = Mode::Insert;
        self.cursor_direction = Direction::Start;
        Ok(())
    }

    pub fn enter_normal_mode(&mut self) -> anyhow::Result<()> {
        if self.mode == Mode::Insert {
            // This is necessary for cursor to not overflow after exiting insert mode
            self.selection_set =
                self.selection_set
                    .apply(self.selection_set.mode.clone(), |selection| {
                        let range = {
                            if let Ok(position) = self
                                .buffer()
                                .char_to_position(selection.extended_range().start)
                            {
                                let start = selection.extended_range().start
                                    - if position.column > 0 { 1 } else { 0 };
                                (start..start + 1).into()
                            } else {
                                selection.extended_range()
                            }
                        };
                        Ok(selection.clone().set_range(range))
                    })?;
            self.clamp()?;
            self.buffer_mut().reparse_tree()?;
        }
        // TODO: continue from here, need to add test: upon exiting insert mode, should close all panels
        // Maybe we should call this function the exit_insert_mode?

        self.mode = Mode::Normal;
        self.selection_set.unset_initial_range();
        Ok(())
    }

    #[cfg(test)]
    pub fn jump_chars(&self) -> Vec<char> {
        self.jumps()
            .into_iter()
            .map(|jump| jump.character)
            .collect_vec()
    }

    pub fn jumps(&self) -> Vec<&Jump> {
        self.jumps
            .as_ref()
            .map(|jumps| jumps.iter().collect())
            .unwrap_or_default()
    }

    // TODO: handle mouse click
    #[allow(dead_code)]
    pub fn set_cursor_position(&mut self, row: u16, column: u16) -> anyhow::Result<Dispatches> {
        let start = (self.buffer.borrow().line_to_char(row as usize)?) + column.into();
        let primary = self
            .selection_set
            .primary
            .clone()
            .set_range((start..start).into());
        Ok(self.update_selection_set(
            SelectionSet {
                mode: self.selection_set.mode.clone(),
                primary,
                ..self.selection_set.clone()
            },
            true,
        ))
    }

    /// Get the selection that preserves the syntactic structure of the current selection.
    ///
    /// Returns a valid edit transaction if there is any, otherwise `Left(current_selection)`.
    fn get_valid_selection(
        &self,
        current_selection: &Selection,
        selection_mode: &SelectionMode,
        direction: &Movement,
        get_actual_edit_transaction: impl Fn(
            /* current */ &Selection,
            /* next */ &Selection,
        ) -> anyhow::Result<EditTransaction>,
    ) -> anyhow::Result<Either<Selection, EditTransaction>> {
        let current_selection = current_selection.clone();

        let buffer = self.buffer.borrow();

        // Loop until the edit transaction does not result in errorneous node
        let mut next_selection = Selection::get_selection_(
            &buffer,
            &current_selection,
            selection_mode,
            direction,
            &self.cursor_direction,
            &self.selection_set.filters,
        )?
        .selection;

        if next_selection.eq(&current_selection) {
            return Ok(Either::Left(current_selection));
        }

        loop {
            let edit_transaction =
                get_actual_edit_transaction(&current_selection, &next_selection)?;
            edit_transaction.selections();
            let current_node = buffer.get_current_node(&current_selection, false)?;

            let new_buffer = {
                let mut new_buffer = self.buffer.borrow().clone();
                if new_buffer
                    .apply_edit_transaction(&edit_transaction, self.selection_set.clone(), true)
                    .is_err()
                {
                    continue;
                }
                new_buffer
            };

            let text_at_next_selection: Rope = buffer.slice(&next_selection.extended_range())?;

            let next_nodes = edit_transaction
                .selections()
                .into_iter()
                .map(|selection| -> anyhow::Result<_> {
                    new_buffer.get_current_node(selection, false)
                })
                .collect::<Result<Vec<_>, _>>()?;

            // Why don't we just use `tree.root_node().has_error()` instead?
            // Because I assume we want to be able to exchange even if some part of the tree
            // contains error
            if !selection_mode.is_node()
                || (!text_at_next_selection.to_string().trim().is_empty()
                    && next_nodes.iter().all(|next_node| {
                        current_node.kind_id() == next_node.kind_id()
                            && current_node.byte_range().len() == next_node.byte_range().len()
                    })
                    && !new_buffer.has_syntax_error_at(edit_transaction.range()))
            {
                return Ok(Either::Right(get_actual_edit_transaction(
                    &current_selection,
                    &next_selection,
                )?));
            }

            // Get the next selection
            let new_selection = Selection::get_selection_(
                &buffer,
                &next_selection,
                selection_mode,
                direction,
                &self.cursor_direction,
                &self.selection_set.filters,
            )?
            .selection;

            if next_selection.eq(&new_selection) {
                return Ok(Either::Left(current_selection));
            }

            next_selection = new_selection;
        }
    }

    /// Replace the next selection with the current selection without
    /// making the syntax tree invalid
    fn replace_faultlessly(
        &mut self,
        selection_mode: &SelectionMode,
        movement: Movement,
    ) -> anyhow::Result<Dispatches> {
        let buffer = self.buffer.borrow().clone();
        let get_edit_transaction = |current_selection: &Selection,
                                    next_selection: &Selection|
         -> anyhow::Result<_> {
            let current_selection_range = current_selection.extended_range();
            let text_at_current_selection: Rope = buffer.slice(&current_selection_range)?;
            let text_at_next_selection: Rope = buffer.slice(&next_selection.extended_range())?;

            Ok(EditTransaction::from_action_groups(
                [
                    ActionGroup::new(
                        [Action::Edit(Edit {
                            range: current_selection_range,
                            new: text_at_next_selection.clone(),
                        })]
                        .to_vec(),
                    ),
                    ActionGroup::new(
                        [
                            Action::Edit(Edit {
                                range: next_selection.extended_range(),
                                new: text_at_current_selection.clone(),
                            }),
                            Action::Select(
                                current_selection.clone().set_range(
                                    (next_selection.extended_range().start
                                        ..(next_selection.extended_range().start
                                            + text_at_current_selection.len_chars()))
                                        .into(),
                                ),
                            ),
                        ]
                        .to_vec(),
                    ),
                ]
                .to_vec(),
            ))
        };

        let edit_transactions = self.selection_set.map(|selection| {
            self.get_valid_selection(selection, selection_mode, &movement, get_edit_transaction)
        });

        self.apply_edit_transaction(EditTransaction::merge(
            edit_transactions
                .into_iter()
                .filter_map(|transaction| transaction.ok())
                .filter_map(|transaction| transaction.map_right(Some).right_or(None))
                .collect(),
        ))
    }

    pub fn exchange(&mut self, movement: Movement) -> anyhow::Result<Dispatches> {
        let mode = self.selection_set.mode.clone();
        self.replace_faultlessly(&mode, movement)
    }

    pub fn add_cursor(&mut self, direction: &Movement) -> anyhow::Result<()> {
        self.selection_set.add_selection(
            &self.buffer.borrow(),
            direction,
            &self.cursor_direction,
        )?;
        self.recalculate_scroll_offset();
        Ok(())
    }

    #[cfg(test)]
    pub fn get_selected_texts(&self) -> Vec<String> {
        let buffer = self.buffer.borrow();
        let mut selections = self
            .selection_set
            .map(|selection| -> anyhow::Result<_> {
                Ok((
                    selection.extended_range(),
                    buffer.slice(&selection.extended_range())?.to_string(),
                ))
            })
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        selections.sort_by(|a, b| a.0.start.0.cmp(&b.0.start.0));
        selections
            .into_iter()
            .map(|selection| selection.1)
            .collect()
    }

    pub fn text(&self) -> String {
        let buffer = self.buffer.borrow().clone();
        buffer.rope().slice(0..buffer.len_chars()).to_string()
    }

    pub fn dimension(&self) -> Dimension {
        self.rectangle.dimension()
    }

    fn apply_scroll(&mut self, direction: Direction, scroll_height: usize) {
        self.scroll_offset = match direction {
            Direction::Start => self.scroll_offset.saturating_sub(scroll_height as u16),
            Direction::End => self.scroll_offset.saturating_add(scroll_height as u16),
        };
    }

    pub fn backspace(&mut self) -> anyhow::Result<Dispatches> {
        let edit_transaction =
            EditTransaction::from_action_groups(self.selection_set.map(|selection| {
                let start = CharIndex(selection.extended_range().start.0.saturating_sub(1));
                ActionGroup::new(
                    [
                        Action::Edit(Edit {
                            range: (start..selection.extended_range().start).into(),
                            new: Rope::from(""),
                        }),
                        Action::Select(selection.clone().set_range((start..start).into())),
                    ]
                    .to_vec(),
                )
            }));

        self.apply_edit_transaction(edit_transaction)
    }

    pub fn delete_word_backward(&mut self, short: bool) -> Result<Dispatches, anyhow::Error> {
        let action_groups = self
            .selection_set
            .map(|current_selection| -> anyhow::Result<_> {
                let current_range = current_selection.extended_range();
                if current_range.start.0 == 0 && current_range.end.0 == 0 {
                    // Do nothing if cursor is at the beginning of the file
                    return Ok(ActionGroup::new(Vec::new()));
                }

                let len_chars = self.buffer().rope().len_chars();
                let start = CharIndex(current_range.start.0.min(len_chars).saturating_sub(1));

                let previous_word = {
                    let get_word = |movement: Movement| {
                        Selection::get_selection_(
                            &self.buffer(),
                            &current_selection.clone().set_range((start..start).into()),
                            &if short {
                                SelectionMode::WordShort
                            } else {
                                SelectionMode::WordLong
                            },
                            &movement,
                            &self.cursor_direction,
                            &self.selection_set.filters,
                        )
                    };
                    let current_word = get_word(Movement::Current)?.selection;
                    if current_word.extended_range().start <= start {
                        current_word
                    } else {
                        get_word(Movement::Previous)?.selection
                    }
                };

                let previous_word_range = previous_word.extended_range();
                let end = previous_word_range
                    .end
                    .min(current_range.start)
                    .max(start + 1);
                let start = previous_word_range.start;
                Ok(ActionGroup::new(
                    [
                        Action::Edit(Edit {
                            range: (start..end).into(),
                            new: Rope::from(""),
                        }),
                        Action::Select(current_selection.clone().set_range((start..start).into())),
                    ]
                    .to_vec(),
                ))
            })
            .into_iter()
            .flatten()
            .collect();
        let edit_transaction = EditTransaction::from_action_groups(action_groups);
        self.apply_edit_transaction(edit_transaction)
    }

    /// Replace the parent node of the current node with the current node
    pub fn replace_with_movement(&mut self, movement: &Movement) -> anyhow::Result<Dispatches> {
        let buffer = self.buffer.borrow().clone();
        let edit_transactions = self.selection_set.map(|selection| {
            let get_edit_transaction =
                |current_selection: &Selection, other_selection: &Selection| -> anyhow::Result<_> {
                    let range = current_selection
                        .extended_range()
                        .start
                        .min(other_selection.extended_range().start)
                        ..current_selection
                            .extended_range()
                            .end
                            .max(other_selection.extended_range().end);
                    let new: Rope = buffer.slice(&current_selection.extended_range())?;

                    let new_len_chars = new.len_chars();
                    Ok(EditTransaction::from_action_groups(
                        [ActionGroup::new(
                            [
                                Action::Edit(Edit {
                                    range: range.clone().into(),
                                    new,
                                }),
                                Action::Select(current_selection.clone().set_range(
                                    (range.start..(range.start + new_len_chars)).into(),
                                )),
                            ]
                            .to_vec(),
                        )]
                        .to_vec(),
                    ))
                };
            self.get_valid_selection(
                selection,
                &self.selection_set.mode,
                movement,
                get_edit_transaction,
            )
        });
        let edit_transaction = EditTransaction::merge(
            edit_transactions
                .into_iter()
                .filter_map(|edit_transaction| edit_transaction.ok())
                .filter_map(|edit_transaction| edit_transaction.map_right(Some).right_or(None))
                .collect(),
        );
        self.apply_edit_transaction(edit_transaction)
    }

    pub fn buffer(&self) -> Ref<Buffer> {
        self.buffer.borrow()
    }

    pub fn buffer_rc(&self) -> Rc<RefCell<Buffer>> {
        self.buffer.clone()
    }

    pub fn buffer_mut(&mut self) -> RefMut<Buffer> {
        self.buffer.borrow_mut()
    }

    fn update_buffer(&mut self, s: &str) {
        self.buffer.borrow_mut().update(s)
    }

    fn scroll(&mut self, direction: Direction, scroll_height: usize) -> anyhow::Result<Dispatches> {
        let dispatch = self.update_selection_set(
            self.selection_set
                .apply(self.selection_set.mode.clone(), |selection| {
                    let position = selection.extended_range().start.to_position(&self.buffer());
                    let line = if direction == Direction::End {
                        position.line.saturating_add(scroll_height)
                    } else {
                        position.line.saturating_sub(scroll_height)
                    };
                    let position = Position { line, ..position };
                    let start = position.to_char_index(&self.buffer())?;
                    Ok(selection.clone().set_range((start..start).into()))
                })?,
            false,
        );
        self.align_cursor_to_center();

        Ok(dispatch)
    }

    pub fn replace_previous_word(&mut self, completion: &str) -> anyhow::Result<Dispatches> {
        // TODO: this algo is not correct, because SelectionMode::Word select small word, we need to change to Big Word
        let selection = self.get_selection_set(&SelectionMode::WordShort, Movement::Current)?;
        Ok(self.update_selection_set(selection, false).chain(
            [Dispatch::ToEditor(ReplaceCurrentSelectionWith(
                completion.to_string(),
            ))]
            .to_vec()
            .into(),
        ))
    }

    fn open(&mut self, direction: Direction) -> Result<Dispatches, anyhow::Error> {
        let edit_transaction = EditTransaction::from_action_groups(
            self.selection_set
                .map(|selection| {
                    let buffer = self.buffer.borrow();
                    let get_selection = |movement: Movement| -> Option<Selection> {
                        let s = Selection::get_selection_(
                            &buffer,
                            selection,
                            &self.selection_set.mode,
                            &movement,
                            &self.cursor_direction,
                            &self.selection_set.filters,
                        )
                        .ok()?
                        .selection;
                        if s.range() == selection.range() {
                            None
                        } else {
                            Some(s)
                        }
                    };
                    let previous_selection = get_selection(Movement::Previous);
                    let next_selection = get_selection(Movement::Next);
                    let in_between = next_selection
                        .or(previous_selection)
                        .and_then(|other_selection| {
                            let current_range = selection.range();
                            let other_range = other_selection.range();
                            let in_between_range = current_range.end.min(other_range.end)
                                ..current_range.start.max(other_range.start);
                            buffer.slice(&in_between_range.into()).ok()
                        })
                        .unwrap_or_default();
                    let in_between_len = in_between.len_chars();
                    Some(ActionGroup::new(
                        [
                            Action::Edit(Edit {
                                range: {
                                    let start = match direction {
                                        Direction::Start => selection.range().start,
                                        Direction::End => selection.range().end,
                                    };
                                    (start..start).into()
                                },
                                new: in_between,
                            }),
                            Action::Select(selection.clone().set_range({
                                let start = match direction {
                                    Direction::Start => selection.range().start,
                                    Direction::End => selection.range().end + in_between_len,
                                };
                                (start..start).into()
                            })),
                        ]
                        .to_vec(),
                    ))
                })
                .into_iter()
                .flatten()
                .collect_vec(),
        );

        Ok(self
            .apply_edit_transaction(edit_transaction)?
            .append(Dispatch::ToEditor(EnterInsertMode(direction))))
    }

    pub fn open_new_line(&mut self) -> anyhow::Result<Dispatches> {
        let edit_transaction = EditTransaction::from_action_groups(
            self.selection_set
                .map(|selection| {
                    let buffer = self.buffer.borrow();
                    let cursor_index = selection.to_char_index(&self.cursor_direction);
                    let line_index = buffer.char_to_line(cursor_index).ok()?;
                    let line_start = buffer.line_to_char(line_index).ok()?;
                    let current_line = self
                        .buffer
                        .borrow()
                        .get_line_by_char_index(cursor_index)
                        .ok()?;
                    let leading_whitespaces = current_line
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .join("");
                    Some(ActionGroup::new(
                        [
                            Action::Edit(Edit {
                                range: {
                                    let start = line_start + current_line.len_chars();
                                    (start..start).into()
                                },
                                new: format!("{}\n", leading_whitespaces).into(),
                            }),
                            Action::Select(selection.clone().set_range({
                                let start = line_start
                                    + current_line.len_chars()
                                    + leading_whitespaces.len();
                                (start..start).into()
                            })),
                        ]
                        .to_vec(),
                    ))
                })
                .into_iter()
                .flatten()
                .collect_vec(),
        );

        let dispatches = self.apply_edit_transaction(edit_transaction)?;
        self.enter_insert_mode(Direction::End)?;
        Ok(dispatches)
    }

    pub fn apply_positional_edits(
        &mut self,
        edits: Vec<PositionalEdit>,
    ) -> anyhow::Result<Dispatches> {
        let edit_transaction = EditTransaction::from_action_groups(
            edits
                .into_iter()
                .enumerate()
                .filter_map(|(index, edit)| {
                    let range = edit.range.start.to_char_index(&self.buffer()).ok()?
                        ..edit.range.end.to_char_index(&self.buffer()).ok()?;
                    let next_text_len = edit.new_text.chars().count();

                    let action_edit = Action::Edit(Edit {
                        range: range.clone().into(),
                        new: edit.new_text.into(),
                    });

                    let action_select = Action::Select(Selection::new({
                        let end = range.start + next_text_len;
                        (end..end).into()
                    }));

                    Some(if index == 0 {
                        ActionGroup::new(vec![action_edit, action_select])
                    } else {
                        ActionGroup::new(vec![action_edit])
                    })
                })
                .collect(),
        );
        self.apply_edit_transaction(edit_transaction)
    }

    pub fn apply_positional_edit(&mut self, edit: PositionalEdit) -> anyhow::Result<Dispatches> {
        self.apply_positional_edits(vec![edit])
    }

    pub fn save(&mut self) -> anyhow::Result<Dispatches> {
        let Some(path) = self.buffer.borrow_mut().save(self.selection_set.clone())? else {
            return Ok(Default::default());
        };
        self.clamp()?;
        self.cursor_keep_primary_only();
        self.enter_normal_mode()?;
        Ok(Dispatches::one(Dispatch::RemainOnlyCurrentComponent)
            .append(Dispatch::DocumentDidSave { path })
            .chain(self.get_document_did_change_dispatch())
            .append(Dispatch::RemainOnlyCurrentComponent))
    }

    /// Clamp everything that might be out of bound after the buffer content is modified elsewhere
    fn clamp(&mut self) -> anyhow::Result<()> {
        let len_chars = self.buffer().len_chars();
        self.selection_set = self.selection_set.clamp(CharIndex(len_chars))?;

        let len_lines = self.buffer().len_lines();
        self.scroll_offset = self.scroll_offset.clamp(0, len_lines as u16);

        Ok(())
    }

    pub fn enclose(&mut self, open: String, close: String) -> anyhow::Result<Dispatches> {
        let edit_transaction = EditTransaction::from_action_groups(
            self.selection_set
                .map(|selection| -> anyhow::Result<_> {
                    let old = self.buffer().slice(&selection.extended_range())?;
                    Ok(ActionGroup::new(
                        [
                            Action::Edit(Edit {
                                range: selection.extended_range(),
                                new: format!("{}{}{}", open, old, close).into(),
                            }),
                            Action::Select(
                                selection.clone().set_range(
                                    (selection.extended_range().start
                                        ..selection.extended_range().end + 2)
                                        .into(),
                                ),
                            ),
                        ]
                        .to_vec(),
                    ))
                })
                .into_iter()
                .flatten()
                .collect_vec(),
        );

        self.apply_edit_transaction(edit_transaction)
    }

    fn transform_selection(
        &mut self,
        transformation: Transformation,
    ) -> anyhow::Result<Dispatches> {
        let edit_transaction = EditTransaction::from_action_groups(
            self.selection_set
                .map(|selection| -> anyhow::Result<_> {
                    let new: Rope = transformation
                        .apply(
                            self.buffer()
                                .slice(&selection.extended_range())?
                                .to_string(),
                        )
                        .into();
                    let new_char_count = new.chars().count();
                    let range = selection.extended_range();
                    Ok(ActionGroup::new(
                        [
                            Action::Edit(Edit { range, new }),
                            Action::Select(
                                selection
                                    .clone()
                                    .set_range((range.start..range.start + new_char_count).into()),
                            ),
                        ]
                        .to_vec(),
                    ))
                })
                .into_iter()
                .flatten()
                .collect_vec(),
        );
        self.apply_edit_transaction(edit_transaction)
    }

    pub fn display_mode(&self) -> String {
        let selection_mode = self.selection_set.mode.display();
        let filters = self
            .selection_set
            .filters
            .display()
            .map(|display| format!("({})", display))
            .unwrap_or_default();
        let mode = match &self.mode {
            Mode::Normal => "MOVE",
            Mode::Insert => "INSERT",
            Mode::MultiCursor => "MULTI CURSOR",
            Mode::FindOneChar => "FIND ONE CHAR",
            Mode::Exchange => "EXCHANGE",
            Mode::UndoTree => "UNDO TREE",
            Mode::Replace => "REPLACE",
        };
        let cursor_count = self.selection_set.len();
        let mode = format!("{}:{}{} x {}", mode, selection_mode, filters, cursor_count);
        if self.jumps.is_some() {
            format!("{} (JUMPING)", mode)
        } else {
            mode
        }
    }

    pub fn visible_line_range(&self) -> Range<usize> {
        let start = self.scroll_offset;
        let end = (start as usize + self.rectangle.height as usize).min(self.buffer().len_lines());

        start as usize..end
    }

    fn handle_multi_cursor_mode(
        &mut self,
        context: &Context,
        key_event: KeyEvent,
    ) -> Result<Dispatches, anyhow::Error> {
        match key_event {
            key!("esc") => {
                self.mode = Mode::Normal;
                Ok(Default::default())
            }

            other => self.handle_normal_mode(context, other),
        }
    }

    pub fn add_cursor_to_all_selections(&mut self) -> Result<(), anyhow::Error> {
        self.selection_set
            .add_all(&self.buffer.borrow(), &self.cursor_direction)?;
        self.recalculate_scroll_offset();
        Ok(())
    }

    pub fn cursor_keep_primary_only(&mut self) {
        self.selection_set.only();
    }

    fn enter_single_character_mode(&mut self) {
        self.mode = Mode::FindOneChar;
    }

    fn handle_find_one_char_mode(
        &mut self,
        key_event: KeyEvent,
    ) -> Result<Dispatches, anyhow::Error> {
        match key_event.code {
            KeyCode::Char(c) => {
                self.mode = Mode::Normal;
                self.set_selection_mode(SelectionMode::Find {
                    search: Search {
                        search: c.to_string(),
                        mode: crate::context::LocalSearchConfigMode::Regex(
                            crate::list::grep::RegexConfig {
                                escaped: true,
                                case_sensitive: true,
                                match_whole_word: false,
                            },
                        ),
                    },
                })
            }
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                Ok(Default::default())
            }
            _ => Ok(Default::default()),
        }
    }

    pub fn set_decorations(&mut self, decorations: &[super::suggestive_editor::Decoration]) {
        self.buffer.borrow_mut().set_decorations(decorations)
    }

    fn half_page_height(&self) -> usize {
        (self.dimension().height / 2) as usize
    }

    pub fn match_literal(&mut self, search: &str) -> anyhow::Result<Dispatches> {
        self.set_selection_mode(SelectionMode::Find {
            search: Search {
                mode: LocalSearchConfigMode::Regex(crate::list::grep::RegexConfig {
                    escaped: true,
                    case_sensitive: false,
                    match_whole_word: false,
                }),
                search: search.to_string(),
            },
        })
    }

    fn handle_exchange_mode(
        &mut self,
        context: &Context,
        key_event: KeyEvent,
    ) -> Result<Dispatches, anyhow::Error> {
        self.handle_normal_mode(context, key_event)
    }

    pub fn move_to_line_start(&mut self) -> anyhow::Result<Dispatches> {
        Ok([
            Dispatch::ToEditor(SelectLine(Movement::Current)),
            Dispatch::ToEditor(EnterInsertMode(Direction::Start)),
        ]
        .to_vec()
        .into())
    }

    pub fn move_to_line_end(&mut self) -> anyhow::Result<Dispatches> {
        Ok([
            Dispatch::ToEditor(SelectLine(Movement::Current)),
            Dispatch::ToEditor(EnterInsertMode(Direction::End)),
        ]
        .to_vec()
        .into())
    }

    pub fn select_all(&mut self) -> Dispatches {
        [
            Dispatch::ToEditor(MoveSelection(Movement::First)),
            Dispatch::ToEditor(ToggleHighlightMode),
            Dispatch::ToEditor(MoveSelection(Movement::Last)),
        ]
        .to_vec()
        .into()
    }

    fn move_selection_with_selection_mode_without_global_mode(
        &mut self,
        movement: Movement,
        selection_mode: SelectionMode,
    ) -> Result<Dispatches, anyhow::Error> {
        let dispatches = self.select(selection_mode, movement)?;
        self.current_view_alignment = None;
        log::info!("dispatches = {:?}", dispatches);

        Ok(dispatches)
    }

    pub fn scroll_page_down(&mut self) -> Result<Dispatches, anyhow::Error> {
        self.scroll(Direction::End, self.half_page_height())
    }

    pub fn scroll_page_up(&mut self) -> Result<Dispatches, anyhow::Error> {
        self.scroll(Direction::Start, self.half_page_height())
    }

    #[cfg(test)]
    pub fn current_view_alignment(&self) -> Option<ViewAlignment> {
        self.current_view_alignment
    }

    pub fn switch_view_alignment(&mut self) {
        self.current_view_alignment = Some(match self.current_view_alignment {
            Some(ViewAlignment::Top) => {
                self.align_cursor_to_center();
                ViewAlignment::Center
            }
            Some(ViewAlignment::Center) => {
                self.align_cursor_to_bottom();
                ViewAlignment::Bottom
            }
            None | Some(ViewAlignment::Bottom) => {
                self.align_cursor_to_top();
                ViewAlignment::Top
            }
        })
    }

    fn handle_undo_tree_mode(
        &mut self,
        context: &Context,
        key_event: KeyEvent,
    ) -> Result<Dispatches, anyhow::Error> {
        self.handle_normal_mode(context, key_event)
    }

    fn navigate_undo_tree(&mut self, movement: Movement) -> Result<Dispatches, anyhow::Error> {
        let selection_set = self.buffer_mut().undo_tree_apply_movement(movement)?;

        Ok(selection_set
            .map(|selection_set| self.update_selection_set(selection_set, false))
            .unwrap_or_default()
            .chain(self.get_document_did_change_dispatch()))
    }

    pub fn set_scroll_offset(&mut self, scroll_offset: u16) {
        self.scroll_offset = scroll_offset
    }

    pub(crate) fn set_language(&mut self, language: Language) -> Result<(), anyhow::Error> {
        self.buffer_mut().set_language(language)
    }

    pub fn render_area(&self) -> Dimension {
        let Dimension { height, width } = self.dimension();
        Dimension {
            height: height.saturating_sub(WINDOW_TITLE_HEIGHT as u16),
            width,
        }
    }

    pub(crate) fn apply_syntax_highlighting(
        &mut self,
        context: &mut Context,
    ) -> anyhow::Result<()> {
        let source_code = self.text();
        let mut buffer = self.buffer_mut();
        if let Some(language) = buffer.language() {
            let highlighted_spans = context.highlight(language, &source_code)?;
            buffer.update_highlighted_spans(highlighted_spans);
        }
        Ok(())
    }

    fn filters_push(&mut self, _context: &Context, filter: Filter) -> Dispatches {
        let selection_set = self.selection_set.clone().filter_push(filter);
        self.update_selection_set(selection_set, true)
            .append(Dispatch::ToEditor(MoveSelection(Movement::Current)))
    }

    pub(crate) fn apply_dispatches(
        &mut self,
        context: &mut Context,
        dispatches: Vec<DispatchEditor>,
    ) -> anyhow::Result<Dispatches> {
        let mut result = Vec::new();
        for dispatch in dispatches {
            result.extend(self.apply_dispatch(context, dispatch)?.into_vec());
        }
        Ok(result.into())
    }

    fn filters_clear(&mut self) -> Dispatches {
        let selection_set = self.selection_set.clone().filter_clear();
        self.update_selection_set(selection_set, true)
    }

    pub fn find_submenu_title(title: &str, scope: Scope) -> String {
        let scope = match scope {
            Scope::Local => "Local".to_string(),
            Scope::Global => "Global".to_string(),
        };
        format!("Find ({scope}): {title}")
    }

    fn enter_exchange_mode(&mut self) {
        self.mode = Mode::Exchange
    }

    fn kill_line(&mut self, direction: Direction) -> Result<Dispatches, anyhow::Error> {
        let edit_transaction = EditTransaction::from_action_groups(
            self.selection_set
                .map(|selection| -> anyhow::Result<_> {
                    let buffer = self.buffer();
                    let cursor = selection.get_anchor(&self.cursor_direction);
                    let line_range = buffer.get_line_range_by_char_index(cursor)?;
                    let (delete_range, cursor_start) = match direction {
                        Direction::Start => {
                            let start = line_range.start();
                            let range = (start..cursor).into();
                            let slice = buffer.slice(&range)?.to_string();
                            let start = if slice.is_empty() { start - 1 } else { start };
                            let range = (start..cursor).into();
                            (range, start)
                        }
                        Direction::End => {
                            let range = (cursor..line_range.end()).into();
                            let slice = buffer.slice(&range)?.to_string();
                            let range = if slice == "\n" || range.end.0 == buffer.len_chars() {
                                range
                            } else {
                                (cursor..(line_range.end() - 1)).into()
                            };
                            (range, cursor)
                        }
                    };
                    Ok(ActionGroup::new(
                        [
                            Action::Edit(Edit {
                                range: delete_range,
                                new: Rope::new(),
                            }),
                            Action::Select(
                                selection
                                    .clone()
                                    .set_range((cursor_start..cursor_start).into()),
                            ),
                        ]
                        .to_vec(),
                    ))
                })
                .into_iter()
                .flatten()
                .collect_vec(),
        );
        let dispatches = self.apply_edit_transaction(edit_transaction)?;
        self.enter_insert_mode(Direction::Start)?;
        Ok(dispatches)
    }

    fn enter_multicursor_mode(&mut self) {
        self.mode = Mode::MultiCursor
    }

    fn enter_replace_mode(&mut self) {
        self.mode = Mode::Replace
    }

    fn handle_replace_mode(
        &mut self,
        context: &Context,
        key_event: KeyEvent,
    ) -> Result<Dispatches, anyhow::Error> {
        self.handle_normal_mode(context, key_event)
    }

    pub(crate) fn scroll_offset(&self) -> u16 {
        self.scroll_offset
    }

    pub(crate) fn set_regex_highlight_rules(
        &mut self,
        regex_highlight_rules: Vec<RegexHighlightRule>,
    ) {
        self.regex_highlight_rules = regex_highlight_rules
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
pub enum ViewAlignment {
    Top,
    Center,
    Bottom,
}

pub enum HandleEventResult {
    Handled(Dispatches),
    Ignored(KeyEvent),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchEditor {
    Surround(String, String),
    SetScrollOffset(u16),
    ShowJumps,
    ScrollPageDown,
    ScrollPageUp,
    AlignViewTop,
    AlignViewCenter,
    AlignViewBottom,
    Transform(Transformation),
    SetSelectionMode(SelectionMode),
    Save,
    Exchange(Movement),
    FindOneChar,
    MoveSelection(Movement),
    SwitchViewAlignment,
    SelectKids,
    Copy,
    ReplaceCut,
    SelectAll,
    SetContent(String),
    SetDecorations(Vec<Decoration>),
    SetRectangle(Rectangle),
    ToggleHighlightMode,
    Change {
        cut: bool,
    },
    EnterUndoTreeMode,
    EnterInsertMode(Direction),
    ReplaceWithMovement(Movement),
    ReplaceWithCopiedText,
    SelectLine(Movement),
    Backspace,
    Delete {
        cut: bool,
    },
    Insert(String),
    MoveToLineStart,
    MoveToLineEnd,
    MatchLiteral(String),
    OpenNewLine,
    Open(Direction),
    ToggleBookmark,
    EnterInsideMode(InsideKind),
    EnterNormalMode,
    EnterExchangeMode,
    EnterReplaceMode,
    EnterMultiCursorMode,
    FilterPush(Filter),
    FilterClear,
    CursorAddToAllSelections,
    CursorKeepPrimaryOnly,
    Raise,
    ReplacePattern {
        config: crate::context::LocalSearchConfig,
    },
    Undo,
    Redo,
    KillLine(Direction),
    Reset,
    DeleteWordBackward {
        short: bool,
    },
    SetLanguage(shared::language::Language),
    ApplySyntaxHighlight,
    ReplaceCurrentSelectionWith(String),
    ReplacePreviousWord(String),
    ApplyPositionalEdit(crate::lsp::completion::PositionalEdit),
    SelectLineAt(usize),
    ShowKeymapLegendNormalMode,
    ShowKeymapLegendInsertMode,
    Paste(Direction),
    SwapCursorWithAnchor,
    MoveCharacterBack,
    MoveCharacterForward,
    ShowKeymapLegendHelp,
    EnterExchangeModeJump,
    EnterReplaceModeJump,
    AddCursor(Movement),
}
