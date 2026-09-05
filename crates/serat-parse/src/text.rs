//! Collecting a block's plain text from its inline descendants.
//!
//! `Block::text` is documented as "plain text of the block, with inline markup
//! removed" — this is what produces that string. Only the inline node kinds that
//! carry readable text contribute; the rest (emphasis, links, wikilinks and so on)
//! are transparent wrappers walked through for their own text, which is what turns
//! `**bold** and [[a link]]` into `bold and a link` rather than reproducing the
//! markup.

use comrak::nodes::{Node, NodeValue};

/// Collect the readable text of every descendant of `node`, depth-first, in
/// document order.
pub(crate) fn collect<'a>(node: Node<'a>) -> String {
    let mut out = String::new();
    collect_into(node, &mut out);
    out
}

pub(crate) fn collect_into<'a>(node: Node<'a>, out: &mut String) {
    match &node.data().value {
        NodeValue::Text(text) => out.push_str(text),
        NodeValue::Code(code) => out.push_str(&code.literal),
        NodeValue::HtmlInline(html) => out.push_str(html),
        NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
        // Everything else that can appear inline (Emph, Strong, Link, Image,
        // WikiLink, Strikethrough, Superscript, Subscript, Underline, Highlight,
        // Insert, FootnoteReference, Math, Raw, ...) carries no text of its own here
        // — its children are what has the text, so falling through to the recursion
        // below is correct for all of them without naming each one.
        _ => {}
    }
    for child in node.children() {
        collect_into(child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::parse;

    #[test]
    fn emphasis_and_links_contribute_only_their_text() {
        let doc = parse("Some **bold** and a [link](https://example.com) here.\n");
        let paragraph = doc.first_child().expect("paragraph");
        assert_eq!(collect(paragraph), "Some bold and a link here.");
    }

    #[test]
    fn a_soft_break_becomes_a_space_rather_than_disappearing() {
        let doc = parse("line one\nline two\n");
        let paragraph = doc.first_child().expect("paragraph");
        assert_eq!(collect(paragraph), "line one line two");
    }
}
