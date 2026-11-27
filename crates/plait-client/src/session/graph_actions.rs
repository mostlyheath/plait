//! Durable reservations for host operations without an idempotency key.
use super::Journal;
use crate::{
    host::{
        self,
        graph::{GraphReply, MutationReply, NodeLifecycle},
    },
    machines::MachineId,
    tabs::HostKey,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphAction {
    pub host: HostKey,
    pub node: String,
    pub project: PathBuf,
    pub delete: bool,
    revision: Option<u64>,
}

impl GraphAction {
    pub fn new(host: HostKey, node: String, project: PathBuf, delete: bool) -> Self {
        Self {
            host,
            node,
            project,
            delete,
            revision: None,
        }
    }
    fn matches(&self, other: &Self) -> bool {
        self.host == other.host && self.node == other.node
    }
}

impl Journal {
    fn update_graph_actions<T>(
        &self,
        machine: &MachineId,
        edit: impl FnOnce(&mut Vec<GraphAction>) -> Result<T, String>,
    ) -> Result<T, String> {
        static WRITER: Mutex<()> = Mutex::new(());
        let _guard = WRITER
            .lock()
            .map_err(|_| "Graph action storage unavailable.")?;
        let directory = self.record_directory("desktop-machines")?;
        let name = format!("{}.graph-actions", machine.as_str());
        let mut records: Vec<GraphAction> =
            Self::read_private_record(&directory, &name, super::FILE_LIMIT)?
                .map(|bytes| {
                    serde_json::from_slice(&bytes)
                        .map_err(|e| format!("Invalid saved graph actions: {e}"))
                })
                .transpose()?
                .unwrap_or_default();
        let before = records.clone();
        let result = edit(&mut records)?;
        if before != records {
            let bytes = serde_json::to_vec(&records).map_err(|e| e.to_string())?;
            Self::write_private_record(&directory, &name, &bytes, super::FILE_LIMIT)?;
        }
        Ok(result)
    }

    pub fn dispatch_graph_action(
        &self,
        machine: &MachineId,
        action: &GraphAction,
        retry: bool,
        client: &host::Client,
    ) -> Result<MutationReply, String> {
        self.reserve_graph_action(machine, action, retry)?;
        match host::graph::mutate_draft(client, action.node.clone(), action.delete) {
            Ok(reply) => {
                self.finish_graph_action(machine, action, Some(reply.graph_revision))?;
                Ok(reply)
            }
            Err(failure) => {
                // A failed reconnect cannot resolve an earlier ambiguous transmission.
                if matches!(failure.error, host::Error::Rejected(_))
                    || (!retry && !failure.transmitted)
                {
                    self.finish_graph_action(machine, action, None)?;
                }
                Err(failure.error.to_string())
            }
        }
    }

    fn reserve_graph_action(
        &self,
        machine: &MachineId,
        action: &GraphAction,
        retry: bool,
    ) -> Result<(), String> {
        self.update_graph_actions(machine, |records| {
            if let Some(record) = records.iter().find(|r| r.matches(action)) {
                if retry && record.revision.is_none() && record.delete == action.delete {
                    return Ok(());
                }
                return Err("A draft action is already pending.".into());
            }
            if retry {
                return Err("Draft action already resolved. Refresh the graph.".into());
            }
            records.push(action.clone());
            Ok(())
        })
    }

    /// A definite rejection releases the reservation. An ambiguous transport error keeps it.
    fn finish_graph_action(
        &self,
        machine: &MachineId,
        action: &GraphAction,
        revision: Option<u64>,
    ) -> Result<(), String> {
        self.update_graph_actions(machine, |records| {
            if let Some(revision) = revision {
                if let Some(record) = records.iter_mut().find(|r| r.matches(action)) {
                    record.revision = Some(revision);
                }
            } else {
                records.retain(|r| !r.matches(action));
            }
            Ok(())
        })
    }

    pub fn reconcile_graph_actions(
        &self,
        machine: &MachineId,
        host: &HostKey,
        project: &Path,
        reply: &GraphReply,
    ) -> Result<Vec<crate::graph_view::PendingDraft>, String> {
        self.update_graph_actions(machine, |records| {
            if records.iter().any(|r| {
                &r.host == host && r.revision.is_some_and(|rev| rev > reply.graph_revision())
            }) {
                return Err("Waiting for the host to reflect the draft action.".into());
            }
            if let GraphReply::View(view) = reply {
                records.retain_mut(|r| {
                    if &r.host != host {
                        return true;
                    }
                    match view.nodes.iter().find(|n| n.id == r.node) {
                        Some(n) => {
                            if r.project != project {
                                r.project = project.to_owned();
                            }
                            r.delete
                                || (n.lifecycle != NodeLifecycle::Thread
                                    && !(r.revision.is_some() && n.reason.is_some()))
                        }
                        None => r.project != project || view.truncated || r.revision.is_none(),
                    }
                });
            }
            Ok(records
                .iter()
                .filter(|r| &r.host == host)
                .map(|r| crate::graph_view::PendingDraft {
                    node: r.node.clone(),
                    retry_delete: r.revision.is_none().then_some(r.delete),
                })
                .collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::graph::{GraphNode, GraphView};

    #[cfg(all(feature = "test-host", feature = "local-host"))]
    #[test]
    fn disconnected_first_attempt_releases_but_retry_preserves_the_original_action() {
        let directory = plait_test_support::temp_dir()
            .join(format!("plait-draft-retry-{}", std::process::id()));
        let machine = MachineId::default();
        let host = HostKey {
            id: "host".into(),
            store: "/store".into(),
        };
        let action = GraphAction::new(host.clone(), "n".into(), "/repo".into(), false);
        let offline = host::Client::at_socket(directory.join("offline"));
        let (journal, _) = Journal::open(&directory, "/store".into()).unwrap();
        let dispatch =
            |client, retry| journal.dispatch_graph_action(&machine, &action, retry, client);
        let pending = || {
            journal
                .reconcile_graph_actions(
                    &machine,
                    &host,
                    &action.project,
                    &GraphReply::Unchanged { graph_revision: 1 },
                )
                .unwrap()
        };
        assert!(dispatch(&offline, false).is_err());
        assert!(pending().is_empty());

        let lost_reply = host::Client::with_test_reads(|request| {
            assert_eq!(
                request,
                serde_json::json!({"op":"start_draft","node_id":"n"})
            );
            serde_json::Value::Null
        });
        assert!(dispatch(&lost_reply, false).is_err());
        assert!(dispatch(&offline, true).is_err());
        let mut view = crate::graph_view::GraphViewState::default();
        view.target(1, host.clone(), action.project.clone(), None);
        let (epoch, generation, _, _) = view.begin_poll(false).unwrap();
        view.apply_poll(
            epoch,
            generation,
            Ok((GraphReply::Unchanged { graph_revision: 1 }, pending())),
        );
        assert!(view.begin_mutation("n", true).is_none());
        assert!(view.begin_mutation("n", false).is_some());
        let reconnected = host::Client::with_test_reads(|request| {
            assert_eq!(
                request,
                serde_json::json!({"op":"start_draft","node_id":"n"})
            );
            serde_json::json!({"ok":true,"result":{"graph_revision":2,"node_id":"n"}})
        });
        assert_eq!(dispatch(&reconnected, true).unwrap().graph_revision, 2);
        assert!(dispatch(&reconnected, true).is_err());
        drop(journal);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn delete_retry_follows_a_renamed_project() {
        let directory = plait_test_support::temp_dir()
            .join(format!("plait-graph-rename-{}", std::process::id()));
        let (journal, _) = Journal::open(&directory, "/store".into()).unwrap();
        let machine = MachineId::default();
        let host = HostKey {
            id: "host".into(),
            store: "/store".into(),
        };
        let mut action = GraphAction::new(host.clone(), "node".into(), "/old".into(), true);
        journal
            .reserve_graph_action(&machine, &action, false)
            .unwrap();
        action.project = "/renamed".into();
        journal
            .reserve_graph_action(&machine, &action, true)
            .unwrap();

        let mut view = GraphView {
            graph_revision: 1,
            nodes: vec![GraphNode {
                id: action.node.clone(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let reconcile = |view| {
            journal
                .reconcile_graph_actions(&machine, &host, &action.project, &GraphReply::View(view))
                .unwrap()
        };
        assert_eq!(
            reconcile(view.clone()),
            [crate::graph_view::PendingDraft {
                node: action.node.clone(),
                retry_delete: Some(true)
            }]
        );
        journal
            .reserve_graph_action(&machine, &action, true)
            .unwrap();
        journal
            .finish_graph_action(&machine, &action, Some(2))
            .unwrap();
        view.graph_revision = 2;
        view.nodes.clear();
        assert!(reconcile(view).is_empty());
        drop(journal);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reservations_survive_session_rewrites_and_reject_stale_reads() {
        let directory = plait_test_support::temp_dir()
            .join(format!("plait-graph-actions-{}", std::process::id()));
        let machine = MachineId::default();
        let other = MachineId::new("remote").unwrap();
        let host = HostKey {
            id: "host-a".into(),
            store: "/store".into(),
        };
        let action = GraphAction::new(host.clone(), "node".into(), "/repo".into(), true);
        let (journal, session) = Journal::open(&directory, "/store".into()).unwrap();
        journal
            .reserve_graph_action(&machine, &action, false)
            .unwrap();
        journal.queue(session);
        journal.flush().unwrap();
        drop(journal);
        let (journal, _) = Journal::open(&directory, "/store".into()).unwrap();
        assert!(
            journal
                .reserve_graph_action(&machine, &action, false)
                .is_err()
        );
        journal
            .reserve_graph_action(&machine, &action, true)
            .unwrap();
        let mut opposite = action.clone();
        opposite.delete = false;
        assert!(
            journal
                .reserve_graph_action(&machine, &opposite, true)
                .is_err()
        );
        journal
            .reserve_graph_action(&other, &action, false)
            .unwrap();
        let mut different_host = action.clone();
        different_host.host.id = "host-b".into();
        journal
            .reserve_graph_action(&machine, &different_host, false)
            .unwrap();
        journal
            .finish_graph_action(&machine, &action, Some(8))
            .unwrap();
        let reconcile =
            |reply| journal.reconcile_graph_actions(&machine, &host, &action.project, &reply);
        assert!(reconcile(GraphReply::Unchanged { graph_revision: 7 }).is_err());
        assert!(
            reconcile(GraphReply::View(GraphView {
                graph_revision: 7,
                ..Default::default()
            }))
            .is_err()
        );
        assert_eq!(
            reconcile(GraphReply::View(GraphView {
                graph_revision: 8,
                nodes: vec![GraphNode {
                    id: action.node.clone(),
                    ..Default::default()
                }],
                ..Default::default()
            }))
            .unwrap(),
            [crate::graph_view::PendingDraft {
                node: "node".into(),
                retry_delete: None
            }]
        );
        assert!(
            reconcile(GraphReply::View(GraphView {
                graph_revision: 8,
                ..Default::default()
            }))
            .unwrap()
            .is_empty()
        );
        assert!(
            journal
                .reserve_graph_action(&machine, &different_host, false)
                .is_err()
        );
        assert!(
            journal
                .reserve_graph_action(&other, &action, false)
                .is_err()
        );
        drop(journal);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
