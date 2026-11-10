//! Nerd Font icons (D-005). Decorative only: with icons off every icon becomes empty
//! space of the same width, so the layout does not change.

/// Icons used by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Object view (default tab, object tabs).
    ObjectView,
    /// Query tab.
    Query,
    Server,
    Database,
    Folder,
    Table,
    View,
    Function,
    Column,
    Role,
    /// Any other database object.
    Object,
}

/// Icon followed by a space (two cells), or two spaces when icons are off.
pub fn icon(icon: Icon, enabled: bool) -> &'static str {
    if !enabled {
        return "  ";
    }
    match icon {
        Icon::ObjectView => "\u{f1c9} ", // nf-fa-file_code_o
        Icon::Query => "\u{f120} ",      // nf-fa-terminal
        Icon::Server => "\u{f233} ",     // nf-fa-server
        Icon::Database => "\u{f1c0} ",   // nf-fa-database
        Icon::Folder => "\u{f07b} ",     // nf-fa-folder
        Icon::Table => "\u{f0ce} ",      // nf-fa-table
        Icon::View => "\u{f06e} ",       // nf-fa-eye
        Icon::Function => "\u{f0ad} ",   // nf-fa-wrench
        Icon::Column => "\u{f0db} ",     // nf-fa-columns
        Icon::Role => "\u{f007} ",       // nf-fa-user
        Icon::Object => "\u{f111} ",     // nf-fa-circle
    }
}

/// Every icon, always drawn (also with icons off): the sample row next to the icons
/// checkbox in Settings. Icons mean the terminal has a Nerd Font; boxes or question
/// marks mean it has not (D-005).
pub const ALL: [Icon; 11] = [
    Icon::Server,
    Icon::Database,
    Icon::Folder,
    Icon::Table,
    Icon::View,
    Icon::Function,
    Icon::Column,
    Icon::Role,
    Icon::Query,
    Icon::ObjectView,
    Icon::Object,
];

pub fn sample() -> String {
    ALL.iter().map(|&i| icon(i, true)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_width_on_and_off() {
        for i in [
            Icon::ObjectView,
            Icon::Query,
            Icon::Server,
            Icon::Database,
            Icon::Folder,
            Icon::Table,
            Icon::View,
            Icon::Function,
            Icon::Column,
            Icon::Role,
            Icon::Object,
        ] {
            assert_eq!(
                icon(i, true).chars().count(),
                icon(i, false).chars().count()
            );
            assert_ne!(icon(i, true), icon(i, false));
        }
    }
}
