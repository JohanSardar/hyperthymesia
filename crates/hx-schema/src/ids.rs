use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! string_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(
    /// e.g. `macos-agent:mbp-main`, `t3code:mbp-main:statev2`.
    ConnectorInstance
);
string_id!(SourceKey);
string_id!(Revision);
string_id!(DeviceId);
string_id!(
    /// One boot session of a device. Monotonic clocks are only comparable within a boot.
    BootId
);

impl Revision {
    /// The revision used by append-only sources.
    pub fn append_only() -> Self {
        Self("0".into())
    }
    /// Revision derived from content, for snapshot sources without native revisions.
    pub fn digest(bytes: &[u8]) -> Self {
        Self(format!("sha256:{}", crate::sha256_hex(bytes)))
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ObservationId(#[schemars(with = "String")] pub ulid::Ulid);

impl ObservationId {
    pub fn generate() -> Self {
        Self(ulid::Ulid::generate())
    }
}

impl fmt::Display for ObservationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct Kind(String);

impl Kind {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// `namespace.name[.more]`, lowercase ascii, digits and `_`.
    pub fn is_valid(s: &str) -> bool {
        let mut parts = 0;
        for p in s.split('.') {
            if p.is_empty()
                || !p
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
            {
                return false;
            }
            parts += 1;
        }
        parts >= 2
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
