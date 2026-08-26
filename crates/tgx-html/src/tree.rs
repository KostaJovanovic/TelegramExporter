//! Desktop's emission model.
//!
//! Derived from a real export and reproduced exactly, because the pages are
//! compared line by line:
//!
//! * One space of indentation per level.
//! * **A blank line separates two consecutive *tag* lines**; text sits at
//!   column 0 with no blank line on either side of it. That one rule generates
//!   the whole document, including its `<head>`.
//! * Attributes are written in **alphabetical order** (`class` before `href`
//!   before `id` before `src` before `style` before `title`).
//!
//! The third rule is why `open`/`void` take a slice of pairs and sort it rather
//! than accepting a pre-built string: a caller writing attributes in the order
//! that reads best would diff on every tag that has two of them.

use crate::escape::attr_value;

/// Writes Desktop's indented tag/text layout into a string buffer.
pub struct Tree {
    buf: String,
    indent: usize,
    last_was_tag: bool,
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

impl Tree {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            indent: 0,
            last_was_tag: false,
        }
    }

    /// Start at a given depth — used when a fragment is spliced into a page.
    pub fn at(indent: usize) -> Self {
        Self {
            buf: String::new(),
            indent,
            last_was_tag: false,
        }
    }

    pub fn indent(&self) -> usize {
        self.indent
    }

    pub fn into_string(self) -> String {
        self.buf
    }

    pub fn as_str(&self) -> &str {
        &self.buf
    }

    fn tag_line(&mut self, line: &str) {
        if self.last_was_tag {
            self.buf.push('\n');
        }
        for _ in 0..self.indent {
            self.buf.push(' ');
        }
        self.buf.push_str(line);
        self.buf.push('\n');
        self.last_was_tag = true;
    }

    /// Write already-escaped content at column 0, as Desktop does.
    pub fn text(&mut self, raw: &str) {
        self.buf.push_str(raw);
        self.buf.push('\n');
        self.last_was_tag = false;
    }

    fn attrs(pairs: &[(&str, String)]) -> String {
        if pairs.is_empty() {
            return String::new();
        }
        let mut sorted: Vec<&(&str, String)> = pairs.iter().collect();
        sorted.sort_by_key(|(k, _)| *k);
        let body: Vec<String> = sorted
            .iter()
            .map(|(k, v)| format!("{k}=\"{}\"", attr_value(v)))
            .collect();
        format!(" {}", body.join(" "))
    }

    pub fn open(&mut self, tag: &str, attrs: &[(&str, String)]) {
        let line = format!("<{tag}{}>", Self::attrs(attrs));
        self.tag_line(&line);
        self.indent += 1;
    }

    pub fn close(&mut self, tag: &str) {
        self.indent = self.indent.saturating_sub(1);
        let line = format!("</{tag}>");
        self.tag_line(&line);
    }

    pub fn void(&mut self, tag: &str, attrs: &[(&str, String)]) {
        let line = format!("<{tag}{}/>", Self::attrs(attrs));
        self.tag_line(&line);
    }

    /// A tag wrapping one line of already-escaped content.
    pub fn leaf(&mut self, tag: &str, content: &str, attrs: &[(&str, String)]) {
        self.open(tag, attrs);
        self.text(content);
        self.close(tag);
    }

    /// A raw line emitted verbatim at the current indent, treated as a tag for
    /// blank-line purposes. Used for the doctype.
    pub fn raw_tag(&mut self, line: &str) {
        self.tag_line(line);
    }
}

/// Convenience for the very common single-attribute case.
pub fn a(key: &str, value: impl Into<String>) -> (&str, String) {
    (key, value.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_space_per_level() {
        let mut t = Tree::new();
        t.open("a", &[]);
        t.open("b", &[]);
        t.close("b");
        t.close("a");
        assert_eq!(t.as_str(), "<a>\n\n <b>\n\n </b>\n\n</a>\n");
    }

    #[test]
    fn a_blank_line_separates_two_tag_lines() {
        let mut t = Tree::new();
        t.open("div", &[]);
        t.open("div", &[]);
        assert_eq!(t.as_str(), "<div>\n\n <div>\n");
    }

    #[test]
    fn text_sits_at_column_zero_with_no_blank_lines_around_it() {
        // This is the rule that generates the whole document. Text kills the
        // pending blank line and does not get one after it either.
        let mut t = Tree::new();
        t.open("div", &[]);
        t.text("hello");
        t.close("div");
        // The closing tag returns to the opening tag's indent — the reference's
        // <script>/</script> pair both sit at two spaces.
        assert_eq!(t.as_str(), "<div>\nhello\n</div>\n");
    }

    #[test]
    fn leaf_is_open_text_close() {
        let mut t = Tree::new();
        t.leaf("title", "Exported Data", &[]);
        assert_eq!(t.as_str(), "<title>\nExported Data\n</title>\n");
    }

    #[test]
    fn attributes_are_alphabetical_not_insertion_ordered() {
        let mut t = Tree::new();
        // Deliberately supplied in the order that reads best.
        t.void(
            "meta",
            &[a("name", "viewport"), a("content", "width=device-width")],
        );
        assert_eq!(
            t.as_str(),
            "<meta content=\"width=device-width\" name=\"viewport\"/>\n"
        );
    }

    #[test]
    fn the_reference_head_ordering_is_reproduced() {
        // From the reference: <link href="css/style.css" rel="stylesheet"/>
        // and <script src="js/script.js" type="text/javascript">
        let mut t = Tree::new();
        t.void(
            "link",
            &[a("rel", "stylesheet"), a("href", "css/style.css")],
        );
        assert_eq!(
            t.as_str(),
            "<link href=\"css/style.css\" rel=\"stylesheet\"/>\n"
        );
    }

    #[test]
    fn attribute_values_are_escaped() {
        let mut t = Tree::new();
        t.open("div", &[a("title", "a\"b<c")]);
        assert!(t.as_str().contains("title=\"a&quot;b&lt;c\""));
    }

    #[test]
    fn close_never_underflows() {
        let mut t = Tree::new();
        t.close("div"); // unbalanced, but must not panic
        assert_eq!(t.as_str(), "</div>\n");
    }

    #[test]
    fn the_reference_head_is_reproduced_byte_for_byte() {
        // Transcribed from `cat -A` on the reference export. Two things here
        // are not guessable and both fall out of the one blank-line rule:
        //
        //   * the doctype is emitted as *text*, not a tag line — which is why
        //     <html> follows it with no blank line between them;
        //   * <title> is one text line at column 0, which is why the viewport
        //     <meta> after it gets no blank line either.
        let mut t = Tree::new();
        t.text("<!DOCTYPE html>");
        t.open("html", &[]);
        t.open("head", &[]);
        t.void("meta", &[a("charset", "utf-8")]);
        t.text("<title>Exported Data</title>");
        t.void(
            "meta",
            &[
                a("content", "width=device-width, initial-scale=1.0"),
                a("name", "viewport"),
            ],
        );
        t.void(
            "link",
            &[a("href", "css/style.css"), a("rel", "stylesheet")],
        );
        t.open(
            "script",
            &[a("src", "js/script.js"), a("type", "text/javascript")],
        );
        t.close("script");
        t.close("head");

        let expect = concat!(
            "<!DOCTYPE html>\n",
            "<html>\n",
            "\n",
            " <head>\n",
            "\n",
            "  <meta charset=\"utf-8\"/>\n",
            "<title>Exported Data</title>\n",
            "  <meta content=\"width=device-width, initial-scale=1.0\" name=\"viewport\"/>\n",
            "\n",
            "  <link href=\"css/style.css\" rel=\"stylesheet\"/>\n",
            "\n",
            "  <script src=\"js/script.js\" type=\"text/javascript\">\n",
            "\n",
            "  </script>\n",
            "\n",
            " </head>\n",
        );
        assert_eq!(t.as_str(), expect);
    }
}
