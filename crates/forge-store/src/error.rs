use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("open the database {}", database.display())]
    Open {
        database: PathBuf,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("the store writer stopped")]
    WriterStopped,
    #[error("the store writer dropped the write")]
    WriteDropped(#[source] tokio::sync::oneshot::error::RecvError),
    #[error("the store's authorization worker dropped the check")]
    AuthorizationDropped(#[source] tokio::sync::oneshot::error::RecvError),
    #[error("wait for a store read slot")]
    ReadSlot(#[source] tokio::sync::AcquireError),
    #[error("the store's blocking task stopped")]
    Task(#[source] tokio::task::JoinError),
    #[error("SQLite failed")]
    Sql(#[source] Box<rusqlite::Error>),
    #[error("decode a stored {kind}")]
    Decode {
        kind: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("encode a {kind} for storage")]
    Encode {
        kind: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sql(Box::new(error))
    }
}

impl StorageError {
    pub(crate) fn decode<T>(source: serde_json::Error) -> Self {
        Self::Decode {
            kind: std::any::type_name::<T>(),
            source,
        }
    }

    pub(crate) fn encode<T: ?Sized>(source: serde_json::Error) -> Self {
        Self::Encode {
            kind: std::any::type_name::<T>(),
            source,
        }
    }
}

pub(crate) enum Staged<E> {
    Failed(E),
    Storage(StorageError),
}

impl<E> From<StorageError> for Staged<E> {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

macro_rules! operation_error {
    ($name:ident($cause:ty), $message:literal $(, $field:ident: $type:ty)* $(,)?) => {
        #[derive(Debug)]
        pub struct $name {
            $(pub $field: $type,)*
            pub cause: Box<$cause>,
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, $message $(, $field = self.$field)*)
            }
        }

        impl std::error::Error for $name {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&*self.cause)
            }
        }
    };
}

pub(crate) use operation_error;
