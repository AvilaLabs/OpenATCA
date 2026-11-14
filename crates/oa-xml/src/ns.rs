//! Namespace resolution over a parsed [`Doc`]. Core (pure); allocates nothing.

use crate::{span_bytes, AttrIx, Doc, ElemIx, Span};

/// The namespace an element or attribute name is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ns {
    /// No namespace (no default declared, `xmlns=""`, or an unprefixed attribute).
    None,
    /// The reserved `xml` prefix.
    Xml,
    /// A declared namespace: the span of its URI in the input.
    Uri(Span),
}

/// The namespace of element `e`, from its prefix and the declarations in scope.
pub fn resolve_elem_ns(doc: &Doc<'_>, e: ElemIx) -> Ns {
    match doc.elem_prefix.get(e.0 as usize) {
        Some(&prefix) => lookup(doc, e, prefix),
        None => Ns::None,
    }
}

/// The namespace of attribute `a` of element `e`. An unprefixed attribute is in no namespace,
/// whatever default namespace is declared.
pub fn resolve_attr_ns(doc: &Doc<'_>, e: ElemIx, a: AttrIx) -> Ns {
    match doc.attr_prefix.get(a.0 as usize) {
        Some(&prefix) if prefix.len > 0 => lookup(doc, e, prefix),
        _ => Ns::None,
    }
}

fn lookup(doc: &Doc<'_>, start: ElemIx, prefix: Span) -> Ns {
    let want = span_bytes(doc.input, prefix);
    if want == b"xml" {
        return Ns::Xml;
    }
    let mut e = start;
    // Parents precede children, so the walk is at most one step per row.
    let mut steps = doc.elem_parent.len();
    while e != ElemIx::NONE && steps > 0 {
        steps -= 1;
        let i = e.0 as usize;
        let first = doc.elem_first_attr.get(i).map_or(0, |a| a.0 as usize);
        let count = doc.elem_attr_count.get(i).map_or(0, |&c| c as usize);
        for a in first..first.saturating_add(count) {
            let (Some(&ap), Some(&an), Some(&av)) = (
                doc.attr_prefix.get(a),
                doc.attr_name.get(a),
                doc.attr_value.get(a),
            ) else {
                continue;
            };
            let name = span_bytes(doc.input, an);
            let pre = span_bytes(doc.input, ap);
            let hit = if want.is_empty() {
                pre.is_empty() && name == b"xmlns"
            } else {
                pre == b"xmlns" && name == want
            };
            if hit {
                return if av.len == 0 { Ns::None } else { Ns::Uri(av) };
            }
        }
        e = doc.elem_parent.get(i).copied().unwrap_or(ElemIx::NONE);
    }
    Ns::None
}
