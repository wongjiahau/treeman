use std::path::PathBuf;

use isahc::prelude::*;

#[derive(Debug, PartialEq, Eq)]
pub struct GetHighlightQueryResult {
    pub query: String,
    is_cache: bool,
}

pub fn cache_dir() -> PathBuf {
    grammar::cache_dir().join("tree_sitter_highlight_queries")
}

pub fn clear_cache() -> anyhow::Result<()> {
    let path = cache_dir();
    if path.exists() {
        Ok(std::fs::remove_dir_all(path)?)
    } else {
        Ok(())
    }
}

/// Get highlight query from cache or `nvim-treesitter` repo.
pub fn get_highlight_query(language_id: &str) -> anyhow::Result<GetHighlightQueryResult> {
    let cache_dir = cache_dir();
    std::fs::create_dir_all(cache_dir.clone())?;
    let cache_path = cache_dir.join(format!("{}.scm", language_id));
    if let Ok(text) = std::fs::read_to_string(cache_path.clone()) {
        return Ok(GetHighlightQueryResult {
            query: text,
            is_cache: true,
        });
    }

    let nvim_tree_sitter_highlight_query_url = format!("https://raw.githubusercontent.com/nvim-treesitter/nvim-treesitter/master/queries/{}/highlights.scm", language_id);

    let current = isahc::get(nvim_tree_sitter_highlight_query_url)?.text()?;
    let parent = get_highlight_query_parents(&current)
        .into_iter()
        .map(|parent| -> anyhow::Result<_> { Ok(get_highlight_query(&parent)?.query) })
        .collect::<Result<Vec<_>, _>>()?
        .join("\n\n");

    let result = format!("{}\n\n{}", parent, current);
    std::fs::write(cache_path, &result)?;

    Ok(GetHighlightQueryResult {
        query: result,
        is_cache: false,
    })
}

/// This function extracts the parent of a Tree-sitter highlight query parents,
/// based on the format defined by `nvim-treesitter`.
///
/// Reference:
///   - https://github.com/nvim-treesitter/nvim-treesitter/blob/8f5513a1f2ec6ee5b378c2e32e53fc3c2a8f1e13/CONTRIBUTING.md#inheriting-languages
fn get_highlight_query_parents(content: &str) -> Vec<String> {
    regex::Regex::new(r"inherits:\s*([\w,]+)")
        .unwrap()
        .captures(&content)
        .and_then(|capture| capture.get(1))
        .map(|content| {
            content
                .as_str()
                .split(",")
                .map(|s| s.to_string())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod test_language {
    use super::*;
    #[test]
    fn test_get_highlight_query() -> anyhow::Result<()> {
        clear_cache()?;
        let result1 = get_highlight_query("tsx")?;
        assert_eq!(result1.is_cache, false);
        assert!(result1.query.contains("\"require\" @keyword.import"));
        let result2 = get_highlight_query("tsx")?;
        assert_eq!(result2.is_cache, true);
        assert_eq!(result1.query, result2.query);

        Ok(())
    }

    #[test]
    fn test_get_highlight_query_parents() {
        assert_eq!(
            get_highlight_query_parents("; inherits: ecma,jsx"),
            vec!["ecma", "jsx"],
        );
        assert_eq!(
            get_highlight_query_parents("; inherits: html"),
            vec!["html"],
        )
    }
}
