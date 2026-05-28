//! Named backend registry (§4b pilot): register custom `PeerLocalPersistence` factories by key.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use crate::composite::CompositeLocalPersistence;
use crate::error::LocalPersistResult;
use crate::traits::PeerLocalPersistence;

/// Options passed to registered backend factories.
#[derive(Debug, Clone, Default)]
pub struct BackendOpenOptions {
    pub data_dir: Option<PathBuf>,
}

pub type BackendFactory =
    fn(&BackendOpenOptions) -> LocalPersistResult<Arc<dyn PeerLocalPersistence>>;

fn registry() -> &'static RwLock<HashMap<&'static str, BackendFactory>> {
    static REG: OnceLock<RwLock<HashMap<&'static str, BackendFactory>>> = OnceLock::new();
    REG.get_or_init(|| {
        let mut map: HashMap<&'static str, BackendFactory> = HashMap::new();
        map.insert("composite", composite_factory);
        RwLock::new(map)
    })
}

fn composite_factory(opts: &BackendOpenOptions) -> LocalPersistResult<Arc<dyn PeerLocalPersistence>> {
    let data_dir = opts.data_dir.clone().ok_or_else(|| {
        crate::error::LocalPersistError::new(
            crate::error::LocalPersistReason::Unavailable,
            "composite backend requires data_dir",
        )
    })?;
    Ok(Arc::new(CompositeLocalPersistence::open(data_dir)?))
}

/// Register a custom backend factory (pilot / embedding). Built-in `"composite"` is pre-registered.
pub fn register_backend(name: &'static str, factory: BackendFactory) {
    registry()
        .write()
        .expect("backend registry lock")
        .insert(name, factory);
}

/// Open a registered backend by name.
pub fn open_registered(
    name: &str,
    opts: &BackendOpenOptions,
) -> LocalPersistResult<Arc<dyn PeerLocalPersistence>> {
    let map = registry().read().expect("backend registry lock");
    let factory = map.get(name).ok_or_else(|| {
        crate::error::LocalPersistError::new(
            crate::error::LocalPersistReason::Unavailable,
            format!("unknown registered backend: {name}"),
        )
    })?;
    factory(opts)
}

/// List registered backend names (built-ins + custom).
pub fn registered_backend_names() -> Vec<&'static str> {
    registry()
        .read()
        .expect("backend registry lock")
        .keys()
        .copied()
        .collect()
}
