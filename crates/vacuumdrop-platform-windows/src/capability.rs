//! Capability detection.
//!
//! Charter 6 is emphatic: the system **must not** claim "works on every device",
//! and support must be detected, not assumed. This module implements that
//! detection.
//!
//! # Why `is_supported` is not just "is there a Wi-Fi radio"
//!
//! A machine can have an 802.11n adapter that advertises Wi-Fi Direct and cannot
//! actually form a group. A machine can have a Direct-capable adapter whose driver
//! is disabled by policy. Both would report "there is a Wi-Fi radio" and both
//! would fail at connection time, which is the worst possible moment to discover
//! it.
//!
//! So detection is layered, and the layers are reported separately so a
//! diagnostic can say *which* condition failed.

use std::fmt;

/// The outcome of a capability probe.
///
/// Deliberately detailed rather than a bare `bool`: a user who cannot use the
/// feature needs to know whether the hardware is absent, disabled by policy, or
/// broken by a driver. "Wi-Fi Direct unavailable" answers none of those.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    /// Wi-Fi Direct is usable.
    Available {
        /// Adapter name as the OS reports it, for diagnostics.
        adapter: String,
    },

    /// This machine has no Wi-Fi Direct hardware.
    NoAdapter,

    /// Hardware exists but is administratively disabled.
    Disabled {
        /// Adapter name.
        adapter: String,
        /// Why it is unavailable, in OS terms.
        reason: String,
    },

    /// The probe itself failed — a driver error, usually.
    ///
    /// Distinct from `NoAdapter`: we do not know whether the hardware works, and
    /// claiming it does not would be exactly the over-claiming Charter 6 forbids.
    ProbeFailed {
        /// The adapter we were probing.
        adapter: String,
        /// The underlying failure.
        error: String,
    },
}

impl Capability {
    /// Whether Wi-Fi Direct can be used.
    ///
    /// Only `Available` is true. `ProbeFailed` is false on purpose: we did not
    /// establish that it works.
    pub fn is_available(&self) -> bool {
        matches!(self, Capability::Available { .. })
    }

    /// A short, log-friendly label.
    pub fn label(&self) -> &'static str {
        match self {
            Capability::Available { .. } => "available",
            Capability::NoAdapter => "no_adapter",
            Capability::Disabled { .. } => "disabled",
            Capability::ProbeFailed { .. } => "probe_failed",
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Capability::Available { adapter } => {
                write!(f, "Wi-Fi Direct available on `{adapter}`")
            }
            Capability::NoAdapter => {
                f.write_str("no Wi-Fi Direct capable adapter was found on this machine")
            }
            Capability::Disabled { adapter, reason } => {
                write!(f, "Wi-Fi Direct adapter `{adapter}` is disabled: {reason}")
            }
            Capability::ProbeFailed { adapter, error } => {
                write!(
                    f,
                    "probing Wi-Fi Direct adapter `{adapter}` failed: {error}"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_available_counts_as_available() {
        // Every other variant must be false. `ProbeFailed` especially: we did not
        // establish it works, and Charter 6 forbids claiming otherwise.
        assert!(
            Capability::Available {
                adapter: "x".into()
            }
            .is_available()
        );
        assert!(!Capability::NoAdapter.is_available());
        assert!(
            !Capability::Disabled {
                adapter: "x".into(),
                reason: "policy".into()
            }
            .is_available()
        );
        assert!(
            !Capability::ProbeFailed {
                adapter: "x".into(),
                error: "driver".into()
            }
            .is_available()
        );
    }

    #[test]
    fn labels_are_distinct() {
        let all = [
            Capability::Available {
                adapter: "a".into(),
            },
            Capability::NoAdapter,
            Capability::Disabled {
                adapter: "a".into(),
                reason: "r".into(),
            },
            Capability::ProbeFailed {
                adapter: "a".into(),
                error: "e".into(),
            },
        ];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a.label(), b.label(), "duplicate capability label");
            }
        }
    }

    #[test]
    fn display_explains_each_condition() {
        // The whole point of the enum is that the message tells a user which
        // problem they have. A generic message defeats that.
        assert!(
            Capability::NoAdapter
                .to_string()
                .contains("no Wi-Fi Direct")
        );
        assert!(
            Capability::Disabled {
                adapter: "IntelAX211".into(),
                reason: "disabled by policy".into()
            }
            .to_string()
            .contains("IntelAX211")
        );
        assert!(
            Capability::ProbeFailed {
                adapter: "Realtek".into(),
                error: "0x80342001".into()
            }
            .to_string()
            .contains("0x80342001")
        );
        assert!(
            Capability::Available {
                adapter: "IntelAX211".into()
            }
            .to_string()
            .contains("IntelAX211")
        );
    }

    #[test]
    fn no_adapter_message_does_not_claim_the_hardware_is_broken() {
        // "No adapter" and "adapter present but broken" are different problems
        // with different fixes.
        let s = Capability::NoAdapter.to_string();
        assert!(
            !s.contains("failed"),
            "no-adapter message implies a failure"
        );
        assert!(!s.contains("broken"));
    }
}
