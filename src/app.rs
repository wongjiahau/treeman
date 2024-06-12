use crate::{
    buffer::Buffer,
    clipboard::CopiedTexts,
    components::{
        component::{Component, ComponentId, GetGridResult},
        dropdown::{DropdownItem, DropdownRender},
        editor::{DispatchEditor, Editor, Movement},
        keymap_legend::{
            Keymap, KeymapLegendBody, KeymapLegendConfig, KeymapLegendSection, Keymaps,
        },
        prompt::{Prompt, PromptConfig, PromptHistoryKey},
        suggestive_editor::{
            DispatchSuggestiveEditor, Info, SuggestiveEditor, SuggestiveEditorFilter,
        },
    },
    context::{Context, GlobalMode, LocalSearchConfigMode, QuickfixListSource, Search},
    frontend::Frontend,
    git,
    grid::{Grid, LineUpdate},
    history::History,
    layout::Layout,
    list::{self, grep::RegexConfig, WalkBuilderConfig},
    lsp::{
        completion::CompletionItem,
        goto_definition_response::GotoDefinitionResponse,
        manager::LspManager,
        process::{FromEditor, LspNotification, ResponseContext},
        symbols::Symbols,
        workspace_edit::WorkspaceEdit,
    },
    position::Position,
    quickfix_list::{Location, QuickfixList, QuickfixListItem, QuickfixListType},
    screen::{Screen, Window},
    selection::{Filter, FilterKind, FilterMechanism, FilterTarget, SelectionMode},
    syntax_highlight::{HighlighedSpans, SyntaxHighlightRequest},
    ui_tree::{ComponentKind, KindedComponent},
};
use event::event::Event;
use itertools::Itertools;
use name_variant::NamedVariant;
use shared::{canonicalized_path::CanonicalizedPath, language::Language};
use std::{
    any::TypeId,
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        mpsc::{Receiver, Sender},
        Arc, Mutex,
    },
};
use DispatchEditor::*;

pub(crate) struct App<T: Frontend> {
    context: Context,

    sender: Sender<AppMessage>,

    /// Used for receiving message from various sources:
    /// - Events from crossterm
    /// - Notifications from language server
    receiver: Receiver<AppMessage>,

    lsp_manager: LspManager,
    enable_lsp: bool,

    working_directory: CanonicalizedPath,
    global_title: Option<String>,

    layout: Layout,

    frontend: Arc<Mutex<T>>,

    syntax_highlight_request_sender: Option<Sender<SyntaxHighlightRequest>>,

    /// Used for navigating between opened files
    file_path_history: History<CanonicalizedPath>,
}

const GLOBAL_TITLE_BAR_HEIGHT: u16 = 1;
impl<T: Frontend> App<T> {
    #[cfg(test)]
    pub(crate) fn new(
        frontend: Arc<Mutex<T>>,
        working_directory: CanonicalizedPath,
    ) -> anyhow::Result<App<T>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self::from_channel(frontend, working_directory, sender, receiver)
    }

    #[cfg(test)]
    pub(crate) fn disable_lsp(&mut self) {
        self.enable_lsp = false
    }

    pub(crate) fn from_channel(
        frontend: Arc<Mutex<T>>,
        working_directory: CanonicalizedPath,
        sender: Sender<AppMessage>,
        receiver: Receiver<AppMessage>,
    ) -> anyhow::Result<App<T>> {
        let dimension = frontend.lock().unwrap().get_terminal_dimension()?;
        let app = App {
            context: Context::new(working_directory.clone()),
            receiver,
            lsp_manager: LspManager::new(sender.clone(), working_directory.clone()),
            enable_lsp: true,
            sender,
            layout: Layout::new(
                dimension.decrement_height(GLOBAL_TITLE_BAR_HEIGHT),
                &working_directory,
            )?,
            working_directory,
            frontend,
            syntax_highlight_request_sender: None,
            global_title: None,

            file_path_history: History::new(),
        };
        Ok(app)
    }
    fn update_highlighted_spans(
        &self,
        component_id: ComponentId,
        highlighted_spans: HighlighedSpans,
    ) -> Result<(), anyhow::Error> {
        self.layout
            .update_highlighted_spans(component_id, highlighted_spans)
    }

    pub(crate) fn run(
        mut self,
        entry_path: Option<CanonicalizedPath>,
    ) -> Result<(), anyhow::Error> {
        {
            let mut frontend = self.frontend.lock().unwrap();
            frontend.enter_alternate_screen()?;
            frontend.enable_raw_mode()?;
            frontend.enable_mouse_capture()?;
        }

        if let Some(entry_path) = entry_path {
            self.open_file(&entry_path, OpenFileOption::Focus)?;
        }

        self.render()?;

        while let Ok(message) = self.receiver.recv() {
            match message {
                AppMessage::Event(event) => self.handle_event(event),
                AppMessage::LspNotification(notification) => {
                    self.handle_lsp_notification(notification).map(|_| false)
                }
                AppMessage::QuitAll => {
                    self.quit()?;
                    Ok(true)
                }
                AppMessage::SyntaxHighlightResponse {
                    component_id,
                    highlighted_spans,
                } => self
                    .update_highlighted_spans(component_id, highlighted_spans)
                    .map(|_| false),
            }
            .unwrap_or_else(|e| {
                self.show_global_info(Info::new("ERROR".to_string(), e.to_string()));
                false
            });

            if self.should_quit() {
                break;
            }

            self.render()?;
        }

        self.quit()
    }

    pub(crate) fn quit(&mut self) -> anyhow::Result<()> {
        let mut frontend = self.frontend.lock().unwrap();
        frontend.leave_alternate_screen()?;
        frontend.disable_raw_mode()?;
        // self.lsp_manager.shutdown();

        std::process::exit(0);
    }

    pub(crate) fn components(&self) -> Vec<KindedComponent> {
        self.layout.components()
    }

    /// Returns true if the app should quit.
    fn handle_event(&mut self, event: Event) -> anyhow::Result<bool> {
        // Pass event to focused window
        let component = self.current_component();
        self.context
            .set_contextual_keymaps(component.borrow().contextual_keymaps());
        match event {
            Event::Resize(columns, rows) => {
                self.resize(Dimension {
                    height: rows,
                    width: columns,
                });
            }
            event => {
                let dispatches = component.borrow_mut().handle_event(&self.context, event);
                self.handle_dispatches_result(dispatches)
                    .unwrap_or_else(|e| {
                        self.show_global_info(Info::new("ERROR".to_string(), e.to_string()))
                    });
            }
        }

        Ok(false)
    }

    /// Return true if there's no more windows
    fn should_quit(&mut self) -> bool {
        self.layout.components().is_empty()
    }

    fn render(&mut self) -> Result<(), anyhow::Error> {
        let screen = self.get_screen()?;
        self.render_screen(screen)?;
        Ok(())
    }

    pub(crate) fn get_screen(&mut self) -> Result<Screen, anyhow::Error> {
        // Recalculate layout before each render
        self.layout.recalculate_layout();

        // Generate layout
        let dimension = self.layout.terminal_dimension();
        // Render every window
        let (windows, cursors): (Vec<_>, Vec<_>) = self
            .components()
            .into_iter()
            .map(|component| {
                let rectangle = component.component().borrow().rectangle().clone();
                let focused_component_id = self.layout.focused_component_id();
                let focused = component.component().borrow().id() == focused_component_id;
                let GetGridResult { grid, cursor } = component
                    .component()
                    .borrow()
                    .get_grid(&self.context, focused);
                let cursor_position = if focused {
                    if let Some(cursor) = cursor {
                        let cursor_position = cursor.position();
                        // If cursor position is not in view
                        if cursor_position.line >= (rectangle.dimension().height) as usize {
                            None
                        } else {
                            let calibrated_position = Position::new(
                                cursor_position.line + rectangle.origin.line,
                                cursor_position.column + rectangle.origin.column,
                            );
                            Some(cursor.set_position(calibrated_position))
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                let window = Window::new(grid, rectangle.clone());

                (window, cursor_position)
            })
            .unzip();
        let borders = self.layout.borders();
        let cursor = cursors.into_iter().find_map(|cursor| cursor);
        let screen = Screen::new(windows, borders, cursor, self.context.theme().ui.border);

        // Set the global title
        let global_title_window = {
            let mode = self
                .context
                .mode()
                .map(|mode| mode.display())
                .unwrap_or_else(|| self.current_component().borrow().editor().display_mode());

            let mode = format!("[{}]", mode);

            let title = if let Some(title) = self.global_title.as_ref() {
                title.clone()
            } else {
                let branch = if let Some(current_branch) = self.current_branch() {
                    format!(" ({}) ", current_branch,)
                } else {
                    " ".to_string()
                };
                format!(
                    "{}{}{}",
                    self.working_directory.display_absolute(),
                    branch,
                    mode
                )
            };

            let grid = Grid::new(Dimension {
                height: 1,
                width: dimension.width,
            })
            .render_content(
                &title,
                crate::grid::RenderContentLineNumber::NoLineNumber,
                Vec::new(),
                [LineUpdate {
                    line_index: 0,
                    style: self.context.theme().ui.global_title,
                }]
                .to_vec(),
                self.context.theme(),
            );
            Window::new(
                grid,
                crate::rectangle::Rectangle {
                    width: dimension.width,
                    height: 1,
                    origin: Position {
                        line: dimension.height as usize,
                        column: 0,
                    },
                },
            )
        };
        let screen = screen.add_window(global_title_window);

        Ok(screen)
    }

    fn current_branch(&self) -> Option<String> {
        // Open the repository
        let repo = git2::Repository::open(self.working_directory.display_absolute()).ok()?;

        // Get the current branch
        let head = repo.head().ok()?;
        let branch = head.shorthand()?;
        Some(branch.to_string())
    }

    fn render_screen(&mut self, screen: Screen) -> Result<(), anyhow::Error> {
        let mut frontend = self.frontend.lock().unwrap();
        frontend.hide_cursor()?;
        let cursor = screen.cursor();
        frontend.render_screen(screen)?;
        if let Some(cursor) = cursor {
            frontend.show_cursor(&cursor)?;
        }

        Ok(())
    }

    fn handle_dispatches_result(
        &mut self,
        dispatches: anyhow::Result<Dispatches>,
    ) -> anyhow::Result<()> {
        self.handle_dispatches(dispatches?)
    }

    pub(crate) fn handle_dispatches(
        &mut self,
        dispatches: Dispatches,
    ) -> Result<(), anyhow::Error> {
        for dispatch in dispatches.into_vec() {
            self.handle_dispatch(dispatch)?;
        }
        Ok(())
    }

    fn get_request_params(&self) -> Option<RequestParams> {
        if self.current_component().borrow().type_id() != TypeId::of::<SuggestiveEditor>() {
            None
        } else {
            self.current_component()
                .borrow()
                .editor()
                .get_request_params()
        }
    }

    pub(crate) fn handle_dispatch(&mut self, dispatch: Dispatch) -> Result<(), anyhow::Error> {
        log::info!("App::handle_dispatch = {}", dispatch.variant_name());
        match dispatch {
            Dispatch::CloseCurrentWindow => {
                self.close_current_window();
            }
            Dispatch::CloseCurrentWindowAndFocusParent => {
                self.close_current_window_and_focus_parent();
            }
            Dispatch::OpenSearchPrompt { scope } => self.open_search_prompt(scope)?,
            Dispatch::OpenFile(path) => {
                self.open_file(&path, OpenFileOption::Focus)?;
            }

            Dispatch::OpenFileFromPathBuf(path) => {
                self.open_file(&path.try_into()?, OpenFileOption::Focus)?;
            }

            Dispatch::OpenFilePicker(kind) => {
                self.open_file_picker(kind)?;
            }
            Dispatch::RequestCompletion => {
                if let Some(params) = self.get_request_params() {
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::TextDocumentCompletion(params),
                    )?;
                }
            }
            Dispatch::ResolveCompletionItem(completion_item) => {
                if let Some(params) = self.get_request_params() {
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::CompletionItemResolve {
                            completion_item,
                            params,
                        },
                    )?
                }
            }
            Dispatch::RequestReferences {
                include_declaration,
                scope,
            } => {
                if let Some(params) = self.get_request_params() {
                    let params =
                        params
                            .set_kind(Some(scope))
                            .set_description(if include_declaration {
                                "References (include declaration)"
                            } else {
                                "References (exclude declaration)"
                            });
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::TextDocumentReferences {
                            params,
                            include_declaration,
                        },
                    )?;
                }
            }
            Dispatch::RequestHover => {
                if let Some(params) = self.get_request_params() {
                    let params = params.set_description("Hover");
                    self.lsp_manager
                        .send_message(params.path.clone(), FromEditor::TextDocumentHover(params))?;
                }
            }
            Dispatch::RequestDefinitions(scope) => {
                if let Some(params) = self.get_request_params() {
                    let params = params.set_kind(Some(scope)).set_description("Definitions");
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::TextDocumentDefinition(params),
                    )?;
                }
            }
            Dispatch::RequestDeclarations(scope) => {
                if let Some(params) = self.get_request_params() {
                    let params = params.set_kind(Some(scope)).set_description("Declarations");
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::TextDocumentDeclaration(params),
                    )?;
                }
            }
            Dispatch::RequestImplementations(scope) => {
                if let Some(params) = self.get_request_params() {
                    let params = params
                        .set_kind(Some(scope))
                        .set_description("Implementations");
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::TextDocumentImplementation(params),
                    )?;
                }
            }
            Dispatch::RequestTypeDefinitions(scope) => {
                if let Some(params) = self.get_request_params() {
                    let params = params
                        .set_kind(Some(scope))
                        .set_description("Type Definitions");
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::TextDocumentTypeDefinition(params),
                    )?;
                }
            }
            Dispatch::RequestDocumentSymbols => {
                if let Some(params) = self.get_request_params() {
                    let params = params.set_description("Document Symbols");
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::TextDocumentDocumentSymbol(params),
                    )?;
                }
            }
            Dispatch::PrepareRename => {
                if let Some(params) = self.get_request_params() {
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::TextDocumentPrepareRename(params),
                    )?;
                }
            }
            Dispatch::RenameSymbol { new_name } => {
                if let Some(params) = self.get_request_params() {
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::TextDocumentRename { params, new_name },
                    )?;
                }
            }
            Dispatch::RequestCodeAction { diagnostics } => {
                if let Some(params) = self.get_request_params() {
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::TextDocumentCodeAction {
                            params,
                            diagnostics,
                        },
                    )?;
                }
            }
            Dispatch::RequestSignatureHelp => {
                if let Some(params) = self.get_request_params() {
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::TextDocumentSignatureHelp(params),
                    )?;
                }
            }
            Dispatch::DocumentDidChange {
                path,
                content,
                language,
                component_id,
            } => {
                if let Some(language) = language {
                    self.request_syntax_highlight(component_id, language, content.clone())?;
                    // let highlight_spans = self.context.highlight(language, &content)?;
                    // self.update_highlighted_spans(component_id, highlight_spans)?
                }
                if let Some(path) = path {
                    self.lsp_manager.send_message(
                        path.clone(),
                        FromEditor::TextDocumentDidChange {
                            content,
                            file_path: path,
                            version: 2,
                        },
                    )?;
                }
            }
            Dispatch::DocumentDidSave { path } => {
                self.lsp_manager.send_message(
                    path.clone(),
                    FromEditor::TextDocumentDidSave { file_path: path },
                )?;
            }
            Dispatch::ShowGlobalInfo(info) => self.show_global_info(info),
            Dispatch::SetQuickfixList(r#type) => {
                self.set_quickfix_list_type(Default::default(), r#type)?;
            }
            Dispatch::GotoQuickfixListItem(movement) => self.goto_quickfix_list_item(movement)?,
            Dispatch::ApplyWorkspaceEdit(workspace_edit) => {
                self.apply_workspace_edit(workspace_edit)?;
            }
            Dispatch::ShowKeymapLegend(keymap_legend_config) => {
                self.show_keymap_legend(keymap_legend_config)
            }

            #[cfg(test)]
            Dispatch::Custom(_) => unreachable!(),
            Dispatch::RemainOnlyCurrentComponent => self.layout.remain_only_current_component(),
            Dispatch::ToEditor(dispatch_editor) => self.handle_dispatch_editor(dispatch_editor)?,
            Dispatch::GotoLocation(location) => self.go_to_location(&location)?,
            Dispatch::OpenMoveToIndexPrompt => self.open_move_to_index_prompt()?,
            Dispatch::RunCommand(command) => self.run_command(command)?,
            Dispatch::QuitAll => self.quit_all()?,
            Dispatch::OpenCommandPrompt => self.open_command_prompt()?,
            Dispatch::SaveQuitAll => self.save_quit_all()?,
            Dispatch::RevealInExplorer(path) => self.reveal_path_in_explorer(&path)?,
            Dispatch::OpenYesNoPrompt(prompt) => self.open_yes_no_prompt(prompt)?,
            Dispatch::OpenMoveFilePrompt(path) => self.open_move_file_prompt(path)?,
            Dispatch::OpenAddPathPrompt(path) => self.open_add_path_prompt(path)?,
            Dispatch::DeletePath(path) => self.delete_path(&path)?,
            Dispatch::Null => {
                // do nothing
            }
            Dispatch::MoveFile { from, to } => self.move_file(from, to)?,
            Dispatch::AddPath(path) => self.add_path(path)?,
            Dispatch::RefreshFileExplorer => {
                self.layout.refresh_file_explorer(&self.working_directory)?
            }
            Dispatch::SetClipboardContent {
                copied_texts: contents,
                use_system_clipboard,
            } => self
                .context
                .set_clipboard_content(contents, use_system_clipboard)?,
            Dispatch::SetGlobalMode(mode) => self.set_global_mode(mode),

            #[cfg(test)]
            Dispatch::HandleKeyEvent(key_event) => {
                self.handle_event(Event::Key(key_event))?;
            }
            Dispatch::GetRepoGitHunks(diff_mode) => self.get_repo_git_hunks(diff_mode)?,
            Dispatch::SaveAll => self.save_all()?,
            #[cfg(test)]
            Dispatch::TerminalDimensionChanged(dimension) => self.resize(dimension),
            #[cfg(test)]
            Dispatch::SetGlobalTitle(title) => self.set_global_title(title),
            Dispatch::OpenOmitPrompt {
                kind,
                target,
                make_mechanism,
            } => self.open_omit_prompt(kind, target, make_mechanism)?,

            Dispatch::LspExecuteCommand { command } => {
                if let Some(params) = self.get_request_params() {
                    self.lsp_manager.send_message(
                        params.path.clone(),
                        FromEditor::WorkspaceExecuteCommand { params, command },
                    )?
                };
            }
            Dispatch::UpdateLocalSearchConfig {
                update,
                scope,
                show_config_after_enter,
            } => self.update_local_search_config(update, scope, show_config_after_enter)?,
            Dispatch::UpdateGlobalSearchConfig { update } => {
                self.update_global_search_config(update)?;
            }
            Dispatch::OpenSetGlobalSearchFilterGlobPrompt { filter_glob } => {
                self.open_set_global_search_filter_glob_prompt(filter_glob)?
            }
            Dispatch::ShowSearchConfig { scope } => self.show_search_config(scope),
            Dispatch::OpenUpdateReplacementPrompt { scope } => {
                self.open_update_replacement_prompt(scope)?
            }
            Dispatch::OpenUpdateSearchPrompt { scope } => self.open_update_search_prompt(scope)?,
            Dispatch::Replace { scope } => match scope {
                Scope::Local => self.handle_dispatch_editor(ReplacePattern {
                    config: self.context.local_search_config().clone(),
                })?,
                Scope::Global => self.global_replace()?,
            },
            #[cfg(test)]
            Dispatch::HandleLspNotification(notification) => {
                self.handle_lsp_notification(notification)?
            }
            Dispatch::SetTheme(theme) => {
                let context = std::mem::take(&mut self.context);
                self.context = context.set_theme(theme.clone());
            }
            #[cfg(test)]
            Dispatch::HandleKeyEvents(key_events) => self.handle_key_events(key_events)?,
            Dispatch::CloseDropdown => self.layout.close_dropdown(),
            Dispatch::CloseEditorInfo => self.layout.close_editor_info(),
            Dispatch::RenderDropdown { render } => {
                if let Some(dropdown) = self.layout.open_dropdown() {
                    self.render_dropdown(dropdown, render)?;
                }
            }
            #[cfg(test)]
            Dispatch::OpenPrompt {
                config,
                key,
                current_line,
            } => self.open_prompt(config, key, current_line)?,
            Dispatch::ShowEditorInfo(info) => self.show_editor_info(info)?,
            Dispatch::ReceiveCodeActions(code_actions) => {
                self.open_code_actions_prompt(code_actions)?;
            }
            Dispatch::OtherWindow => self.layout.cycle_window(),
            Dispatch::GoToPreviousFile => self.go_to_previous_file()?,
            Dispatch::GoToNextFile => self.go_to_next_file()?,
            Dispatch::PushPromptHistory { key, line } => self.push_history_prompt(key, line),
            Dispatch::OpenThemePrompt => self.open_theme_prompt()?,
        }
        Ok(())
    }

    pub(crate) fn current_component(&self) -> Rc<RefCell<dyn Component>> {
        self.layout.get_current_component()
    }

    fn close_current_window(&mut self) {
        self.layout.close_current_window()
    }

    fn local_search(&mut self) -> anyhow::Result<()> {
        let config = self.context.local_search_config();
        let search = config.search();
        if !search.is_empty() {
            self.handle_dispatch_editor_custom(
                SetSelectionMode(SelectionMode::Find {
                    search: Search {
                        mode: config.mode,
                        search,
                    },
                }),
                self.current_component(),
            )?;
        }

        Ok(())
    }

    fn resize(&mut self, dimension: Dimension) {
        self.layout
            .set_terminal_dimension(dimension.decrement_height(GLOBAL_TITLE_BAR_HEIGHT));
    }

    fn open_move_to_index_prompt(&mut self) -> anyhow::Result<()> {
        self.open_prompt(
            PromptConfig {
                title: "Move to index".to_string(),
                on_enter: DispatchPrompt::MoveSelectionByIndex,
                items: vec![],
                enter_selects_first_matching_item: false,
                leaves_current_line_empty: true,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::MoveToIndex,
            None,
        )
    }

    fn open_rename_prompt(&mut self, current_name: Option<String>) -> anyhow::Result<()> {
        self.open_prompt(
            PromptConfig {
                title: "Rename".to_string(),
                on_enter: DispatchPrompt::RenameSymbol,
                items: vec![],
                enter_selects_first_matching_item: false,
                leaves_current_line_empty: false,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::Rename,
            current_name,
        )
    }

    fn open_search_prompt(&mut self, scope: Scope) -> anyhow::Result<()> {
        let config = self.context.get_local_search_config(scope);
        let mode = config.mode;
        self.open_prompt(
            PromptConfig {
                title: format!("{:?} search ({})", scope, mode.display()),
                items: self.words(),
                on_enter: DispatchPrompt::UpdateLocalSearchConfigSearch {
                    scope,
                    show_config_after_enter: false,
                },
                enter_selects_first_matching_item: false,
                leaves_current_line_empty: true,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::Search(scope),
            None,
        )
    }

    fn open_add_path_prompt(&mut self, path: CanonicalizedPath) -> anyhow::Result<()> {
        self.open_prompt(
            PromptConfig {
                title: "Add path".to_string(),
                on_enter: DispatchPrompt::AddPath,
                items: Vec::new(),
                enter_selects_first_matching_item: false,
                leaves_current_line_empty: false,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::AddPath,
            Some(path.display_absolute()),
        )
    }

    fn open_move_file_prompt(&mut self, path: CanonicalizedPath) -> anyhow::Result<()> {
        self.open_prompt(
            PromptConfig {
                title: "Move path".to_string(),
                on_enter: DispatchPrompt::MovePath { from: path.clone() },
                items: Vec::new(),
                enter_selects_first_matching_item: false,
                leaves_current_line_empty: false,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::MovePath,
            Some(path.display_absolute()),
        )
    }

    fn open_symbol_picker(&mut self, symbols: Symbols) -> anyhow::Result<()> {
        self.open_prompt(
            PromptConfig {
                title: "Symbols".to_string(),
                items: symbols
                    .symbols
                    .clone()
                    .into_iter()
                    .map(|symbol| symbol.into())
                    .collect_vec(),
                on_enter: DispatchPrompt::SelectSymbol { symbols },
                enter_selects_first_matching_item: true,
                leaves_current_line_empty: true,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::Symbol,
            None,
        )
    }

    fn open_command_prompt(&mut self) -> anyhow::Result<()> {
        self.open_prompt(
            PromptConfig {
                title: "Command".to_string(),
                on_enter: DispatchPrompt::RunCommand,
                items: crate::command::COMMANDS
                    .iter()
                    .flat_map(|command| command.to_dropdown_items())
                    .collect(),
                enter_selects_first_matching_item: true,
                leaves_current_line_empty: true,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::Command,
            None,
        )
    }

    fn open_file_picker(&mut self, kind: FilePickerKind) -> anyhow::Result<()> {
        let working_directory = self.working_directory.clone();
        self.open_prompt(
            PromptConfig {
                title: format!("Open file: {}", kind.display()),
                on_enter: DispatchPrompt::OpenFile { working_directory },
                items: {
                    match kind {
                        FilePickerKind::NonGitIgnored => {
                            // Note: we should not use CanonicalizedPath here, as it is resource-intensive
                            list::WalkBuilderConfig::non_git_ignored_files(
                                self.working_directory.clone(),
                            )?
                        }
                        FilePickerKind::GitStatus(diff_mode) => {
                            git::GitRepo::try_from(&self.working_directory)?
                                .diff_entries(diff_mode)?
                                .into_iter()
                                .map(|entry| entry.new_path().into_path_buf())
                                .collect_vec()
                        }
                        FilePickerKind::Opened => self
                            .layout
                            .get_opened_files()
                            .into_iter()
                            .map(|path| path.into_path_buf())
                            .collect_vec(),
                    }
                    .into_iter()
                    .map(|path| {
                        DropdownItem::new({
                            let name = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let icon = shared::canonicalized_path::get_path_icon(&path);
                            format!("{icon} {name}")
                        })
                        .set_group(path.parent().map(|parent| {
                            let relative = parent
                                .strip_prefix(&self.working_directory)
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|_| parent.display().to_string());
                            format!("{} {}", shared::icons::get_icon_config().folder, relative,)
                        }))
                        .set_dispatches(Dispatches::one(
                            crate::app::Dispatch::OpenFileFromPathBuf(path),
                        ))
                    })
                    .collect_vec()
                },
                enter_selects_first_matching_item: true,
                leaves_current_line_empty: true,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::OpenFile,
            None,
        )
    }

    /// This only opens the file in the background but does not focus it.
    /// If you need to focus it, use `Self::go_to_file` instead.
    fn open_file(
        &mut self,
        path: &CanonicalizedPath,
        option: OpenFileOption,
    ) -> anyhow::Result<Rc<RefCell<SuggestiveEditor>>> {
        if option.store_history() {
            self.file_path_history.push(path.clone())
        }
        // Check if the file is opened before
        // so that we won't notify the LSP twice
        if let Some(matching_editor) = self.layout.open_file(path, option.is_focus()) {
            return Ok(matching_editor);
        }

        let buffer = Buffer::from_path(path, true)?;
        let language = buffer.language();
        let content = buffer.content();
        let buffer = Rc::new(RefCell::new(buffer));
        let editor = SuggestiveEditor::from_buffer(buffer, SuggestiveEditorFilter::CurrentWord);
        let component_id = editor.id();
        let component = Rc::new(RefCell::new(editor));

        self.layout.add_suggestive_editor(component.clone());

        if option.is_focus() {
            self.layout
                .replace_and_focus_current_suggestive_editor(component.clone())
        }

        if let Some(language) = language {
            self.request_syntax_highlight(component_id, language, content)?;
        }
        if self.enable_lsp {
            self.lsp_manager.open_file(path.clone())?;
        }
        Ok(component)
    }

    pub(crate) fn handle_lsp_notification(
        &mut self,
        notification: LspNotification,
    ) -> anyhow::Result<()> {
        match notification {
            LspNotification::Hover(hover) => self.show_editor_info(Info::new(
                "Hover Info".to_string(),
                hover.contents.join("\n\n"),
            )),
            LspNotification::Definition(context, response) => {
                match response {
                    GotoDefinitionResponse::Single(location) => self.go_to_location(&location)?,
                    GotoDefinitionResponse::Multiple(locations) => {
                        if locations.is_empty() {
                            self.show_global_info(Info::new(
                                "Goto definition info".to_string(),
                                "No definitions found".to_string(),
                            ));
                        } else {
                            self.set_quickfix_list_type(
                                context,
                                QuickfixListType::Items(
                                    locations.into_iter().map(QuickfixListItem::from).collect(),
                                ),
                            )?;
                        }
                    }
                }

                Ok(())
            }
            LspNotification::References(context, locations) => self.set_quickfix_list_type(
                context,
                QuickfixListType::Items(
                    locations.into_iter().map(QuickfixListItem::from).collect(),
                ),
            ),
            LspNotification::Completion(_context, completion) => {
                self.handle_dispatch_suggestive_editor(DispatchSuggestiveEditor::Completion(
                    completion,
                ))?;

                Ok(())
            }
            LspNotification::Initialized(language) => {
                // Need to notify LSP that the file is opened
                self.lsp_manager.initialized(
                    language,
                    self.layout
                        .buffers()
                        .into_iter()
                        .filter_map(|buffer| buffer.borrow().path())
                        .collect_vec(),
                );
                Ok(())
            }
            LspNotification::PublishDiagnostics(params) => {
                self.update_diagnostics(
                    params
                        .uri
                        .to_file_path()
                        .map_err(|err| {
                            anyhow::anyhow!("Couldn't convert URI to file path: {:?}", err)
                        })?
                        .try_into()?,
                    params.diagnostics,
                )?;
                Ok(())
            }
            LspNotification::PrepareRenameResponse(response) => {
                let editor = self.current_component();

                let current_name = {
                    let editor = editor.borrow();
                    let buffer = editor.editor().buffer();
                    response
                        .range
                        .map(|range| {
                            let range = buffer.position_to_char(range.start)?
                                ..buffer.position_to_char(range.end)?;
                            buffer.slice(&range.into())
                        })
                        .transpose()
                        .unwrap_or_default()
                        .map(|rope| rope.to_string())
                };
                self.open_rename_prompt(current_name)?;

                Ok(())
            }
            LspNotification::Error(error) => {
                self.show_global_info(Info::new("LSP Error".to_string(), error));
                Ok(())
            }
            LspNotification::WorkspaceEdit(workspace_edit) => {
                self.apply_workspace_edit(workspace_edit)
            }
            LspNotification::CodeAction(code_actions) => {
                self.handle_dispatch(Dispatch::ReceiveCodeActions(code_actions))?;
                Ok(())
            }
            LspNotification::SignatureHelp(signature_help) => {
                self.handle_signature_help(signature_help)?;
                Ok(())
            }
            LspNotification::Symbols(symbols) => {
                self.open_symbol_picker(symbols)?;
                Ok(())
            }
            LspNotification::CompletionItemResolve(completion_item) => {
                self.update_current_completion_item(completion_item.into())
            }
        }
    }

    fn update_diagnostics(
        &mut self,
        path: CanonicalizedPath,
        diagnostics: Vec<lsp_types::Diagnostic>,
    ) -> anyhow::Result<()> {
        let component = self.open_file(&path, OpenFileOption::Background)?;

        component
            .borrow_mut()
            .editor_mut()
            .buffer_mut()
            .set_diagnostics(diagnostics);
        Ok(())
    }

    pub(crate) fn get_quickfix_list(&self) -> Option<QuickfixList> {
        self.context.quickfix_list_state().as_ref().map(|state| {
            QuickfixList::new(
                self.layout.get_quickfix_list_items(&state.source),
                self.layout.buffers(),
            )
            .set_current_item_index(state.current_item_index)
        })
    }

    fn goto_quickfix_list_item(&mut self, movement: Movement) -> anyhow::Result<()> {
        if let Some(mut quickfix_list) = self.get_quickfix_list() {
            if let Some((current_item_index, dispatches)) = quickfix_list.get_item(movement) {
                self.context
                    .set_quickfix_list_current_item_index(current_item_index);
                self.handle_dispatches(dispatches)?;
                self.render_quickfix_list(
                    quickfix_list.set_current_item_index(current_item_index),
                )?;
            }
        }

        Ok(())
    }

    fn show_global_info(&mut self, info: Info) {
        self.layout.show_global_info(info).unwrap_or_else(|err| {
            log::error!("Error showing info: {:?}", err);
        });
    }

    fn go_to_location(&mut self, Location { path, range }: &Location) -> Result<(), anyhow::Error> {
        let component = self.open_file(path, OpenFileOption::Focus)?;
        let dispatches = component
            .borrow_mut()
            .editor_mut()
            .set_position_range(range.clone())?;
        self.handle_dispatches(dispatches)
    }

    fn set_quickfix_list_type(
        &mut self,
        context: ResponseContext,
        r#type: QuickfixListType,
    ) -> anyhow::Result<()> {
        let title = context.description.unwrap_or_default();
        self.context.set_mode(Some(GlobalMode::QuickfixListItem));
        match r#type {
            QuickfixListType::Diagnostic(severity_range) => {
                self.context
                    .set_quickfix_list_source(QuickfixListSource::Diagnostic(severity_range));
            }
            QuickfixListType::Items(items) => {
                self.layout.clear_quickfix_list_items();
                items
                    .into_iter()
                    .group_by(|item| item.location().path.clone())
                    .into_iter()
                    .map(|(path, items)| -> anyhow::Result<()> {
                        let editor = self.open_file(&path, OpenFileOption::Background)?;
                        editor
                            .borrow_mut()
                            .editor_mut()
                            .buffer_mut()
                            .update_quickfix_list_items(items.collect_vec());
                        Ok(())
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                self.context
                    .set_quickfix_list_source(QuickfixListSource::Custom);
            }
            QuickfixListType::Bookmark => {
                self.context
                    .set_quickfix_list_source(QuickfixListSource::Bookmark);
            }
        }
        match context.scope {
            None | Some(Scope::Global) => {
                self.goto_quickfix_list_item(Movement::Current)?;
                Ok(())
            }
            Some(Scope::Local) => self.handle_dispatch(Dispatch::ToEditor(SetSelectionMode(
                SelectionMode::LocalQuickfix { title },
            ))),
        }
    }

    fn apply_workspace_edit(&mut self, workspace_edit: WorkspaceEdit) -> Result<(), anyhow::Error> {
        // TODO: should we wrap this in a transaction so that if one of the edit/operation fails, the whole transaction fails?
        // Such that it won't leave the workspace in an half-edited messed up state
        for edit in workspace_edit.edits {
            let component = self.open_file(&edit.path, OpenFileOption::Background)?;
            let dispatches = component
                .borrow_mut()
                .editor_mut()
                .apply_positional_edits(edit.edits)?;

            self.handle_dispatches(dispatches)?;

            let dispatches = component.borrow_mut().editor_mut().save()?;

            self.handle_dispatches(dispatches)?;
        }
        use crate::lsp::workspace_edit::ResourceOperation;
        for operation in workspace_edit.resource_operations {
            match operation {
                ResourceOperation::Create(path) => self.add_path(path)?,
                ResourceOperation::Rename { old, new } => self.move_file(old, new)?,
                ResourceOperation::Delete(path) => self.delete_path(&path)?,
            }
        }
        Ok(())
    }

    fn show_keymap_legend(&mut self, keymap_legend_config: KeymapLegendConfig) {
        self.layout.show_keymap_legend(keymap_legend_config)
    }

    fn global_replace(&mut self) -> anyhow::Result<()> {
        let working_directory = self.working_directory.clone();
        let global_search_config = self.context.global_search_config();
        let walk_builder_config = WalkBuilderConfig {
            root: working_directory.clone().into(),
            include: global_search_config.include_glob(),
            exclude: global_search_config.exclude_glob(),
        };
        let config = self.context.global_search_config().local_config();
        let affected_paths = list::grep::replace(walk_builder_config, config.clone())?;
        self.layout.reload_buffers(affected_paths)
    }

    fn global_search(&mut self) -> anyhow::Result<()> {
        let working_directory = self.working_directory.clone();

        let global_search_config = self.context.global_search_config();
        let walk_builder_config = WalkBuilderConfig {
            root: working_directory.clone().into(),
            include: global_search_config.include_glob(),
            exclude: global_search_config.exclude_glob(),
        };
        let config = global_search_config.local_config();
        if config.search().is_empty() {
            return Ok(());
        }
        let locations = match config.mode {
            LocalSearchConfigMode::Regex(regex) => {
                list::grep::run(&config.search(), walk_builder_config, regex)
            }
            LocalSearchConfigMode::AstGrep => {
                list::ast_grep::run(config.search().clone(), walk_builder_config)
            }
            LocalSearchConfigMode::CaseAgnostic => {
                list::case_agnostic::run(config.search().clone(), walk_builder_config)
            }
        }?;
        self.set_quickfix_list_type(
            ResponseContext::default().set_description("Global search"),
            QuickfixListType::Items(
                locations
                    .into_iter()
                    .map(|location| QuickfixListItem::new(location, None))
                    .collect_vec(),
            ),
        )?;
        Ok(())
    }

    pub(crate) fn quit_all(&self) -> Result<(), anyhow::Error> {
        Ok(self.sender.send(AppMessage::QuitAll)?)
    }

    pub(crate) fn sender(&self) -> Sender<AppMessage> {
        self.sender.clone()
    }

    fn run_command(&mut self, command: String) -> anyhow::Result<()> {
        let dispatch = crate::command::find(&command)
            .map(|cmd| cmd.dispatch())
            .ok_or_else(|| anyhow::anyhow!("Unknown command: {}", command))?;
        self.handle_dispatch(dispatch)
    }

    fn save_quit_all(&mut self) -> anyhow::Result<()> {
        self.save_all()?;
        self.quit_all()?;
        Ok(())
    }

    fn save_all(&self) -> anyhow::Result<()> {
        self.layout.save_all()
    }

    fn open_yes_no_prompt(&mut self, prompt: YesNoPrompt) -> anyhow::Result<()> {
        self.handle_dispatch(Dispatch::ShowKeymapLegend(KeymapLegendConfig {
            title: "Prompt".to_string(),
            body: KeymapLegendBody::MultipleSections {
                sections: [KeymapLegendSection {
                    title: prompt.title,
                    keymaps: Keymaps::new(&[
                        Keymap::new("y", "Yes".to_string(), *prompt.yes),
                        Keymap::new("n", "No".to_string(), Dispatch::Null),
                    ]),
                }]
                .to_vec(),
            },
        }))
    }

    fn delete_path(&mut self, path: &CanonicalizedPath) -> anyhow::Result<()> {
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
        self.layout.remove_suggestive_editor(path);
        self.layout.refresh_file_explorer(&self.working_directory)?;
        Ok(())
    }

    fn move_file(&mut self, from: CanonicalizedPath, to: PathBuf) -> anyhow::Result<()> {
        use std::fs;
        self.add_path_parent(&to)?;
        fs::rename(from.clone(), to.clone())?;
        self.layout.refresh_file_explorer(&self.working_directory)?;
        let to = to.try_into()?;
        self.reveal_path_in_explorer(&to)?;
        self.lsp_manager.send_message(
            from.clone(),
            FromEditor::WorkspaceDidRenameFiles {
                old: from.clone(),
                new: to,
            },
        )?;
        self.layout.remove_suggestive_editor(&from);
        Ok(())
    }
    fn add_path_parent(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(new_dir) = path.parent() {
            std::fs::create_dir_all(new_dir)?;
        }
        Ok(())
    }

    fn add_path(&mut self, path: String) -> anyhow::Result<()> {
        if PathBuf::from(path.clone()).exists() {
            return Err(anyhow::anyhow!("The path \"{}\" already exists", path));
        };
        if path.ends_with(&std::path::MAIN_SEPARATOR.to_string()) {
            std::fs::create_dir_all(path.clone())?;
        } else {
            let path: PathBuf = path.clone().into();
            self.add_path_parent(&path)?;
            std::fs::File::create(&path)?;
        }
        self.layout.refresh_file_explorer(&self.working_directory)?;
        self.reveal_path_in_explorer(&path.try_into()?)?;

        Ok(())
    }

    fn request_syntax_highlight(
        &self,
        component_id: ComponentId,
        language: Language,
        content: String,
    ) -> anyhow::Result<()> {
        if let Some(sender) = &self.syntax_highlight_request_sender {
            sender.send(SyntaxHighlightRequest {
                component_id,
                language,
                source_code: content,
            })?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn get_current_selected_texts(&self) -> Vec<String> {
        let _content = self.current_component().borrow().content();
        self.current_component()
            .borrow()
            .editor()
            .get_selected_texts()
    }

    #[cfg(test)]
    pub(crate) fn get_file_content(&self, path: &CanonicalizedPath) -> String {
        self.layout
            .get_existing_editor(path)
            .unwrap()
            .borrow()
            .content()
    }

    pub(crate) fn handle_dispatch_editor(
        &mut self,
        dispatch_editor: DispatchEditor,
    ) -> anyhow::Result<()> {
        self.handle_dispatch_editor_custom(dispatch_editor, self.current_component())
    }

    fn handle_dispatch_editor_custom(
        &mut self,
        dispatch_editor: DispatchEditor,
        component: Rc<RefCell<dyn Component>>,
    ) -> anyhow::Result<()> {
        let dispatches = component
            .borrow_mut()
            .handle_dispatch_editor(&mut self.context, dispatch_editor)?;

        self.handle_dispatches(dispatches)?;
        Ok(())
    }

    fn get_repo_git_hunks(&mut self, diff_mode: git::DiffMode) -> anyhow::Result<()> {
        let working_directory = self.working_directory.clone();
        let repo = git::GitRepo::try_from(&working_directory)?;
        let diffs = repo.diffs(diff_mode)?;
        self.set_quickfix_list_type(
            ResponseContext::default().set_description("Git Hunks"),
            QuickfixListType::Items(
                diffs
                    .into_iter()
                    .flat_map(|file_diff| {
                        file_diff
                            .hunks()
                            .iter()
                            .map(|hunk| {
                                let line_range = hunk.line_range();
                                let location = Location {
                                    path: file_diff.path().clone(),
                                    range: Position {
                                        line: line_range.start,
                                        column: 0,
                                    }..Position {
                                        line: line_range.end,
                                        column: 0,
                                    },
                                };
                                QuickfixListItem::new(location, hunk.to_info())
                            })
                            .collect_vec()
                    })
                    .collect_vec(),
            ),
        )
    }

    #[cfg(test)]
    fn set_global_title(&mut self, title: String) {
        self.global_title = Some(title)
    }

    pub(crate) fn set_syntax_highlight_request_sender(
        &mut self,
        sender: Sender<SyntaxHighlightRequest>,
    ) {
        self.syntax_highlight_request_sender = Some(sender);
    }

    #[cfg(test)]
    pub(crate) fn get_current_file_path(&self) -> Option<CanonicalizedPath> {
        self.current_component().borrow().path()
    }

    fn set_global_mode(&mut self, mode: Option<GlobalMode>) {
        self.context.set_mode(mode);
    }

    fn open_omit_prompt(
        &mut self,
        kind: FilterKind,
        target: FilterTarget,
        make_mechanism: MakeFilterMechanism,
    ) -> anyhow::Result<()> {
        self.open_prompt(
            PromptConfig {
                title: format!(
                    "Omit: {:?} selection by {:?} matching {:?}",
                    kind, target, make_mechanism
                ),
                on_enter: DispatchPrompt::PushFilter {
                    kind,
                    target,
                    make_mechanism,
                },
                items: Vec::new(),
                enter_selects_first_matching_item: false,
                leaves_current_line_empty: true,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::Omit,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn context(&self) -> &Context {
        &self.context
    }

    fn update_local_search_config(
        &mut self,
        update: LocalSearchConfigUpdate,
        scope: Scope,
        show_legend: bool,
    ) -> Result<(), anyhow::Error> {
        self.context.update_local_search_config(update, scope);
        match scope {
            Scope::Local => self.local_search()?,
            Scope::Global => {
                self.global_search()?;
            }
        }

        if show_legend {
            self.show_search_config(scope);
        }
        Ok(())
    }

    fn update_global_search_config(
        &mut self,
        update: GlobalSearchConfigUpdate,
    ) -> anyhow::Result<()> {
        self.context.update_global_search_config(update)?;
        self.global_search()?;
        self.show_search_config(Scope::Global);
        Ok(())
    }

    fn open_set_global_search_filter_glob_prompt(
        &mut self,
        filter_glob: GlobalSearchFilterGlob,
    ) -> anyhow::Result<()> {
        self.open_prompt(
            PromptConfig {
                title: format!("Set global search {:?} files glob", filter_glob),
                on_enter: DispatchPrompt::GlobalSearchConfigSetGlob { filter_glob },
                items: Vec::new(),
                enter_selects_first_matching_item: false,
                leaves_current_line_empty: false,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::FilterGlob(filter_glob),
            None,
        )
    }

    fn show_search_config(&mut self, scope: Scope) {
        fn show_checkbox(title: &str, checked: bool) -> String {
            format!("[{}] {title}", if checked { "X" } else { " " })
        }
        let global_search_confing = match scope {
            Scope::Local => None,
            Scope::Global => Some(self.context.global_search_config()),
        };
        let local_search_config = global_search_confing
            .map(|config| config.local_config())
            .unwrap_or_else(|| self.context.local_search_config());
        let update_keymap =
            |key: &'static str, description: String, update: LocalSearchConfigUpdate| -> Keymap {
                Keymap::new(
                    key,
                    description,
                    Dispatch::UpdateLocalSearchConfig {
                        update,
                        scope,
                        show_config_after_enter: true,
                    },
                )
            };
        let update_mode_keymap = |key: &'static str,
                                  name: String,
                                  mode: LocalSearchConfigMode,
                                  checked: bool|
         -> Keymap {
            let description = show_checkbox(&name, checked);
            update_keymap(key, description, LocalSearchConfigUpdate::Mode(mode))
        };
        let regex = match local_search_config.mode {
            LocalSearchConfigMode::Regex(regex) => Some(regex),
            LocalSearchConfigMode::AstGrep => None,
            LocalSearchConfigMode::CaseAgnostic => None,
        };
        self.show_keymap_legend(KeymapLegendConfig {
            title: format!("Configure Search ({:?})", scope),
            body: KeymapLegendBody::MultipleSections {
                sections: [
                    KeymapLegendSection {
                        title: "Inputs".to_string(),
                        keymaps: Keymaps::new(
                            &[
                                Keymap::new(
                                    "s",
                                    format!("Search = {}", local_search_config.search()),
                                    Dispatch::OpenUpdateSearchPrompt { scope },
                                ),
                                Keymap::new(
                                    "r",
                                    format!("Replacement = {}", local_search_config.replacement()),
                                    Dispatch::OpenUpdateReplacementPrompt { scope },
                                ),
                            ]
                            .into_iter()
                            .chain(
                                global_search_confing
                                    .map(|config| {
                                        [
                                            Keymap::new(
                                                "I",
                                                format!(
                                                    "Include files (glob) = {}",
                                                    config
                                                        .include_glob()
                                                        .map(|glob| glob.to_string())
                                                        .unwrap_or_default()
                                                ),
                                                Dispatch::OpenSetGlobalSearchFilterGlobPrompt {
                                                    filter_glob: GlobalSearchFilterGlob::Include,
                                                },
                                            ),
                                            Keymap::new(
                                                "E",
                                                format!(
                                                    "Exclude files (glob) = {}",
                                                    config
                                                        .exclude_glob()
                                                        .map(|glob| glob.to_string())
                                                        .unwrap_or_default()
                                                ),
                                                Dispatch::OpenSetGlobalSearchFilterGlobPrompt {
                                                    filter_glob: GlobalSearchFilterGlob::Exclude,
                                                },
                                            ),
                                        ]
                                        .to_vec()
                                    })
                                    .unwrap_or_default(),
                            )
                            .collect_vec(),
                        ),
                    },
                    KeymapLegendSection {
                        title: "Mode".to_string(),
                        keymaps: Keymaps::new(&[
                            update_mode_keymap(
                                "a",
                                "AST Grep".to_string(),
                                LocalSearchConfigMode::AstGrep,
                                local_search_config.mode == LocalSearchConfigMode::AstGrep,
                            ),
                            update_mode_keymap(
                                "c",
                                "Case Agnostic".to_string(),
                                LocalSearchConfigMode::CaseAgnostic,
                                local_search_config.mode == LocalSearchConfigMode::CaseAgnostic,
                            ),
                            update_mode_keymap(
                                "l",
                                "Literal".to_string(),
                                LocalSearchConfigMode::Regex(RegexConfig {
                                    escaped: true,
                                    ..regex.unwrap_or_default()
                                }),
                                regex.map(|regex| regex.escaped).unwrap_or(false),
                            ),
                            update_mode_keymap(
                                "x",
                                "Regex".to_string(),
                                LocalSearchConfigMode::Regex(RegexConfig {
                                    escaped: false,
                                    ..regex.unwrap_or_default()
                                }),
                                regex.map(|regex| !regex.escaped).unwrap_or(false),
                            ),
                        ]),
                    },
                    KeymapLegendSection {
                        title: "Actions".to_string(),
                        keymaps: Keymaps::new(&[Keymap::new(
                            "R",
                            "Replace all".to_string(),
                            Dispatch::Replace { scope },
                        )]),
                    },
                ]
                .into_iter()
                .chain(regex.map(|regex| {
                    KeymapLegendSection {
                        title: "Options".to_string(),
                        keymaps: Keymaps::new(
                            &[
                                update_mode_keymap(
                                    "i",
                                    "Case-sensitive".to_string(),
                                    LocalSearchConfigMode::Regex(RegexConfig {
                                        case_sensitive: !regex.case_sensitive,
                                        ..regex
                                    }),
                                    regex.case_sensitive,
                                ),
                                update_mode_keymap(
                                    "w",
                                    "Match whole word".to_string(),
                                    LocalSearchConfigMode::Regex(RegexConfig {
                                        match_whole_word: !regex.match_whole_word,
                                        ..regex
                                    }),
                                    regex.match_whole_word,
                                ),
                            ]
                            .into_iter()
                            .collect_vec(),
                        ),
                    }
                }))
                .collect(),
            },
        })
    }

    fn open_update_replacement_prompt(&mut self, scope: Scope) -> Result<(), anyhow::Error> {
        self.open_prompt(
            PromptConfig {
                title: format!("Set Replace ({:?})", scope),
                on_enter: DispatchPrompt::UpdateLocalSearchConfigReplacement { scope },
                items: Vec::new(),
                enter_selects_first_matching_item: false,
                leaves_current_line_empty: false,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::Replacement(scope),
            None,
        )
    }

    fn open_update_search_prompt(&mut self, scope: Scope) -> Result<(), anyhow::Error> {
        self.open_prompt(
            PromptConfig {
                title: format!("Set Search ({:?})", scope),
                on_enter: DispatchPrompt::UpdateLocalSearchConfigSearch {
                    scope,
                    show_config_after_enter: true,
                },
                items: self.words(),
                enter_selects_first_matching_item: false,
                leaves_current_line_empty: false,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::Search(scope),
            None,
        )
    }

    fn words(&self) -> Vec<DropdownItem> {
        self.current_component()
            .borrow()
            .editor()
            .buffer()
            .words()
            .into_iter()
            .map(|word| {
                DropdownItem::new(word.clone()).set_dispatches(Dispatches::one(Dispatch::ToEditor(
                    ReplaceCurrentSelectionWith(word),
                )))
            })
            .collect_vec()
    }

    fn go_to_previous_file(&mut self) -> anyhow::Result<()> {
        if let Some(path) = self.file_path_history.undo() {
            self.open_file(&path, OpenFileOption::FocusNoHistory)?;
        }
        Ok(())
    }

    fn go_to_next_file(&mut self) -> anyhow::Result<()> {
        if let Some(path) = self.file_path_history.redo() {
            self.open_file(&path, OpenFileOption::FocusNoHistory)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn get_current_component_content(&self) -> String {
        self.current_component().borrow().editor().content()
    }

    #[cfg(test)]
    fn handle_key_events(&mut self, key_events: Vec<event::KeyEvent>) -> anyhow::Result<()> {
        for key_event in key_events.into_iter() {
            self.handle_event(Event::Key(key_event.to_owned()))?;
        }
        Ok(())
    }

    pub(crate) fn handle_dispatch_suggestive_editor(
        &mut self,
        dispatch: DispatchSuggestiveEditor,
    ) -> anyhow::Result<()> {
        let component = self
            .layout
            .get_component_by_kind(ComponentKind::SuggestiveEditor)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "App::handle_dispatch_suggestive_editor Cannot find suggestive editor"
                )
            })?;
        let dispatches = component
            .borrow_mut()
            .as_any_mut()
            .downcast_mut::<SuggestiveEditor>()
            .ok_or_else(|| {
                anyhow::anyhow!("App::handle_dispatch_suggestive_editor Failed to downcast")
            })?
            .handle_dispatch(dispatch)?;
        self.handle_dispatches(dispatches)
    }

    #[cfg(test)]
    pub(crate) fn completion_dropdown_is_open(&self) -> bool {
        self.layout.completion_dropdown_is_open()
    }

    #[cfg(test)]
    pub(crate) fn current_completion_dropdown(&self) -> Option<Rc<RefCell<dyn Component>>> {
        self.layout.current_completion_dropdown()
    }

    fn open_prompt(
        &mut self,
        prompt_config: PromptConfig,
        key: PromptHistoryKey,
        current_line: Option<String>,
    ) -> anyhow::Result<()> {
        let history = self.context.get_prompt_history(key, current_line);
        let (prompt, dispatches) = Prompt::new(prompt_config, key, history);

        self.layout
            .add_and_focus_prompt(ComponentKind::Prompt, Rc::new(RefCell::new(prompt)));
        self.handle_dispatches(dispatches)
    }

    fn render_dropdown(
        &mut self,
        editor: Rc<RefCell<Editor>>,
        render: DropdownRender,
    ) -> Result<(), anyhow::Error> {
        let dispatches = editor
            .borrow_mut()
            .render_dropdown(&mut self.context, &render)?;
        editor.borrow_mut().set_title(render.title);

        match render.info {
            Some(info) => {
                self.layout.show_dropdown_info(info)?;
            }
            _ => self.layout.hide_dropdown_info(),
        }
        self.handle_dispatches(dispatches)
    }

    #[cfg(test)]
    pub(crate) fn get_dropdown_infos_count(&self) -> usize {
        self.layout.get_dropdown_infos_count()
    }

    pub(crate) fn render_quickfix_list(
        &mut self,
        quickfix_list: QuickfixList,
    ) -> anyhow::Result<()> {
        let dispatches = self.layout.show_quickfix_list(quickfix_list)?;
        self.handle_dispatches(dispatches)
    }

    fn show_editor_info(&mut self, info: Info) -> anyhow::Result<()> {
        self.layout.show_editor_info(info)
    }

    #[cfg(test)]
    pub(crate) fn editor_info_open(&self) -> bool {
        self.layout.editor_info_open()
    }

    #[cfg(test)]
    pub(crate) fn editor_info_content(&self) -> Option<String> {
        self.layout.editor_info_content()
    }

    fn reveal_path_in_explorer(&mut self, path: &CanonicalizedPath) -> anyhow::Result<()> {
        let dispatches = self.layout.reveal_path_in_explorer(path)?;
        self.handle_dispatches(dispatches)
    }

    #[cfg(test)]
    pub(crate) fn file_explorer_content(&self) -> String {
        self.layout.file_explorer_content()
    }

    fn open_code_actions_prompt(
        &mut self,
        code_actions: Vec<crate::lsp::code_action::CodeAction>,
    ) -> anyhow::Result<()> {
        self.open_prompt(
            PromptConfig {
                on_enter: DispatchPrompt::Null,
                items: code_actions
                    .into_iter()
                    .map(move |code_action| code_action.into())
                    .collect(),
                title: "Code Actions".to_string(),
                enter_selects_first_matching_item: true,
                leaves_current_line_empty: true,
                fire_dispatches_on_change: None,
            },
            PromptHistoryKey::CodeAction,
            None,
        )?;
        Ok(())
    }

    fn close_current_window_and_focus_parent(&mut self) {
        self.layout.close_current_window_and_focus_parent()
    }

    #[cfg(test)]
    pub(crate) fn opened_files_count(&self) -> usize {
        self.layout.get_opened_files().len()
    }

    #[cfg(test)]
    pub(crate) fn quickfix_list_info(&self) -> Option<String> {
        self.layout.quickfix_list_info()
    }

    #[cfg(test)]
    pub(crate) fn get_component_by_kind(
        &self,
        kind: ComponentKind,
    ) -> Option<Rc<RefCell<dyn Component>>> {
        self.layout.get_component_by_kind(kind)
    }

    fn hide_editor_info(&mut self) {
        self.layout.hide_editor_info()
    }

    #[cfg(test)]
    pub(crate) fn components_order(&self) -> Vec<ComponentKind> {
        self.layout
            .components()
            .into_iter()
            .map(|c| c.kind())
            .collect_vec()
    }

    fn handle_signature_help(
        &mut self,
        signature_help: Option<crate::lsp::signature_help::SignatureHelp>,
    ) -> anyhow::Result<()> {
        if let Some(info) = signature_help.and_then(|s| s.into_info()) {
            if self.current_component().borrow().editor().mode
                == crate::components::editor::Mode::Insert
            {
                self.show_editor_info(info)?;
            }
        } else {
            self.hide_editor_info()
        }
        Ok(())
    }

    fn push_history_prompt(&mut self, key: PromptHistoryKey, line: String) {
        self.context.push_history_prompt(key, line)
    }

    fn open_theme_prompt(&mut self) -> anyhow::Result<()> {
        self.open_prompt(
            PromptConfig {
                on_enter: DispatchPrompt::Null,
                items: crate::themes::themes()?
                    .into_iter()
                    .map(|theme| {
                        DropdownItem::new(theme.name.to_string())
                            .set_dispatches(Dispatches::one(Dispatch::SetTheme(theme.clone())))
                    })
                    .collect_vec(),
                title: "Theme".to_string(),
                enter_selects_first_matching_item: true,
                leaves_current_line_empty: true,
                fire_dispatches_on_change: Some(Dispatches::one(Dispatch::SetTheme(
                    self.context.theme().clone(),
                ))),
            },
            PromptHistoryKey::Theme,
            None,
        )
    }

    fn update_current_completion_item(
        &mut self,
        completion_item: CompletionItem,
    ) -> anyhow::Result<()> {
        self.handle_dispatch_suggestive_editor(
            DispatchSuggestiveEditor::UpdateCurrentCompletionItem(completion_item),
        )
    }

    #[cfg(test)]
    pub(crate) fn lsp_request_sent(&self, from_editor: &FromEditor) -> bool {
        self.lsp_manager.lsp_request_sent(from_editor)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Dimension {
    pub(crate) height: u16,
    pub(crate) width: u16,
}

impl Dimension {
    #[cfg(test)]
    pub(crate) fn area(&self) -> usize {
        self.height as usize * self.width as usize
    }

    #[cfg(test)]
    pub(crate) fn positions(&self) -> std::collections::HashSet<Position> {
        (0..self.height as usize)
            .flat_map(|line| (0..self.width as usize).map(move |column| Position { column, line }))
            .collect()
    }

    fn decrement_height(&self, global_title_bar_height: u16) -> Dimension {
        Dimension {
            height: self.height.saturating_sub(global_title_bar_height),
            width: self.width,
        }
    }
}

#[must_use]
#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) struct Dispatches(Vec<Dispatch>);
impl From<Vec<Dispatch>> for Dispatches {
    fn from(value: Vec<Dispatch>) -> Self {
        Self(value)
    }
}
impl Dispatches {
    pub(crate) fn into_vec(self) -> Vec<Dispatch> {
        self.0
    }

    pub(crate) fn new(dispatches: Vec<Dispatch>) -> Dispatches {
        Dispatches(dispatches)
    }

    pub(crate) fn chain(self, other: Dispatches) -> Dispatches {
        self.0.into_iter().chain(other.0).collect_vec().into()
    }

    pub(crate) fn append(self, other: Dispatch) -> Dispatches {
        self.0.into_iter().chain(Some(other)).collect_vec().into()
    }

    pub(crate) fn append_some(self, dispatch: Option<Dispatch>) -> Dispatches {
        if let Some(dispatch) = dispatch {
            self.append(dispatch)
        } else {
            self
        }
    }

    pub(crate) fn one(edit: Dispatch) -> Dispatches {
        Dispatches(vec![edit])
    }
}

#[must_use]
#[derive(Clone, Debug, PartialEq, NamedVariant)]
/// Dispatch are for child component to request action from the root node
pub(crate) enum Dispatch {
    SetTheme(crate::themes::Theme),
    CloseCurrentWindow,
    OpenFilePicker(FilePickerKind),
    OpenSearchPrompt {
        scope: Scope,
    },
    OpenFile(CanonicalizedPath),
    OpenFileFromPathBuf(PathBuf),
    ShowGlobalInfo(Info),
    RequestCompletion,
    RequestSignatureHelp,
    RequestHover,
    RequestDefinitions(Scope),
    RequestDeclarations(Scope),
    RequestImplementations(Scope),
    RequestTypeDefinitions(Scope),
    RequestReferences {
        scope: Scope,
        include_declaration: bool,
    },
    PrepareRename,
    RequestCodeAction {
        diagnostics: Vec<lsp_types::Diagnostic>,
    },
    RenameSymbol {
        new_name: String,
    },
    DocumentDidChange {
        component_id: ComponentId,
        path: Option<CanonicalizedPath>,
        content: String,
        language: Option<Language>,
    },
    DocumentDidSave {
        path: CanonicalizedPath,
    },
    SetQuickfixList(QuickfixListType),
    GotoQuickfixListItem(Movement),
    ApplyWorkspaceEdit(WorkspaceEdit),
    ShowKeymapLegend(KeymapLegendConfig),
    RemainOnlyCurrentComponent,

    #[cfg(test)]
    /// Used for testing
    Custom(String),
    ToEditor(DispatchEditor),
    RequestDocumentSymbols,
    GotoLocation(Location),
    OpenMoveToIndexPrompt,
    RunCommand(String),
    QuitAll,
    OpenCommandPrompt,
    SaveQuitAll,
    RevealInExplorer(CanonicalizedPath),
    OpenYesNoPrompt(YesNoPrompt),
    OpenMoveFilePrompt(CanonicalizedPath),
    OpenAddPathPrompt(CanonicalizedPath),
    DeletePath(CanonicalizedPath),
    Null,
    MoveFile {
        from: CanonicalizedPath,
        to: PathBuf,
    },
    AddPath(String),
    RefreshFileExplorer,
    SetClipboardContent {
        copied_texts: CopiedTexts,
        use_system_clipboard: bool,
    },
    SetGlobalMode(Option<GlobalMode>),
    #[cfg(test)]
    HandleKeyEvent(event::KeyEvent),
    #[cfg(test)]
    HandleKeyEvents(Vec<event::KeyEvent>),
    GetRepoGitHunks(git::DiffMode),
    SaveAll,
    #[cfg(test)]
    TerminalDimensionChanged(Dimension),
    #[cfg(test)]
    SetGlobalTitle(String),
    OpenOmitPrompt {
        kind: FilterKind,
        target: FilterTarget,
        make_mechanism: MakeFilterMechanism,
    },
    LspExecuteCommand {
        command: crate::lsp::code_action::Command,
    },
    UpdateLocalSearchConfig {
        update: LocalSearchConfigUpdate,
        scope: Scope,
        show_config_after_enter: bool,
    },
    UpdateGlobalSearchConfig {
        update: GlobalSearchConfigUpdate,
    },
    OpenSetGlobalSearchFilterGlobPrompt {
        filter_glob: GlobalSearchFilterGlob,
    },
    ShowSearchConfig {
        scope: Scope,
    },
    OpenUpdateReplacementPrompt {
        scope: Scope,
    },
    OpenUpdateSearchPrompt {
        scope: Scope,
    },
    Replace {
        scope: Scope,
    },
    #[cfg(test)]
    HandleLspNotification(LspNotification),
    CloseDropdown,
    RenderDropdown {
        render: DropdownRender,
    },
    #[cfg(test)]
    OpenPrompt {
        config: PromptConfig,
        key: PromptHistoryKey,
        current_line: Option<String>,
    },
    ShowEditorInfo(Info),
    ReceiveCodeActions(Vec<crate::lsp::code_action::CodeAction>),
    OtherWindow,
    CloseCurrentWindowAndFocusParent,
    CloseEditorInfo,
    GoToPreviousFile,
    GoToNextFile,
    PushPromptHistory {
        key: PromptHistoryKey,
        line: String,
    },
    OpenThemePrompt,
    ResolveCompletionItem(lsp_types::CompletionItem),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GlobalSearchConfigUpdate {
    SetGlob(GlobalSearchFilterGlob, String),
}

#[derive(Clone, Hash, Debug, PartialEq, Eq, Copy)]
pub(crate) enum GlobalSearchFilterGlob {
    Include,
    Exclude,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LocalSearchConfigUpdate {
    Mode(LocalSearchConfigMode),
    Replacement(String),
    Search(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct YesNoPrompt {
    pub(crate) title: String,
    pub(crate) yes: Box<Dispatch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FilePickerKind {
    NonGitIgnored,
    GitStatus(git::DiffMode),
    Opened,
}
impl FilePickerKind {
    pub(crate) fn display(&self) -> String {
        match self {
            FilePickerKind::NonGitIgnored => "Not Git Ignored".to_string(),
            FilePickerKind::GitStatus(diff_mode) => format!("Git Status ({})", diff_mode.display()),
            FilePickerKind::Opened => "Opened".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestParams {
    pub(crate) path: CanonicalizedPath,
    pub(crate) position: Position,
    pub(crate) context: ResponseContext,
}
impl RequestParams {
    pub(crate) fn set_kind(self, scope: Option<Scope>) -> Self {
        Self {
            context: ResponseContext {
                scope,
                ..self.context
            },
            ..self
        }
    }

    pub(crate) fn set_description(self, description: &str) -> Self {
        Self {
            context: ResponseContext {
                description: Some(description.to_string()),
                ..self.context
            },
            ..self
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, Copy)]
pub(crate) enum Scope {
    Local,
    Global,
}

#[derive(Debug)]
pub(crate) enum AppMessage {
    LspNotification(LspNotification),
    Event(Event),
    QuitAll,
    SyntaxHighlightResponse {
        component_id: ComponentId,
        highlighted_spans: HighlighedSpans,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MakeFilterMechanism {
    Literal,
    Regex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DispatchPrompt {
    PushFilter {
        kind: FilterKind,
        target: FilterTarget,
        make_mechanism: MakeFilterMechanism,
    },
    GlobalSearchConfigSetGlob {
        filter_glob: GlobalSearchFilterGlob,
    },
    MoveSelectionByIndex,
    RenameSymbol,
    UpdateLocalSearchConfigSearch {
        scope: Scope,
        show_config_after_enter: bool,
    },
    AddPath,
    MovePath {
        from: CanonicalizedPath,
    },
    Null,
    // TODO: remove the following variants
    // Because the following action already embeds dispatches
    SelectSymbol {
        symbols: Symbols,
    },
    RunCommand,
    OpenFile {
        working_directory: CanonicalizedPath,
    },
    UpdateLocalSearchConfigReplacement {
        scope: Scope,
    },
    #[cfg(test)]
    SetContent,
}
impl DispatchPrompt {
    pub(crate) fn to_dispatches(&self, text: &str) -> anyhow::Result<Dispatches> {
        match self.clone() {
            DispatchPrompt::PushFilter {
                kind,
                target,
                make_mechanism: mechanism,
            } => {
                let mechanism = match mechanism {
                    MakeFilterMechanism::Literal => FilterMechanism::Literal(text.to_string()),
                    MakeFilterMechanism::Regex => FilterMechanism::Regex(regex::Regex::new(text)?),
                };
                Ok(Dispatches::new(
                    [Dispatch::ToEditor(FilterPush(Filter::new(
                        kind, target, mechanism,
                    )))]
                    .to_vec(),
                ))
            }
            DispatchPrompt::GlobalSearchConfigSetGlob { filter_glob } => Ok(Dispatches::new(
                [Dispatch::UpdateGlobalSearchConfig {
                    update: GlobalSearchConfigUpdate::SetGlob(filter_glob, text.to_string()),
                }]
                .to_vec(),
            )),
            DispatchPrompt::MoveSelectionByIndex => {
                let index = text.parse::<usize>()?.saturating_sub(1);
                Ok(Dispatches::new(
                    [Dispatch::ToEditor(MoveSelection(Movement::Index(index)))].to_vec(),
                ))
            }
            DispatchPrompt::RenameSymbol => Ok(Dispatches::new(vec![Dispatch::RenameSymbol {
                new_name: text.to_string(),
            }])),
            DispatchPrompt::UpdateLocalSearchConfigSearch {
                scope,
                show_config_after_enter,
            } => Ok(Dispatches::new(
                [Dispatch::UpdateLocalSearchConfig {
                    update: LocalSearchConfigUpdate::Search(text.to_string()),
                    scope,
                    show_config_after_enter,
                }]
                .to_vec(),
            )),
            DispatchPrompt::AddPath => {
                Ok(Dispatches::new([Dispatch::AddPath(text.into())].to_vec()))
            }
            DispatchPrompt::MovePath { from } => Ok(Dispatches::new(
                [Dispatch::MoveFile {
                    from,
                    to: text.into(),
                }]
                .to_vec(),
            )),
            DispatchPrompt::SelectSymbol { symbols } => {
                // TODO: make Prompt generic over the item type,
                // so that we don't have to do this,
                // i.e. we can just return the symbol directly,
                // instead of having to find it again.
                if let Some(symbol) = symbols
                    .symbols
                    .iter()
                    .find(|symbol| text == symbol.display())
                {
                    Ok(Dispatches::new(vec![Dispatch::GotoLocation(
                        symbol.location.clone(),
                    )]))
                } else {
                    Ok(Dispatches::new(vec![]))
                }
            }
            DispatchPrompt::RunCommand => Ok(Dispatches::new(
                [Dispatch::RunCommand(text.to_string())]
                    .into_iter()
                    .collect(),
            )),
            DispatchPrompt::OpenFile { working_directory } => {
                let path = working_directory.join(text)?;
                Ok(Dispatches::new(vec![Dispatch::OpenFile(path)]))
            }
            DispatchPrompt::UpdateLocalSearchConfigReplacement { scope } => Ok(Dispatches::new(
                [Dispatch::UpdateLocalSearchConfig {
                    scope,
                    update: LocalSearchConfigUpdate::Replacement(text.to_owned()),
                    show_config_after_enter: true,
                }]
                .to_vec(),
            )),
            #[cfg(test)]
            DispatchPrompt::SetContent => Ok(Dispatches::new(
                [Dispatch::ToEditor(SetContent(text.to_string()))].to_vec(),
            )),
            DispatchPrompt::Null => Ok(Default::default()),
        }
    }
}

#[derive(PartialEq)]
enum OpenFileOption {
    Focus,
    FocusNoHistory,
    Background,
}
impl OpenFileOption {
    fn is_focus(&self) -> bool {
        self != &OpenFileOption::Background
    }

    fn store_history(&self) -> bool {
        self == &OpenFileOption::Focus
    }
}
