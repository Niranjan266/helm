//! Feature modules. Each one implements [`crate::router::Module`].

use std::sync::Arc;

use crate::router::Module;

pub mod devices;
pub mod system;

/// All modules shipped in this build.
pub fn all() -> Vec<Arc<dyn Module>> {
    vec![
        Arc::new(system::SystemModule),
        Arc::new(devices::DevicesModule),
    ]
}
