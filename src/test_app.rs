/// NOTE: all test cases that involves the clipboard should not be run in parallel
///   otherwise the the test suite will fail because multiple tests are trying to
///   access the clipboard at the same time.
#[cfg(test)]
use itertools::Itertools;

use lsp_types::Url;
use my_proc_macros::{hex, key, keys};

use serial_test::serial;

use std::{
    ops::Range,
    path::PathBuf,
    sync::{Arc, Mutex},
};
pub(crate) use Dispatch::*;
pub(crate) use DispatchEditor::*;

pub(crate) use Movement::*;
pub(crate) use SelectionMode::*;

use shared::canonicalized_path::CanonicalizedPath;

use crate::{
    app::{App, Dimension, Dispatch, LocalSearchConfigUpdate, Scope},
    char_index_range::CharIndexRange,
    components::{
        component::Component,
        editor::{Direction, DispatchEditor, Mode, Movement, ViewAlignment},
        suggestive_editor::{DispatchSuggestiveEditor, Info, SuggestiveEditorFilter},
    },
    context::{GlobalMode, LocalSearchConfigMode},
    frontend::mock::MockFrontend,
    grid::StyleKey,
    integration_test::TestRunner,
    list::grep::RegexConfig,
    lsp::{
        code_action::CodeAction,
        completion::{
            Completion, CompletionItem, CompletionItemEdit, CompletionSource, PositionalEdit,
        },
        documentation::Documentation,
        signature_help::SignatureInformation,
        workspace_edit::{TextDocumentEdit, WorkspaceEdit},
    },
    position::Position,
    quickfix_list::{DiagnosticSeverityRange, Location, QuickfixListItem},
    selection::SelectionMode,
    style::Style,
    themes::Theme,
    ui_tree::ComponentKind,
};
use crate::{lsp::process::LspNotification, themes::Color};

pub(crate) enum Step {
    App(Dispatch),
    ExpectMulti(Vec<ExpectKind>),
    Expect(ExpectKind),
    Editor(DispatchEditor),
    SuggestiveEditor(DispatchSuggestiveEditor),
    ExpectLater(Box<dyn Fn() -> ExpectKind>),
    ExpectCustom(Box<dyn Fn()>),
}

#[derive(Debug)]
pub(crate) enum ExpectKind {
    FileExplorerContent(String),
    EditorInfoContent(&'static str),
    EditorInfoOpen(bool),
    QuickfixListCurrentLine(&'static str),
    DropdownInfosCount(usize),
    QuickfixListContent(String),
    CompletionDropdownContent(&'static str),
    CompletionDropdownIsOpen(bool),
    CompletionDropdownSelectedItem(&'static str),
    JumpChars(&'static [char]),
    CurrentLine(&'static str),
    Not(Box<ExpectKind>),
    CurrentComponentContent(&'static str),
    EditorCursorPosition(Position),
    EditorGridCursorPosition(Position),
    CurrentMode(Mode),
    FileContent(CanonicalizedPath, String),
    FileContentEqual(CanonicalizedPath, CanonicalizedPath),
    CurrentSelectedTexts(&'static [&'static str]),
    CurrentCursorDirection(Direction),
    CurrentViewAlignment(Option<ViewAlignment>),
    ComponentsLength(usize),
    Quickfixes(Box<[QuickfixListItem]>),
    AppGrid(String),
    AppGridContains(&'static str),
    EditorGrid(&'static str),
    CurrentPath(CanonicalizedPath),
    GridCellBackground(
        /*Row*/ usize,
        /*Column*/ usize,
        /*Background color*/ Color,
    ),
    GridCellLine(/*Row*/ usize, /*Column*/ usize, Color),
    GridCellStyleKey(Position, Option<StyleKey>),
    HighlightSpans(std::ops::Range<usize>, StyleKey),
    DiagnosticsRanges(Vec<CharIndexRange>),
    BufferQuickfixListItems(Vec<Range<Position>>),
    ComponentCount(usize),
    CurrentComponentPath(Option<CanonicalizedPath>),
    OpenedFilesCount(usize),
    QuickfixListInfo(&'static str),
    ComponentsOrder(Vec<ComponentKind>),
    CurrentComponentTitle(&'static str),
    CurrentSelectionMode(SelectionMode),
}
fn log<T: std::fmt::Debug>(s: T) {
    println!("===========\n{s:?}",);
}
impl ExpectKind {
    fn run(&self, app: &mut App<MockFrontend>) {
        log(self);
        let (result, context) = self.get_result(app).unwrap();
        assert!(result, "{context}",)
    }
    fn get_result(&self, app: &mut App<MockFrontend>) -> anyhow::Result<(bool, String)> {
        let context = app.context();
        fn contextualize<T: PartialEq + std::fmt::Debug>(a: T, b: T) -> (bool, String) {
            (a == b, format!("\n{a:?}\n == \n{b:?}\n",))
        }
        fn to_vec(strs: &[&str]) -> Vec<String> {
            strs.iter().map(|t| t.to_string()).collect()
        }
        let component = app.current_component();
        Ok(match self {
            CurrentComponentContent(expected_content) => contextualize(
                app.get_current_component_content(),
                expected_content.to_string(),
            ),
            FileContent(path, expected_content) => {
                contextualize(app.get_file_content(path), expected_content.clone())
            }
            FileContentEqual(left, right) => {
                contextualize(app.get_file_content(left), app.get_file_content(right))
            }
            CurrentSelectedTexts(selected_texts) => {
                contextualize(app.get_current_selected_texts(), to_vec(selected_texts))
            }
            ComponentsLength(length) => contextualize(app.components().len(), *length),
            Quickfixes(expected_quickfixes) => contextualize(
                app.get_quickfix_list()
                    .map(|q| {
                        q.items()
                            .into_iter()
                            .map(|quickfix| {
                                let info = quickfix
                                    .info()
                                    .as_ref()
                                    .map(|info| info.clone().set_decorations(Vec::new()));
                                quickfix.set_info(info)
                            })
                            .collect_vec()
                            .into_boxed_slice()
                    })
                    .unwrap_or_default(),
                expected_quickfixes.clone(),
            ),
            EditorGrid(grid) => contextualize(
                component.borrow().editor().get_grid(context).to_string(),
                grid.to_string(),
            ),
            AppGrid(grid) => {
                let actual = app.get_screen()?.stringify().trim_matches('\n').to_string();
                println!("actual =\n{}", actual);
                contextualize(actual, grid.to_string().trim_matches('\n').to_string())
            }
            CurrentPath(path) => contextualize(app.get_current_file_path().unwrap(), path.clone()),
            Not(expect_kind) => {
                let (result, context) = expect_kind.get_result(app)?;
                (!result, format!("NOT ({context})"))
            }
            CurrentMode(mode) => contextualize(&component.borrow().editor().mode, mode),
            EditorCursorPosition(position) => contextualize(
                &component.borrow().editor().get_cursor_position().unwrap(),
                position,
            ),
            EditorGridCursorPosition(position) => contextualize(
                component
                    .borrow()
                    .editor()
                    .get_grid(context)
                    .cursor
                    .unwrap()
                    .position(),
                position,
            ),
            CurrentLine(line) => contextualize(
                component.borrow().editor().current_line().unwrap(),
                line.to_string(),
            ),
            JumpChars(chars) => {
                contextualize(component.borrow().editor().jump_chars(), chars.to_vec())
            }
            CurrentViewAlignment(view_alignment) => contextualize(
                component.borrow().editor().current_view_alignment(),
                *view_alignment,
            ),
            GridCellBackground(row_index, column_index, background_color) => contextualize(
                component.borrow().editor().get_grid(context).grid.rows[*row_index][*column_index]
                    .background_color,
                *background_color,
            ),
            GridCellLine(row_index, column_index, underline_color) => contextualize(
                component.borrow().editor().get_grid(context).grid.rows[*row_index][*column_index]
                    .line
                    .unwrap()
                    .color,
                *underline_color,
            ),
            GridCellStyleKey(position, style_key) => contextualize(
                component.borrow().editor().get_grid(context).grid.rows[position.line]
                    [position.column]
                    .source
                    .clone(),
                style_key.clone(),
            ),
            CompletionDropdownIsOpen(is_open) => {
                contextualize(app.completion_dropdown_is_open(), *is_open)
            }
            CompletionDropdownContent(content) => contextualize(
                app.current_completion_dropdown()
                    .unwrap()
                    .borrow()
                    .content(),
                content.to_string(),
            ),
            CompletionDropdownSelectedItem(item) => contextualize(
                app.current_completion_dropdown()
                    .unwrap()
                    .borrow()
                    .editor()
                    .get_selected_texts()[0]
                    .trim(),
                item,
            ),
            QuickfixListContent(content) => {
                let expected = app.get_quickfix_list().unwrap().render().content;
                let actual = content.to_string();
                println!("expected =\n{expected}");
                println!("actual =\n{actual}");
                contextualize(expected, actual)
            }
            DropdownInfosCount(expected) => {
                contextualize(app.get_dropdown_infos_count(), *expected)
            }
            QuickfixListCurrentLine(expected) => {
                let component = app
                    .get_component_by_kind(ComponentKind::QuickfixList)
                    .unwrap();
                let actual = component.borrow().editor().current_line().unwrap();
                contextualize(actual, expected.to_string())
            }
            EditorInfoOpen(expected) => contextualize(app.editor_info_open(), *expected),
            EditorInfoContent(expected) => {
                contextualize(app.editor_info_content(), Some(expected.to_string()))
            }
            AppGridContains(substring) => {
                let content = app.get_screen().unwrap().stringify();
                println!("content =\n{}", content);
                contextualize(content.contains(substring), true)
            }
            FileExplorerContent(expected) => contextualize(expected, &app.file_explorer_content()),
            CurrentCursorDirection(expected) => contextualize(
                expected,
                &app.current_component().borrow().editor().cursor_direction,
            ),
            HighlightSpans(expected_range, expected_key) => contextualize(
                expected_key,
                &app.current_component()
                    .borrow()
                    .editor()
                    .buffer()
                    .highlighted_spans()
                    .into_iter()
                    .find(|span| &span.byte_range == expected_range)
                    .unwrap()
                    .style_key,
            ),
            DiagnosticsRanges(expected) => contextualize(
                expected.to_vec(),
                app.current_component()
                    .borrow()
                    .editor()
                    .buffer()
                    .diagnostics()
                    .into_iter()
                    .map(|d| d.range)
                    .collect_vec(),
            ),
            BufferQuickfixListItems(expected) => contextualize(
                expected,
                &app.current_component()
                    .borrow()
                    .editor()
                    .buffer()
                    .quickfix_list_items()
                    .into_iter()
                    .map(|d| d.location().range.clone())
                    .collect_vec(),
            ),
            ComponentCount(expected) => contextualize(expected, &app.components().len()),
            CurrentComponentPath(expected) => {
                contextualize(expected, &app.current_component().borrow().path())
            }
            OpenedFilesCount(expected) => contextualize(expected, &app.opened_files_count()),
            QuickfixListInfo(expected) => {
                contextualize(*expected, &app.quickfix_list_info().unwrap())
            }
            ComponentsOrder(expected) => contextualize(expected, &app.components_order()),
            CurrentComponentTitle(expected) => {
                contextualize(*expected, &app.current_component().borrow().title(context))
            }
            CurrentSelectionMode(expected) => contextualize(
                expected,
                &app.current_component().borrow().editor().selection_set.mode,
            ),
        })
    }
}

pub(crate) use ExpectKind::*;
pub(crate) use Step::*;
pub(crate) struct State {
    temp_dir: CanonicalizedPath,
    main_rs: CanonicalizedPath,
    foo_rs: CanonicalizedPath,
    git_ignore: CanonicalizedPath,
}
impl State {
    pub(crate) fn main_rs(&self) -> CanonicalizedPath {
        self.main_rs.clone()
    }

    pub(crate) fn foo_rs(&self) -> CanonicalizedPath {
        self.foo_rs.clone()
    }

    pub(crate) fn new_path(&self, path: &str) -> PathBuf {
        self.temp_dir.to_path_buf().join(path)
    }

    pub(crate) fn gitignore(&self) -> CanonicalizedPath {
        self.git_ignore.clone()
    }

    pub(crate) fn temp_dir(&self) -> CanonicalizedPath {
        self.temp_dir.clone()
    }
}

pub(crate) fn execute_test(callback: impl Fn(State) -> Box<[Step]>) -> anyhow::Result<()> {
    run_test(|mut app, temp_dir| {
        let steps = {
            callback(State {
                main_rs: temp_dir.join("src/main.rs").unwrap(),
                foo_rs: temp_dir.join("src/foo.rs").unwrap(),
                git_ignore: temp_dir.join(".gitignore").unwrap(),
                temp_dir,
            })
        };

        for step in steps.iter() {
            match step.to_owned() {
                Step::App(dispatch) => {
                    log(dispatch);
                    app.handle_dispatch(dispatch.to_owned())?
                }
                Step::Expect(expect_kind) => expect_kind.run(&mut app),
                ExpectLater(f) => f().run(&mut app),
                Editor(dispatch) => {
                    log(dispatch);
                    app.handle_dispatch_editor(dispatch.to_owned())?
                }
                ExpectCustom(f) => {
                    f();
                }
                ExpectMulti(expect_kinds) => {
                    for expect_kind in expect_kinds.iter() {
                        expect_kind.run(&mut app)
                    }
                }
                SuggestiveEditor(dispatch) => {
                    log(dispatch);
                    app.handle_dispatch_suggestive_editor(dispatch.to_owned())?
                }
            };
        }
        Ok(())
    })
}

fn run_test(
    callback: impl Fn(App<MockFrontend>, CanonicalizedPath) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    TestRunner::run(|temp_dir| {
        let mock_frontend = Arc::new(Mutex::new(MockFrontend::default()));
        let mut app = App::new(mock_frontend, temp_dir.clone())?;
        app.disable_lsp();
        callback(app, temp_dir)
    })
}

#[test]
#[serial]
fn copy_replace_from_different_file() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            App(OpenFile(s.foo_rs())),
            Editor(SetSelectionMode(LineTrimmed)),
            Editor(SelectAll),
            Editor(Copy),
            App(OpenFile(s.foo_rs())),
            Editor(SetSelectionMode(LineTrimmed)),
            Editor(SelectAll),
            Editor(Copy),
            App(OpenFile(s.main_rs())),
            Editor(SetSelectionMode(LineTrimmed)),
            Editor(SelectAll),
            Editor(ReplaceWithCopiedText),
            Expect(FileContentEqual(s.main_rs, s.foo_rs)),
        ])
    })
}

#[test]
#[serial]
/// Should work across different files (via system clipboard)
fn replace_cut() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent("fn main() { call_main() }".to_string())),
            App(OpenFile(s.foo_rs())),
            Editor(SetContent("fn foo() { call_foo() }".to_string())),
            Editor(MatchLiteral("call_foo()".to_string())),
            Editor(Copy),
            App(OpenFile(s.main_rs())),
            Editor(MatchLiteral("call_main()".to_string())),
            Editor(ReplaceCut),
            Expect(CurrentComponentContent("fn main() { call_foo() }")),
            App(OpenFile(s.foo_rs())),
            Editor(ReplaceCut),
            Expect(CurrentComponentContent("fn foo() { call_main() }")),
        ])
    })
}

#[test]
#[serial]
fn copy_replace() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent("fn main() { let x = 1; }".to_string())),
            Editor(SetSelectionMode(SelectionMode::Token)),
            Editor(Copy),
            Editor(MoveSelection(Movement::Next)),
            Editor(ReplaceWithCopiedText),
            Expect(CurrentComponentContent("fn fn() { let x = 1; }")),
            Expect(CurrentSelectedTexts(&["fn"])),
            Editor(MoveSelection(Next)),
            Editor(ReplaceWithCopiedText),
            Expect(CurrentComponentContent("fn fnfn) { let x = 1; }")),
        ])
    })
}

#[test]
#[serial]
fn cut_replace() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent("fn main() { let x = 1; }".to_string())),
            Editor(SetSelectionMode(Token)),
            Editor(Change { cut: true }),
            Editor(EnterNormalMode),
            Expect(CurrentComponentContent(" main() { let x = 1; }")),
            Editor(MoveSelection(Current)),
            Expect(CurrentSelectedTexts(&["main"])),
            Editor(ReplaceWithCopiedText),
            Expect(CurrentComponentContent(" fn() { let x = 1; }")),
        ])
    })
}

#[test]
#[serial]
fn highlight_mode_cut() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent(
                "fn f(){ let x = S(a); let y = S(b); }".to_string(),
            )),
            Editor(SetSelectionMode(Token)),
            Editor(ToggleVisualMode),
            Editor(MoveSelection(Next)),
            Editor(MoveSelection(Next)),
            Editor(MoveSelection(Next)),
            Expect(CurrentSelectedTexts(&["fn f()"])),
            Editor(Change { cut: true }),
            Expect(CurrentComponentContent("{ let x = S(a); let y = S(b); }")),
            Editor(ReplaceWithCopiedText),
            Expect(CurrentComponentContent(
                "fn f(){ let x = S(a); let y = S(b); }",
            )),
        ])
    })
}

#[test]
#[serial]
fn highlight_mode_copy() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent(
                "fn f(){ let x = S(a); let y = S(b); }".to_string(),
            )),
            Editor(SetSelectionMode(SelectionMode::Token)),
            Editor(ToggleVisualMode),
            Editor(MoveSelection(Movement::Next)),
            Editor(MoveSelection(Movement::Next)),
            Editor(MoveSelection(Movement::Next)),
            Expect(CurrentSelectedTexts(&["fn f()"])),
            Editor(Copy),
            Editor(Reset),
            Editor(MoveSelection(Next)),
            Expect(CurrentSelectedTexts(&["{"])),
            Editor(ReplaceWithCopiedText),
            Expect(CurrentComponentContent(
                "fn f()fn f() let x = S(a); let y = S(b); }",
            )),
        ])
    })
}

#[test]
#[serial]
fn highlight_mode_replace() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent(
                "fn f(){ let x = S(a); let y = S(b); }".to_string(),
            )),
            Editor(SetSelectionMode(SelectionMode::Token)),
            Editor(ToggleVisualMode),
            Editor(MoveSelection(Movement::Next)),
            Editor(MoveSelection(Movement::Next)),
            Editor(MoveSelection(Movement::Next)),
            Expect(CurrentSelectedTexts(&["fn f()"])),
            Editor(Copy),
            Editor(Reset),
            Editor(MatchLiteral("{".to_string())),
            Editor(SetSelectionMode(SelectionMode::SyntaxNodeCoarse)),
            Expect(CurrentSelectedTexts(&["{ let x = S(a); let y = S(b); }"])),
            Editor(ReplaceWithCopiedText),
            Expect(CurrentComponentContent("fn f()fn f()")),
        ])
    })
}

#[test]
#[serial]
fn multi_paste() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Expect(CurrentComponentPath(Some(s.main_rs()))),
            Editor(SetContent(
                "fn f(){ let x = S(spongebob_squarepants); let y = S(b); }".to_string(),
            )),
            Editor(MatchLiteral("let x = S(spongebob_squarepants);".to_owned())),
            Editor(SetSelectionMode(SelectionMode::SyntaxNodeCoarse)),
            Expect(CurrentSelectedTexts(&["let x = S(spongebob_squarepants);"])),
            Editor(CursorAddToAllSelections),
            Editor(MoveSelection(Movement::FirstChild)),
            Editor(MoveSelection(Movement::Next)),
            Expect(CurrentSelectedTexts(&["S(spongebob_squarepants)", "S(b)"])),
            Editor(Change { cut: true }),
            Editor(EnterInsertMode(Direction::Start)),
            Editor(Insert("Some(".to_owned())),
            Editor(Paste(Direction::End)),
            Editor(Insert(")".to_owned())),
            Expect(CurrentComponentContent(
                "fn f(){ let x = Some(S(spongebob_squarepants)); let y = Some(S(b)); }",
            )),
            Editor(CursorKeepPrimaryOnly),
            App(SetClipboardContent(".hello".to_owned())),
            Expect(CurrentMode(Mode::Insert)),
            Editor(Paste(Direction::End)),
            Expect(CurrentComponentContent(
                "fn f(){ let x = Some(S(spongebob_squarepants)).hello; let y = Some(S(b)); }",
            )),
        ])
    })
}

#[test]
fn signature_help() -> anyhow::Result<()> {
    execute_test(|s| {
        fn signature_help() -> LspNotification {
            LspNotification::SignatureHelp(Some(crate::lsp::signature_help::SignatureHelp {
                signatures: [SignatureInformation {
                    label: "Signature Help".to_string(),
                    documentation: Some(crate::lsp::documentation::Documentation {
                        content: "spongebob".to_string(),
                    }),
                    active_parameter_byte_range: None,
                }]
                .to_vec(),
            }))
        }
        Box::new([
            App(OpenFile(s.main_rs())),
            Expect(ComponentsLength(1)),
            Editor(SetContent(
                "fn f(){ let x = S(a); let y = S(b); }".to_string(),
            )),
            Editor(SetSelectionMode(SelectionMode::Token)),
            Expect(CurrentMode(Mode::Normal)),
            //
            // Signature help should not be shown in normal mode
            App(HandleLspNotification(signature_help())),
            Expect(ExpectKind::ComponentsOrder(vec![
                ComponentKind::SuggestiveEditor,
            ])),
            //
            // Signature help should only be shown in insert mode
            Editor(EnterInsertMode(Direction::End)),
            App(HandleLspNotification(signature_help())),
            Expect(ExpectKind::ComponentsOrder(vec![
                ComponentKind::SuggestiveEditor,
                ComponentKind::EditorInfo,
            ])),
            //
            // Receiving signature help again should increase the components length
            App(HandleLspNotification(signature_help())),
            Expect(ExpectKind::ComponentsOrder(vec![
                ComponentKind::SuggestiveEditor,
                ComponentKind::EditorInfo,
            ])),
            //
            // Pressing esc should close signature help
            App(HandleKeyEvent(key!("esc"))),
            Expect(ExpectKind::ComponentsOrder(vec![
                ComponentKind::SuggestiveEditor,
            ])),
            //
            // Receiving null signature help should close the signature help
            Editor(EnterInsertMode(Direction::End)),
            App(HandleLspNotification(signature_help())),
            Expect(ExpectKind::ComponentsOrder(vec![
                ComponentKind::SuggestiveEditor,
                ComponentKind::EditorInfo,
            ])),
            App(HandleLspNotification(LspNotification::SignatureHelp(None))),
            Expect(ExpectKind::ComponentsOrder(vec![
                ComponentKind::SuggestiveEditor,
            ])),
        ])
    })
}

#[test]
pub(crate) fn repo_git_hunks() -> Result<(), anyhow::Error> {
    execute_test(|s| {
        let path_new_file = s.new_path("new_file.md");
        fn strs_to_strings(strs: &[&str]) -> Option<Info> {
            Some(Info::new(
                "Git Hunk Diff".to_string(),
                strs.iter().map(|s| s.to_string()).join("\n").to_string(),
            ))
        }

        Box::new([
            // Delete the first line of main.rs
            App(OpenFile(s.main_rs().clone())),
            Editor(SetSelectionMode(LineTrimmed)),
            Editor(Delete { backward: false }),
            // Insert a comment at the first line of foo.rs
            App(OpenFile(s.foo_rs().clone())),
            Editor(Insert("// Hello".to_string())),
            // Save the files,
            App(SaveAll),
            // Add a new file
            App(AddPath(path_new_file.display().to_string())),
            // Get the repo hunks
            App(GetRepoGitHunks(
                crate::git::DiffMode::UnstagedAgainstCurrentBranch,
            )),
            Step::ExpectLater(Box::new(move || {
                Quickfixes(Box::new([
                    QuickfixListItem::new(
                        Location {
                            path: path_new_file.clone().try_into().unwrap(),
                            range: Position { line: 0, column: 0 }..Position { line: 0, column: 0 },
                        },
                        strs_to_strings(&["[This file is untracked or renamed]"]),
                    ),
                    QuickfixListItem::new(
                        Location {
                            path: s.foo_rs(),
                            range: Position { line: 0, column: 0 }..Position { line: 1, column: 0 },
                        },
                        strs_to_strings(&[
                            "pub(crate) struct Foo {",
                            "// Hellopub(crate) struct Foo {",
                        ]),
                    ),
                    QuickfixListItem::new(
                        Location {
                            path: s.main_rs(),
                            range: Position { line: 0, column: 0 }..Position { line: 0, column: 0 },
                        },
                        strs_to_strings(&["mod foo;"]),
                    ),
                ]))
            })),
        ])
    })
}

#[test]
pub(crate) fn non_git_ignored_files() -> Result<(), anyhow::Error> {
    execute_test(|s| {
        let temp_dir = s.temp_dir();
        Box::new([
            // Ignore *.txt files
            App(OpenFile(s.gitignore())),
            Editor(Insert("*.txt\n".to_string())),
            App(SaveAll),
            // Add new txt file
            App(AddPath(s.new_path("temp.txt").display().to_string())),
            // Add a hidden file
            App(AddPath(s.new_path(".bashrc").display().to_string())),
            // Add a file under `.git` folder
            App(AddPath(s.new_path(".git/hello").display().to_string())),
            // Add a new Rust file
            App(AddPath(s.new_path("src/rust.rs").display().to_string())),
            ExpectCustom(Box::new(move || {
                let paths = crate::list::WalkBuilderConfig::non_git_ignored_files(temp_dir.clone())
                    .unwrap();

                // Expect all the paths are files, not directory for example
                assert!(paths.iter().all(|file| file.is_file()));

                let paths = paths
                    .into_iter()
                    .flat_map(|path| {
                        CanonicalizedPath::try_from(path)
                            .unwrap()
                            .display_relative_to(&s.temp_dir())
                    })
                    .collect_vec();

                // Expect "temp.txt" is not in the list, since it is git-ignored
                assert!(!paths.contains(&"temp.txt".to_string()));

                // Expect the unstaged file "src/rust.rs" is in the list
                assert!(paths.contains(&"src/rust.rs".to_string()));

                // Expect the staged file "main.rs" is in the list
                assert!(paths.contains(&"src/main.rs".to_string()));

                // Expect the hidden file ".bashrc" is in the list
                assert!(paths.contains(&".bashrc".to_string()));

                // Expect files under ".git" is ignored
                assert!(!paths.contains(&".git/hello".to_string()));
            })),
        ])
    })
}

#[test]
fn align_view_bottom_with_outbound_parent_lines() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(SetGlobalTitle("[GLOBAL TITLE]".to_string())),
            App(OpenFile(s.main_rs())),
            App(TerminalDimensionChanged(Dimension {
                width: 200,
                height: 6,
            })),
            Editor(SetSelectionMode(LineTrimmed)),
            Editor(SelectAll),
            Editor(Delete { backward: false }),
            Editor(Insert(
                "
fn first () {
  second();
  third();
  fourth(); // this line is long
  fifth();
}"
                .trim()
                .to_string(),
            )),
            Editor(MatchLiteral("fifth()".to_string())),
            Editor(AlignViewTop),
            Expect(AppGrid(
                "
 🦀  src/main.rs
1│fn first () {
5│  █ifth();
6│}

[GLOBAL TITLE]
"
                .to_string(),
            )),
            Editor(AlignViewBottom),
            Expect(AppGrid(
                "
 🦀  src/main.rs
1│fn first () {
3│  third();
4│  fourth(); // this line is long
5│  █ifth();
[GLOBAL TITLE]
"
                .to_string(),
            )),
            // Resize the terminal dimension sucht that the fourth line will be wrapped
            App(TerminalDimensionChanged(Dimension {
                width: 20,
                height: 6,
            })),
            Editor(AlignViewBottom),
            Expect(AppGrid(
                "
 🦀  src/main.rs
1│fn first () {
4│  fourth(); //
↪│this line is long
5│  █ifth();
[GLOBAL TITLE]
"
                .to_string(),
            )),
        ])
    })
}

#[test]
fn global_bookmarks() -> Result<(), anyhow::Error> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetSelectionMode(WordShort)),
            Editor(ToggleBookmark),
            App(OpenFile(s.foo_rs())),
            Editor(SetSelectionMode(WordShort)),
            Editor(ToggleBookmark),
            App(SetQuickfixList(
                crate::quickfix_list::QuickfixListType::Bookmark,
            )),
            Expect(Quickfixes(Box::new([
                QuickfixListItem::new(
                    Location {
                        path: s.foo_rs(),
                        range: Position { line: 0, column: 0 }..Position { line: 0, column: 3 },
                    },
                    None,
                ),
                QuickfixListItem::new(
                    Location {
                        path: s.main_rs(),
                        range: Position { line: 0, column: 0 }..Position { line: 0, column: 3 },
                    },
                    None,
                ),
            ]))),
        ])
    })
}

#[test]
fn local_lsp_references() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent(
                "fn f(){ let x = S(spongebob_squarepants); let y = S(b); }".to_string(),
            )),
            App(HandleLspNotification(LspNotification::References(
                crate::lsp::process::ResponseContext {
                    scope: Some(Scope::Local),
                    component_id: Default::default(),
                    description: None,
                },
                [
                    Location {
                        path: s.main_rs(),
                        range: Position { line: 0, column: 0 }..Position { line: 0, column: 2 },
                    },
                    Location {
                        path: s.main_rs(),
                        range: Position { line: 0, column: 3 }..Position { line: 0, column: 4 },
                    },
                ]
                .to_vec(),
            ))),
            Editor(CursorAddToAllSelections),
            Expect(CurrentSelectedTexts(&["fn", "f"])),
        ])
    })
}

#[test]
fn global_diagnostics() -> Result<(), anyhow::Error> {
    execute_test(|s| {
        let publish_diagnostics = |path: CanonicalizedPath| {
            LspNotification::PublishDiagnostics(lsp_types::PublishDiagnosticsParams {
                uri: path.to_url().unwrap(),
                diagnostics: [lsp_types::Diagnostic {
                    range: lsp_types::Range::new(
                        lsp_types::Position {
                            line: 0,
                            character: 0,
                        },
                        lsp_types::Position {
                            line: 0,
                            character: 3,
                        },
                    ),
                    message: "To err is normal, but to err again is not.".to_string(),
                    ..Default::default()
                }]
                .to_vec(),
                version: None,
            })
        };
        Box::new([
            App(OpenFile(s.main_rs())),
            App(HandleLspNotification(publish_diagnostics(s.main_rs()))),
            App(HandleLspNotification(publish_diagnostics(s.foo_rs()))),
            App(SetQuickfixList(
                crate::quickfix_list::QuickfixListType::Diagnostic(DiagnosticSeverityRange::All),
            )),
            Expect(Quickfixes(Box::new([
                QuickfixListItem::new(
                    Location {
                        path: s.foo_rs(),
                        range: Position { line: 0, column: 0 }..Position { line: 0, column: 3 },
                    },
                    Some(Info::new(
                        "Diagnostics".to_string(),
                        "To err is normal, but to err again is not.".to_string(),
                    )),
                ),
                QuickfixListItem::new(
                    Location {
                        path: s.main_rs(),
                        range: Position { line: 0, column: 0 }..Position { line: 0, column: 3 },
                    },
                    Some(Info::new(
                        "Diagnostics".to_string(),
                        "To err is normal, but to err again is not.".to_string(),
                    )),
                ),
            ]))),
        ])
    })
}

fn test_global_search_replace(
    TestGlobalSearchReplaceArgs {
        mode,
        main_content,
        foo_content,
        search,
        replacement,
        main_replaced,
        foo_replaced,
    }: TestGlobalSearchReplaceArgs,
) -> anyhow::Result<()> {
    execute_test(|s| {
        let new_dispatch = |update: LocalSearchConfigUpdate| -> Dispatch {
            UpdateLocalSearchConfig {
                update,
                scope: Scope::Global,
                show_config_after_enter: true,
            }
        };
        let main_rs = s.main_rs();
        Box::new([
            App(OpenFile(s.foo_rs())),
            Editor(SetContent(foo_content.to_string())),
            App(OpenFile(s.main_rs())),
            Editor(SetContent(main_content.to_string())),
            App(SaveAll),
            App(new_dispatch(LocalSearchConfigUpdate::Mode(mode))),
            App(new_dispatch(LocalSearchConfigUpdate::Search(
                search.to_string(),
            ))),
            App(new_dispatch(LocalSearchConfigUpdate::Replacement(
                replacement.to_string(),
            ))),
            App(Dispatch::Replace {
                scope: Scope::Global,
            }),
            Expect(FileContent(s.main_rs(), main_replaced.to_string())),
            Expect(FileContent(s.foo_rs(), foo_replaced.to_string())),
            // Expect the main.rs buffer to be updated as well
            ExpectLater(Box::new(move || {
                FileContent(main_rs.clone(), main_rs.read().unwrap())
            })),
            // Apply undo to main_rs
            App(OpenFile(s.main_rs())),
            Editor(Undo),
            // Expect the content of the main.rs buffer to be reverted
            Expect(FileContent(s.main_rs(), main_content.to_string())),
        ])
    })
}
struct TestGlobalSearchReplaceArgs {
    mode: LocalSearchConfigMode,
    main_content: &'static str,
    foo_content: &'static str,
    search: &'static str,
    replacement: &'static str,
    main_replaced: &'static str,
    foo_replaced: &'static str,
}

#[test]
fn global_search_replace_regex() -> Result<(), anyhow::Error> {
    test_global_search_replace(TestGlobalSearchReplaceArgs {
        mode: LocalSearchConfigMode::Regex(RegexConfig {
            escaped: true,
            case_sensitive: false,
            match_whole_word: false,
        }),
        main_content: "main foo",
        foo_content: "foo foo",
        search: "foo",
        replacement: "haha",
        main_replaced: "main haha",
        foo_replaced: "haha haha",
    })
}

#[test]
fn global_search_replace_ast_grep() -> Result<(), anyhow::Error> {
    test_global_search_replace(TestGlobalSearchReplaceArgs {
        mode: LocalSearchConfigMode::AstGrep,
        main_content: "fn main() {\n    let x = a.b.foo();\n}\n",
        foo_content: "fn main() { let x = (1+1).foo(); }",
        search: "$X.foo()",
        replacement: "foo($X)",
        // Note: the replaced content has newline characters because after replacement,
        // the formatter will be applied
        main_replaced: "fn main() {\n    let x = foo(a.b);\n}\n",
        foo_replaced: "fn main() {\n    let x = foo((1 + 1));\n}\n",
    })
}

#[test]
fn global_search_replace_case_agnostic() -> Result<(), anyhow::Error> {
    test_global_search_replace(TestGlobalSearchReplaceArgs {
        mode: LocalSearchConfigMode::CaseAgnostic,
        main_content: "HelloWorld, this is good",
        foo_content: "im-lisp (hello-world and say 'HELLO_WORLD')",
        search: "hello world",
        replacement: "bye sky",
        main_replaced: "ByeSky, this is good",
        foo_replaced: "im-lisp (bye-sky and say 'BYE_SKY')",
    })
}

#[test]
fn quickfix_list() -> Result<(), anyhow::Error> {
    execute_test(|s| {
        let new_dispatch = |update: LocalSearchConfigUpdate| -> Dispatch {
            UpdateLocalSearchConfig {
                update,
                scope: Scope::Global,
                show_config_after_enter: false,
            }
        };
        Box::new([
            App(OpenFile(s.foo_rs())),
            Editor(SetContent(
                "
hello
foo balatuga // Line 2 (this line is purposely made longer than Line 10 to test sorting)







foo a // Line 10
"
                .trim()
                .to_string(),
            )),
            App(OpenFile(s.main_rs())),
            Editor(SetContent("foo d\nfoo c".to_string())),
            App(SaveAll),
            App(new_dispatch(LocalSearchConfigUpdate::Search(
                "foo".to_string(),
            ))),
            Expect(QuickfixListContent(
                // Line 10 should be placed below Line 2 (sorted numerically, not lexicograhically)
                format!(
                    "
■┬ {}
 ├─ 2:1  foo balatuga // Line 2 (this line is purposely made longer than Line 10 to test sorting)
 └─ 10:1  foo a // Line 10

■┬ {}
 ├─ 1:1  foo d
 └─ 2:1  foo c",
                    s.foo_rs().display_absolute(),
                    s.main_rs().display_absolute()
                )
                .trim()
                .to_string(),
            )),
            Expect(QuickfixListCurrentLine("├─ 2:1  foo balatuga // Line 2 (this line is purposely made longer than Line 10 to test sorting)")),
            Expect(CurrentPath(s.foo_rs())),
            Expect(CurrentLine("foo balatuga // Line 2 (this line is purposely made longer than Line 10 to test sorting)")),
            Expect(CurrentSelectedTexts(&["foo"])),
            Expect(ComponentCount(2)),
            Editor(MoveSelection(Next)),
            Expect(ComponentCount(2)),
            Expect(QuickfixListCurrentLine("└─ 10:1  foo a // Line 10")),
            Expect(CurrentLine("foo a // Line 10")),
            Expect(CurrentSelectedTexts(&["foo"])),
            Editor(MoveSelection(Next)),
            Expect(CurrentLine("foo d")),
            Expect(CurrentSelectedTexts(&["foo"])),
            Editor(MoveSelection(Next)),
            Expect(CurrentLine("foo c")),
            Expect(CurrentSelectedTexts(&["foo"])),
            Editor(MoveSelection(Previous)),
            Expect(CurrentLine("foo d")),
            Expect(CurrentSelectedTexts(&["foo"])),
            Editor(MoveSelection(Previous)),
            Expect(CurrentLine("foo a // Line 10")),
            Expect(CurrentSelectedTexts(&["foo"])),
            Editor(MoveSelection(Previous)),
            Expect(CurrentLine("foo balatuga // Line 2 (this line is purposely made longer than Line 10 to test sorting)")),
            Expect(CurrentSelectedTexts(&["foo"])),
        ])
    })
}

#[test]
fn quickfix_list_show_info_if_possible() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent(
                "
fn main() { 
  let x = 123 
}
"
                .trim()
                .to_string(),
            )),
            App(SetQuickfixList(
                crate::quickfix_list::QuickfixListType::Items(
                    [QuickfixListItem::new(
                        Location {
                            path: s.main_rs(),
                            range: Position { line: 1, column: 2 }..Position { line: 1, column: 5 },
                        },
                        Some(Info::new(
                            "Hello world".to_string(),
                            "This is fine".to_string(),
                        )),
                    )]
                    .to_vec(),
                ),
            )),
            App(SetGlobalMode(Some(GlobalMode::QuickfixListItem))),
            Expect(ExpectKind::QuickfixListInfo("This is fine")),
        ])
    })
}

#[test]
fn diagnostic_info() -> Result<(), anyhow::Error> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.foo_rs())),
            App(Dispatch::HandleLspNotification(
                LspNotification::PublishDiagnostics(lsp_types::PublishDiagnosticsParams {
                    uri: Url::from_file_path(s.foo_rs()).unwrap(),
                    diagnostics: [lsp_types::Diagnostic::new_simple(
                        lsp_types::Range::new(
                            lsp_types::Position::new(0, 1),
                            lsp_types::Position::new(0, 2),
                        ),
                        "Hello world".to_string(),
                    )]
                    .to_vec(),
                    version: None,
                }),
            )),
            Editor(SetSelectionMode(Diagnostic(DiagnosticSeverityRange::All))),
            Expect(EditorInfoOpen(true)),
            Expect(EditorInfoContent("Hello world")),
            App(HandleKeyEvent(key!("esc"))),
            Expect(EditorInfoOpen(false)),
            App(Dispatch::HandleLspNotification(
                LspNotification::PublishDiagnostics(lsp_types::PublishDiagnosticsParams {
                    uri: Url::from_file_path(s.foo_rs()).unwrap(),
                    diagnostics: Default::default(),
                    version: None,
                }),
            )),
            Editor(MoveSelection(Next)),
            Expect(EditorInfoOpen(false)),
        ])
    })
}

#[test]
fn diagnostic_severity_decoration_precedence() -> Result<(), anyhow::Error> {
    use lsp_types::DiagnosticSeverity as S;
    let diagnostics = [(1, 2, S::ERROR), (0, 3, S::HINT)];
    let expect = |column: usize, underline_color: Color| {
        GridCellLine(
            // The columns of the following assertions are added by 2,
            // because of line number and the separator between the line number and the
            // content.
            1,
            column + 2,
            underline_color,
        )
    };
    execute_test(|s| {
        let diagnostic =
            |column_start: u32, column_end: u32, severity: lsp_types::DiagnosticSeverity| {
                lsp_types::Diagnostic {
                    range: lsp_types::Range::new(
                        lsp_types::Position::new(0, column_start),
                        lsp_types::Position::new(0, column_end),
                    ),
                    severity: Some(severity),
                    ..Default::default()
                }
            };
        let hint_color = hex!("#abcdef");
        let error_color = hex!("#fedbac");
        let theme = {
            let mut theme = Theme::default();
            theme.diagnostic.hint = Style::default().undercurl(hint_color);
            theme.diagnostic.error = Style::default().undercurl(error_color);
            theme
        };
        Box::new([
            App(OpenFile(s.foo_rs())),
            App(SetTheme(theme.clone())),
            Editor(SetContent(
                "who lives in a pineapple? spongebob squarepants".to_string(),
            )),
            App(TerminalDimensionChanged(Dimension {
                height: 3,
                width: 80,
            })),
            App(Dispatch::HandleLspNotification(
                LspNotification::PublishDiagnostics(lsp_types::PublishDiagnosticsParams {
                    uri: Url::from_file_path(s.foo_rs()).unwrap(),
                    diagnostics: diagnostics
                        .into_iter()
                        .map(|(start, end, severity)| diagnostic(start, end, severity))
                        .collect_vec(),
                    version: None,
                }),
            )),
            ExpectMulti(
                (0..1)
                    .map(|column| expect(column, hint_color))
                    .collect_vec(),
            ),
            ExpectMulti(
                (1..2)
                    .map(|column| expect(column, error_color))
                    .collect_vec(),
            ),
            ExpectMulti(
                (2..3)
                    .map(|column| expect(column, hint_color))
                    .collect_vec(),
            ),
        ])
    })
}

#[test]
fn same_range_diagnostics_should_be_merged() -> Result<(), anyhow::Error> {
    execute_test(|s| {
        let diagnostic = |info: &str| {
            lsp_types::Diagnostic::new_simple(
                lsp_types::Range::new(
                    lsp_types::Position::new(0, 1),
                    lsp_types::Position::new(0, 2),
                ),
                info.to_string(),
            )
        };
        let expected_info = "foo\n=======\nbar\n=======\nspam";
        Box::new([
            App(OpenFile(s.foo_rs())),
            App(Dispatch::HandleLspNotification(
                LspNotification::PublishDiagnostics(lsp_types::PublishDiagnosticsParams {
                    uri: Url::from_file_path(s.foo_rs()).unwrap(),
                    diagnostics: [diagnostic("foo"), diagnostic("bar"), diagnostic("spam")]
                        .to_vec(),
                    version: None,
                }),
            )),
            Editor(SetSelectionMode(Diagnostic(DiagnosticSeverityRange::All))),
            Expect(EditorInfoContent(expected_info)),
            // Expect there's only one diagnostic, by the fact that moving to the first and
            // last diagnostic still renders the same info
            Editor(MoveSelection(First)),
            Expect(EditorInfoContent(expected_info)),
            Editor(MoveSelection(Last)),
            Expect(EditorInfoContent(expected_info)),
        ])
    })
}

#[test]
fn code_action() -> anyhow::Result<()> {
    execute_test(|s| {
        let code_action = |new_text: &str| CodeAction {
            title: format!("Use {}", new_text),
            kind: None,
            edit: Some(WorkspaceEdit {
                edits: [TextDocumentEdit {
                    path: s.main_rs(),
                    edits: [PositionalEdit {
                        range: Position::new(0, 2)..Position::new(0, 6),
                        new_text: new_text.to_string(),
                    }]
                    .to_vec(),
                }]
                .to_vec(),
                resource_operations: Vec::new(),
            }),
            command: None,
        };
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent("a.to_s".to_string())),
            App(ReceiveCodeActions(
                [code_action("to_soup"), code_action("to_string")].to_vec(),
            )),
            App(HandleKeyEvents(keys!("i n g enter").to_vec())),
            Expect(CurrentComponentContent("a.to_string")),
        ])
    })
}

#[test]
fn opening_new_file_should_replace_current_window() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Expect(ExpectKind::ComponentCount(1)),
            App(OpenFile(s.foo_rs())),
            Expect(ExpectKind::ComponentCount(1)),
        ])
    })
}

#[test]
fn should_be_able_to_handle_key_event_even_when_no_file_is_opened() -> anyhow::Result<()> {
    execute_test(|_| {
        Box::new([
            Expect(CurrentComponentContent("")),
            App(HandleKeyEvents(keys!("i h e l l o").to_vec())),
            Expect(CurrentComponentContent("hello")),
        ])
    })
}

#[test]
fn cycle_window() -> anyhow::Result<()> {
    {
        let completion_item = |label: &str, documentation: Option<&str>| CompletionItem {
            source: CompletionSource::Null,
            label: label.to_string(),
            edit: Some(CompletionItemEdit::PositionalEdit(PositionalEdit {
                range: Position::new(0, 0)..Position::new(0, 6),
                new_text: label.to_string(),
            })),
            documentation: documentation.map(Documentation::new),
            sort_text: None,
            kind: None,
            detail: None,
        };
        execute_test(|s| {
            Box::new([
                App(OpenFile(s.main_rs())),
                Editor(SetContent("".to_string())),
                Editor(EnterInsertMode(Direction::Start)),
                SuggestiveEditor(DispatchSuggestiveEditor::CompletionFilter(
                    SuggestiveEditorFilter::CurrentWord,
                )),
                // Pretend that the LSP server returned a completion
                SuggestiveEditor(DispatchSuggestiveEditor::Completion(Completion {
                    source: CompletionSource::Null,
                    trigger_characters: vec![".".to_string()],
                    items: Some(completion_item(
                        "Spongebob squarepants",
                        Some("krabby patty maker"),
                    ))
                    .into_iter()
                    .map(|item| item.into())
                    .collect(),
                })),
                Expect(ComponentCount(3)),
                Editor(Insert("sponge".to_string())),
                Expect(CurrentComponentContent("sponge")),
                App(OtherWindow),
                Expect(ComponentCount(3)),
                Expect(CurrentComponentContent(" Spongebob squarepants")),
                App(OtherWindow),
                Expect(CurrentComponentContent("krabby patty maker")),
                App(OtherWindow),
                Expect(CurrentComponentContent("sponge")),
                App(OtherWindow),
                App(CloseCurrentWindow),
                Expect(CurrentComponentContent("sponge")),
            ])
        })
    }
}

#[test]
fn esc_in_normal_mode_in_suggestive_editor_should_close_all_other_windows() -> anyhow::Result<()> {
    {
        let completion_item = |label: &str, documentation: Option<&str>| CompletionItem {
            source: CompletionSource::Null,
            label: label.to_string(),
            edit: Some(CompletionItemEdit::PositionalEdit(PositionalEdit {
                range: Position::new(0, 0)..Position::new(0, 6),
                new_text: label.to_string(),
            })),
            documentation: documentation.map(Documentation::new),
            sort_text: None,
            kind: None,
            detail: None,
        };
        execute_test(|s| {
            Box::new([
                App(OpenFile(s.main_rs())),
                Editor(SetContent("".to_string())),
                Editor(EnterInsertMode(Direction::Start)),
                SuggestiveEditor(DispatchSuggestiveEditor::CompletionFilter(
                    SuggestiveEditorFilter::CurrentWord,
                )),
                // Pretend that the LSP server returned a completion
                SuggestiveEditor(DispatchSuggestiveEditor::Completion(Completion {
                    source: CompletionSource::Null,
                    trigger_characters: vec![".".to_string()],
                    items: Some(completion_item(
                        "Spongebob squarepants",
                        Some("krabby patty maker"),
                    ))
                    .into_iter()
                    .map(|item| item.into())
                    .collect(),
                })),
                Expect(ComponentCount(3)),
                App(HandleKeyEvent(key!("esc"))),
                Expect(ComponentCount(1)),
                Expect(CurrentComponentPath(Some(s.main_rs()))),
            ])
        })
    }
}

#[test]
fn saving_in_insert_mode_in_suggestive_editor_should_close_all_other_windows() -> anyhow::Result<()>
{
    {
        let completion_item = |label: &str, documentation: Option<&str>| CompletionItem {
            source: CompletionSource::Null,
            label: label.to_string(),
            edit: Some(CompletionItemEdit::PositionalEdit(PositionalEdit {
                range: Position::new(0, 0)..Position::new(0, 6),
                new_text: label.to_string(),
            })),
            documentation: documentation.map(Documentation::new),
            sort_text: None,
            kind: None,
            detail: None,
        };
        execute_test(|s| {
            Box::new([
                App(OpenFile(s.main_rs())),
                Editor(SetContent("".to_string())),
                Editor(EnterInsertMode(Direction::Start)),
                SuggestiveEditor(DispatchSuggestiveEditor::CompletionFilter(
                    SuggestiveEditorFilter::CurrentWord,
                )),
                // Pretend that the LSP server returned a completion
                SuggestiveEditor(DispatchSuggestiveEditor::Completion(Completion {
                    source: CompletionSource::Null,
                    trigger_characters: vec![".".to_string()],
                    items: Some(completion_item(
                        "Spongebob squarepants",
                        Some("krabby patty maker"),
                    ))
                    .into_iter()
                    .map(|item| item.into())
                    .collect(),
                })),
                Expect(ComponentCount(3)),
                Editor(EnterInsertMode(Direction::Start)),
                Editor(Insert("hello".to_string())),
                Editor(Save),
                Expect(ComponentCount(1)),
                Expect(CurrentComponentPath(Some(s.main_rs()))),
            ])
        })
    }
}

#[test]
fn closing_current_file_should_replace_current_window_with_another_file() -> anyhow::Result<()> {
    {
        execute_test(|s| {
            Box::new([
                App(OpenFile(s.main_rs())),
                App(OpenFile(s.foo_rs())),
                Expect(CurrentComponentPath(Some(s.foo_rs()))),
                App(CloseCurrentWindow),
                Expect(CurrentComponentPath(Some(s.main_rs()))),
                Expect(OpenedFilesCount(1)),
                App(CloseCurrentWindow),
                Expect(OpenedFilesCount(0)),
                Expect(CurrentComponentPath(None)),
            ])
        })
    }
}

#[test]
fn file_path_history() -> anyhow::Result<()> {
    {
        execute_test(|s| {
            Box::new([
                App(OpenFile(s.main_rs())),
                App(OpenFile(s.foo_rs())),
                App(OpenFile(s.foo_rs())),
                Expect(CurrentComponentPath(Some(s.foo_rs()))),
                App(GoToPreviousFile),
                Expect(CurrentComponentPath(Some(s.main_rs()))),
                App(GoToNextFile),
                Expect(CurrentComponentPath(Some(s.foo_rs()))),
            ])
        })
    }
}

#[test]
fn editor_info_should_always_come_after_dropdown() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent("".to_string())),
            Editor(EnterInsertMode(Direction::Start)),
            SuggestiveEditor(DispatchSuggestiveEditor::CompletionFilter(
                SuggestiveEditorFilter::CurrentWord,
            )),
            // Show editor info first
            App(ShowEditorInfo(Info::new(
                "Title".to_string(),
                "hello".to_string(),
            ))),
            // The show dropdown
            SuggestiveEditor(DispatchSuggestiveEditor::Completion(Completion {
                source: CompletionSource::Null,
                trigger_characters: vec![".".to_string()],
                items: Some(CompletionItem::from_label(
                    "Spongebob squarepants".to_string(),
                ))
                .into_iter()
                .map(|item| item.into())
                .collect(),
            })),
            Expect(ComponentCount(3)),
            // But dropdown still comes before editor info
            Expect(ExpectKind::ComponentsOrder(vec![
                ComponentKind::SuggestiveEditor,
                ComponentKind::Dropdown,
                ComponentKind::EditorInfo,
            ])),
        ])
    })
}

#[test]
fn dropdown_can_only_be_rendered_on_suggestive_editor_or_prompt() -> anyhow::Result<()> {
    execute_test(|s| {
        let received_completion = || {
            SuggestiveEditor(DispatchSuggestiveEditor::Completion(Completion {
                source: CompletionSource::Null,
                trigger_characters: vec![".".to_string()],
                items: Some(CompletionItem::from_label(
                    "Spongebob squarepants".to_string(),
                ))
                .into_iter()
                .map(|item| item.into())
                .collect(),
            }))
        };
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent("hello".to_string())),
            Editor(EnterInsertMode(Direction::Start)),
            SuggestiveEditor(DispatchSuggestiveEditor::CompletionFilter(
                SuggestiveEditorFilter::CurrentWord,
            )),
            // The show dropdown
            received_completion(),
            Expect(ComponentCount(2)),
            Expect(CurrentComponentContent("hello")),
            App(OtherWindow),
            Expect(CurrentComponentContent(" Spongebob squarepants")),
            received_completion(),
            Expect(ComponentCount(2)),
        ])
    })
}

#[test]
fn only_children_of_root_can_remove_all_other_components() -> anyhow::Result<()> {
    execute_test(|s| {
        let received_completion = || {
            SuggestiveEditor(DispatchSuggestiveEditor::Completion(Completion {
                source: CompletionSource::Null,
                trigger_characters: vec![".".to_string()],
                items: Some(CompletionItem::from_label(
                    "Spongebob squarepants".to_string(),
                ))
                .into_iter()
                .map(|item| item.into())
                .collect(),
            }))
        };
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent("hello".to_string())),
            Editor(EnterInsertMode(Direction::Start)),
            SuggestiveEditor(DispatchSuggestiveEditor::CompletionFilter(
                SuggestiveEditorFilter::CurrentWord,
            )),
            // The show dropdown
            received_completion(),
            Expect(ComponentCount(2)),
            Expect(CurrentComponentContent("hello")),
            App(OtherWindow),
            Expect(CurrentComponentContent(" Spongebob squarepants")),
            App(RemainOnlyCurrentComponent),
            Expect(ComponentCount(2)),
        ])
    })
}

#[test]
fn preserve_selection_after_file_changes() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent("hello world bar".to_string())),
            Editor(MatchLiteral("world".to_string())),
            Expect(CurrentSelectedTexts(&["world"])),
            App(OpenFile(s.foo_rs())),
            App(OpenFile(s.main_rs())),
            Expect(CurrentSelectedTexts(&["world"])),
        ])
    })
}

#[test]
fn open_search_prompt_in_file_explorer() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            App(RevealInExplorer(s.main_rs())),
            Expect(CurrentComponentTitle("File Explorer")),
            App(OpenSearchPrompt {
                scope: Scope::Local,
            }),
            Expect(Not(Box::new(CurrentComponentTitle("File Explorer")))),
            App(HandleKeyEvents(keys!("m a i n enter").to_vec())),
            Expect(CurrentComponentTitle("File Explorer")),
        ])
    })
}

#[test]
fn global_search_should_not_using_empty_pattern() -> anyhow::Result<()> {
    execute_test(|_| {
        Box::new([
            App(UpdateLocalSearchConfig {
                update: LocalSearchConfigUpdate::Search("".to_string()),
                scope: Scope::Global,
                show_config_after_enter: true,
            }),
            Expect(ExpectKind::Quickfixes(Box::new([]))),
        ])
    })
}

#[test]
fn workspace_edit() -> anyhow::Result<()> {
    execute_test(|s| {
        Box::new([
            App(OpenFile(s.main_rs())),
            Editor(SetContent("who lives in a pineapple".to_string())),
            Editor(MatchLiteral("pineapple".to_string())),
            Expect(CurrentSelectedTexts(&["pineapple"])),
            App(Dispatch::ApplyWorkspaceEdit(WorkspaceEdit {
                edits: [TextDocumentEdit {
                    path: s.main_rs(),
                    edits: [PositionalEdit {
                        range: Position::new(0, 0)..Position::new(0, 0),
                        new_text: "hello ".to_string(),
                    }]
                    .to_vec(),
                }]
                .to_vec(),
                resource_operations: Vec::new(),
            })),
            Expect(CurrentComponentContent("hello who lives in a pineapple")),
            // Expect the selection is still "pineapple"
            Expect(CurrentSelectedTexts(&["pineapple"])),
        ])
    })
}
