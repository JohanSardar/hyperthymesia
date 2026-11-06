use chrono::{DateTime, Utc};
use sqlx::{
    Decode, Encode, Sqlite, Type,
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentsBuffer, SqliteTypeInfo, SqliteValueRef},
};

/// SQLite timestamps are INTEGER microseconds since the Unix epoch. JSON
/// timestamps remain the original strings; this type is only for SQL columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ts(pub i64);

impl Ts {
    pub fn now() -> Self {
        Self::from(Utc::now())
    }
}

impl From<DateTime<Utc>> for Ts {
    fn from(value: DateTime<Utc>) -> Self {
        Self(value.timestamp_micros())
    }
}

impl From<Ts> for DateTime<Utc> {
    fn from(value: Ts) -> Self {
        DateTime::from_timestamp_micros(value.0).expect("stored timestamp in chrono range")
    }
}

impl Type<Sqlite> for Ts {
    fn type_info() -> SqliteTypeInfo {
        <i64 as Type<Sqlite>>::type_info()
    }
    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <i64 as Type<Sqlite>>::compatible(ty)
    }
}

impl Encode<'_, Sqlite> for Ts {
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        <i64 as Encode<Sqlite>>::encode_by_ref(&self.0, buf)
    }
}

impl<'r> Decode<'r, Sqlite> for Ts {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        <i64 as Decode<Sqlite>>::decode(value).map(Self)
    }
}
