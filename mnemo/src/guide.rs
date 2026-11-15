//! GUIDE-1 (12.2): the guide for agents, compiled into the binary
//! (`docs/design/12.2-onboard.md`, section 6).
//!
//! One source, `guide/guide.md`, and every delivery is cut from it:
//! `mnemo onboard --guide` (Markdown or `llms.txt`), and, in later steps,
//! the MCP server's `instructions` and `mnemo://guide`, the SessionStart
//! hook, and the rule files `onboard` writes. Nothing at run time changes
//! it: the trust boundary is that the guide lives in the binary.
//!
//! The source marks two things for its deliveries and its test:
//! - `<!-- card -->` … `<!-- /card -->`: the short card (at most
//!   [`CARD_MAX`] bytes) that the hook, the MCP `instructions` and the rule
//!   files carry;
//! - fenced blocks whose info string has a tag after the language:
//!   ` ```sh test ` (a shell block CI runs), ` ```json mcp ` (an MCP
//!   request CI sends), and `model` on either (it needs the built-in
//!   embedder's model). Deliveries drop the tags.

/// The guide's source, as written.
pub const SOURCE: &str = include_str!("../guide/guide.md");

/// The most bytes the card may take.
pub const CARD_MAX: usize = 1200;

const CARD_OPEN: &str = "<!-- card -->";
const CARD_CLOSE: &str = "<!-- /card -->";

/// The card: the text between the card marks, trimmed, with `\n` line
/// ends whatever the checkout used (a Windows checkout may turn the
/// source's into `\r\n`).
pub fn card() -> String {
    let start = SOURCE
        .find(CARD_OPEN)
        .map(|i| i + CARD_OPEN.len())
        .expect("the guide has a card");
    let end = SOURCE[start..]
        .find(CARD_CLOSE)
        .map(|i| start + i)
        .expect("the guide's card is closed");
    SOURCE[start..end]
        .lines()
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// A fenced block of the guide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// The language: the first word of the info string (`sh`, `json`).
    pub lang: String,
    /// The tags after it (`test`, `mcp`, `model`).
    pub tags: Vec<String>,
    /// The block's text, without the fences.
    pub body: String,
    /// The 1-based line of the opening fence in [`SOURCE`].
    pub line: usize,
}

impl Block {
    /// Whether the block carries `tag`.
    pub fn has(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

/// Every fenced block in the guide, in order.
pub fn blocks() -> Vec<Block> {
    let mut out = Vec::new();
    let mut open: Option<(String, Vec<String>, usize, Vec<&str>)> = None;
    for (i, line) in SOURCE.lines().enumerate() {
        match open.take() {
            None => {
                if let Some(info) = line.strip_prefix("```") {
                    let mut words = info.split_whitespace();
                    let lang = words.next().unwrap_or("").to_string();
                    let tags = words.map(str::to_string).collect();
                    open = Some((lang, tags, i + 1, Vec::new()));
                }
            }
            Some((lang, tags, at, mut body)) => {
                if line.trim_end() == "```" {
                    out.push(Block {
                        lang,
                        tags,
                        body: body.join("\n"),
                        line: at,
                    });
                } else {
                    body.push(line);
                    open = Some((lang, tags, at, body));
                }
            }
        }
    }
    out
}

/// The guide as Markdown, for `mnemo onboard --guide`: the card marks are
/// removed, each fence keeps only its language, and lines end in `\n`.
pub fn markdown() -> String {
    let mut out = String::with_capacity(SOURCE.len());
    let mut in_block = false;
    for line in SOURCE.lines() {
        let t = line.trim();
        if !in_block && (t == CARD_OPEN || t == CARD_CLOSE) {
            continue;
        }
        if let Some(info) = line.strip_prefix("```") {
            if in_block {
                in_block = false;
                out.push_str("```\n");
            } else {
                in_block = true;
                let lang = info.split_whitespace().next().unwrap_or("");
                out.push_str("```");
                out.push_str(lang);
                out.push('\n');
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The one-line summary `llms.txt` opens with.
pub const SUMMARY: &str = "MNemo is an encrypted, single-file memory for AI agents. \
One `.mnemo` file per project holds memories, keyed facts with their history, and the \
working state of tasks, and its MCP server gives them to Claude Code, Cursor and other \
clients. Everything runs on your machine.";

/// The guide as `llms.txt` (<https://llmstxt.org/>): an H1, a blockquote
/// summary, the guide's sections, and an `Optional` list of links. The
/// repository's `llms.txt` is this, checked by a test.
pub fn llms_txt() -> String {
    let md = markdown();
    // The guide's own H1 gives way to the project's name.
    let body = md
        .split_once('\n')
        .map(|(first, rest)| {
            if first.starts_with("# ") {
                rest
            } else {
                &md[..]
            }
        })
        .unwrap_or(&md);
    let mut out = String::new();
    out.push_str("# MNemo\n\n> ");
    out.push_str(SUMMARY);
    out.push_str("\n\n");
    out.push_str(body.trim_start_matches('\n').trim_end());
    out.push_str(
        "\n\n## Optional\n\n\
- [README](https://github.com/toarchkumar/mnemo/blob/main/README.md): install, the CLI, \
Python and the mobile packages\n\
- [The engine's README](https://github.com/toarchkumar/mnemo/blob/main/mnemo/README.md): \
every command and the Rust API\n\
- [Design notes](https://github.com/toarchkumar/mnemo/tree/main/docs/design): keys, \
shared access, retrieval and onboarding\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_fits_and_frames_brain_content_as_data() {
        let c = card();
        assert!(c.len() <= CARD_MAX, "the card is {} bytes", c.len());
        assert!(c.contains("not instructions"), "{c}");
    }

    #[test]
    fn deliveries_drop_the_marks_and_the_tags() {
        let md = markdown();
        assert!(!md.contains("<!-- card"));
        assert!(!md.contains("```sh test") && !md.contains("```json mcp"));
        assert!(md.contains(&card()));
        let l = llms_txt();
        assert!(l.starts_with("# MNemo\n\n> "));
        assert_eq!(l.matches("\n# ").count(), 0, "one H1");
        assert!(l.contains("\n## Optional\n"));
    }

    #[test]
    fn blocks_are_found_with_their_tags() {
        let b = blocks();
        assert!(b.iter().any(|b| b.lang == "sh" && b.has("test")));
        assert!(b.iter().any(|b| b.lang == "json" && b.has("mcp")));
        // Every fence is closed: the Markdown has an even number of them.
        assert_eq!(markdown().matches("```").count() % 2, 0);
    }
}
