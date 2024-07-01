use crossterm::event::KeyCode;
use SelectionMode::*;

use convert_case::Case;
use event::KeyEvent;
use itertools::Itertools;

use crate::{
    app::{Dispatch, Dispatches, FilePickerKind, MakeFilterMechanism, Scope},
    components::{editor::Movement, keymap_legend::KeymapLegendSection},
    context::{Context, LocalSearchConfigMode, Search},
    git::DiffMode,
    list::grep::RegexConfig,
    quickfix_list::{DiagnosticSeverityRange, QuickfixListType},
    selection::{FilterKind, FilterTarget, SelectionMode},
    surround::EnclosureKind,
    transformation::Transformation,
};

use super::{
    editor::{Direction, DispatchEditor, Editor, HandleEventResult, SurroundKind},
    keymap_legend::{Keymap, KeymapLegendBody, KeymapLegendConfig, Keymaps},
    suggestive_editor::Info,
};

use DispatchEditor::*;
use Movement::*;
impl Editor {
    pub(crate) fn keymap_core_movements(&self) -> KeymapLegendSection {
        KeymapLegendSection {
            title: "Movements (Core)".to_string(),
            keymaps: Keymaps::new(&[
                Keymap::new(
                    "p",
                    "Previous".to_string(),
                    Dispatch::ToEditor(MoveSelection(Movement::Previous)),
                ),
                Keymap::new(
                    "n",
                    "Next".to_string(),
                    Dispatch::ToEditor(MoveSelection(Next)),
                ),
                Keymap::new("u", "Up".to_string(), Dispatch::ToEditor(MoveSelection(Up))),
                Keymap::new(
                    "d",
                    "Down".to_string(),
                    Dispatch::ToEditor(MoveSelection(Down)),
                ),
                Keymap::new(
                    ",",
                    "First".to_string(),
                    Dispatch::ToEditor(MoveSelection(Movement::First)),
                ),
                Keymap::new(
                    ".",
                    "Last".to_string(),
                    Dispatch::ToEditor(MoveSelection(Movement::Last)),
                ),
                Keymap::new(
                    "j",
                    "Jump".to_string(),
                    Dispatch::ToEditor(DispatchEditor::ShowJumps {
                        use_current_selection_mode: true,
                    }),
                ),
                Keymap::new(
                    "-",
                    "Parent Line".to_string(),
                    Dispatch::ToEditor(MoveSelection(ToParentLine)),
                ),
                Keymap::new(
                    "0",
                    "To Index (1-based)".to_string(),
                    Dispatch::OpenMoveToIndexPrompt,
                ),
            ]),
        }
    }

    pub(crate) fn keymap_other_movements(&self) -> KeymapLegendSection {
        KeymapLegendSection {
            title: "Movements (Other)".to_string(),
            keymaps: Keymaps::new(&[
                Keymap::new(
                    "%",
                    "Swap cursor with anchor".to_string(),
                    Dispatch::ToEditor(DispatchEditor::SwapCursorWithAnchor),
                ),
                Keymap::new(
                    "ctrl+d",
                    "Scroll page down".to_string(),
                    Dispatch::ToEditor(ScrollPageDown),
                ),
                Keymap::new(
                    "ctrl+u",
                    "Scroll page up".to_string(),
                    Dispatch::ToEditor(ScrollPageUp),
                ),
                Keymap::new("[", "Go back".to_string(), Dispatch::ToEditor(GoBack)),
                Keymap::new("]", "Go forward".to_string(), Dispatch::ToEditor(GoForward)),
                Keymap::new(
                    "{",
                    "Go to previous file".to_string(),
                    Dispatch::GoToPreviousFile,
                ),
                Keymap::new(
                    "}",
                    "Go to next selection".to_string(),
                    Dispatch::GoToNextFile,
                ),
            ]),
        }
    }

    pub(crate) fn keymap_selection_modes(&self, context: &Context) -> KeymapLegendSection {
        KeymapLegendSection {
            title: "Selection mode".to_string(),
            keymaps: Keymaps::new(&[
                Keymap::new(
                    "b",
                    "Between".to_string(),
                    Dispatch::ShowKeymapLegend(self.between_mode_keymap_legend_config()),
                ),
                Keymap::new(
                    "l",
                    "Line (Trimmed)".to_string(),
                    Dispatch::ToEditor(SetSelectionMode(LineTrimmed)),
                ),
                Keymap::new(
                    "L",
                    "Line (Full)".to_string(),
                    Dispatch::ToEditor(SetSelectionMode(LineFull)),
                ),
                Keymap::new(
                    "f",
                    "Find (Local)".to_string(),
                    Dispatch::ShowKeymapLegend(
                        self.find_keymap_legend_config(context, Scope::Local),
                    ),
                ),
                Keymap::new(
                    "g",
                    "Find (Global)".to_string(),
                    Dispatch::ShowKeymapLegend(
                        self.find_keymap_legend_config(context, Scope::Global),
                    ),
                ),
                Keymap::new(
                    "s",
                    "Syntax Node (Coarse)".to_string(),
                    Dispatch::ToEditor(SetSelectionMode(SyntaxNodeCoarse)),
                ),
                Keymap::new(
                    "S",
                    "Syntax Node (Fine)".to_string(),
                    Dispatch::ToEditor(SetSelectionMode(SyntaxNodeFine)),
                ),
                Keymap::new(
                    "t",
                    "Token".to_string(),
                    Dispatch::ToEditor(SetSelectionMode(Token)),
                ),
                Keymap::new(
                    "h",
                    "Hieroglyph".to_string(),
                    Dispatch::ToEditor(SetSelectionMode(Column)),
                ),
                Keymap::new(
                    "w",
                    "Word (Short)".to_string(),
                    Dispatch::ToEditor(SetSelectionMode(WordShort)),
                ),
                Keymap::new(
                    "W",
                    "Word (Long)".to_string(),
                    Dispatch::ToEditor(SetSelectionMode(WordLong)),
                ),
            ]),
        }
    }
    pub(crate) fn keymap_actions(&self) -> KeymapLegendSection {
        KeymapLegendSection {
            title: "Action".to_string(),
            keymaps: Keymaps::new(&[
                Keymap::new(
                    "i",
                    "Insert (before selection)".to_string(),
                    Dispatch::ToEditor(EnterInsertMode(Direction::Start)),
                ),
                Keymap::new(
                    "a",
                    "Insert (after selection)".to_string(),
                    Dispatch::ToEditor(EnterInsertMode(Direction::End)),
                ),
                Keymap::new("y", "Yeet".to_string(), Dispatch::ToEditor(Change)),
                Keymap::new(
                    "k",
                    "Kill (until next selection)".to_string(),
                    Dispatch::ToEditor(Delete { backward: false }),
                ),
                Keymap::new(
                    "K",
                    "Kill (until previous selection)".to_string(),
                    Dispatch::ToEditor(Delete { backward: true }),
                ),
                Keymap::new(
                    "^",
                    "Raise".to_string(),
                    Dispatch::ToEditor(Replace(Parent)),
                ),
                Keymap::new(
                    "m",
                    "Mark (Toggle)".to_string(),
                    Dispatch::ToEditor(ToggleBookmark),
                ),
                Keymap::new(
                    "o",
                    "Open (after selection)".to_string(),
                    Dispatch::ToEditor(Open(Direction::End)),
                ),
                Keymap::new(
                    "O",
                    "Open (before selection)".to_string(),
                    Dispatch::ToEditor(Open(Direction::Start)),
                ),
                Keymap::new(
                    "ctrl+r",
                    "Replace with pattern".to_string(),
                    Dispatch::ToEditor(ReplaceWithPattern),
                ),
                Keymap::new(
                    "ctrl+p",
                    "Replace (with previous copied text)".to_string(),
                    Dispatch::ToEditor(ReplaceWithPreviousCopiedText),
                ),
                Keymap::new(
                    "ctrl+n",
                    "Replace (with next copied text)".to_string(),
                    Dispatch::ToEditor(ReplaceWithNextCopiedText),
                ),
                Keymap::new(
                    "e",
                    "Toggle Extend Mode".to_string(),
                    Dispatch::ToEditor(ToggleVisualMode),
                ),
                Keymap::new("enter", "Save".to_string(), Dispatch::ToEditor(Save)),
                Keymap::new(
                    "!",
                    "Transform".to_string(),
                    Dispatch::ShowKeymapLegend(self.transform_keymap_legend_config()),
                ),
            ]),
        }
    }

    fn keymap_clipboard_related_actions(&self, use_system_clipboard: bool) -> KeymapLegendSection {
        KeymapLegendSection {
            title: "Clipboard-related actions".to_string(),
            keymaps: Keymaps::new(&[
                Keymap::new(
                    "c",
                    "Copy".to_string(),
                    Dispatch::ToEditor(Copy {
                        use_system_clipboard,
                    }),
                ),
                Keymap::new(
                    "v",
                    "Paste (after selection)".to_string(),
                    Dispatch::ToEditor(Paste {
                        direction: Direction::End,
                        use_system_clipboard,
                    }),
                ),
                Keymap::new(
                    "V",
                    "Paste (before selection)".to_string(),
                    Dispatch::ToEditor(Paste {
                        direction: Direction::Start,
                        use_system_clipboard,
                    }),
                ),
                Keymap::new(
                    "r",
                    "Replace".to_string(),
                    Dispatch::ToEditor(ReplaceWithCopiedText {
                        use_system_clipboard,
                        cut: false,
                    }),
                ),
                Keymap::new(
                    "R",
                    "Replace Cut".to_string(),
                    Dispatch::ToEditor(ReplaceWithCopiedText {
                        use_system_clipboard,
                        cut: true,
                    }),
                ),
                Keymap::new(
                    "Y",
                    "Yeet Cut".to_string(),
                    Dispatch::ToEditor(ChangeCut {
                        use_system_clipboard,
                    }),
                ),
            ]),
        }
    }

    fn keymap_universal(&self) -> KeymapLegendSection {
        KeymapLegendSection {
            title: "Universal keymaps (works in every mode)".to_string(),
            keymaps: Keymaps::new(&[
                Keymap::new(
                    "ctrl+l",
                    "Switch view alignment".to_string(),
                    Dispatch::ToEditor(SwitchViewAlignment),
                ),
                Keymap::new("ctrl+o", "Other window".to_string(), Dispatch::OtherWindow),
                Keymap::new(
                    "ctrl+q",
                    "Close current window".to_string(),
                    Dispatch::CloseCurrentWindow,
                ),
                Keymap::new(
                    "ctrl+v",
                    "Paste".to_string(),
                    Dispatch::ToEditor(Paste {
                        direction: Direction::End,
                        use_system_clipboard: false,
                    }),
                ),
                Keymap::new("ctrl+y", "Redo".to_string(), Dispatch::ToEditor(Redo)),
                Keymap::new("ctrl+z", "Undo".to_string(), Dispatch::ToEditor(Undo)),
            ]),
        }
    }

    pub(crate) fn insert_mode_keymap_legend_config(&self) -> KeymapLegendConfig {
        KeymapLegendConfig {
            title: "Insert mode keymaps".to_string(),
            body: KeymapLegendBody::MultipleSections {
                sections: [
                    KeymapLegendSection {
                        title: "GNU Readline movements".to_string(),
                        keymaps: Keymaps::new(&[
                            Keymap::new(
                                "ctrl+b",
                                "Move back a character".to_string(),
                                Dispatch::ToEditor(MoveCharacterBack),
                            ),
                            Keymap::new(
                                "ctrl+f",
                                "Move forward a character".to_string(),
                                Dispatch::ToEditor(MoveCharacterForward),
                            ),
                            Keymap::new(
                                "ctrl+a",
                                "Move to line start".to_string(),
                                Dispatch::ToEditor(MoveToLineStart),
                            ),
                            Keymap::new(
                                "ctrl+e",
                                "Move to line end".to_string(),
                                Dispatch::ToEditor(MoveToLineEnd),
                            ),
                            Keymap::new(
                                "ctrl+k",
                                "Kill line forward".to_string(),
                                Dispatch::ToEditor(KillLine(Direction::End)),
                            ),
                            Keymap::new(
                                "ctrl+u",
                                "Kill line backward".to_string(),
                                Dispatch::ToEditor(KillLine(Direction::Start)),
                            ),
                            Keymap::new(
                                "ctrl+w",
                                "Delete word (long) backward".to_string(),
                                Dispatch::ToEditor(DeleteWordBackward { short: false }),
                            ),
                            Keymap::new(
                                "alt+backspace",
                                "Delete word (short) backward".to_string(),
                                Dispatch::ToEditor(DeleteWordBackward { short: true }),
                            ),
                        ]),
                    },
                    KeymapLegendSection {
                        title: "Common".to_string(),
                        keymaps: Keymaps::new(&[
                            Keymap::new(
                                "left",
                                "Move back a character".to_string(),
                                Dispatch::ToEditor(MoveCharacterBack),
                            ),
                            Keymap::new(
                                "right",
                                "Move forward a character".to_string(),
                                Dispatch::ToEditor(MoveCharacterForward),
                            ),
                            Keymap::new(
                                "esc",
                                "Enter normal mode".to_string(),
                                Dispatch::ToEditor(EnterNormalMode),
                            ),
                            Keymap::new(
                                "backspace",
                                "Delete character backward".to_string(),
                                Dispatch::ToEditor(Backspace),
                            ),
                            Keymap::new(
                                "enter",
                                "Enter new line".to_string(),
                                Dispatch::ToEditor(Insert("\n".to_string())),
                            ),
                            Keymap::new(
                                "tab",
                                "Enter tab".to_string(),
                                Dispatch::ToEditor(Insert("\t".to_string())),
                            ),
                            Keymap::new(
                                "home",
                                "Move to line start".to_string(),
                                Dispatch::ToEditor(MoveToLineStart),
                            ),
                            Keymap::new(
                                "end",
                                "Move to line end".to_string(),
                                Dispatch::ToEditor(MoveToLineEnd),
                            ),
                        ]),
                    },
                ]
                .into_iter()
                .chain(Some(self.keymap_universal()))
                .collect_vec(),
            },
        }
    }

    pub(crate) fn handle_insert_mode(&mut self, event: KeyEvent) -> anyhow::Result<Dispatches> {
        if let Some(dispatches) = self
            .insert_mode_keymap_legend_config()
            .keymaps()
            .iter()
            .find(|keymap| &event == keymap.event())
            .map(|keymap| Dispatches::one(keymap.dispatch()))
        {
            Ok(dispatches)
        } else if let KeyCode::Char(c) = event.code {
            return self.insert(&c.to_string());
        } else {
            Ok(Default::default())
        }
    }

    pub(crate) fn handle_universal_key(
        &mut self,
        event: KeyEvent,
    ) -> anyhow::Result<HandleEventResult> {
        if let Some(keymap) = self.keymap_universal().keymaps.get(&event) {
            Ok(HandleEventResult::Handled(Dispatches::one(
                keymap.dispatch(),
            )))
        } else {
            Ok(HandleEventResult::Ignored(event))
        }
    }

    pub(crate) fn keymap_others(&self, context: &Context) -> KeymapLegendSection {
        KeymapLegendSection {
            title: "Others".to_string(),
            keymaps: Keymaps::new(&[
                Keymap::new(
                    "space",
                    "Search (List)".to_string(),
                    Dispatch::ShowKeymapLegend(self.space_keymap_legend_config(context)),
                ),
                Keymap::new(
                    "\\",
                    "Clipboard-related actions (use system clipboard)".to_string(),
                    Dispatch::ShowKeymapLegend(KeymapLegendConfig {
                        title: "Clipboard-related actions (use system clipboard)".to_string(),
                        body: KeymapLegendBody::SingleSection {
                            keymaps: self.keymap_clipboard_related_actions(true).keymaps,
                        },
                    }),
                ),
                Keymap::new(
                    "'",
                    "Find literal".to_string(),
                    Dispatch::ShowKeymapLegend(self.show_literal_keymap_legend_config()),
                ),
                Keymap::new("*", "Select all".to_string(), Dispatch::ToEditor(SelectAll)),
                Keymap::new(
                    ":",
                    "Open command prompt".to_string(),
                    Dispatch::OpenCommandPrompt,
                ),
                Keymap::new(
                    "/",
                    "Omit selection".to_string(),
                    Dispatch::ShowKeymapLegend(self.omit_mode_keymap_legend_config()),
                ),
                Keymap::new(
                    "?",
                    "Help (Normal mode)".to_string(),
                    Dispatch::ToEditor(DispatchEditor::ShowKeymapLegendHelp),
                ),
                Keymap::new(
                    "esc",
                    "Remain only this window".to_string(),
                    Dispatch::RemainOnlyCurrentComponent,
                ),
            ]),
        }
    }

    pub(crate) fn keymap_movement_actions(&self) -> KeymapLegendSection {
        KeymapLegendSection {
            keymaps: Keymaps::new(&[
                Keymap::new(
                    ";",
                    "Replace".to_string(),
                    Dispatch::ToEditor(EnterReplaceMode),
                ),
                Keymap::new(
                    "q",
                    "Multi-cursor".to_string(),
                    Dispatch::ToEditor(EnterMultiCursorMode),
                ),
                Keymap::new(
                    "x",
                    "Exchange".to_string(),
                    Dispatch::ToEditor(EnterExchangeMode),
                ),
            ]),
            title: "Movement-action submodes".to_string(),
        }
    }

    pub(crate) fn help_keymap_legend_config(&self) -> KeymapLegendConfig {
        KeymapLegendConfig {
            title: "Help".to_string(),
            body: KeymapLegendBody::SingleSection {
                keymaps: Keymaps::new(&[
                    Keymap::new(
                        "i",
                        "Insert mode".to_string(),
                        Dispatch::ToEditor(ShowKeymapLegendInsertMode),
                    ),
                    Keymap::new(
                        "n",
                        "Normal mode".to_string(),
                        Dispatch::ToEditor(ShowKeymapLegendNormalMode),
                    ),
                ]),
            },
        }
    }

    pub(crate) fn normal_mode_keymap_legend_config(&self, context: &Context) -> KeymapLegendConfig {
        KeymapLegendConfig {
            title: "Normal mode".to_string(),
            body: KeymapLegendBody::MultipleSections {
                sections: [
                    self.keymap_core_movements(),
                    self.keymap_movement_actions(),
                    self.keymap_other_movements(),
                    self.keymap_selection_modes(context),
                    self.keymap_actions(),
                    self.keymap_clipboard_related_actions(false),
                    self.keymap_others(context),
                    self.keymap_universal(),
                ]
                .to_vec(),
            },
        }
    }
    fn normal_mode_keymaps(&self, context: &Context) -> Keymaps {
        Keymaps::new(
            &self
                .normal_mode_keymap_legend_config(context)
                .keymaps()
                .into_iter()
                .cloned()
                .collect_vec(),
        )
    }
    pub(crate) fn handle_normal_mode(
        &mut self,
        context: &Context,
        event: KeyEvent,
    ) -> anyhow::Result<Dispatches> {
        if let Some(keymap) = self.normal_mode_keymaps(context).get(&event) {
            return Ok([keymap.dispatch()].to_vec().into());
        }
        log::info!("unhandled event: {:?}", event);
        Ok(vec![].into())
    }

    pub(crate) fn transform_keymap_legend_config(&self) -> KeymapLegendConfig {
        KeymapLegendConfig {
            title: "Transform".to_string(),

            body: KeymapLegendBody::MultipleSections {
                sections: [
                    KeymapLegendSection {
                        title: "Letter case".to_string(),
                        keymaps: Keymaps::new(
                            &[
                                ("a", "aLtErNaTiNg CaSe", Case::Toggle),
                                ("c", "camelCase", Case::Camel),
                                ("l", "lowercase", Case::Lower),
                                ("k", "kebab-case", Case::Kebab),
                                ("K", "Upper-Kebab", Case::UpperKebab),
                                ("p", "PascalCase", Case::Pascal),
                                ("s", "snake_case", Case::Snake),
                                ("S", "UPPER_SNAKE_CASE", Case::UpperSnake),
                                ("t", "Title Case", Case::Title),
                                ("u", "UPPERCASE", Case::Upper),
                            ]
                            .into_iter()
                            .map(|(key, description, case)| {
                                Keymap::new(
                                    key,
                                    description.to_string(),
                                    Dispatch::ToEditor(Transform(Transformation::Case(case))),
                                )
                            })
                            .collect_vec(),
                        ),
                    },
                    KeymapLegendSection {
                        title: "Other".to_string(),
                        keymaps: Keymaps::new(&[
                            Keymap::new(
                                "j",
                                "Join".to_string(),
                                Dispatch::ToEditor(Transform(Transformation::Join)),
                            ),
                            Keymap::new(
                                "w",
                                "Wrap".to_string(),
                                Dispatch::ToEditor(Transform(Transformation::Wrap)),
                            ),
                        ]),
                    },
                ]
                .to_vec(),
            },
        }
    }

    fn space_keymap_legend_config(&self, context: &Context) -> KeymapLegendConfig {
        KeymapLegendConfig {
            title: "Space".to_string(),

            body: KeymapLegendBody::MultipleSections {
                sections: context
                    .contextual_keymaps()
                    .into_iter()
                    .chain([KeymapLegendSection {
                        title: "Pick".to_string(),
                        keymaps: Keymaps::new(
                            &[
                                ("b", "Buffers", FilePickerKind::Opened),
                                (
                                    "f",
                                    "Files (Non git ignored)",
                                    FilePickerKind::NonGitIgnored,
                                ),
                            ]
                            .into_iter()
                            .map(|(key, description, kind)| {
                                Keymap::new(
                                    key,
                                    description.to_string(),
                                    Dispatch::OpenFilePicker(kind),
                                )
                            })
                            .chain(
                                [
                                    ("g", DiffMode::UnstagedAgainstCurrentBranch),
                                    ("G", DiffMode::UnstagedAgainstMainBranch),
                                ]
                                .into_iter()
                                .map(|(key, diff_mode)| {
                                    Keymap::new(
                                        key,
                                        format!("Git status ({})", diff_mode.display()),
                                        Dispatch::OpenFilePicker(FilePickerKind::GitStatus(
                                            diff_mode,
                                        )),
                                    )
                                }),
                            )
                            .chain(Some(Keymap::new(
                                "s",
                                "Symbols".to_string(),
                                Dispatch::RequestDocumentSymbols,
                            )))
                            .chain(Some(Keymap::new(
                                "t",
                                "Theme".to_string(),
                                Dispatch::OpenThemePrompt,
                            )))
                            .collect_vec(),
                        ),
                    }])
                    .chain(Some(KeymapLegendSection {
                        title: "Multi-cursor".to_string(),
                        keymaps: Keymaps::new(&[
                            Keymap::new(
                                "a",
                                "Add cursor to all selections".to_string(),
                                Dispatch::ToEditor(DispatchEditor::CursorAddToAllSelections),
                            ),
                            Keymap::new(
                                "o",
                                "Keep only primary cursor".to_string(),
                                Dispatch::ToEditor(DispatchEditor::CursorKeepPrimaryOnly),
                            ),
                        ]),
                    }))
                    .chain(Some(KeymapLegendSection {
                        title: "Misc".to_string(),
                        keymaps: Keymaps::new(
                            &Some(Keymap::new(
                                "e",
                                "Reveal current file in Explorer".to_string(),
                                Dispatch::RevealInExplorer(self.path().unwrap_or_else(|| {
                                    context.current_working_directory().clone()
                                })),
                            ))
                            .into_iter()
                            .chain(Some(Keymap::new(
                                "z",
                                "Undo Tree".to_string(),
                                Dispatch::ToEditor(DispatchEditor::EnterUndoTreeMode),
                            )))
                            .chain(
                                self.buffer()
                                    .get_current_node(self.selection_set.primary_selection(), false)
                                    .ok()
                                    .flatten()
                                    .map(|node| {
                                        Keymap::new(
                                            "x",
                                            "Tree-sitter node S-expression".to_string(),
                                            Dispatch::ShowEditorInfo(Info::new(
                                                "Tree-sitter node S-expression".to_string(),
                                                node.to_sexp(),
                                            )),
                                        )
                                    }),
                            )
                            .collect_vec(),
                        ),
                    }))
                    .collect(),
            },
        }
    }

    pub(crate) fn find_keymap_legend_config(
        &self,
        context: &Context,
        scope: Scope,
    ) -> KeymapLegendConfig {
        let search_keymaps = {
            let config = context.get_local_search_config(scope);
            KeymapLegendSection {
                title: "Text".to_string(),
                keymaps: Keymaps::new(
                    &[
                        Keymap::new(
                            ",",
                            "Configure Search".to_string(),
                            Dispatch::ShowSearchConfig { scope },
                        ),
                        Keymap::new(
                            "s",
                            "Search".to_string(),
                            Dispatch::OpenSearchPrompt { scope },
                        ),
                    ]
                    .into_iter()
                    .chain(
                        self.buffer()
                            .slice(&self.selection_set.primary_selection().extended_range())
                            .map(|search| {
                                Keymap::new(
                                    "c",
                                    "Current selection".to_string(),
                                    Dispatch::UpdateLocalSearchConfig {
                                        scope,
                                        update: crate::app::LocalSearchConfigUpdate::Search(
                                            search.to_string(),
                                        ),
                                        show_config_after_enter: false,
                                    },
                                )
                            }),
                    )
                    .chain(config.last_search().map(|search| {
                        Keymap::new(
                            "p",
                            "Search (using previous search)".to_string(),
                            Dispatch::UpdateLocalSearchConfig {
                                scope,
                                update: crate::app::LocalSearchConfigUpdate::Search(
                                    search.search.to_string(),
                                ),
                                show_config_after_enter: false,
                            },
                        )
                    }))
                    .collect_vec(),
                ),
            }
        };
        let misc_keymaps = KeymapLegendSection {
            title: "Misc".to_string(),
            keymaps: Keymaps::new(
                &[
                    Keymap::new(
                        "m",
                        "Mark".to_string(),
                        match scope {
                            Scope::Global => Dispatch::SetQuickfixList(QuickfixListType::Bookmark),
                            Scope::Local => Dispatch::ToEditor(SetSelectionMode(Bookmark)),
                        },
                    ),
                    Keymap::new(
                        "q",
                        "Quickfix".to_string(),
                        match scope {
                            Scope::Global => Dispatch::SetGlobalMode(Some(
                                crate::context::GlobalMode::QuickfixListItem,
                            )),
                            Scope::Local => Dispatch::ToEditor(SetSelectionMode(LocalQuickfix {
                                title: "LOCAL QUICKFIX".to_string(),
                            })),
                        },
                    ),
                ]
                .into_iter()
                .chain(
                    [
                        ("g", DiffMode::UnstagedAgainstCurrentBranch),
                        ("G", DiffMode::UnstagedAgainstMainBranch),
                    ]
                    .map(|(key, diff_mode)| {
                        Keymap::new(
                            key,
                            format!("Git hunk ({})", diff_mode.display()),
                            match scope {
                                Scope::Global => Dispatch::GetRepoGitHunks(diff_mode),
                                Scope::Local => {
                                    Dispatch::ToEditor(SetSelectionMode(GitHunk(diff_mode)))
                                }
                            },
                        )
                    }),
                )
                .collect_vec(),
            ),
        };
        let diagnostics_keymaps = {
            let keymaps = [
                ("a", "All", DiagnosticSeverityRange::All),
                ("e", "Error", DiagnosticSeverityRange::Error),
                ("h", "Hint", DiagnosticSeverityRange::Hint),
                ("I", "Information", DiagnosticSeverityRange::Information),
                ("w", "Warning", DiagnosticSeverityRange::Warning),
            ]
            .into_iter()
            .map(|(char, description, severity)| {
                Keymap::new(
                    char,
                    description.to_string(),
                    match scope {
                        Scope::Local => Dispatch::ToEditor(SetSelectionMode(Diagnostic(severity))),
                        Scope::Global => {
                            Dispatch::SetQuickfixList(QuickfixListType::Diagnostic(severity))
                        }
                    },
                )
            })
            .collect_vec();
            KeymapLegendSection {
                title: "Diagnostics".to_string(),
                keymaps: Keymaps::new(&keymaps),
            }
        };
        let lsp_keymaps = {
            let keymaps = Keymaps::new(&[
                Keymap::new(
                    "d",
                    "Definitions".to_string(),
                    Dispatch::RequestDefinitions(scope),
                ),
                Keymap::new(
                    "D",
                    "Declarations".to_string(),
                    Dispatch::RequestDeclarations(scope),
                ),
                Keymap::new(
                    "i",
                    "Implementations".to_string(),
                    Dispatch::RequestImplementations(scope),
                ),
                Keymap::new(
                    "r",
                    "References".to_string(),
                    Dispatch::RequestReferences {
                        include_declaration: false,
                        scope,
                    },
                ),
                Keymap::new(
                    "R",
                    "References (include declaration)".to_string(),
                    Dispatch::RequestReferences {
                        include_declaration: true,
                        scope,
                    },
                ),
                Keymap::new(
                    "t",
                    "Type Definitions".to_string(),
                    Dispatch::RequestTypeDefinitions(scope),
                ),
            ]);

            KeymapLegendSection {
                title: "LSP".to_string(),
                keymaps,
            }
        };
        KeymapLegendConfig {
            title: format!(
                "Find ({})",
                match scope {
                    Scope::Local => "Local",
                    Scope::Global => "Global",
                }
            ),

            body: KeymapLegendBody::MultipleSections {
                sections: Some(search_keymaps)
                    .into_iter()
                    .chain(Some(misc_keymaps))
                    .chain(Some(diagnostics_keymaps))
                    .chain(Some(lsp_keymaps))
                    .collect_vec(),
            },
        }
    }

    pub(crate) fn between_mode_keymap_legend_config(&self) -> KeymapLegendConfig {
        KeymapLegendConfig {
            title: "Between".to_string(),

            body: KeymapLegendBody::MultipleSections {
                sections: [
                    KeymapLegendSection {
                        title: "Select".to_string(),
                        keymaps: Keymaps::new(&[
                            Keymap::new(
                                "a",
                                "Around".to_string(),
                                Dispatch::ShowKeymapLegend(
                                    self.select_surround_keymap_legend_config(SurroundKind::Around),
                                ),
                            ),
                            Keymap::new(
                                "i",
                                "Inside".to_string(),
                                Dispatch::ShowKeymapLegend(
                                    self.select_surround_keymap_legend_config(SurroundKind::Inside),
                                ),
                            ),
                        ]),
                    },
                    KeymapLegendSection {
                        title: "Action".to_string(),
                        keymaps: Keymaps::new(&[
                            Keymap::new(
                                "c",
                                "Change Surround".to_string(),
                                Dispatch::ShowKeymapLegend(
                                    self.change_surround_from_keymap_legend_config(),
                                ),
                            ),
                            Keymap::new(
                                "d",
                                "Delete Surround".to_string(),
                                Dispatch::ShowKeymapLegend(
                                    self.delete_surround_keymap_legend_config(),
                                ),
                            ),
                        ]),
                    },
                    KeymapLegendSection {
                        title: "Surround".to_string(),
                        keymaps: generate_enclosures_keymaps(|enclosure| {
                            let (open, close) = enclosure.open_close_symbols_str();
                            Dispatch::ToEditor(Surround(open.to_string(), close.to_string()))
                        }),
                    },
                ]
                .to_vec(),
            },
        }
    }

    pub(crate) fn select_surround_keymap_legend_config(
        &self,
        kind: SurroundKind,
    ) -> KeymapLegendConfig {
        KeymapLegendConfig {
            title: format!("Select Surround ({:?})", kind),

            body: KeymapLegendBody::SingleSection {
                keymaps: generate_enclosures_keymaps(|enclosure| {
                    Dispatch::ToEditor(SelectSurround {
                        enclosure,
                        kind: kind.clone(),
                    })
                }),
            },
        }
    }

    pub(crate) fn delete_surround_keymap_legend_config(&self) -> KeymapLegendConfig {
        KeymapLegendConfig {
            title: "Delete Surround".to_string(),

            body: KeymapLegendBody::SingleSection {
                keymaps: generate_enclosures_keymaps(|enclosure| {
                    Dispatch::ToEditor(DeleteSurround(enclosure))
                }),
            },
        }
    }

    pub(crate) fn change_surround_from_keymap_legend_config(
        &self,
    ) -> super::keymap_legend::KeymapLegendConfig {
        KeymapLegendConfig {
            title: "Change Surround from:".to_string(),

            body: KeymapLegendBody::SingleSection {
                keymaps: generate_enclosures_keymaps(|enclosure| {
                    Dispatch::ShowKeymapLegend(
                        self.change_surround_to_keymap_legend_config(enclosure),
                    )
                }),
            },
        }
    }

    pub(crate) fn change_surround_to_keymap_legend_config(
        &self,
        from: EnclosureKind,
    ) -> super::keymap_legend::KeymapLegendConfig {
        KeymapLegendConfig {
            title: format!("Change Surround from {} to:", from.to_str()),

            body: KeymapLegendBody::SingleSection {
                keymaps: generate_enclosures_keymaps(|enclosure| {
                    Dispatch::ToEditor(ChangeSurround {
                        from,
                        to: enclosure,
                    })
                }),
            },
        }
    }

    pub(crate) fn omit_mode_keymap_legend_config(&self) -> KeymapLegendConfig {
        let filter_mechanism_keymaps = |kind: FilterKind, target: FilterTarget| -> Dispatch {
            Dispatch::ShowKeymapLegend(KeymapLegendConfig {
                title: format!("Omit: {:?} {:?} matching", kind, target),

                body: KeymapLegendBody::SingleSection {
                    keymaps: Keymaps::new(
                        [
                            Keymap::new(
                                "l",
                                "Literal".to_string(),
                                Dispatch::OpenOmitPrompt {
                                    kind,
                                    target,
                                    make_mechanism: MakeFilterMechanism::Literal,
                                },
                            ),
                            Keymap::new(
                                "r",
                                "Regex".to_string(),
                                Dispatch::OpenOmitPrompt {
                                    kind,
                                    target,
                                    make_mechanism: MakeFilterMechanism::Regex,
                                },
                            ),
                        ]
                        .as_ref(),
                    ),
                },
            })
        };
        let filter_target_keymaps = |kind: FilterKind| -> Dispatch {
            Dispatch::ShowKeymapLegend(KeymapLegendConfig {
                title: format!("Omit: {:?}", kind),

                body: KeymapLegendBody::SingleSection {
                    keymaps: Keymaps::new(
                        [
                            Keymap::new(
                                "c",
                                "Content".to_string(),
                                filter_mechanism_keymaps(kind, FilterTarget::Content),
                            ),
                            Keymap::new(
                                "i",
                                "Info".to_string(),
                                filter_mechanism_keymaps(kind, FilterTarget::Info),
                            ),
                        ]
                        .as_ref(),
                    ),
                },
            })
        };
        KeymapLegendConfig {
            title: "Omit".to_string(),

            body: KeymapLegendBody::SingleSection {
                keymaps: Keymaps::new(
                    [
                        Keymap::new("c", "Clear".to_string(), Dispatch::ToEditor(FilterClear)),
                        Keymap::new(
                            "k",
                            "keep".to_string(),
                            filter_target_keymaps(FilterKind::Keep),
                        ),
                        Keymap::new(
                            "r",
                            "remove".to_string(),
                            filter_target_keymaps(FilterKind::Remove),
                        ),
                    ]
                    .as_ref(),
                ),
            },
        }
    }

    pub(crate) fn show_literal_keymap_legend_config(&self) -> KeymapLegendConfig {
        KeymapLegendConfig {
            title: "Find literal".to_string(),

            body: KeymapLegendBody::SingleSection {
                keymaps: Keymaps::new(
                    &[
                        ("b", "Binary", r"\b[01]+\b"),
                        ("f", "Float", r"[-+]?\d*\.\d+|\d+"),
                        ("i", "Integer", r"-?\d+"),
                        ("n", "Natural", r"\d+"),
                    ]
                    .into_iter()
                    .map(|(key, description, regex)| {
                        let search = Search {
                            search: regex.to_string(),
                            mode: LocalSearchConfigMode::Regex(RegexConfig {
                                escaped: false,
                                match_whole_word: false,
                                case_sensitive: false,
                            }),
                        };
                        let dispatch = Dispatch::ToEditor(SetSelectionMode(Find { search }));
                        Keymap::new(key, description.to_string(), dispatch)
                    })
                    .chain([
                        Keymap::new(
                            "o",
                            "One character".to_string(),
                            Dispatch::ToEditor(FindOneChar),
                        ),
                        Keymap::new(
                            "space",
                            "Empty line".to_string(),
                            Dispatch::ToEditor(SetSelectionMode(EmptyLine)),
                        ),
                    ])
                    .collect_vec(),
                ),
            },
        }
    }
}

fn generate_enclosures_keymaps(get_dispatch: impl Fn(EnclosureKind) -> Dispatch) -> Keymaps {
    Keymaps::new(
        &[
            EnclosureKind::AngularBrackets,
            EnclosureKind::Parentheses,
            EnclosureKind::SquareBrackets,
            EnclosureKind::CurlyBraces,
            EnclosureKind::DoubleQuotes,
            EnclosureKind::SingleQuotes,
            EnclosureKind::Backticks,
        ]
        .into_iter()
        .map(|enclosure| {
            let (key, _) = enclosure.open_close_symbols_str();
            let description = enclosure.to_str();
            Keymap::new(key, description.to_string(), get_dispatch(enclosure))
        })
        .collect_vec(),
    )
}
