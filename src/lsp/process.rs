use crate::app::{RequestParams, Scope};
use crate::lsp::completion::CompletionSource;
use anyhow::Context;
use lsp_types::notification::Notification;
use lsp_types::request::{
    GotoDeclarationParams, GotoImplementationParams, GotoTypeDefinitionParams, Request,
};
use lsp_types::*;
use shared::canonicalized_path::CanonicalizedPath;
use shared::language::Language;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};

use std::process::{self};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::app::AppMessage;
use crate::components::component::ComponentId;
use crate::utils::consolidate_errors;

use super::code_action::CodeAction;
use super::completion::{Completion, CompletionItem};
use super::goto_definition_response::GotoDefinitionResponse;
use super::hover::Hover;
use super::prepare_rename_response::PrepareRenameResponse;
use super::signature_help::SignatureHelp;
use super::symbols::Symbols;
use super::workspace_edit::WorkspaceEdit;
use crate::quickfix_list::Location;

struct LspServerProcess {
    language: Language,
    stdin: process::ChildStdin,

    /// This is hacky, but we need to keep the stdout around so that it doesn't get dropped
    stdout: Option<process::ChildStdout>,
    stderr: Option<process::ChildStderr>,

    server_capabilities: Option<ServerCapabilities>,
    current_working_directory: CanonicalizedPath,
    next_request_id: RequestId,
    pending_response_requests: HashMap<RequestId, PendingResponseRequest>,
    app_message_sender: Sender<AppMessage>,

    receiver: Receiver<LspServerProcessMessage>,
    sender: Sender<LspServerProcessMessage>,
}

type RequestId = u64;

#[derive(Debug)]
struct PendingResponseRequest {
    method: String,
    context: ResponseContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LspNotification {
    Initialized(Language),
    PublishDiagnostics(PublishDiagnosticsParams),
    Completion(ResponseContext, Completion),
    Hover(Hover),
    Definition(ResponseContext, GotoDefinitionResponse),
    References(ResponseContext, Vec<Location>),
    PrepareRenameResponse(ResponseContext, PrepareRenameResponse),
    Error(String),
    WorkspaceEdit(WorkspaceEdit),
    CodeAction(ResponseContext, Vec<CodeAction>),
    SignatureHelp(Option<SignatureHelp>),
    Symbols(ResponseContext, Symbols),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ResponseContext {
    /// This indicates that this request was sent by a component,
    /// and the response should be sent back to that component.
    ///
    /// If the response of this request need not be sent back to a component,
    /// just use the default value `ComponentId::default()`.
    ///
    /// This field is purposefully not an `Option` so that we do not need to
    /// use `unwrap()` to obtain the `component_id`.
    pub(crate) component_id: ComponentId,
    pub(crate) scope: Option<Scope>,
    pub(crate) description: Option<String>,
}
impl ResponseContext {
    pub(crate) fn set_description(self, descrption: &str) -> Self {
        Self {
            description: Some(descrption.to_owned()),
            ..self
        }
    }
}

#[derive(Debug)]
enum LspServerProcessMessage {
    FromLspServer(serde_json::Value),
    FromEditor(FromEditor),
}

#[derive(Debug)]
enum FromEditor {
    RequestHover(RequestParams),
    RequestCompletion(RequestParams),
    RequestDefinition(RequestParams),
    RequestReferences {
        params: RequestParams,
        include_declaration: bool,
    },
    TextDocumentDidOpen {
        file_path: CanonicalizedPath,
        language_id: String,
        version: usize,
        content: String,
    },
    Shutdown,
    TextDocumentDidChange {
        file_path: CanonicalizedPath,
        version: i32,
        content: String,
    },
    TextDocumentDidSave {
        file_path: CanonicalizedPath,
    },
    PrepareRenameSymbol(RequestParams),
    RenameSymbol {
        params: RequestParams,
        new_name: String,
    },
    RequestCodeAction {
        params: RequestParams,
        diagnostics: Vec<lsp_types::Diagnostic>,
    },
    RequestSignatureHelp(RequestParams),
    RequestDeclaration(RequestParams),
    RequestImplementation(RequestParams),
    RequestTypeDefinition(RequestParams),
    RequestDocumentSymbols(RequestParams),
    WorkspaceDidRenameFiles {
        old: CanonicalizedPath,
        new: CanonicalizedPath,
    },
    WorkspaceExecuteCommand {
        params: RequestParams,
        command: super::code_action::Command,
    },
}

pub(crate) struct LspServerProcessChannel {
    language: Language,
    join_handle: JoinHandle<JoinHandle<()>>,
    sender: Sender<LspServerProcessMessage>,
    is_initialized: bool,
}

impl LspServerProcessChannel {
    pub(crate) fn new(
        language: Language,
        screen_message_sender: Sender<AppMessage>,
        current_working_directory: CanonicalizedPath,
    ) -> Result<Option<LspServerProcessChannel>, anyhow::Error> {
        LspServerProcess::start(language, screen_message_sender, current_working_directory)
    }

    pub(crate) fn request_hover(&self, params: RequestParams) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::RequestHover(params),
        ))
    }

    pub(crate) fn request_definition(&self, params: RequestParams) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::RequestDefinition(params),
        ))
    }

    pub(crate) fn request_references(
        &self,
        params: RequestParams,
        include_declaration: bool,
    ) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::RequestReferences {
                params,
                include_declaration,
            },
        ))
    }

    pub(crate) fn request_declaration(&self, clone: RequestParams) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::RequestDeclaration(clone),
        ))
    }

    pub(crate) fn request_implementation(&self, clone: RequestParams) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::RequestImplementation(clone),
        ))
    }

    pub(crate) fn request_type_definition(
        &self,
        clone: RequestParams,
    ) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::RequestTypeDefinition(clone),
        ))
    }

    pub(crate) fn request_completion(&self, params: RequestParams) -> anyhow::Result<()> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::RequestCompletion(params),
        ))
    }

    pub(crate) fn request_signature_help(&self, params: RequestParams) -> anyhow::Result<()> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::RequestSignatureHelp(params),
        ))
    }

    pub(crate) fn prepare_rename_symbol(&self, params: RequestParams) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::PrepareRenameSymbol(params),
        ))
    }

    pub(crate) fn rename_symbol(
        &self,
        params: RequestParams,
        new_name: String,
    ) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::RenameSymbol { params, new_name },
        ))
    }

    pub(crate) fn request_code_action(
        &self,
        params: RequestParams,
        diagnostics: Vec<lsp_types::Diagnostic>,
    ) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::RequestCodeAction {
                params,
                diagnostics,
            },
        ))
    }

    pub(crate) fn request_document_symbols(
        &self,
        params: RequestParams,
    ) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::RequestDocumentSymbols(params),
        ))
    }

    pub(crate) fn shutdown(self) -> anyhow::Result<()> {
        self.send(LspServerProcessMessage::FromEditor(FromEditor::Shutdown))?;
        self.join_handle
            .join()
            .map_err(|err| anyhow::anyhow!("Unable to join lsp server process [1]: {:?}", err))?
            .join()
            .map_err(|err| anyhow::anyhow!("Unable to join lsp server process [2]: {:?}", err))
    }

    fn send(&self, message: LspServerProcessMessage) -> anyhow::Result<()> {
        self.sender
            .send(message)
            .map_err(|err| anyhow::anyhow!("Unable to send request: {}", err))
    }

    pub(crate) fn documents_did_open(
        &mut self,
        paths: Vec<CanonicalizedPath>,
    ) -> Result<(), anyhow::Error> {
        consolidate_errors(
            "[documents_did_open]",
            paths
                .into_iter()
                .map(|path| self.document_did_open(path))
                .collect(),
        )
    }

    pub(crate) fn document_did_open(&self, path: CanonicalizedPath) -> Result<(), anyhow::Error> {
        let content = path.read()?;
        let Some(language_id) = self.language.id() else {
            return Ok(());
        };
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::TextDocumentDidOpen {
                file_path: path,
                language_id: language_id.to_string(),
                version: 1,
                content,
            },
        ))
    }

    pub(crate) fn document_did_change(
        &self,
        path: &CanonicalizedPath,
        content: &str,
    ) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::TextDocumentDidChange {
                file_path: path.clone(),
                version: 2,
                content: content.to_string(),
            },
        ))
    }
    pub(crate) fn is_initialized(&self) -> bool {
        self.is_initialized
    }

    pub(crate) fn initialized(&mut self) {
        self.is_initialized = true
    }

    pub(crate) fn document_did_save(&self, path: &CanonicalizedPath) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::TextDocumentDidSave {
                file_path: path.clone(),
            },
        ))
    }

    pub(crate) fn document_did_rename(
        &self,
        old: CanonicalizedPath,
        new: CanonicalizedPath,
    ) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::WorkspaceDidRenameFiles { old, new },
        ))
    }

    pub(crate) fn workspace_execute_command(
        &self,
        params: RequestParams,
        command: super::code_action::Command,
    ) -> Result<(), anyhow::Error> {
        self.send(LspServerProcessMessage::FromEditor(
            FromEditor::WorkspaceExecuteCommand { command, params },
        ))
    }
}

impl LspServerProcess {
    fn start(
        language: Language,
        app_message_sender: Sender<AppMessage>,
        current_working_directory: CanonicalizedPath,
    ) -> anyhow::Result<Option<LspServerProcessChannel>> {
        let process_command = match language.lsp_process_command() {
            Some(result) => result,
            None => return Ok(None),
        };

        let mut process = process_command.spawn()?;
        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Unable to obtain stdin"))?;

        let stderr = process
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Unable to obtain stderr"))?;

        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Unable to obtain stdout"))?;
        let (sender, receiver) = std::sync::mpsc::channel::<LspServerProcessMessage>();
        let mut lsp_server_process = LspServerProcess {
            language: language.clone(),
            stdin,
            stdout: Some(stdout),
            stderr: Some(stderr),
            current_working_directory,
            next_request_id: 0,
            pending_response_requests: HashMap::new(),
            server_capabilities: None,
            app_message_sender,
            receiver,
            sender: sender.clone(),
        };

        lsp_server_process.initialize()?;

        let join_handle = std::thread::spawn(move || lsp_server_process.listen());

        Ok(Some(LspServerProcessChannel {
            language,
            join_handle,
            sender,
            is_initialized: false,
        }))
    }

    fn initialize(&mut self) -> anyhow::Result<()> {
        self.send_request::<lsp_request!("initialize")>(
            ResponseContext::default(),
            InitializeParams {
                process_id: None,
                root_uri: Some(Url::parse(&format!(
                    "file://{}",
                    self.current_working_directory.display_absolute()
                ))?),
                initialization_options: self.language.initialization_options(),

                capabilities: ClientCapabilities {
                    workspace: Some(WorkspaceClientCapabilities {
                        apply_edit: Some(true),
                        workspace_edit: Some(WorkspaceEditClientCapabilities {
                            document_changes: Some(true),
                            resource_operations: Some(
                                [
                                    ResourceOperationKind::Rename,
                                    ResourceOperationKind::Create,
                                    ResourceOperationKind::Delete,
                                ]
                                .into_iter()
                                .collect(),
                            ),
                            ..WorkspaceEditClientCapabilities::default()
                        }),
                        file_operations: Some(WorkspaceFileOperationsClientCapabilities {
                            did_rename: Some(true),
                            ..Default::default()
                        }),
                        execute_command: Some(DynamicRegistrationClientCapabilities {
                            dynamic_registration: None,
                        }),
                        ..WorkspaceClientCapabilities::default()
                    }),
                    text_document: Some(TextDocumentClientCapabilities {
                        publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                            related_information: Some(true),
                            tag_support: Some(TagSupport {
                                value_set: vec![
                                    DiagnosticTag::DEPRECATED,
                                    DiagnosticTag::UNNECESSARY,
                                ],
                            }),
                            code_description_support: Some(true),
                            ..PublishDiagnosticsClientCapabilities::default()
                        }),
                        completion: Some(CompletionClientCapabilities {
                            completion_item: Some(CompletionItemCapability {
                                ..CompletionItemCapability::default()
                            }),
                            completion_item_kind: Some(CompletionItemKindCapability {
                                ..Default::default()
                            }),
                            ..CompletionClientCapabilities::default()
                        }),
                        hover: Some(HoverClientCapabilities {
                            content_format: Some(vec![MarkupKind::PlainText]),
                            ..HoverClientCapabilities::default()
                        }),
                        code_action: Some(CodeActionClientCapabilities {
                            code_action_literal_support: Some(CodeActionLiteralSupport {
                                code_action_kind: CodeActionKindLiteralSupport {
                                    value_set: vec![
                                        CodeActionKind::EMPTY,
                                        CodeActionKind::QUICKFIX,
                                        CodeActionKind::REFACTOR,
                                        CodeActionKind::REFACTOR_EXTRACT,
                                        CodeActionKind::REFACTOR_INLINE,
                                        CodeActionKind::REFACTOR_REWRITE,
                                        CodeActionKind::SOURCE,
                                        CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                                        CodeActionKind::SOURCE_FIX_ALL,
                                    ]
                                    .into_iter()
                                    .map(|kind| kind.as_str().to_string())
                                    .collect(),
                                },
                            }),
                            ..Default::default()
                        }),
                        rename: Some(RenameClientCapabilities {
                            prepare_support: Some(true),
                            ..Default::default()
                        }),
                        signature_help: Some(SignatureHelpClientCapabilities {
                            signature_information: Some(SignatureInformationSettings {
                                documentation_format: Some(vec![MarkupKind::PlainText]),
                                parameter_information: Some(ParameterInformationSettings {
                                    label_offset_support: Some(true),
                                }),
                                active_parameter_support: Some(true),
                            }),
                            ..Default::default()
                        }),
                        declaration: Some(GotoCapability {
                            dynamic_registration: Some(true),
                            link_support: None,
                        }),
                        ..TextDocumentClientCapabilities::default()
                    }),
                    ..ClientCapabilities::default()
                },
                workspace_folders: None,
                ..InitializeParams::default()
            },
        )?;
        Ok(())
    }

    pub(crate) fn listen(mut self) -> JoinHandle<()> {
        let mut stdout_reader = BufReader::new(self.stdout.take().unwrap());
        let mut stderr_reader = BufReader::new(self.stderr.take().unwrap());
        let sender = self.sender.clone();
        let handle = thread::spawn(move || {
            fn read_response(
                reader: &mut BufReader<process::ChildStdout>,
                sender: &Sender<LspServerProcessMessage>,
            ) -> anyhow::Result<()> {
                let mut line = String::new();
                reader
                    .read_line(&mut line)
                    .with_context(|| "Failed to read Content-Length")?;

                log::info!("LspServerProcess::listen line = {line:?}");

                let content_length = line
                    .split(':')
                    .nth(1)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Parsing Content-Length: Unable to split line.")
                    })?
                    .trim()
                    .parse::<usize>()
                    .with_context(|| "Parsing Content-Length: Failed to parse number.")?;

                // According to https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#headerPart
                //
                // ... this means that TWO '\r\n' sequences always immediately precede the content part of a message.
                //
                // That's why we have to read an empty line here again.
                reader
                    .read_line(&mut line)
                    .with_context(|| "Failed to read content.")?;

                let mut buffer = vec![0; content_length];
                reader
                    .read_exact(&mut buffer)
                    .with_context(|| "Failed to read buffer into vector.")?;

                let reply = String::from_utf8(buffer)
                    .with_context(|| "Failed to convert content buffer into String.")?;

                // Parse as generic JSON value
                let reply: serde_json::Value = serde_json::from_str(&reply)
                    .with_context(|| "Failed to convert content string into JSON value")?;

                sender
                    .send(LspServerProcessMessage::FromLspServer(reply))
                    .unwrap_or_else(|error| {
                        log::error!("[LspServerProcess] Error sending reply: {:?}", error);
                    });

                Ok(())
            }
            loop {
                if let Err(error) = read_response(&mut stdout_reader, &sender) {
                    let mut stderr = String::new();
                    // Reading the error from stderr is necessary
                    // If not, subsequent read from stdout might result in infinite loop, due to
                    // stdout keeps emitting empty lines only
                    //
                    // Note: this logic is only tested on Rust Analyzer
                    let _ = stderr_reader.read_to_string(&mut stderr).map_err(|err| {
                        log::error!("LspServerResponse::listen failed to read stderr = {err}")
                    });
                    log::info!("LspServerProcess::listen::read_response: {}", error);
                    log::info!("LspServerProcess::listen: stderr = {}", stderr)
                }
            }
        });

        log::info!("[LspServerProcess] Listening for messages from LSP server");
        while let Ok(message) = self.receiver.recv() {
            match message {
                LspServerProcessMessage::FromLspServer(json_value) => self.handle_reply(json_value),
                LspServerProcessMessage::FromEditor(from_editor) => match from_editor {
                    FromEditor::RequestCompletion(params) => self.text_document_completion(params),
                    FromEditor::RequestHover(params) => self.text_document_hover(params),
                    FromEditor::RequestDefinition(params) => self.text_document_definition(params),
                    FromEditor::RequestReferences {
                        params,
                        include_declaration,
                    } => self.text_document_references(params, include_declaration),
                    FromEditor::RequestDeclaration(params) => {
                        self.text_document_declaration(params)
                    }
                    FromEditor::RequestImplementation(params) => {
                        self.text_document_implementation(params)
                    }
                    FromEditor::RequestTypeDefinition(params) => {
                        self.text_document_type_definition(params)
                    }
                    FromEditor::RenameSymbol { params, new_name } => {
                        self.text_document_rename(params, new_name)
                    }
                    FromEditor::PrepareRenameSymbol(params) => {
                        self.text_document_prepare_rename(params)
                    }
                    FromEditor::RequestCodeAction {
                        params,
                        diagnostics,
                    } => self.text_document_code_action(params, diagnostics),
                    FromEditor::RequestDocumentSymbols(params) => {
                        self.text_document_document_symbol(params)
                    }

                    FromEditor::TextDocumentDidOpen {
                        file_path,
                        language_id,
                        version,
                        content,
                    } => self.text_document_did_open(file_path, language_id, version, content),
                    FromEditor::Shutdown => self.shutdown(),
                    FromEditor::TextDocumentDidChange {
                        file_path,
                        version,
                        content,
                    } => self.text_document_did_change(file_path, version, content),
                    FromEditor::TextDocumentDidSave { file_path } => {
                        self.text_document_did_save(file_path)
                    }
                    FromEditor::RequestSignatureHelp(params) => {
                        self.text_document_signature_help(params)
                    }
                    FromEditor::WorkspaceDidRenameFiles { old, new } => {
                        self.workspace_did_rename_files(old, new)
                    }
                    FromEditor::WorkspaceExecuteCommand { params, command } => {
                        self.workspace_execute_command(params, command)
                    }
                },
            }
            .unwrap_or_else(|error| {
                log::info!("[LspServerProcess] Error handling reply: {:?}", error);
            })
        }

        log::info!("[LspServerProcess] Stopped listening for messages from LSP server");
        handle
    }

    fn handle_reply(&mut self, reply: serde_json::Value) -> anyhow::Result<()> {
        // Check if reply is Response or Notification
        // Only Notification contains the `method` field
        match reply.get("method") {
            // reply is Response
            None => {
                // Get the request ID
                let request_id = reply.get("id").unwrap().as_u64().unwrap();

                // Get the method of the request
                let pending_response_request =
                    self.pending_response_requests.remove(&request_id).unwrap();

                // Parse the reply as a Response
                let response = serde_json::from_value::<
                    json_rpc_types::Response<
                        serde_json::Value,
                        (),
                        // Need to specify String here
                        // Otherwise the default will be `str_buf::StrBuf<31>`,
                        // which says the error message can only be 31 bytes long.
                        String,
                    >,
                >(reply)
                .map_err(|e| anyhow::anyhow!("Serde error = {:?}", e))?
                .payload
                .map_err(|e| {
                    self.app_message_sender
                        .send(AppMessage::LspNotification(LspNotification::Error(
                            format!("LSP JSON-RPC Error: {:?}: {}", e.code, e.message),
                        )))
                        .unwrap();
                    anyhow::anyhow!(
                        "LSP JSON-RPC Error: Code={:?} Message={}",
                        e.code,
                        e.message
                    )
                })?;

                let PendingResponseRequest {
                    method,
                    context: response_context,
                } = pending_response_request;

                match method.as_str() {
                    "initialize" => {
                        log::info!("Initialize response: {:?}", response);
                        let payload: <lsp_request!("initialize") as Request>::Result =
                            serde_json::from_value(response)?;

                        // Get the capabilities
                        self.server_capabilities = Some(payload.capabilities);

                        // Send the initialized notification
                        self.send_notification::<lsp_notification!("initialized")>(
                            InitializedParams {},
                        )?;

                        self.app_message_sender.send(AppMessage::LspNotification(
                            LspNotification::Initialized(self.language.clone()),
                        ))?;
                    }
                    "textDocument/completion" => {
                        let payload: <lsp_request!("textDocument/completion") as Request>::Result =
                            serde_json::from_value(response)?;

                        log::info!("Recevied completion");

                        if let Some(payload) = payload {
                            self.app_message_sender
                                .send(AppMessage::LspNotification(LspNotification::Completion(
                                    response_context,
                                    Completion {
                                        trigger_characters: self.trigger_characters(),
                                        items: match payload {
                                            CompletionResponse::Array(items) => items,
                                            CompletionResponse::List(list) => list.items,
                                        }
                                        .into_iter()
                                        .map(CompletionItem::from)
                                        .map(|item| item.into())
                                        .collect(),
                                        source: CompletionSource::Lsp {
                                            language: self
                                                .language
                                                .id()
                                                .map(|id| id.to_string())
                                                .unwrap_or_else(|| "Unknown".to_string()),
                                        },
                                    },
                                )))
                                .unwrap();
                        }
                    }
                    "textDocument/hover" => {
                        let payload: <lsp_request!("textDocument/hover") as Request>::Result =
                            serde_json::from_value(response)?;

                        if let Some(payload) = payload {
                            self.app_message_sender
                                .send(AppMessage::LspNotification(LspNotification::Hover(
                                    payload.into(),
                                )))
                                .unwrap();
                        }
                    }
                    "textDocument/definition" => {
                        let payload: <lsp_request!("textDocument/definition") as Request>::Result =
                            serde_json::from_value(response)?;

                        if let Some(payload) = payload {
                            self.app_message_sender
                                .send(AppMessage::LspNotification(LspNotification::Definition(
                                    response_context,
                                    payload.try_into()?,
                                )))
                                .unwrap();
                        }
                    }
                    "textDocument/references" => {
                        let payload: <lsp_request!("textDocument/references") as Request>::Result =
                            serde_json::from_value(response)?;

                        if let Some(payload) = payload {
                            self.app_message_sender
                                .send(AppMessage::LspNotification(LspNotification::References(
                                    response_context,
                                    payload
                                        .into_iter()
                                        .map(|r| r.try_into())
                                        .collect::<Result<Vec<_>, _>>()?,
                                )))
                                .unwrap();
                        }
                    }
                    "textDocument/declaration" => {
                        let payload: <lsp_request!("textDocument/declaration") as Request>::Result =
                            serde_json::from_value(response)?;

                        if let Some(payload) = payload {
                            self.app_message_sender
                                .send(AppMessage::LspNotification(LspNotification::Definition(
                                    response_context,
                                    payload.try_into()?,
                                )))
                                .unwrap();
                        }
                    }
                    "textDocument/typeDefinition" => {
                        let payload: <lsp_request!("textDocument/typeDefinition") as Request>::Result =
                            serde_json::from_value(response)?;

                        if let Some(payload) = payload {
                            self.app_message_sender
                                .send(AppMessage::LspNotification(LspNotification::Definition(
                                    response_context,
                                    payload.try_into()?,
                                )))
                                .unwrap();
                        }
                    }
                    "textDocument/implementation" => {
                        let payload: <lsp_request!("textDocument/implementation") as Request>::Result =
                            serde_json::from_value(response)?;

                        if let Some(payload) = payload {
                            self.app_message_sender
                                .send(AppMessage::LspNotification(LspNotification::Definition(
                                    response_context,
                                    payload.try_into()?,
                                )))
                                .unwrap();
                        }
                    }
                    "textDocument/prepareRename" => {
                        let payload: <lsp_request!("textDocument/prepareRename") as Request>::Result =
                            serde_json::from_value(response)?;

                        if let Some(payload) = payload {
                            self.app_message_sender
                                .send(AppMessage::LspNotification(
                                    LspNotification::PrepareRenameResponse(
                                        response_context,
                                        payload.into(),
                                    ),
                                ))
                                .unwrap();
                        }
                    }
                    "textDocument/rename" => {
                        let payload: <lsp_request!("textDocument/rename") as Request>::Result =
                            serde_json::from_value(response)?;

                        log::info!("Rename response: {:?}", payload);

                        if let Some(payload) = payload {
                            self.app_message_sender
                                .send(AppMessage::LspNotification(LspNotification::WorkspaceEdit(
                                    payload.try_into()?,
                                )))
                                .unwrap();
                        }
                    }
                    "textDocument/codeAction" => {
                        let payload: <lsp_request!("textDocument/codeAction") as Request>::Result =
                            serde_json::from_value(response)?;

                        log::info!("CodeAction response: {:?}", payload);

                        if let Some(payload) = payload {
                            self.app_message_sender
                                .send(AppMessage::LspNotification(LspNotification::CodeAction(
                                    response_context,
                                    payload
                                        .into_iter()
                                        .map(|r| match r {
                                            CodeActionOrCommand::Command(_) => todo!(),
                                            CodeActionOrCommand::CodeAction(code_action) => {
                                                code_action.try_into()
                                            }
                                        })
                                        .collect::<Result<Vec<_>, _>>()?,
                                )))
                                .unwrap();
                        }
                    }
                    "textDocument/signatureHelp" => {
                        let payload: <lsp_request!("textDocument/signatureHelp") as Request>::Result =
                            serde_json::from_value(response)?;

                        log::info!("SignatureHelp response: {:?}", payload);

                        self.app_message_sender
                            .send(AppMessage::LspNotification(LspNotification::SignatureHelp(
                                payload.map(|payload| payload.into()),
                            )))
                            .unwrap();
                    }
                    "textDocument/documentSymbol" => {
                        let payload: <lsp_request!("textDocument/documentSymbol") as Request>::Result =
                            serde_json::from_value(response)?;

                        log::info!("Symbols response: {:?}", payload);

                        if let Some(payload) = payload {
                            self.app_message_sender
                                .send(AppMessage::LspNotification(LspNotification::Symbols(
                                    response_context,
                                    payload.try_into()?,
                                )))
                                .unwrap();
                        }
                    }
                    _ => {
                        log::info!("Unknown method: {:#?}", method);
                    }
                }
            }

            // reply is Notification
            Some(_) => {
                let request = serde_json::from_value::<
                    json_rpc_types::Request<
                        serde_json::Value,
                        // Need to specify String here
                        // Otherwise the default will be `str_buf::StrBuf<31>`,
                        // which says the error message can only be 31 bytes long.
                        String,
                    >,
                >(reply)
                .map_err(|e| anyhow::anyhow!("Serde error = {:?}", e))?;

                let method = request.method;
                // Parse the reply as Notification
                match method.as_str() {
                    "textDocument/publishDiagnostics" => {
                        let params: <lsp_notification!("textDocument/publishDiagnostics") as Notification>::Params =
                            serde_json::from_value(request.params.ok_or_else(|| anyhow::anyhow!("Missing params"))?)?;

                        self.app_message_sender
                            .send(AppMessage::LspNotification(
                                LspNotification::PublishDiagnostics(params),
                            ))
                            .unwrap();
                    }
                    "workspace/applyEdit" => {
                        let params: <lsp_request!("workspace/applyEdit") as Request>::Params =
                            serde_json::from_value(request.params.unwrap())?;

                        self.app_message_sender
                            .send(AppMessage::LspNotification(LspNotification::WorkspaceEdit(
                                params.edit.try_into()?,
                            )))
                            .unwrap();
                    }
                    "workspace/configuration" => {
                        // Just return null for now, since I don't know how how to handle this properly
                        // This reply is necessary for Graphql LSP to work

                        self.send_reply(request.id, serde_json::Value::Null)?;
                    }

                    _ => log::info!("unhandled Incoming Notification: {}", method),
                }
            }
        }

        Ok(())
    }

    fn trigger_characters(&self) -> Vec<String> {
        self.server_capabilities
            .as_ref()
            .and_then(|capabilities| {
                capabilities
                    .completion_provider
                    .as_ref()
                    .and_then(|provider| provider.trigger_characters.clone())
            })
            .unwrap_or_default()
    }

    pub(crate) fn shutdown(&mut self) -> anyhow::Result<()> {
        self.send_request::<lsp_request!("shutdown")>(ResponseContext::default(), ())?;
        Ok(())
    }

    fn send_notification<N: Notification>(&mut self, params: N::Params) -> anyhow::Result<()> {
        let notification = json_rpc_types::Request {
            id: None,
            jsonrpc: json_rpc_types::Version::V2,
            method: N::METHOD,
            params: Some(params),
        };

        log::info!(
            "Sending notification: {:?} {:?}",
            self.language.id(),
            N::METHOD
        );

        self.send_json(&notification)?;

        Ok(())
    }

    /// Used for sending response to reponses of the LSP server
    fn send_reply(
        &mut self,
        id: Option<json_rpc_types::Id>,
        result: serde_json::Value,
    ) -> anyhow::Result<()> {
        /// Refer https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#responseMessage
        #[derive(serde::Serialize)]
        struct ResponseMessage {
            id: Option<json_rpc_types::Id>,
            result: serde_json::Value,
        }
        let request = ResponseMessage { id, result };

        self.send_json(&request)?;

        Ok(())
    }

    /// Send JSON to the LSP server by writing to the server's stdin
    fn send_json<T: serde::Serialize>(&mut self, value: T) -> anyhow::Result<()> {
        let json = serde_json::to_string(&value)?;

        // The message format is according to https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#contentPart
        write!(
            &mut self.stdin,
            "Content-Length: {}\r\n\r\n{}",
            json.len(),
            json
        )?;
        Ok(())
    }

    /// Returns the request ID
    fn send_request<R: Request>(
        &mut self,
        context: ResponseContext,
        params: R::Params,
    ) -> anyhow::Result<()>
    where
        R::Params: serde::Serialize,
    {
        let id = {
            let result = self.next_request_id;
            self.next_request_id += 1;
            result
        };
        // Convert the request to a JSON-RPC message
        let request = json_rpc_types::Request {
            jsonrpc: json_rpc_types::Version::V2,
            method: R::METHOD,
            params: Some(params),
            id: Some(json_rpc_types::Id::Num(id)),
        };

        self.send_json(&request)?;

        self.pending_response_requests.insert(
            id,
            PendingResponseRequest {
                context,
                method: R::METHOD.to_string(),
            },
        );

        Ok(())
    }

    fn text_document_did_open(
        &mut self,
        file_path: CanonicalizedPath,
        language_id: String,
        version: usize,
        content: String,
    ) -> Result<(), anyhow::Error> {
        self.send_notification::<lsp_notification!("textDocument/didOpen")>(
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: path_buf_to_url(file_path)?,
                    language_id,
                    version: version as i32,
                    text: content,
                },
            },
        )
    }

    fn text_document_did_change(
        &mut self,
        file_path: CanonicalizedPath,
        version: i32,
        content: String,
    ) -> Result<(), anyhow::Error> {
        self.send_notification::<lsp_notification!("textDocument/didChange")>(
            DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: path_buf_to_url(file_path)?,
                    version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: content,
                }],
            },
        )
    }

    fn text_document_did_save(
        &mut self,
        file_path: CanonicalizedPath,
    ) -> Result<(), anyhow::Error> {
        self.send_notification::<lsp_notification!("textDocument/didSave")>(
            DidSaveTextDocumentParams {
                text_document: path_buf_to_text_document_identifier(file_path)?,
                text: None,
            },
        )
    }

    fn workspace_did_rename_files(
        &mut self,
        old: CanonicalizedPath,
        new: CanonicalizedPath,
    ) -> Result<(), anyhow::Error> {
        self.send_notification::<lsp_notification!("workspace/didRenameFiles")>(RenameFilesParams {
            files: [FileRename {
                old_uri: old.display_absolute(),
                new_uri: new.display_absolute(),
            }]
            .to_vec(),
        })
    }

    fn has_capability(&self, f: impl Fn(&ServerCapabilities) -> bool) -> bool {
        self.server_capabilities.as_ref().map(f).unwrap_or(false)
    }

    fn text_document_completion(
        &mut self,
        RequestParams {
            context,
            path,
            position,
            ..
        }: RequestParams,
    ) -> anyhow::Result<()> {
        if !self.has_capability(|c| c.completion_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("textDocument/completion")>(
            context,
            CompletionParams {
                text_document_position: TextDocumentPositionParams {
                    position: position.into(),
                    text_document: path_buf_to_text_document_identifier(path)?,
                },
                work_done_progress_params: WorkDoneProgressParams {
                    work_done_token: None,
                },
                partial_result_params: PartialResultParams {
                    partial_result_token: None,
                },
                context: None,
            },
        )
    }

    fn text_document_hover(
        &mut self,
        RequestParams {
            context,
            path,
            position,
            ..
        }: RequestParams,
    ) -> anyhow::Result<()> {
        if !self.has_capability(|c| c.hover_provider.is_some()) {
            return Ok(());
        };
        self.send_request::<lsp_request!("textDocument/hover")>(
            context,
            HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    position: position.into(),
                    text_document: path_buf_to_text_document_identifier(path)?,
                },
                work_done_progress_params: WorkDoneProgressParams {
                    work_done_token: None,
                },
            },
        )
    }

    fn text_document_definition(
        &mut self,
        RequestParams {
            path,
            position,
            context,
        }: RequestParams,
    ) -> anyhow::Result<()> {
        if !self.has_capability(|c| c.definition_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("textDocument/definition")>(
            context,
            GotoDefinitionParams {
                partial_result_params: Default::default(),
                text_document_position_params: TextDocumentPositionParams {
                    position: position.into(),
                    text_document: path_buf_to_text_document_identifier(path)?,
                },
                work_done_progress_params: Default::default(),
            },
        )
    }

    fn text_document_references(
        &mut self,
        RequestParams {
            path,
            position,
            context,
        }: RequestParams,
        include_declaration: bool,
    ) -> anyhow::Result<()> {
        if !self.has_capability(|c| c.references_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("textDocument/references")>(
            context,
            ReferenceParams {
                context: ReferenceContext {
                    include_declaration,
                },
                partial_result_params: Default::default(),
                text_document_position: TextDocumentPositionParams {
                    position: position.into(),
                    text_document: path_buf_to_text_document_identifier(path)?,
                },
                work_done_progress_params: Default::default(),
            },
        )
    }

    fn text_document_declaration(&mut self, params: RequestParams) -> Result<(), anyhow::Error> {
        if !self.has_capability(|c| c.declaration_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("textDocument/declaration")>(
            params.context,
            GotoDeclarationParams {
                partial_result_params: Default::default(),
                text_document_position_params: TextDocumentPositionParams {
                    position: params.position.into(),
                    text_document: path_buf_to_text_document_identifier(params.path)?,
                },
                work_done_progress_params: Default::default(),
            },
        )
    }

    fn text_document_implementation(&mut self, params: RequestParams) -> Result<(), anyhow::Error> {
        if !self.has_capability(|c| c.implementation_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("textDocument/implementation")>(
            params.context,
            GotoImplementationParams {
                partial_result_params: Default::default(),
                text_document_position_params: TextDocumentPositionParams {
                    position: params.position.into(),
                    text_document: path_buf_to_text_document_identifier(params.path)?,
                },
                work_done_progress_params: Default::default(),
            },
        )
    }

    fn text_document_type_definition(
        &mut self,
        params: RequestParams,
    ) -> Result<(), anyhow::Error> {
        if !self.has_capability(|c| c.type_definition_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("textDocument/typeDefinition")>(
            params.context,
            GotoTypeDefinitionParams {
                partial_result_params: Default::default(),
                text_document_position_params: TextDocumentPositionParams {
                    position: params.position.into(),
                    text_document: path_buf_to_text_document_identifier(params.path)?,
                },
                work_done_progress_params: Default::default(),
            },
        )
    }

    fn text_document_prepare_rename(&mut self, params: RequestParams) -> Result<(), anyhow::Error> {
        if !self.has_capability(|c| c.rename_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("textDocument/prepareRename")>(
            params.context,
            TextDocumentPositionParams {
                position: params.position.into(),
                text_document: path_buf_to_text_document_identifier(params.path)?,
            },
        )
    }

    fn text_document_rename(
        &mut self,
        params: RequestParams,
        new_name: String,
    ) -> Result<(), anyhow::Error> {
        if !self.has_capability(|c| c.rename_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("textDocument/rename")>(
            params.context,
            RenameParams {
                new_name,
                text_document_position: TextDocumentPositionParams {
                    position: params.position.into(),
                    text_document: path_buf_to_text_document_identifier(params.path)?,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            },
        )
    }

    fn text_document_code_action(
        &mut self,
        params: RequestParams,
        diagnostics: Vec<Diagnostic>,
    ) -> Result<(), anyhow::Error> {
        if !self.has_capability(|c| c.code_action_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("textDocument/codeAction")>(
            params.context,
            CodeActionParams {
                context: CodeActionContext {
                    diagnostics,
                    trigger_kind: None,
                    only: None,
                },
                partial_result_params: Default::default(),
                range: Range {
                    start: params.position.into(),
                    end: params.position.into(),
                },
                text_document: path_buf_to_text_document_identifier(params.path)?,
                work_done_progress_params: Default::default(),
            },
        )
    }

    pub(crate) fn text_document_signature_help(
        &mut self,
        params: RequestParams,
    ) -> Result<(), anyhow::Error> {
        if !self.has_capability(|c| c.signature_help_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("textDocument/signatureHelp")>(
            params.context,
            SignatureHelpParams {
                context: None,
                text_document_position_params: TextDocumentPositionParams {
                    position: params.position.into(),
                    text_document: path_buf_to_text_document_identifier(params.path)?,
                },
                work_done_progress_params: Default::default(),
            },
        )
    }

    fn text_document_document_symbol(
        &mut self,
        params: RequestParams,
    ) -> Result<(), anyhow::Error> {
        if !self.has_capability(|c| c.document_symbol_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("textDocument/documentSymbol")>(
            params.context,
            DocumentSymbolParams {
                partial_result_params: Default::default(),
                text_document: path_buf_to_text_document_identifier(params.path)?,
                work_done_progress_params: Default::default(),
            },
        )
    }

    fn workspace_execute_command(
        &mut self,
        params: RequestParams,
        command: super::code_action::Command,
    ) -> Result<(), anyhow::Error> {
        if !self.has_capability(|c| c.execute_command_provider.is_some()) {
            return Ok(());
        }
        self.send_request::<lsp_request!("workspace/executeCommand")>(
            params.context,
            ExecuteCommandParams {
                command: command.command(),
                arguments: command.arguments(),
                work_done_progress_params: WorkDoneProgressParams {
                    work_done_token: None,
                },
            },
        )
    }
}

fn path_buf_to_url(path: CanonicalizedPath) -> Result<Url, anyhow::Error> {
    Ok(Url::parse(&format!("file://{}", path.display_absolute()))?)
}

fn path_buf_to_text_document_identifier(
    path: CanonicalizedPath,
) -> Result<TextDocumentIdentifier, anyhow::Error> {
    Ok(TextDocumentIdentifier {
        uri: path_buf_to_url(path)?,
    })
}
