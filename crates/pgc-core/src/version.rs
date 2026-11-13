//! Server version as a number and the feature table (D-011, D-034).
//!
//! No code enumerates known versions: version checks go through [`Feature`]s, each with
//! the first `server_version_num` that has it.

use std::fmt;

/// `server_version_num`, e.g. `170002` for 17.2, `90624` for 9.6.24.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerVersion(pub u32);

/// Oldest supported server (D-010, D-034): older servers are refused.
pub const MIN_SUPPORTED: ServerVersion = ServerVersion(110000);

/// Newest major version verified with pgtc (D-034); newer servers get a warning.
pub const NEWEST_VERIFIED_MAJOR: u32 = 18;

impl ServerVersion {
    /// Major version: `17` for 170002, `9` for 90624 (9.x majors are reported as 9).
    pub fn major(self) -> u32 {
        self.0 / 10000
    }

    pub fn is_supported(self) -> bool {
        self >= MIN_SUPPORTED
    }

    /// True for a major version newer than [`NEWEST_VERIFIED_MAJOR`].
    pub fn is_unverified(self) -> bool {
        self.major() > NEWEST_VERIFIED_MAJOR
    }

    pub fn has(self, feature: Feature) -> bool {
        self.0 >= feature.since()
    }
}

impl fmt::Display for ServerVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.0;
        if v >= 100000 {
            write!(f, "{}.{}", v / 10000, v % 10000)
        } else {
            write!(f, "{}.{}.{}", v / 10000, (v / 100) % 100, v % 100)
        }
    }
}

macro_rules! features {
    ($($variant:ident = $since:literal;)*) => {
        /// Server features that differ between versions (D-011).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Feature { $($variant),* }

        impl Feature {
            pub const ALL: &'static [Feature] = &[$(Feature::$variant),*];

            /// First `server_version_num` that has the feature.
            pub fn since(self) -> u32 {
                match self { $(Feature::$variant => $since),* }
            }

            /// Name used in templates: `{% if has("NullsNotDistinct") %}`.
            pub fn name(self) -> &'static str {
                match self { $(Feature::$variant => stringify!($variant)),* }
            }
        }
    };
}

// One line per feature; templates test features, not raw version numbers.
features! {
    GeneratedColumns = 120000;
    NullsNotDistinct = 150000;
    TriggerParentId = 130000;
    LastSeqScan = 160000;
    // pg_stat_database / _conflicts columns (D-062)
    StatChecksums = 120000;
    StatSessions = 140000;
    StatLogicalSlotConflicts = 160000;
    StatParallelWorkers = 180000;
}

impl Feature {
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|f| f.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_major() {
        assert_eq!(ServerVersion(170002).to_string(), "17.2");
        assert_eq!(ServerVersion(110000).to_string(), "11.0");
        assert_eq!(ServerVersion(90624).to_string(), "9.6.24");
        assert_eq!(ServerVersion(170002).major(), 17);
        assert_eq!(ServerVersion(190000).major(), 19);
    }

    #[test]
    fn limits() {
        assert!(!ServerVersion(100023).is_supported());
        assert!(ServerVersion(110000).is_supported());
        assert!(!ServerVersion(180001).is_unverified());
        assert!(ServerVersion(190000).is_unverified());
    }

    #[test]
    fn features() {
        assert!(ServerVersion(160000).has(Feature::LastSeqScan));
        assert!(!ServerVersion(150004).has(Feature::LastSeqScan));
        assert_eq!(
            Feature::from_name("NullsNotDistinct"),
            Some(Feature::NullsNotDistinct)
        );
        assert_eq!(Feature::from_name("Nope"), None);
    }
}
