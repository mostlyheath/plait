//! Per-machine connection lifecycle. The caller performs all connection work.
use crate::{
    machines::{MachineId, Machines, Transport},
    remote::ssh::{Reconnect, SshDestination},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Instant, SystemTime},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Connected,
    Offline { since: SystemTime, reason: String },
    Disabled,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attempt {
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
impl From<&Transport> for Attempt {
    fn from(value: &Transport) -> Self {
        match value {
            Transport::Local => Self::Local,
            Transport::Ssh {
                destination,
                port,
                store,
            } => Self::Ssh {
                destination: destination.clone(),
                port: *port,
                store: store.clone(),
            },
            Transport::Tailnet { host, port } => Self::Tailnet {
                host: host.clone(),
                port: *port,
            },
        }
    }
}
#[derive(Debug)]
pub struct MachineConnection {
    pub id: MachineId,
    pub phase: Phase,
    pub epoch: u64,
    retry: Reconnect,
    next_attempt: Option<Instant>,
    attempt: Attempt,
}
#[derive(Default, Debug)]
pub struct Connections {
    entries: BTreeMap<MachineId, MachineConnection>,
    next_epoch: u64,
}
impl Connections {
    pub fn sync(&mut self, machines: &Machines) {
        self.entries.retain(|id, _| machines.get(id).is_some());
        for machine in machines.list() {
            // An edited machine connects to its new address on the next attempt.
            if let Some(entry) = self.entries.get_mut(&machine.id) {
                entry.attempt = (&machine.transport).into();
            }
            self.entries
                .entry(machine.id.clone())
                .or_insert_with(|| MachineConnection {
                    id: machine.id.clone(),
                    phase: Phase::Offline {
                        since: SystemTime::now(),
                        reason: "Not connected.".into(),
                    },
                    epoch: 0,
                    retry: Reconnect::default(),
                    next_attempt: None,
                    attempt: (&machine.transport).into(),
                });
        }
    }
    pub fn get(&self, id: &MachineId) -> Option<&MachineConnection> {
        self.entries.get(id)
    }
    pub fn due(&self, now: Instant) -> Vec<MachineId> {
        self.entries
            .values()
            .filter(|c| {
                matches!(c.phase, Phase::Offline { .. })
                    && c.next_attempt.is_none_or(|at| now >= at)
            })
            .map(|c| c.id.clone())
            .collect()
    }
    pub fn start(&mut self, id: &MachineId, now: Instant) -> Option<(u64, Attempt)> {
        let c = self.entries.get_mut(id)?;
        if !matches!(c.phase, Phase::Offline { .. }) || c.next_attempt.is_some_and(|at| now < at) {
            return None;
        }
        self.next_epoch = self
            .next_epoch
            .checked_add(1)
            .expect("connection epoch exhausted");
        c.epoch = self.next_epoch;
        c.phase = Phase::Connecting;
        c.next_attempt = None;
        Some((c.epoch, c.attempt.clone()))
    }
    pub fn succeeded(&mut self, id: &MachineId, epoch: u64) {
        if let Some(c) = self
            .entries
            .get_mut(id)
            .filter(|c| c.epoch == epoch && c.phase == Phase::Connecting)
        {
            c.phase = Phase::Connected;
            c.retry.reset();
        }
    }
    pub fn failed(&mut self, id: &MachineId, epoch: u64, reason: impl Into<String>, now: Instant) {
        if let Some(c) = self
            .entries
            .get_mut(id)
            .filter(|c| c.epoch == epoch && c.phase == Phase::Connecting)
        {
            c.phase = Phase::Offline {
                since: SystemTime::now(),
                reason: reason.into(),
            };
            c.next_attempt = now.checked_add(c.retry.next_delay());
        }
    }
    pub fn lost(&mut self, id: &MachineId, epoch: u64, now: Instant) {
        if let Some(c) = self
            .entries
            .get_mut(id)
            .filter(|c| c.epoch == epoch && c.phase == Phase::Connected)
        {
            self.next_epoch = self
                .next_epoch
                .checked_add(1)
                .expect("connection epoch exhausted");
            c.epoch = self.next_epoch;
            c.phase = Phase::Offline {
                since: SystemTime::now(),
                reason: "Connection lost.".into(),
            };
            c.next_attempt = now.checked_add(c.retry.next_delay());
        }
    }
    pub fn disable(&mut self, id: &MachineId) {
        if let Some(c) = self.entries.get_mut(id) {
            c.phase = Phase::Disabled;
            c.next_attempt = None;
        }
    }
    pub fn enable(&mut self, id: &MachineId) {
        if let Some(c) = self
            .entries
            .get_mut(id)
            .filter(|c| c.phase == Phase::Disabled)
        {
            c.retry.reset();
            c.phase = Phase::Offline {
                since: SystemTime::now(),
                reason: "Not connected.".into(),
            };
            c.next_attempt = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machines::Transport;
    use std::time::Duration;
    #[test]
    fn connections_independent_stale_backoff_and_lost() {
        let mut machines = Machines::default();
        let a = machines.list()[0].id.clone();
        let b = machines
            .add(
                "remote",
                Transport::Tailnet {
                    host: "host".into(),
                    port: 7,
                },
            )
            .unwrap();
        let mut connections = Connections::default();
        connections.sync(&machines);
        let now = Instant::now();
        let (ea, _) = connections.start(&a, now).unwrap();
        let (eb, _) = connections.start(&b, now).unwrap();
        connections.succeeded(&a, ea);
        connections.failed(&b, eb, "fixed", now);
        assert_eq!(connections.get(&a).unwrap().phase, Phase::Connected);
        connections.succeeded(&b, eb);
        assert!(
            matches!(&connections.get(&b).unwrap().phase, Phase::Offline { reason, .. } if reason == "fixed")
        );
        assert!(!connections.due(now).contains(&b));
        let later = now + Duration::from_secs(1);
        assert!(connections.due(later).contains(&b));
        let (next, _) = connections.start(&b, later).unwrap();
        connections.failed(&b, eb, "stale", later);
        assert_eq!(connections.get(&b).unwrap().phase, Phase::Connecting);
        connections.succeeded(&b, next);
        connections.lost(&b, next, later);
        assert_ne!(connections.get(&b).unwrap().epoch, next);
        connections.succeeded(&b, next);
        assert!(matches!(
            connections.get(&b).unwrap().phase,
            Phase::Offline { .. }
        ));
        connections.disable(&b);
        assert!(
            !connections
                .due(later + Duration::from_secs(20))
                .contains(&b)
        );
        connections.enable(&b);
        assert!(connections.due(later).contains(&b));
        machines.remove(&b).unwrap();
        connections.sync(&machines);
        assert!(connections.get(&b).is_none());
    }
}
