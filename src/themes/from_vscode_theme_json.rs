use super::{Color, DiagnosticStyles, HighlightName, Theme, UiStyles};
use crate::{
    style::{fg, Style},
    themes::SyntaxStyles,
};
use itertools::Itertools;
use my_proc_macros::hex;
use shared::download::cache_download;

#[derive(serde::Deserialize)]
struct ZedThemeManiftest {
    themes: Vec<ZedTheme>,
}

#[derive(serde::Deserialize)]
struct ZedTheme {
    name: String,
    style: ZedThemeStyles,
    appearance: Appearance,
}

#[derive(serde::Deserialize, PartialEq)]
#[serde(rename_all(deserialize = "lowercase"))]
enum Appearance {
    Light,
    Dark,
}

#[derive(serde::Deserialize)]
struct ZedThemeStyles {
    syntax: ZedThemeStyleSyntax,
    #[serde(rename(deserialize = "editor.foreground"))]
    editor_foreground: String,
    #[serde(rename(deserialize = "editor.background"))]
    editor_background: String,
    #[serde(rename(deserialize = "editor.line_number"))]
    editor_line_number: String,
    #[serde(rename(deserialize = "status_bar.background"))]
    status_bar_background: Option<String>,
    #[serde(rename(deserialize = "tab_bar.background"))]
    tab_bar_background: Option<String>,
    #[serde(rename(deserialize = "search.match_background"))]
    search_match_background: Option<String>,
    text: String,
    border: String,
    #[serde(rename(deserialize = "text.accent"))]
    text_accent: Option<String>,
    #[serde(rename(deserialize = "text.muted"))]
    text_muted: Option<String>,
    players: Vec<ZedThemeStylesPlayer>,
    default: Option<String>,
    error: Option<String>,
    warning: Option<String>,
    information: Option<String>,
    hint: Option<String>,
    #[serde(rename(deserialize = "conflict.background"))]
    conflict_background: Option<String>,
}

#[derive(serde::Deserialize)]
struct ZedThemeStylesPlayer {
    selection: String,
    cursor: String,
}

#[derive(serde::Deserialize)]
struct ZedThemeStyleSyntax {
    keyword: Option<ZedThemeStyle>,
    variable: Option<ZedThemeStyle>,
    function: Option<ZedThemeStyle>,
    attribute: Option<ZedThemeStyle>,
    tag: Option<ZedThemeStyle>,
    comment: Option<ZedThemeStyle>,
    constant: Option<ZedThemeStyle>,
    string: Option<ZedThemeStyle>,
    #[serde(rename(deserialize = "string.escape"))]
    string_escape: Option<ZedThemeStyle>,
    #[serde(rename(deserialize = "string.regex"))]
    string_regex: Option<ZedThemeStyle>,
    #[serde(rename(deserialize = "string.special"))]
    string_special: Option<ZedThemeStyle>,
    r#type: Option<ZedThemeStyle>,
    #[serde(rename(deserialize = "comment.documentation"))]
    comment_documentation: Option<ZedThemeStyle>,
    boolean: Option<ZedThemeStyle>,
    number: Option<ZedThemeStyle>,
    operator: Option<ZedThemeStyle>,
    #[serde(rename(deserialize = "punctuation.bracket"))]
    punctuation_bracket: Option<ZedThemeStyle>,
    #[serde(rename(deserialize = "punctuation.special"))]
    punctuation_special: Option<ZedThemeStyle>,
    #[serde(rename(deserialize = "punctuation.delimiter"))]
    punctuation_delimiter: Option<ZedThemeStyle>,
}

#[derive(serde::Deserialize, Clone)]
struct ZedThemeStyle {
    color: String,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Scope {
    String(String),
    Array(Vec<String>),
}
pub fn from_zed_theme(url: &str) -> anyhow::Result<Vec<Theme>> {
    let json_str = cache_download(
        url,
        "zed-themes",
        &std::path::PathBuf::from(url)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string(),
    )?;
    let manifest: ZedThemeManiftest = serde_json5::from_str(&json_str).unwrap();
    Ok(manifest
        .themes
        .into_iter()
        .flat_map(|theme| -> anyhow::Result<Theme> {
            let background = Color::from_hex(&theme.style.editor_background)?;
            let from_hex = |hex: &str| -> anyhow::Result<_> {
                Ok(Color::from_hex(&hex)?.apply_alpha(background))
            };
            let from_some_hex = |hex: Option<String>| {
                hex.and_then(|hex| Some(Color::from_hex(&hex).ok()?.apply_alpha(background)))
            };
            let text_color = from_hex(&theme.style.text)?;
            let to_style = |highlight_name: HighlightName, style: Option<ZedThemeStyle>| {
                style.and_then(|style| Some((highlight_name, fg(from_hex(&style.color).ok()?))))
            };
            let primary_selection_background = theme
                .style
                .players
                .first()
                .and_then(|player| from_hex(&player.selection).ok())
                .unwrap_or_default();
            let cursor = {
                let background = theme
                    .style
                    .players
                    .first()
                    .and_then(|player| from_hex(&player.cursor).ok())
                    .unwrap_or_default();
                let foreground = background.get_contrasting_color();
                Style::new()
                    .background_color(background)
                    .foreground_color(foreground)
            };
            let parent_lines_background =
                primary_selection_background.apply_custom_alpha(background, 0.25);
            let text_accent = theme
                .style
                .text_accent
                .and_then(|hex| from_hex(&hex).ok())
                .unwrap_or_else(|| text_color);
            Ok(Theme {
                name: theme.name,
                syntax: SyntaxStyles::new(&{
                    use HighlightName::*;

                    [
                        to_style(Variable, theme.style.syntax.variable),
                        to_style(Keyword, theme.style.syntax.keyword.clone()),
                        to_style(KeywordModifier, theme.style.syntax.keyword),
                        to_style(Function, theme.style.syntax.function),
                        to_style(Type, theme.style.syntax.r#type.clone()),
                        to_style(TypeBuiltin, theme.style.syntax.r#type),
                        to_style(String, theme.style.syntax.string),
                        to_style(StringEscape, theme.style.syntax.string_escape),
                        to_style(StringRegexp, theme.style.syntax.string_regex),
                        to_style(StringSpecial, theme.style.syntax.string_special),
                        to_style(Comment, theme.style.syntax.comment),
                        to_style(Constant, theme.style.syntax.constant.clone()),
                        to_style(ConstantBuiltin, theme.style.syntax.constant),
                        to_style(Tag, theme.style.syntax.tag),
                        to_style(TagAttribute, theme.style.syntax.attribute),
                        to_style(Boolean, theme.style.syntax.boolean),
                        to_style(Number, theme.style.syntax.number),
                        to_style(Operator, theme.style.syntax.operator),
                        to_style(PunctuationBracket, theme.style.syntax.punctuation_bracket),
                        to_style(
                            PunctuationDelimiter,
                            theme.style.syntax.punctuation_delimiter,
                        ),
                        to_style(PunctuationSpecial, theme.style.syntax.punctuation_special),
                        to_style(
                            CommentDocumentation,
                            theme.style.syntax.comment_documentation,
                        ),
                    ]
                    .into_iter()
                    .flatten()
                    .collect_vec()
                }),
                ui: UiStyles {
                    global_title: Style::new()
                        .foreground_color(text_color)
                        .set_some_background_color(from_some_hex(
                            theme.style.status_bar_background,
                        )),
                    window_title: Style::new()
                        .foreground_color(text_color)
                        .set_some_background_color(from_some_hex(theme.style.tab_bar_background)),
                    parent_lines_background,
                    jump_mark_odd: Style::new()
                        .background_color(hex!("#b5485d"))
                        .foreground_color(hex!("#ffffff")),
                    jump_mark_even: Style::new()
                        .background_color(hex!("#84b701"))
                        .foreground_color(hex!("#ffffff")),
                    background_color: background,
                    text_foreground: from_hex(&theme.style.text)?,
                    primary_selection_background,
                    primary_selection_anchor_background: primary_selection_background,
                    primary_selection_secondary_cursor: cursor,
                    secondary_selection_background: primary_selection_background,
                    secondary_selection_anchor_background: primary_selection_background,
                    secondary_selection_primary_cursor: cursor,
                    secondary_selection_secondary_cursor: cursor,
                    line_number: Style::new()
                        .set_some_foreground_color(from_hex(&theme.style.editor_line_number).ok()),
                    border: Style::new()
                        .foreground_color(from_hex(&theme.style.border).ok().unwrap_or(text_color))
                        .background_color(background),
                    bookmark: Style::new()
                        .set_some_background_color(from_some_hex(theme.style.conflict_background)),
                    possible_selection_background: from_some_hex(
                        theme.style.search_match_background,
                    )
                    .unwrap_or_default(),
                    keymap_hint: Style::new().underline(text_accent),
                    keymap_key: Style::new().bold().foreground_color(text_accent),
                    keymap_arrow: Style::new().set_some_foreground_color(
                        theme.style.text_muted.and_then(|hex| from_hex(&hex).ok()),
                    ),
                    fuzzy_matched_char: Style::new()
                        .foreground_color(text_accent)
                        .underline(text_accent),
                },
                diagnostic: {
                    let default = DiagnosticStyles::default();
                    let undercurl = |hex: Option<String>, default: Style| {
                        from_some_hex(hex)
                            .map(|color| Style::new().undercurl(color))
                            .unwrap_or(default)
                    };
                    DiagnosticStyles {
                        error: undercurl(theme.style.error, default.error),
                        warning: undercurl(theme.style.warning, default.error),
                        information: undercurl(theme.style.information, default.error),
                        hint: undercurl(theme.style.hint, default.error),
                        default: undercurl(theme.style.default, default.error),
                    }
                },
                hunk: if theme.appearance == Appearance::Light {
                    super::HunkStyles::light()
                } else {
                    super::HunkStyles::dark()
                },
            })
        })
        .collect_vec())
}

#[cfg(test)]
mod test_from_vscode_theme_json {
    #[test]
    fn test() -> anyhow::Result<()> {
        super::from_zed_theme(
            "https://raw.githubusercontent.com/zed-industries/zed/main/assets/themes/one/one.json",
        )?;
        Ok(())
    }
}
