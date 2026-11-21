//! Saved desktop machine identities and validated connection settings.
use crate::remote::ssh::SshDestination;
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MACHINE_LIMIT: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct MachineId(String);
impl Default for MachineId {
    fn default() -> Self {
        Self("local".into())
    }
}
impl MachineId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err("Invalid machine ID.".into());
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    fn generated() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self(format!(
            "{time:x}-{:x}-{:x}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
impl<'de> Deserialize<'de> for MachineId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Transport {
    Local,
    Ssh {
        destination: SshDestination,
        port: Option<u16>,
        store: Option<PathBuf>,
    },
    Tailnet {
        host: String,
        port: u16,
    },
}
impl Transport {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Local => Ok(()),
            Self::Ssh { port, store, .. } => {
                if *port == Some(0) {
                    return Err("Port must be nonzero.".into());
                }
                if let Some(path) = store
                    && (!path.is_absolute()
                        || path
                            .to_str()
                            .is_none_or(|s| s.chars().any(char::is_control)))
                {
                    return Err(
                        "Store path must be absolute and contain no control characters.".into(),
                    );
                }
                Ok(())
            }
            Self::Tailnet { host, port } => {
                if *port == 0 || !valid_host(host) {
                    return Err("Invalid tailnet host or port.".into());
                }
                Ok(())
            }
        }
    }
}
fn valid_host(host: &str) -> bool {
    let ip = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    if ip.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    pub id: MachineId,
    pub name: String,
    pub transport: Transport,
}
fn validate_name(name: &str) -> Result<(), String> {
    let n = name.chars().count();
    if n == 0 || n > 64 || name.chars().any(char::is_control) {
        return Err("Machine name must be 1–64 characters without control characters.".into());
    }
    Ok(())
}
fn name_key(name: &str) -> String {
    name.to_lowercase()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Machines(Vec<Machine>);
impl Default for Machines {
    fn default() -> Self {
        Self(vec![Machine {
            id: MachineId("local".into()),
            name: "This Mac".into(),
            transport: Transport::Local,
        }])
    }
}
impl<'de> Deserialize<'de> for Machines {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let mut result = Self::default();
        if let Some(entries) = value.as_array() {
            // Preserve a valid saved local identity and name, regardless of its position.
            for value in entries {
                if let Ok(machine) = serde_json::from_value::<Machine>(value.clone())
                    && machine.transport == Transport::Local
                    && validate_name(&machine.name).is_ok()
                {
                    result.0[0] = machine;
                    break;
                }
            }
            for value in entries {
                let Ok(machine) = serde_json::from_value::<Machine>(value.clone()) else {
                    continue;
                };
                if result.0.len() >= MACHINE_LIMIT {
                    break;
                }
                if machine.transport == Transport::Local
                    || validate_name(&machine.name).is_err()
                    || machine.transport.validate().is_err()
                    || result
                        .0
                        .iter()
                        .any(|m| m.id == machine.id || name_key(&m.name) == name_key(&machine.name))
                {
                    continue;
                }
                result.0.push(machine);
            }
        }
        Ok(result)
    }
}
impl Machines {
    pub fn list(&self) -> &[Machine] {
        &self.0
    }
    pub fn get(&self, id: &MachineId) -> Option<&Machine> {
        self.0.iter().find(|m| &m.id == id)
    }
    pub fn add(
        &mut self,
        name: impl Into<String>,
        transport: Transport,
    ) -> Result<MachineId, String> {
        let name = name.into();
        validate_name(&name)?;
        transport.validate()?;
        if transport == Transport::Local {
            return Err("A local machine already exists.".into());
        }
        if self.0.len() >= MACHINE_LIMIT {
            return Err("Machine limit reached.".into());
        }
        if self.0.iter().any(|m| name_key(&m.name) == name_key(&name)) {
            return Err("Machine name already exists.".into());
        }
        let mut id = MachineId::generated();
        while self.get(&id).is_some() {
            id = MachineId::generated();
        }
        self.0.push(Machine {
            id: id.clone(),
            name,
            transport,
        });
        Ok(id)
    }
    pub fn rename(&mut self, id: &MachineId, name: impl Into<String>) -> Result<(), String> {
        let name = name.into();
        validate_name(&name)?;
        if self
            .0
            .iter()
            .any(|m| &m.id != id && name_key(&m.name) == name_key(&name))
        {
            return Err("Machine name already exists.".into());
        }
        self.0
            .iter_mut()
            .find(|m| &m.id == id)
            .ok_or("Machine not found.")?
            .name = name;
        Ok(())
    }
    pub fn remove(&mut self, id: &MachineId) -> Result<(), String> {
        let index = self
            .0
            .iter()
            .position(|m| &m.id == id)
            .ok_or("Machine not found.")?;
        if self.0[index].transport == Transport::Local {
            return Err("Cannot remove the local machine.".into());
        }
        self.0.remove(index);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn remote() -> Transport {
        Transport::Tailnet {
            host: "node".into(),
            port: 22,
        }
    }
    #[test]
    fn machines_operations_and_limits() {
        let mut list = Machines::default();
        let local = list.list()[0].id.clone();
        assert!(list.remove(&local).is_err());
        assert!(list.add("other", Transport::Local).is_err());
        assert!(list.add("", remote()).is_err());
        assert!(list.add("a\n", remote()).is_err());
        assert!(list.add("a".repeat(65), remote()).is_err());
        assert!(
            list.add(
                "bad",
                Transport::Tailnet {
                    host: "bad host".into(),
                    port: 0
                }
            )
            .is_err()
        );
        assert!(
            list.add(
                "bad",
                Transport::Ssh {
                    destination: SshDestination::new("host").unwrap(),
                    port: Some(0),
                    store: Some("relative".into())
                }
            )
            .is_err()
        );
        let id = list.add("Server", remote()).unwrap();
        assert!(list.add("server", remote()).is_err());
        assert!(list.rename(&id, "THIS MAC").is_err());
        list.rename(&id, "New").unwrap();
        assert_eq!(list.get(&id).unwrap().name, "New");
        list.remove(&id).unwrap();
        assert!(list.remove(&id).is_err());
        for i in 1..MACHINE_LIMIT {
            list.add(format!("host{i}"), remote()).unwrap();
        }
        assert!(list.add("overflow", remote()).is_err());
        assert!(MachineId::new("bad id").is_err());
        assert!(MachineId::new("x".repeat(65)).is_err());
    }
    #[test]
    fn machines_load_drops_bad_and_duplicate_entries() {
        let value = r#"[{"id":"bad id","name":"bad","transport":{"type":"local"}},{"id":"x","name":"X","transport":{"type":"tailnet","host":"node","port":3}},{"id":"y","name":"x","transport":{"type":"tailnet","host":"node","port":3}},{"id":"z","name":"Z","transport":{"type":"ssh","destination":"-bad","port":null,"store":null}},{"id":"local2","name":"Home","transport":{"type":"local"}}]"#;
        let list: Machines = serde_json::from_str(value).unwrap();
        assert_eq!(list.list().len(), 2);
        assert_eq!(list.list()[0].id.as_str(), "local2");
        assert_eq!(
            serde_json::from_str::<Machines>(&serde_json::to_string(&list).unwrap()).unwrap(),
            list
        );
    }
}
