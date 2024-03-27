use crate::{app::Dimension, position::Position, themes::Color};

use itertools::Itertools;
use my_proc_macros::hex;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub rows: Vec<Vec<Cell>>,
    pub width: usize,
}

const DEFAULT_TAB_SIZE: usize = 4;

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct Cell {
    pub symbol: String,
    pub foreground_color: Color,
    pub background_color: Color,
    pub undercurl: Option<Color>,
    pub is_cursor: bool,
    /// For debugging purposes, so that we can trace this Cell is updated by which
    /// decoration, e.g. Diagnostic
    pub source: Option<StyleKey>,
}

fn choose<T>(old: Option<T>, new: Option<T>) -> Option<T> {
    new.or(old)
}

impl Cell {
    pub fn from_char(c: char) -> Self {
        Cell {
            symbol: c.to_string(),
            ..Default::default()
        }
    }

    fn apply_update(&self, update: CellUpdate) -> Cell {
        Cell {
            symbol: update.symbol.clone().unwrap_or(self.symbol.clone()),
            foreground_color: update
                .style
                .foreground_color
                .unwrap_or(self.foreground_color),
            background_color: update
                .style
                .background_color
                .unwrap_or(self.background_color),
            undercurl: choose(self.undercurl, update.style.undercurl),
            is_cursor: update.is_cursor || self.is_cursor,
            source: update.source.or(self.source),
        }
    }

    #[cfg(test)]
    fn set_background_color(self, background_color: Color) -> Cell {
        Cell {
            background_color,
            ..self
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            symbol: " ".to_string(),
            foreground_color: hex!("#ffffff"),
            background_color: hex!("#ffffff"),
            undercurl: None,
            is_cursor: false,
            source: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CellUpdate {
    pub position: Position,
    pub symbol: Option<String>,
    pub style: Style,
    pub is_cursor: bool,

    /// For debugging purposes
    pub source: Option<StyleKey>,
}

impl CellUpdate {
    pub fn new(position: Position) -> Self {
        CellUpdate {
            position,
            symbol: None,
            style: Style::default(),
            is_cursor: false,
            source: None,
        }
    }

    pub fn source(self, source: Option<StyleKey>) -> CellUpdate {
        CellUpdate { source, ..self }
    }

    pub fn move_up(self, scroll_offset: usize) -> Option<CellUpdate> {
        if scroll_offset > self.position.line {
            None
        } else {
            Some(CellUpdate {
                position: Position {
                    line: self.position.line - scroll_offset,
                    ..self.position
                },
                ..self
            })
        }
    }

    pub fn style(self, style: Style) -> CellUpdate {
        CellUpdate { style, ..self }
    }

    pub fn set_is_cursor(self, is_cursor: bool) -> CellUpdate {
        CellUpdate { is_cursor, ..self }
    }

    pub fn set_position_line(self, line: usize) -> CellUpdate {
        CellUpdate {
            position: self.position.set_line(line),
            ..self
        }
    }

    pub(crate) fn move_right(self, by: u16) -> CellUpdate {
        CellUpdate {
            position: self.position.move_right(by),
            ..self
        }
    }

    pub(crate) fn set_symbol(self, symbol: Option<String>) -> CellUpdate {
        CellUpdate { symbol, ..self }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Default, Hash)]
pub struct PositionedCell {
    pub cell: Cell,
    pub position: Position,
}

impl PartialOrd for PositionedCell {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for PositionedCell {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.position.cmp(&other.position)
    }
}
#[cfg(test)]
impl std::fmt::Display for Grid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| cell.symbol.to_string())
                        .collect_vec()
                        .join("")
                        .trim()
                        .to_string()
                })
                .collect_vec()
                .join("\n")
        )
    }
}
impl Grid {
    pub fn new(dimension: Dimension) -> Grid {
        let mut cells: Vec<Vec<Cell>> = vec![];
        cells.resize_with(dimension.height.into(), || {
            let mut cells = vec![];
            cells.resize_with(dimension.width.into(), Cell::default);
            cells
        });
        Grid {
            rows: cells,
            width: dimension.width.into(),
        }
    }

    pub fn to_positioned_cells(&self) -> Vec<PositionedCell> {
        let mut cells = vec![];
        for (row_index, row) in self.rows.iter().enumerate() {
            let mut offset = 0;
            for (column_index, cell) in row.iter().enumerate() {
                let width = get_string_width(cell.symbol.as_str());
                for index in 0..width {
                    let adjusted_column = column_index + offset + index;
                    if adjusted_column >= self.width {
                        break;
                    }
                    cells.push(PositionedCell {
                        cell: Cell {
                            symbol: if index > 0 {
                                " ".to_string()
                            } else {
                                cell.symbol.clone()
                            },
                            ..cell.clone()
                        },
                        position: Position {
                            line: row_index,
                            column: adjusted_column,
                        },
                    });
                }
                offset += width.saturating_sub(1)
            }
        }

        cells
    }

    #[cfg(test)]
    pub fn from_text(dimension: Dimension, text: &str) -> Grid {
        text.lines()
            .enumerate()
            .fold(Grid::new(dimension), |grid, (row_index, line)| {
                grid.set_row(row_index, None, None, line, &Style::default())
            })
    }

    pub fn dimension(&self) -> Dimension {
        Dimension {
            height: self.rows.len() as u16,
            width: self.width as u16,
        }
    }

    pub fn set_line(self, row: usize, title: &str, style: &Style) -> Grid {
        self.set_row(row, None, None, title, style)
    }

    pub fn apply_cell_update(mut self, update: CellUpdate) -> Grid {
        let Position { line, column } = update.position;
        if line < self.rows.len() && column < self.rows[line].len() {
            self.rows[line][column] = self.rows[line][column].apply_update(update);
        }
        self
    }

    pub fn apply_cell_updates(self, updates: Vec<CellUpdate>) -> Grid {
        updates
            .into_iter()
            .fold(self, |grid, update| grid.apply_cell_update(update))
    }

    pub fn merge_vertical(self, bottom: Grid) -> Grid {
        let mut top = self;
        top.rows.extend(bottom.rows);
        top
    }

    pub fn clamp_bottom(self, by: u16) -> Grid {
        let mut grid = self;
        let dimension = grid.dimension();
        let height = dimension.height.saturating_sub(by);

        if dimension.height > height {
            grid.rows.truncate(height as usize);
        }
        grid
    }

    pub(crate) fn clamp_top(self, by: usize) -> Self {
        Self {
            rows: self.rows.into_iter().skip(by).collect_vec(),
            ..self
        }
    }

    pub fn get_cursor_position(&self) -> Option<Position> {
        self.to_positioned_cells().into_iter().find_map(|cell| {
            if cell.cell.is_cursor {
                Some(cell.position)
            } else {
                None
            }
        })
    }

    pub(crate) fn set_row(
        self,
        row_index: usize,
        column_start: Option<usize>,
        column_end: Option<usize>,
        content: &str,
        style: &Style,
    ) -> Self {
        let dimension = self.dimension();
        let grid = self;
        let column_range =
            column_start.unwrap_or(0)..column_end.unwrap_or(dimension.width as usize);
        // Trim or Pad end with spaces
        let content = format!("{:<width$}", content, width = column_range.len());
        let take = grid.dimension().width as usize;
        grid.apply_cell_updates(
            content
                .graphemes(false)
                .take(take)
                .enumerate()
                .map(|(char_index, character)| {
                    let column_index = column_range.start + char_index;
                    CellUpdate {
                        position: Position {
                            line: row_index,
                            column: column_index,
                        },
                        symbol: Some(character.to_string()),
                        style: *style,
                        ..CellUpdate::default()
                    }
                })
                .collect_vec(),
        )
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash, PartialOrd, Ord)]
pub enum StyleKey {
    UiPrimarySelection,
    UiPrimarySelectionAnchors,
    UiSecondarySelection,
    UiSecondarySelectionAnchors,
    DiagnosticsHint,
    DiagnosticsError,
    DiagnosticsWarning,
    DiagnosticsInformation,
    UiBookmark,
    UiPossibleSelection,
    SyntaxKeyword,
    SyntaxFunction,
    SyntaxComment,
    SyntaxString,
    SyntaxType,
    DiagnosticsDefault,
    HunkOld,
    HunkOldEmphasized,
    HunkNew,
    HunkNewEmphasized,
    SyntaxDefault,
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct Style {
    pub foreground_color: Option<Color>,
    pub background_color: Option<Color>,
    pub undercurl: Option<Color>,
}

impl Style {
    pub const fn new() -> Style {
        Style {
            foreground_color: None,
            background_color: None,
            undercurl: None,
        }
    }

    pub const fn foreground_color(self, color: Color) -> Style {
        Style {
            foreground_color: Some(color),
            ..self
        }
    }

    pub const fn background_color(self, color: Color) -> Style {
        Style {
            background_color: Some(color),
            ..self
        }
    }

    pub const fn undercurl(self, color: Option<Color>) -> Style {
        Style {
            undercurl: color,
            ..self
        }
    }

    pub(crate) fn set_some_background_color(self, background_color: Option<Color>) -> Style {
        Style {
            background_color,
            ..self
        }
    }
}

/// TODO: in the future, tab size should be configurable
pub fn get_string_width(str: &str) -> usize {
    match str {
        "\t" => DEFAULT_TAB_SIZE,
        _ => UnicodeWidthStr::width(str),
    }
}

#[cfg(test)]
mod test_grid {

    use my_proc_macros::hex;
    use pretty_assertions::assert_eq;

    use crate::{
        app::Dimension,
        grid::{Cell, CellUpdate, Grid, PositionedCell, Style},
        position::Position,
    };

    use super::get_string_width;

    #[test]
    /// In short, "It is important to recognize that what the user thinks of as a "character" - a basic unit of a writing system for a language—may not be just a single Unicode code point. Instead, that basic unit may be made up of multiple Unicode code points"
    ///
    /// Read more at:
    /// - https://en.wikipedia.org/wiki/Zero-width_joiner
    /// - https://www.unicode.org/reports/tr29/#Grapheme_Cluster_Boundaries
    fn consider_grapheme_cluster() {
        let dimension = Dimension {
            height: 1,
            width: 10,
        };
        let zero_width_joiner = "\u{200D}";
        let red_hair_man = format!("👨{}🦰", zero_width_joiner);
        let grid = Grid::from_text(dimension, &red_hair_man);

        // The symbol of the first cell should be a Redhair-Man, not just a Man, followed by the zero width joiner, then Redhair
        assert_eq!(grid.rows[0][0].symbol, red_hair_man)
    }

    #[test]
    fn set_row_should_pad_char_by_tab_width() {
        let dimension = Dimension {
            height: 1,
            width: 10,
        };
        let tab = '\t';
        let content = format!("{}x{}x", tab, tab);
        let tab_color = hex!("#abcdef");
        let x_color = hex!("#fafafa");
        let tab_style = Style::default().background_color(tab_color);
        let tab_cell_update = |column: usize| CellUpdate {
            position: Position { line: 0, column },
            symbol: None,
            style: tab_style,
            ..Default::default()
        };
        let x_cell_update = |column: usize| CellUpdate {
            position: Position { line: 0, column },
            symbol: None,
            style: Style::default().background_color(x_color),
            ..Default::default()
        };

        let grid = Grid::from_text(dimension, "")
            .set_row(0, None, None, &content, &Style::default())
            // Set the backgroud color of the first and second tab
            .apply_cell_update(tab_cell_update(0))
            .apply_cell_update(tab_cell_update(2))
            // Set the background color of the first and second 'x'
            .apply_cell_update(CellUpdate {
                is_cursor: true,
                ..x_cell_update(1)
            })
            .apply_cell_update(x_cell_update(3));
        let whitespace = |column: usize| PositionedCell {
            cell: Cell::from_char(' ').set_background_color(tab_color),
            position: Position { line: 0, column },
        };
        let tab = |column: usize| PositionedCell {
            cell: Cell::from_char('\t').set_background_color(tab_color),
            position: Position { line: 0, column },
        };
        let x = |column: usize, is_cursor: bool| PositionedCell {
            cell: Cell {
                is_cursor,
                ..Cell::from_char('x').set_background_color(x_color)
            },
            position: Position { line: 0, column },
        };
        let expected = [
            tab(0),
            // 3 whitespaces are added after tab, because the unicode width of tab is two
            whitespace(1),
            whitespace(2),
            whitespace(3),
            x(4, true),
            tab(5),
            whitespace(6),
            whitespace(7),
            whitespace(8),
            x(9, false),
        ]
        .to_vec();
        assert_eq!(grid.to_positioned_cells(), expected);
        let expected_cursor_position = Position { line: 0, column: 4 };
        assert_eq!(grid.get_cursor_position(), Some(expected_cursor_position));
    }

    #[test]
    fn set_row_should_pad_char_by_unicode_width() {
        use unicode_width::UnicodeWidthStr;

        let dimension = Dimension {
            height: 1,
            width: 3,
        };
        let microscope = '🔬';
        assert_eq!(UnicodeWidthStr::width(microscope.to_string().as_str()), 2); // Microscope
        let content = format!("{}x", microscope);
        let microscope_color = hex!("#abcdef");
        let x_color = hex!("#fafafa");
        let microscope_style = Style::default().background_color(microscope_color);

        let grid = Grid::from_text(dimension, "")
            .set_row(0, None, None, &content, &Style::default())
            .apply_cell_update(
                // Set the backgroud color of microscope
                CellUpdate {
                    position: Position { line: 0, column: 0 },
                    symbol: None,
                    style: microscope_style,
                    ..Default::default()
                },
            )
            .apply_cell_update(
                // Set the background color of 'x'
                CellUpdate {
                    position: Position { line: 0, column: 1 },
                    symbol: None,
                    style: Style::default().background_color(x_color),
                    ..Default::default()
                },
            );
        let expected = [
            PositionedCell {
                cell: Cell::from_char(microscope).set_background_color(microscope_color),
                position: Position { line: 0, column: 0 },
            },
            PositionedCell {
                // One whitespace is added after microscope, because the unicode width of microscope is two
                cell: Cell::from_char(' ').set_background_color(microscope_color),
                position: Position { line: 0, column: 1 },
            },
            PositionedCell {
                cell: Cell::from_char('x').set_background_color(x_color),
                position: Position { line: 0, column: 2 },
            },
        ]
        .to_vec();
        assert_eq!(grid.to_positioned_cells(), expected);
    }

    #[test]
    fn test_get_string_width() {
        assert_eq!(get_string_width("\t\t"), 8)
    }
}
