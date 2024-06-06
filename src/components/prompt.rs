use std::{cell::RefCell, rc::Rc};

use my_proc_macros::key;

use crate::{
    app::{Dispatch, DispatchPrompt, Dispatches, GlobalSearchFilterGlob, Scope},
    buffer::Buffer,
    components::editor::{self, DispatchEditor},
    context::Context,
    lsp::completion::{Completion, CompletionSource},
};

use super::{
    component::Component,
    dropdown::DropdownItem,
    editor::{Editor, Mode},
    suggestive_editor::{SuggestiveEditor, SuggestiveEditorFilter},
};

pub(crate) struct Prompt {
    editor: SuggestiveEditor,
    /// This will only be run on user input if the user input matches no dropdown item.
    on_enter: DispatchPrompt,
    enter_selects_first_matching_item: bool,
    prompt_history_key: PromptHistoryKey,
    fire_dispatches_on_change: Option<Dispatches>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PromptConfig {
    pub(crate) on_enter: DispatchPrompt,
    pub(crate) items: Vec<DropdownItem>,
    pub(crate) title: String,
    pub(crate) enter_selects_first_matching_item: bool,
    pub(crate) leaves_current_line_empty: bool,

    /// If defined, the `Dispatches` here is used for undoing the dispatches fired on change.
    pub(crate) fire_dispatches_on_change: Option<Dispatches>,
}

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub(crate) enum PromptHistoryKey {
    MoveToIndex,
    Search(Scope),
    Rename,
    AddPath,
    MovePath,
    Symbol,
    Command,
    OpenFile,
    Omit,
    FilterGlob(GlobalSearchFilterGlob),
    Replacement(Scope),
    CodeAction,
    #[cfg(test)]
    Null,
    Theme,
}

impl Prompt {
    pub(crate) fn new(
        config: PromptConfig,
        prompt_history_key: PromptHistoryKey,
        mut history: Vec<String>,
    ) -> (Self, Dispatches) {
        let text = {
            history.reverse();
            format!(
                "{}{}",
                if config.leaves_current_line_empty {
                    "\n"
                } else {
                    ""
                },
                history.join("\n")
            )
        };
        let mut editor = SuggestiveEditor::from_buffer(
            Rc::new(RefCell::new(Buffer::new(None, &text))),
            SuggestiveEditorFilter::CurrentLine,
        );
        let dispatches = Dispatches::one(Dispatch::ToEditor(if config.leaves_current_line_empty {
            DispatchEditor::EnterInsertMode(editor::Direction::Start)
        } else {
            DispatchEditor::MoveToLineEnd
        }));
        // TODO: set cursor to last line
        editor.set_title(config.title);
        editor.update_completion(Completion {
            source: CompletionSource::PromptItems,
            items: config.items,
            trigger_characters: vec![" ".to_string()],
        });
        let dispatches = dispatches.append(editor.render_completion_dropdown(true));
        (
            Prompt {
                editor,
                on_enter: config.on_enter,
                enter_selects_first_matching_item: config.enter_selects_first_matching_item,
                prompt_history_key,
                fire_dispatches_on_change: config.fire_dispatches_on_change,
            },
            dispatches,
        )
    }
}

impl Component for Prompt {
    fn editor(&self) -> &Editor {
        self.editor.editor()
    }
    fn editor_mut(&mut self) -> &mut Editor {
        self.editor.editor_mut()
    }
    fn handle_dispatch_editor(
        &mut self,
        context: &mut Context,
        dispatch: super::editor::DispatchEditor,
    ) -> anyhow::Result<Dispatches> {
        self.editor.handle_dispatch_editor(context, dispatch)
    }
    fn handle_key_event(
        &mut self,
        context: &Context,
        event: event::KeyEvent,
    ) -> anyhow::Result<Dispatches> {
        match event {
            key!("esc") if self.editor().mode == Mode::Normal => {
                Ok(Dispatches::one(Dispatch::CloseCurrentWindow)
                    .chain(self.fire_dispatches_on_change.clone().unwrap_or_default()))
            }
            key!("ctrl+space") => {
                if self.editor.completion_dropdown_opened() {
                    if let Some(item) = self.editor.completion_dropdown_current_item() {
                        self.editor.set_content(&item.display())?;
                        return self.editor_mut().move_to_line_end();
                    }
                    Ok(Default::default())
                } else {
                    self.editor_mut().handle_key_event(context, event)
                }
            }
            key!("enter") => {
                let (line, dispatches) = if self.enter_selects_first_matching_item
                    && self.editor.completion_dropdown_current_item().is_some()
                {
                    self.editor
                        .completion_dropdown_current_item()
                        .map(|item| (item.display(), item.dispatches))
                        .unwrap_or_default()
                } else {
                    let current_line = self.editor().current_line()?;
                    let dispatches = self.on_enter.to_dispatches(&current_line)?;
                    (current_line, dispatches)
                };

                Ok(Dispatches::one(Dispatch::CloseCurrentWindow)
                    .chain(dispatches)
                    .append(Dispatch::PushPromptHistory {
                        key: self.prompt_history_key,
                        line,
                    }))
            }
            _ => {
                let dispatches = self.editor.handle_key_event(context, event)?;
                Ok(if self.fire_dispatches_on_change.is_some() {
                    dispatches.chain(
                        self.editor
                            .completion_dropdown_current_item()
                            .map(|item| item.dispatches)
                            .unwrap_or_default(),
                    )
                } else {
                    dispatches
                })
            }
        }
    }
}

#[cfg(test)]
mod test_prompt {
    use crate::{
        components::{editor::Direction, suggestive_editor::Info},
        lsp::completion::CompletionItem,
        test_app::*,
    };
    use my_proc_macros::keys;

    use crate::{app::Dispatch, position::Position};

    use super::*;

    #[test]
    fn leaves_current_line_empty() {
        fn test(
            leaves_current_line_empty: bool,
            expected_text: &'static str,
            expected_cursor_position: Position,
        ) {
            execute_test(|s| {
                Box::new([
                    App(OpenFile(s.main_rs())),
                    App(OpenPrompt {
                        current_line: Some("hello".to_string()),
                        key: PromptHistoryKey::Null,
                        config: PromptConfig {
                            on_enter: DispatchPrompt::Null,
                            items: Default::default(),
                            title: "".to_string(),
                            enter_selects_first_matching_item: true,
                            leaves_current_line_empty,
                            fire_dispatches_on_change: None,
                        },
                    }),
                    Expect(CurrentComponentContent(expected_text)),
                    Expect(EditorCursorPosition(expected_cursor_position)),
                ])
            })
            .unwrap();
        }
        test(true, "\nhello", Position::default());
        test(false, "hello", Position::new(0, 5));
    }

    #[test]
    fn prompt_history() {
        execute_test(|s| {
            let open_prompt = OpenPrompt {
                key: PromptHistoryKey::Null,
                current_line: None,
                config: PromptConfig {
                    on_enter: DispatchPrompt::Null,
                    items: Default::default(),
                    title: "".to_string(),
                    enter_selects_first_matching_item: true,
                    leaves_current_line_empty: true,
                    fire_dispatches_on_change: None,
                },
            };
            Box::new([
                App(OpenFile(s.main_rs())),
                App(open_prompt.clone()),
                Expect(CurrentComponentContent("\n")),
                App(HandleKeyEvents(keys!("h e l l o enter").to_vec())),
                App(open_prompt.clone()),
                Expect(CurrentComponentContent("\nhello")),
                App(HandleKeyEvents(keys!("h e l l o enter").to_vec())),
                App(open_prompt.clone()),
                // Duplicates should not be repeated
                Expect(CurrentComponentContent("\nhello")),
                App(open_prompt.clone()),
                App(HandleKeyEvents(keys!("y o enter").to_vec())),
                App(open_prompt.clone()),
                // Latest entry should appear on first
                Expect(CurrentComponentContent("\nyo\nhello")),
                // Enter 'hello' again
                App(open_prompt.clone()),
                App(HandleKeyEvents(keys!("h e l l o enter").to_vec())),
                // 'hello' should appear on top
                App(open_prompt.clone()),
                Expect(CurrentComponentContent("\nhello\nyo")),
            ])
        })
        .unwrap();
    }

    #[test]
    fn current_line() {
        execute_test(|s| {
            Box::new([
                App(OpenFile(s.main_rs())),
                App((OpenPrompt {
                    key: PromptHistoryKey::Null,
                    current_line: Some("spongebob squarepants".to_string()),
                    config: PromptConfig {
                        on_enter: DispatchPrompt::Null,
                        items: Default::default(),
                        title: "".to_string(),
                        enter_selects_first_matching_item: true,
                        leaves_current_line_empty: true,
                        fire_dispatches_on_change: None,
                    },
                })
                .clone()),
                Expect(CurrentComponentContent("\nspongebob squarepants")),
            ])
        })
        .unwrap();
    }

    #[test]
    fn enter_selects_first_matching_item() {
        fn run_test(
            enter_selects_first_matching_item: bool,
            input_text: &str,
            expected_invoked_text: &'static str,
        ) {
            execute_test(|s| {
                Box::new([
                    App(OpenFile(s.main_rs())),
                    App(OpenPrompt {
                        key: PromptHistoryKey::Null,
                        current_line: None,
                        config: super::PromptConfig {
                            on_enter: DispatchPrompt::SetContent,
                            items: ["foo".to_string(), "bar".to_string()]
                                .into_iter()
                                .map(|str| {
                                    let item: DropdownItem = str.clone().into();
                                    item.set_dispatches(Dispatches::one(ToEditor(SetContent(str))))
                                })
                                .collect(),

                            title: "".to_string(),
                            enter_selects_first_matching_item,
                            leaves_current_line_empty: true,
                            fire_dispatches_on_change: None,
                        },
                    }),
                    Expect(CompletionDropdownIsOpen(true)),
                    App(HandleKeyEvents(
                        event::parse_key_events(input_text).unwrap(),
                    )),
                    App(HandleKeyEvent(key!("enter"))),
                    Expect(CurrentComponentContent(expected_invoked_text)),
                ])
            })
            .unwrap()
        }

        run_test(true, "f", "foo");
        run_test(false, "f", "f");
    }

    #[test]
    fn ctrl_space_replace_current_content_with_first_highlighted_suggestion() -> anyhow::Result<()>
    {
        execute_test(|_| {
            Box::new([
                App(Dispatch::OpenPrompt {
                    key: PromptHistoryKey::Null,
                    current_line: None,
                    config: super::PromptConfig {
                        on_enter: DispatchPrompt::SetContent,
                        items: ["foo_bar".to_string()]
                            .into_iter()
                            .map(|item| item.into())
                            .collect(),

                        title: "".to_string(),
                        enter_selects_first_matching_item: true,
                        leaves_current_line_empty: true,
                        fire_dispatches_on_change: None,
                    },
                }),
                App(HandleKeyEvents(keys!("f o o _ b ctrl+space").to_vec())),
                Expect(CurrentComponentContent("foo_bar")),
                Expect(EditorCursorPosition(Position { line: 0, column: 7 })),
            ])
        })
    }

    #[test]
    fn fire_dispatches_on_change() -> anyhow::Result<()> {
        execute_test(|_| {
            Box::new([
                App(Dispatch::OpenPrompt {
                    key: PromptHistoryKey::Null,
                    current_line: None,
                    config: super::PromptConfig {
                        on_enter: DispatchPrompt::Null,
                        items: [
                            "foo_bar".to_string(),
                            "zazam".to_string(),
                            "boque".to_string(),
                        ]
                        .into_iter()
                        .map(|item| item.into())
                        .map(|item: DropdownItem| {
                            let content = item.display();
                            item.set_dispatches(Dispatches::one(Dispatch::ShowEditorInfo(
                                Info::new("".to_string(), content),
                            )))
                        })
                        .collect(),

                        title: "".to_string(),
                        enter_selects_first_matching_item: true,
                        leaves_current_line_empty: true,
                        fire_dispatches_on_change: Some(Dispatches::one(Dispatch::ShowEditorInfo(
                            Info::new("".to_string(), "back to square one".to_string()),
                        ))),
                    },
                }),
                App(HandleKeyEvents(keys!("f o o _").to_vec())),
                Expect(EditorInfoContent("foo_bar")),
                App(HandleKeyEvents(keys!("ctrl+u z a m").to_vec())),
                Expect(EditorInfoContent("zazam")),
                App(HandleKeyEvents(keys!("ctrl+u q").to_vec())),
                Expect(EditorInfoContent("boque")),
                App(HandleKeyEvents(keys!("esc esc").to_vec())),
                Expect(EditorInfoContent("back to square one")),
            ])
        })
    }

    #[test]
    fn filter_without_matching_items_should_clear_suggestions() -> anyhow::Result<()> {
        execute_test(|_| {
            Box::new([
                App(Dispatch::OpenPrompt {
                    key: PromptHistoryKey::Null,
                    current_line: None,
                    config: super::PromptConfig {
                        on_enter: DispatchPrompt::SetContent,
                        items: [CompletionItem::from_label(
                            "spongebob squarepants".to_string(),
                        )]
                        .into_iter()
                        .map(|item| item.into())
                        .collect(),

                        title: "".to_string(),
                        enter_selects_first_matching_item: true,
                        leaves_current_line_empty: true,
                        fire_dispatches_on_change: None,
                    },
                }),
                App(TerminalDimensionChanged(crate::app::Dimension {
                    height: 10,
                    width: 50,
                })),
                Expect(AppGridContains("squarepants")),
                App(HandleKeyEvents(keys!("f o o").to_vec())),
                Expect(Not(Box::new(AppGridContains("squarepants")))),
            ])
        })
    }

    #[test]
    fn suggestion_should_update_with_ctrl_k_and_ctrl_u() -> Result<(), anyhow::Error> {
        execute_test(|s| {
            Box::new([
                App(OpenFile(s.main_rs())),
                Editor(SetContent("".to_string())),
                Editor(EnterInsertMode(Direction::Start)),
                App(Dispatch::OpenPrompt {
                    key: PromptHistoryKey::Null,
                    current_line: None,
                    config: super::PromptConfig {
                        on_enter: DispatchPrompt::SetContent,
                        items: ["Patrick", "Spongebob", "Squidward"]
                            .into_iter()
                            .map(|item| item.to_string().into())
                            .collect(),

                        title: "".to_string(),
                        enter_selects_first_matching_item: true,
                        leaves_current_line_empty: true,
                        fire_dispatches_on_change: None,
                    },
                }),
                // Expect the completion dropdown to be open,
                Expect(CompletionDropdownContent("Patrick\nSpongebob\nSquidward")),
                // Type in 'pa'
                App(HandleKeyEvents(keys!("p a").to_vec())),
                // Expect only 'Patrick' remains in the completion dropdown
                Expect(CompletionDropdownContent("Patrick")),
                // Clear 'pa' using ctrl+u
                App(HandleKeyEvent(key!("ctrl+u"))),
                // Expect all items are shown again
                Expect(CompletionDropdownContent("Patrick\nSpongebob\nSquidward")),
                //
                //
                // Perform the same test for ctrl+k
                App(HandleKeyEvents(keys!("p a").to_vec())),
                Expect(CompletionDropdownContent("Patrick")),
                App(HandleKeyEvents(keys!("ctrl+a ctrl+k").to_vec())),
                Expect(CompletionDropdownContent("Patrick\nSpongebob\nSquidward")),
            ])
        })
    }
}
