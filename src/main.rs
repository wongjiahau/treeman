mod buffer;
mod git;

pub(crate) mod char_index_range;
mod cli;
mod clipboard;
pub(crate) mod command;
mod components;
mod context;
mod edit;
pub(crate) mod frontend;
mod grid;
#[cfg(test)]
mod integration_test;

mod layout;
pub(crate) mod list;
mod lsp;
mod position;

mod app;
pub(crate) mod history;
mod non_empty_extensions;
mod quickfix_list;
mod rectangle;
mod screen;
mod selection;
pub(crate) mod selection_mode;
pub(crate) mod selection_range;
pub(crate) mod soft_wrap;
pub(crate) mod style;
pub(crate) mod surround;
pub(crate) mod syntax_highlight;
mod terminal;
#[cfg(test)]
mod test_app;
pub(crate) mod themes;
pub(crate) mod transformation;
pub(crate) mod tree_sitter_traversal;
pub(crate) mod ui_tree;
pub(crate) mod undo_tree;
mod utils;

use std::sync::{Arc, Mutex};

use anyhow::Context;
use frontend::crossterm::Crossterm;
use log::LevelFilter;
use shared::canonicalized_path::CanonicalizedPath;

use app::App;

use crate::app::AppMessage;

fn main() {
    cli::cli().unwrap();
}

#[derive(Default)]
pub(crate) struct RunConfig {
    pub(crate) entry_path: Option<CanonicalizedPath>,
    pub(crate) working_directory: Option<CanonicalizedPath>,
}

pub(crate) fn run(config: RunConfig) -> anyhow::Result<()> {
    std::fs::create_dir_all(grammar::cache_dir()).context("Failed to create cache_dir")?;
    simple_logging::log_to_file(grammar::default_log_file(), LevelFilter::Info)?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let syntax_highlighter_sender = syntax_highlight::start_thread(sender.clone());
    let mut app = App::from_channel(
        Arc::new(Mutex::new(Crossterm::default())),
        config.working_directory.unwrap_or(".".try_into()?),
        sender,
        receiver,
    )?;
    app.set_syntax_highlight_request_sender(syntax_highlighter_sender);

    let sender = app.sender();

    let crossterm_join_handle = std::thread::spawn(move || loop {
        if crossterm::event::read()
            .map_err(|error| anyhow::anyhow!("{:?}", error))
            .and_then(|event| Ok(sender.send(AppMessage::Event(event.into()))?))
            .is_err()
        {
            break;
        }
    });

    app.run(config.entry_path)
        .map_err(|error| anyhow::anyhow!("screen.run {:?}", error))?;

    crossterm_join_handle.join().unwrap();

    Ok(())
}
