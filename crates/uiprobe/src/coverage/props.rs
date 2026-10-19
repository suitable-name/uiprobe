//! The property names the inventory scanner reads as an element's label, beyond the std list.
//!
//! The std list (`accessible-label`, `text`, `label`, `title`, `hint`, `name`) fits apps that name
//! the visible text of a wrapper component with one of those. An app whose wrapper components
//! carry the label the screen reader announces in a property of their own (for example, in
//! Indicatrix: `reader_label` on `TipButton` / `TipField` and `slider_label` on `SliderRow`, where
//! the running accessible label is the reader label when the site sets one and `text` otherwise)
//! needs two extension points, both set on [`super::Coverage`]:
//!
//! * `label_props`: extra names read as labels (a site's literal under one of them becomes its
//!   [`super::inventory::Label`]);
//! * `reader_props`: names whose LITERAL wins over `text` on a non-std wrapper site. They are read
//!   as labels too, so naming a property here is enough.
//!
//! Names are matched exactly as spelled in the source (`reader_label` does not match
//! `reader-label`), like the std names.

/// The configured extra label property names.
#[derive(Debug, Clone, Default)]
pub struct LabelProps {
    /// Extra names read as labels (`Coverage::label_props`).
    pub(crate) extra: Vec<String>,
    /// Names whose literal beats `text` on non-std wrapper sites (`Coverage::reader_props`).
    pub(crate) reader: Vec<String>,
}

impl LabelProps {
    /// Every configured name read as a label (extra names, then reader names).
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.extra.iter().chain(&self.reader).map(String::as_str)
    }
}
