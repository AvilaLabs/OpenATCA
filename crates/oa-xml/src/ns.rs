//! Namespace resolution over a parsed [`Doc`]. Core (pure); allocates nothing.
//!
//! Each element carries the head of a chain of the declarations in scope (see [`Doc`]), so a
//! lookup follows at most [`crate::Limits::max_ns_bindings`] links (64 by default), with no
//! walk over ancestors and no scan of unrelated attributes.

use crate::{bytes_of, AttrIx, Doc, ElemIx, NsIx, Span};

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
// Budget: returned per lookup, never stored in quantity; a tag and one span.
const _: () = assert!(core::mem::size_of::<Ns>() == 12);

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

fn lookup(doc: &Doc<'_>, e: ElemIx, prefix: Span) -> Ns {
    let Some(want) = bytes_of(doc.input, prefix) else {
        return Ns::None;
    };
    if want == b"xml" {
        return Ns::Xml;
    }
    let mut n = doc
        .elem_ns_head
        .get(e.0 as usize)
        .copied()
        .unwrap_or(NsIx::NONE);
    // A chain only ever points backwards, so the walk is at most one step per row.
    let mut steps = doc.ns_prefix.len();
    while n != NsIx::NONE && steps > 0 {
        steps -= 1;
        let i = n.0 as usize;
        let (Some(&np), Some(&nu)) = (doc.ns_prefix.get(i), doc.ns_uri.get(i)) else {
            break;
        };
        if bytes_of(doc.input, np) == Some(want) {
            return if nu.len == 0 { Ns::None } else { Ns::Uri(nu) };
        }
        n = doc.ns_next.get(i).copied().unwrap_or(NsIx::NONE);
    }
    Ns::None
}
