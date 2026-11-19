use rusqlite::Connection;
use std::path::Path;
use std::sync::Arc;

type Open = dyn Fn(&Path) -> rusqlite::Result<Connection> + Send + Sync;

#[derive(Clone)]
pub struct Connections(Arc<Open>);

impl Connections {
    #[must_use]
    pub fn system() -> Self {
        Self::new(|path| Connection::open(path))
    }

    pub fn new(
        open: impl Fn(&Path) -> rusqlite::Result<Connection> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(open))
    }

    pub(crate) fn open(&self, path: &Path) -> rusqlite::Result<Connection> {
        (self.0)(path)
    }
}

impl std::fmt::Debug for Connections {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connections").finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct Environment {
    pub connections: Connections,
}

impl Environment {
    #[must_use]
    pub fn system() -> Self {
        Self {
            connections: Connections::system(),
        }
    }
}
