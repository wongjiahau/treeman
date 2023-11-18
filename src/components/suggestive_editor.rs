use crate::app::Dispatch;
use crate::context::Context;
use crate::grid::StyleKey;
use crate::lsp::code_action::CodeAction;
use crate::lsp::completion::CompletionItemEdit;
use crate::lsp::signature_help::SignatureHelp;

use crate::selection_range::SelectionRange;
use crate::{
    buffer::Buffer,
    lsp::completion::{Completion, CompletionItem},
};

use itertools::Itertools;
use my_proc_macros::key;
use shared::icons::get_icon_config;
use std::{cell::RefCell, rc::Rc};

use super::component::ComponentId;
use super::{
    component::Component,
    dropdown::{Dropdown, DropdownConfig, DropdownItem},
    editor::{Editor, Mode},
};

/// Editor with auto-complete
pub struct SuggestiveEditor {
    editor: Editor,
    info_panel: Option<Rc<RefCell<Editor>>>,

    menu: Rc<RefCell<Dropdown<CodeAction>>>,
    menu_opened: bool,

    dropdown: Rc<RefCell<Dropdown<CompletionItem>>>,
    dropdown_opened: bool,
    trigger_characters: Vec<String>,
    filter: SuggestiveEditorFilter,
}

pub enum SuggestiveEditorFilter {
    CurrentWord,
    CurrentLine,
}

impl DropdownItem for CodeAction {
    fn label(&self) -> String {
        self.title()
    }
    fn info(&self) -> Option<Info> {
        None
    }

    fn group(&self) -> String {
        self.kind.clone().unwrap_or("".to_string())
    }
}

impl DropdownItem for CompletionItem {
    fn emoji(&self) -> String {
        self.kind
            .map(|kind| {
                get_icon_config()
                    .completion
                    .get(&format!("{:?}", kind))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("({:?})", kind))
            })
            .unwrap_or_default()
    }
    fn label(&self) -> String {
        self.label()
    }
    fn info(&self) -> Option<Info> {
        let kind = self.kind.map(|kind| {
            convert_case::Casing::to_case(&format!("{:?}", kind), convert_case::Case::Title)
        });
        let detail = self.detail.clone();

        let documentation = self.documentation().map(|d| d.content);
        Some(Info::new(
            [].into_iter()
                .chain(kind)
                .chain(detail)
                .chain(documentation)
                .collect_vec()
                .join("\n==========\n"),
        ))
    }

    fn group(&self) -> String {
        self.kind
            .map(|kind| format!("{:?}", kind))
            .unwrap_or(self.label.to_string())
    }
}

impl Component for SuggestiveEditor {
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
        if self.editor.mode == Mode::Insert && event == key!("esc") {
            self.close_all_subcomponents();
            self.editor.enter_normal_mode()?;
            return Ok(vec![]);
        }
        let dispatches = if self.editor.mode == Mode::Insert && self.dropdown_opened() {
            match event {
                key!("ctrl+n") | key!("down") => {
                    if self.dropdown_opened() {
                        self.dropdown.borrow_mut().next_item();
                    }
                    return Ok(vec![]);
                }
                key!("ctrl+p") | key!("up") => {
                    self.dropdown.borrow_mut().previous_item();
                    return Ok(vec![]);
                }
                key!("tab") => {
                    if let Some(completion) = self.dropdown.borrow_mut().current_item() {
                        let dispatches = match completion.edit {
                            None => self
                                .editor
                                .replace_previous_word(&completion.label(), context),
                            Some(edit) => match edit {
                                CompletionItemEdit::PositionalEdit(edit) => {
                                    self.editor.apply_positional_edit(edit)
                                }
                            },
                        }?;
                        self.dropdown_opened = false;
                        self.menu_opened = false;
                        self.info_panel = None;
                        return Ok(dispatches);
                    }
                    return Ok(vec![]);
                }
                key!("ctrl+enter") => self.editor.open_new_line()?,

                // Every other character typed in Insert mode should update the dropdown to show
                // relevant completions.
                event => self.editor.handle_key_event(context, event)?,
            }
        } else if self.editor.mode == Mode::Normal && self.menu_opened() {
            match event {
                key!("ctrl+n") | key!("down") => {
                    if self.menu_opened() {
                        self.menu.borrow_mut().next_item();
                    }
                    return Ok(vec![]);
                }
                key!("ctrl+p") | key!("up") => {
                    self.menu.borrow_mut().previous_item();
                    return Ok(vec![]);
                }
                key!("tab") => {
                    if let Some(code_action) = self.menu.borrow_mut().current_item() {
                        let dispatches = vec![Dispatch::ApplyWorkspaceEdit(code_action.edit)];
                        self.menu_opened = false;
                        return Ok(dispatches);
                    }
                    return Ok(vec![]);
                }
                key!("esc") => {
                    self.menu_opened = false;
                    return Ok(vec![]);
                }

                // Every other character typed in Insert mode should update the dropdown to show
                // relevant completions.
                event => self.editor.handle_key_event(context, event)?,
            }
        } else {
            let dispatches = self.editor.handle_key_event(context, event)?;

            if self.editor.mode == Mode::Insert {
                self.dropdown_opened = true;
            }

            dispatches
        };

        let filter = match self.filter {
            SuggestiveEditorFilter::CurrentWord => {
                // We need to subtract 1 because we need to get the character
                // before the cursor, not the character at the cursor
                let cursor_position = self.editor().get_cursor_position()?.sub_column(1);

                match self.editor().buffer().get_char_at_position(cursor_position) {
                    // The filter should be empty if the current character is a trigger
                    // character, so that we can show all the completion items.
                    Some(current_char)
                        if self.trigger_characters.contains(&current_char.to_string()) =>
                    {
                        "".to_string()
                    }

                    // If the current character is not a trigger character, we should
                    // filter based on the current word under the cursor.
                    _ => self.editor.get_current_word()?,
                }
            }
            SuggestiveEditorFilter::CurrentLine => self.editor().current_line()?,
        };

        self.dropdown.borrow_mut().set_filter(&filter)?;

        let dispatches = dispatches
            .into_iter()
            .chain(if self.editor.mode == Mode::Insert {
                self.editor
                    .get_request_params()
                    .map(|params| {
                        vec![
                            Dispatch::RequestCompletion(params.clone()),
                            Dispatch::RequestSignatureHelp(params),
                        ]
                    })
                    .unwrap_or_default()
            } else {
                vec![]
            })
            .collect::<Vec<_>>();

        Ok(dispatches)
    }

    fn children(&self) -> Vec<Option<Rc<RefCell<dyn Component>>>> {
        vec![
            if self.dropdown_opened() {
                Some(self.dropdown.clone() as Rc<RefCell<dyn Component>>)
            } else {
                None
            },
            if self.menu_opened() {
                Some(self.menu.clone() as Rc<RefCell<dyn Component>>)
            } else {
                None
            },
            self.info_panel
                .clone()
                .map(|info_panel| info_panel as Rc<RefCell<dyn Component>>),
        ]
    }

    fn remove_child(&mut self, component_id: ComponentId) {
        if self.dropdown.borrow().id() == component_id {
            self.dropdown_opened = false
        }
        if self.menu.borrow().id() == component_id {
            self.menu_opened = false
        }
        if matches!(&self.info_panel, Some(info_panel) if info_panel.borrow().id() == component_id)
        {
            self.info_panel = None;
        }
    }
}

impl SuggestiveEditor {
    pub fn from_buffer(buffer: Rc<RefCell<Buffer>>, filter: SuggestiveEditorFilter) -> Self {
        Self {
            editor: Editor::from_buffer(buffer),
            info_panel: None,
            menu: Rc::new(RefCell::new(Dropdown::new(DropdownConfig {
                title: "Menu".to_string(),
            }))),
            menu_opened: false,
            dropdown: Rc::new(RefCell::new(Dropdown::new(DropdownConfig {
                title: "Completion".to_string(),
            }))),
            trigger_characters: vec![],
            filter,
            dropdown_opened: false,
        }
    }

    pub fn show_signature_help(&mut self, signature_help: Option<SignatureHelp>) {
        if self.editor.mode != Mode::Insert {
            return;
        }
        if let Some(info) = signature_help.and_then(|s| s.into_info()) {
            self.show_info("Signature help", info);
        } else {
            self.info_panel = None;
        }
    }

    pub fn show_info(&mut self, title: &str, info: Info) {
        let mut editor = Editor::from_text(tree_sitter_md::language(), &info.content);
        editor.set_decorations(info.decorations());
        editor.set_title(title.into());
        self.info_panel = Some(Rc::new(RefCell::new(editor)));
    }

    pub fn set_code_actions(&mut self, code_actions: Vec<CodeAction>) {
        if self.editor.mode != Mode::Normal || code_actions.is_empty() {
            return;
        }

        self.menu_opened = true;
        self.menu.borrow_mut().set_items(code_actions);
    }

    pub fn set_completion(&mut self, completion: Completion) {
        if self.editor.mode != Mode::Insert {
            return;
        }

        self.dropdown_opened = true;
        self.dropdown.borrow_mut().set_items(completion.items);
        self.trigger_characters = completion.trigger_characters;
    }

    pub fn enter_insert_mode(&mut self) -> Result<(), anyhow::Error> {
        self.editor
            .enter_insert_mode(super::editor::Direction::Start)
    }

    pub fn current_item(&mut self) -> Option<CompletionItem> {
        self.dropdown.borrow_mut().current_item()
    }

    pub fn dropdown_opened(&self) -> bool {
        self.dropdown_opened
            && !self.dropdown.borrow().filtered_items().is_empty()
            && self.editor.mode == Mode::Insert
    }

    #[cfg(test)]
    pub fn filtered_dropdown_items(&self) -> Vec<String> {
        self.dropdown
            .borrow()
            .filtered_items()
            .iter()
            .map(|item| item.label())
            .collect()
    }

    fn menu_opened(&self) -> bool {
        self.menu_opened
    }

    fn close_all_subcomponents(&mut self) {
        self.info_panel = None;
        self.dropdown_opened = false;
        self.menu_opened = false;
    }
}

#[cfg(test)]
mod test_suggestive_editor {
    use crate::{
        app::Dispatch,
        buffer::Buffer,
        components::{
            component::Component,
            editor::{Direction, Mode},
        },
        lsp::completion::{Completion, CompletionItem, CompletionItemEdit, PositionalEdit},
        position::Position,
    };
    use lsp_types::CompletionItemKind;
    use my_proc_macros::keys;
    use shared::canonicalized_path::CanonicalizedPath;
    use std::{cell::RefCell, rc::Rc};

    use super::{SuggestiveEditor, SuggestiveEditorFilter};
    use pretty_assertions::assert_eq;

    fn dummy_completion() -> Completion {
        Completion {
            trigger_characters: vec![".".to_string()],
            items: vec![
                CompletionItem::from_label("Spongebob".to_string()),
                CompletionItem::from_label("Patrick".to_string()),
                CompletionItem::from_label("Squidward".to_string()),
            ],
        }
    }

    fn editor(filter: SuggestiveEditorFilter) -> SuggestiveEditor {
        SuggestiveEditor::from_buffer(
            Rc::new(RefCell::new(Buffer::new(tree_sitter_md::language(), ""))),
            filter,
        )
    }

    #[test]
    fn navigate_dropdown() {
        let mut editor = editor(SuggestiveEditorFilter::CurrentWord);

        // Enter insert mode
        editor
            .editor_mut()
            .enter_insert_mode(Direction::Start)
            .unwrap();

        // Pretend that the LSP server returned a completion
        editor.set_completion(dummy_completion());

        // Expect the completion dropdown to be open
        assert!(editor.dropdown_opened());

        // Expect the completion dropdown to show all the items
        assert_eq!(
            editor.filtered_dropdown_items(),
            vec!["Patrick", "Spongebob", "Squidward"]
        );

        // Expect the selected item to be the first item
        assert_eq!(
            editor.current_item(),
            Some(CompletionItem::from_label("Patrick".to_string()))
        );

        // Go to the next item using the down arrow key
        editor.handle_events(keys!("down")).unwrap();

        // Expect the selected item to be the second item
        assert_eq!(
            editor.current_item(),
            Some(CompletionItem::from_label("Spongebob".to_string()))
        );

        // Go to the previous item using the up arrow key
        editor.handle_events(keys!("up")).unwrap();

        // Expect the selected item to be the first item
        assert_eq!(
            editor.current_item(),
            Some(CompletionItem::from_label("Patrick".to_string()))
        );

        // Go to the next item using ctrl+n
        editor.handle_events(keys!("ctrl+n")).unwrap();

        // Expect the selected item to be the second item
        assert_eq!(
            editor.current_item(),
            Some(CompletionItem::from_label("Spongebob".to_string()))
        );

        // Go to the previous item using ctrl+p
        editor.handle_events(keys!("ctrl+p")).unwrap();

        // Expect the selected item to be the first item
        assert_eq!(
            editor.current_item(),
            Some(CompletionItem::from_label("Patrick".to_string()))
        );
    }

    #[test]
    fn trigger_characters() {
        let mut editor = editor(SuggestiveEditorFilter::CurrentWord);

        // Enter insert mode
        editor
            .editor_mut()
            .enter_insert_mode(Direction::Start)
            .unwrap();

        // Pretend that the LSP server returned a completion
        editor.set_completion(dummy_completion());

        // Type in 'pa'
        editor.handle_events(keys!("p a")).unwrap();

        // Expect the completion dropdown to be open,
        // and the dropdown items to be filtered
        assert!(editor.dropdown_opened());
        assert_eq!(editor.filtered_dropdown_items(), vec!["Patrick"]);

        // Type in one of the trigger characters, '.'
        editor.handle_events(keys!(".")).unwrap();

        // Expect the completion dropdown to be open,
        // and the dropdown items to be unfiltered (showing all items)
        assert!(editor.dropdown_opened());

        // Expect the completion dropdown to show all the items
        assert_eq!(
            editor.filtered_dropdown_items(),
            vec!["Patrick", "Spongebob", "Squidward"]
        );
    }

    #[test]
    fn completion_without_edit() {
        let mut editor = editor(SuggestiveEditorFilter::CurrentWord);

        // Enter insert mode
        editor
            .editor_mut()
            .enter_insert_mode(Direction::Start)
            .unwrap();

        // Pretend that the LSP server returned a completion
        editor.set_completion(dummy_completion());

        // Type in 'pa'
        editor.handle_events(keys!("p a")).unwrap();

        // Expect the completion dropdown to be open,
        // and the dropdown items to be filtered
        assert!(editor.dropdown_opened());
        assert_eq!(editor.filtered_dropdown_items(), vec!["Patrick"]);

        // Press enter
        editor.handle_events(keys!("tab")).unwrap();

        // Expect the completion dropdown to be closed
        assert!(!editor.dropdown_opened());

        // Expect the buffer to contain the selected item
        assert_eq!(editor.editor().text(), "Patrick");
    }

    #[test]
    fn completion_with_emoji() -> anyhow::Result<()> {
        let mut editor = editor(SuggestiveEditorFilter::CurrentWord);

        // Enter insert mode
        editor.editor_mut().enter_insert_mode(Direction::Start)?;

        // Enter s
        editor.handle_events(keys!("s"))?;

        // Pretend that the LSP server returned a completion
        // That is without edit, but contains `kind`, which means it has emoji
        editor.set_completion(Completion {
            trigger_characters: vec![".".to_string()],
            items: [CompletionItem {
                label: "Spongebob".to_string(),
                edit: None,
                documentation: None,
                sort_text: None,
                kind: Some(CompletionItemKind::FUNCTION),
                detail: None,
            }]
            .to_vec(),
        });

        // Expect the dropdown to contains emoji
        let dropdown_content = editor.dropdown.borrow().content();
        assert_eq!(dropdown_content, "ƒ Spongebob");

        // Press enter
        editor.handle_events(keys!("tab"))?;

        // Expect the content of the buffer to be applied with the new edit,
        // resulting in 'Spongebob', and does not contain emoji
        assert_eq!(editor.editor().text(), "Spongebob");

        Ok(())
    }

    #[test]
    fn completion_with_edit() -> anyhow::Result<()> {
        let mut editor = editor(SuggestiveEditorFilter::CurrentWord);

        // Enter insert mode
        editor.editor_mut().enter_insert_mode(Direction::Start)?;

        // Enter a word 'sponge'
        editor.handle_events(keys!("s p o n g e")).unwrap();

        // Pretend that the LSP server returned a completion
        editor.set_completion(Completion {
            trigger_characters: vec![".".to_string()],
            items: vec![CompletionItem {
                label: "Spongebob".to_string(),
                edit: Some(CompletionItemEdit::PositionalEdit(PositionalEdit {
                    range: Position::new(0, 0)..Position::new(0, 6),
                    new_text: "Spongebob".to_string(),
                })),
                documentation: None,
                sort_text: None,
                kind: None,
                detail: None,
            }],
        });

        // Press enter
        editor.handle_events(keys!("tab")).unwrap();

        // Expect the content of the buffer to be applied with the new edit,
        // resulting in 'Spongebob'
        assert_eq!(editor.editor().text(), "Spongebob");

        // Type in 'end'
        editor.handle_events(keys!("e n d")).unwrap();

        // Expect the content of the buffer to be 'Spongebobend'
        assert_eq!(editor.editor().text(), "Spongebobend");
        Ok(())
    }

    #[test]
    fn filter_with_current_word() {
        let mut editor = editor(SuggestiveEditorFilter::CurrentWord);

        // Enter insert mode
        editor
            .editor_mut()
            .enter_insert_mode(Direction::Start)
            .unwrap();

        // Pretend that the LSP server returned a completion
        editor.set_completion(dummy_completion());

        // Type in 'pa'
        editor.handle_events(keys!("p a")).unwrap();

        // Expect the completion dropdown to be open,
        // and the dropdown items to be filtered
        assert!(editor.dropdown_opened());
        assert_eq!(editor.filtered_dropdown_items(), vec!["Patrick"]);

        // Type in space, then 's'
        editor.handle_events(keys!("space s")).unwrap();

        // Expect the completion dropdown to be open,
        // and the dropdown items to be filtered by the current word, 's'
        assert_eq!(
            editor.filtered_dropdown_items(),
            vec!["Spongebob", "Squidward"]
        );
    }

    #[test]
    #[ignore]
    fn filter_with_current_line() -> anyhow::Result<()> {
        let mut editor = editor(SuggestiveEditorFilter::CurrentLine);

        // Enter insert mode
        editor
            .editor_mut()
            .enter_insert_mode(Direction::Start)
            .unwrap();

        // Pretend that the LSP server returned a completion
        editor.set_completion(dummy_completion());

        // Type in 'pa'
        editor.handle_events(keys!("p a"))?;

        // Expect the completion dropdown to be open,
        // and the dropdown items to be filtered
        assert!(editor.dropdown_opened());
        assert_eq!(editor.filtered_dropdown_items(), vec!["Patrick"]);

        // Type in space, then 's'
        editor.handle_events(keys!("space s"))?;

        // Expect the completion dropdown to be hidden,
        // and the dropdown items to be filtered by the current line, 'pa s'
        assert!(!editor.dropdown_opened());
        assert_eq!(editor.filtered_dropdown_items(), Vec::new() as Vec<String>);

        // Type in enter
        editor.handle_events(keys!("tab"))?;

        // Expect a new line is added
        assert_eq!(editor.editor().text(), "pa s\n");

        // Expect the current line is empty
        assert_eq!(editor.editor().current_line()?, "");

        // Expect the completion dropdown to be open,
        // and all dropdown items to be shown,
        // because the current line is empty
        assert!(editor.dropdown_opened());
        assert_eq!(
            editor.filtered_dropdown_items(),
            vec!["Patrick", "Spongebob", "Squidward"]
        );

        // Close the dropdown menu
        editor.handle_events(keys!("ctrl+e"))?;

        // Enter a next line
        editor.handle_events(keys!("enter h e l l o"))?;

        // Expect the content to be updated
        assert_eq!(editor.editor().text(), "pa s\n\nhello");

        // Expect the current line is 'hello'
        assert_eq!(editor.editor().current_line()?, "hello");

        // Go to the previous line
        editor.handle_events(keys!("esc l p p p"))?;

        // Expect the current line is empty
        assert_eq!(editor.editor().current_line()?, "");

        // Type in 's'
        editor.editor_mut().enter_insert_mode(Direction::Start)?;
        editor.handle_events(keys!("s"))?;

        // Expect the current line is 's'
        assert_eq!(editor.editor().current_line()?, "s");

        assert_eq!(editor.editor().text(), "pa s\ns\nhello",);

        // Expect the completion dropdown to be open,
        // and the dropdown items to be filtered by the current line, 's'
        assert!(editor.dropdown_opened());
        assert_eq!(
            editor.filtered_dropdown_items(),
            vec!["Spongebob", "Squidward"]
        );
        Ok(())
    }

    #[test]
    fn setting_completion_when_not_in_insert_mode() {
        let mut editor = editor(SuggestiveEditorFilter::CurrentWord);

        // Expect the editor to not be in insert mode
        assert_ne!(editor.editor().mode, Mode::Insert);

        // Pretend that the LSP server returned a completion
        editor.set_completion(dummy_completion());

        // Expect the completion dropdown to not be opened,
        // since the editor is not in insert mode
        assert!(!editor.dropdown_opened());
    }

    #[test]
    fn dropdown_should_be_excluded_from_descendants_by_dropdown_opened() {
        let mut editor = editor(SuggestiveEditorFilter::CurrentWord);

        // Enter insert mode
        editor
            .editor_mut()
            .enter_insert_mode(Direction::Start)
            .unwrap();

        // Pretend that the LSP server returned a completion
        editor.set_completion(dummy_completion());

        // Expect the completion dropdown to be opened
        assert!(editor.dropdown_opened());

        // Expect the dropdown to be included in descendants
        assert!(editor
            .descendants()
            .iter()
            .any(|d| d.borrow().id() == editor.dropdown.borrow().id()));

        // Set the dropdown to be closed
        editor.dropdown_opened = false;

        // Expect the dropdown to be excluded from descendants
        assert!(!editor
            .descendants()
            .iter()
            .any(|d| d.borrow().id() == editor.dropdown.borrow().id()));
    }

    #[test]
    fn typing_in_insert_mode_should_request_completion() {
        let mut editor = editor(SuggestiveEditorFilter::CurrentWord);

        let file = tempfile::NamedTempFile::new().unwrap();

        let path: CanonicalizedPath = file.path().to_path_buf().try_into().unwrap();

        editor.editor_mut().buffer_mut().set_path(path);

        // Enter insert mode
        editor
            .editor_mut()
            .enter_insert_mode(Direction::Start)
            .unwrap();

        // Type something
        let dispatches = editor.handle_events(keys!("p")).unwrap();

        // Expect the completion request to be sent
        assert!(dispatches
            .into_iter()
            .any(|dispatch| matches!(&dispatch, Dispatch::RequestCompletion(_))));

        // Enter normal mode
        editor.handle_events(keys!("esc")).unwrap();

        // Type something
        let dispatches = editor.handle_events(keys!("l")).unwrap();

        // Expect the completion request to not be sent
        assert!(!dispatches
            .into_iter()
            .any(|dispatch| matches!(&dispatch, Dispatch::RequestCompletion(_))));
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Info {
    content: String,
    decorations: Vec<Decoration>,
}
impl Info {
    pub(crate) fn new(content: String) -> Info {
        Info {
            content,
            decorations: Vec::new(),
        }
    }

    pub fn content(&self) -> &String {
        &self.content
    }

    pub fn decorations(&self) -> &Vec<Decoration> {
        &self.decorations
    }

    pub fn take(self) -> (String, Vec<Decoration>) {
        let Self {
            content,
            decorations,
        } = self;
        (content, decorations)
    }

    pub fn set_decorations(self, decorations: Vec<Decoration>) -> Info {
        Info {
            decorations,
            ..self
        }
    }

    pub(crate) fn join(self, other: Info) -> Info {
        let separator = "=".repeat(10).to_string();
        let content = format!("{}\n{}\n{}", self.content, separator, other.content);
        let other_decorations = other
            .decorations
            .into_iter()
            .map(|decoration| decoration.increase_byte(separator.len() + 2))
            .collect_vec();
        let decorations = self
            .decorations
            .into_iter()
            .chain(other_decorations)
            .collect_vec();
        Info {
            content,
            decorations,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Decoration {
    selection_range: SelectionRange,
    style_key: StyleKey,
    adjustments: Vec<Adjustment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Adjustment {
    IncreaseByte(usize),
}

impl Decoration {
    fn increase_byte(mut self, len: usize) -> Decoration {
        self.adjustments.push(Adjustment::IncreaseByte(len));
        self
    }

    pub(crate) fn selection_range(&self) -> &SelectionRange {
        &self.selection_range
    }

    pub(crate) fn style_key(&self) -> &StyleKey {
        &self.style_key
    }

    pub(crate) fn new(selection_range: SelectionRange, style_key: StyleKey) -> Decoration {
        Decoration {
            selection_range,
            style_key,
            adjustments: Default::default(),
        }
    }

    pub(crate) fn move_left(self, count: usize) -> Decoration {
        Decoration {
            selection_range: self.selection_range.move_left(count),
            ..self
        }
    }
}
