// SPDX-License-Identifier: MIT OR Apache-2.0
//! The `org.freedesktop.portal.Settings` interface (version 2) served on the
//! session bus. Every read is answered from the shared `Values`; this module
//! never talks to settingsd.
use crate::appearance::{Values, valid_filters};
use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};

pub const BUS_NAME: &str = "org.freedesktop.portal.Desktop";
pub const OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
pub const INTERFACE: &str = "org.freedesktop.portal.Settings";
pub const VERSION: u32 = 2;

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.portal.Error")]
pub enum PortalError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotFound(String),
    InvalidArgument(String),
}

pub struct Portal {
    values: Arc<RwLock<Values>>,
}

impl Portal {
    pub fn new(values: Arc<RwLock<Values>>) -> Self {
        Self { values }
    }
}

fn current(values: &RwLock<Values>) -> Values {
    values
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn lookup(
    values: &RwLock<Values>,
    namespace: &str,
    key: &str,
) -> Result<Value<'static>, PortalError> {
    current(values)
        .get(namespace, key)
        .ok_or_else(|| PortalError::NotFound(format!("{namespace} {key} is not served")))
}

#[zbus::interface(name = "org.freedesktop.portal.Settings")]
impl Portal {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }

    /// Deprecated compatibility read. The reply keeps the historical extra
    /// variant layer, so a `v` holding a `v`.
    #[zbus(name = "Read")]
    fn read(&self, namespace: &str, key: &str) -> Result<OwnedValue, PortalError> {
        let inner = lookup(&self.values, namespace, key)?;
        OwnedValue::try_from(Value::Value(Box::new(inner)))
            .map_err(|error| PortalError::ZBus(zbus::Error::from(error)))
    }

    /// The value with exactly one variant layer.
    #[zbus(name = "ReadOne")]
    fn read_one(&self, namespace: &str, key: &str) -> Result<OwnedValue, PortalError> {
        let inner = lookup(&self.values, namespace, key)?;
        OwnedValue::try_from(inner).map_err(|error| PortalError::ZBus(zbus::Error::from(error)))
    }

    /// Every served key in the namespaces matching the globs.
    #[zbus(name = "ReadAll")]
    fn read_all(
        &self,
        namespaces: Vec<String>,
    ) -> Result<BTreeMap<String, BTreeMap<String, Value<'static>>>, PortalError> {
        if !valid_filters(&namespaces) {
            return Err(PortalError::InvalidArgument(
                "ReadAll accepts at most 64 filters of at most 256 bytes each".into(),
            ));
        }
        Ok(current(&self.values).matching(&namespaces))
    }

    /// Emitted only for keys whose value changed, from the name owner.
    #[zbus(signal, name = "SettingChanged")]
    async fn setting_changed(
        emitter: &SignalEmitter<'_>,
        namespace: &str,
        key: &str,
        value: &Value<'_>,
    ) -> zbus::Result<()>;
}
