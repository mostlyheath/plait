//! Pure project grouping, run off the UI thread when host state changes.

use crate::{
    host::{ThreadSummary, workspace},
    navigation::WorktreeOf,
};
use plait_protocol::host::git::BranchSummary;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostId(pub String);
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CopyRef {
    pub host: HostId,
    pub path: PathBuf,
}
#[derive(Clone, Debug, Default)]
pub struct ProjectLinks {
    pub linked: Vec<BTreeSet<CopyRef>>,
    pub unlinked: BTreeSet<(CopyRef, CopyRef)>,
}
#[derive(Clone, Debug)]
pub struct HostProjects {
    pub host: HostId,
    pub name: String,
    pub online: bool,
    pub projects: Vec<workspace::Project>,
    pub threads: Vec<ThreadSummary>,
    pub worktrees: BTreeMap<PathBuf, WorktreeOf>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupId(pub String);
#[derive(Clone, Debug)]
pub struct Copy {
    pub host: HostId,
    pub host_name: String,
    pub path: PathBuf,
    pub branch: Option<BranchSummary>,
    pub online: bool,
    pub vcs: Option<String>,
}
#[derive(Clone, Debug)]
pub struct ProjectGroup {
    pub id: GroupId,
    pub name: String,
    pub copies: Vec<Copy>,
    pub threads: Vec<(HostId, ThreadSummary)>,
    pub last_activity: u64,
}

fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}
fn merge(parent: &mut [usize], a: usize, b: usize, forbidden: &[(usize, usize)]) -> bool {
    let a = find(parent, a);
    let b = find(parent, b);
    if a == b {
        return true;
    }
    for &(x, y) in forbidden {
        let x = find(parent, x);
        let y = find(parent, y);
        if (x == a && y == b) || (x == b && y == a) {
            return false;
        }
    }
    parent[b] = a;
    true
}

pub fn group(
    hosts: &[HostProjects],
    links: &ProjectLinks,
    filter: Option<&HostId>,
) -> Vec<ProjectGroup> {
    let mut entries = BTreeMap::new();
    let mut threads_by_copy: BTreeMap<CopyRef, Vec<&ThreadSummary>> = BTreeMap::new();
    for host in hosts {
        let mut mains: BTreeMap<&Path, &Path> = host
            .projects
            .iter()
            .filter_map(|p| {
                p.main_checkout
                    .as_deref()
                    .map(|main| (p.path.as_path(), main))
            })
            .collect();
        for (path, worktree) in &host.worktrees {
            mains.insert(path.as_path(), worktree.main.as_path());
        }
        let main_path = |path: &Path| {
            let mut current = path;
            let mut visited = BTreeSet::new();
            while visited.insert(current) {
                match mains.get(current) {
                    Some(next) => current = next,
                    None => return current.to_path_buf(),
                }
            }
            visited.into_iter().min().unwrap_or(current).to_path_buf()
        };
        for thread in &host.threads {
            threads_by_copy
                .entry(CopyRef {
                    host: host.host.clone(),
                    path: main_path(&thread.checkout),
                })
                .or_default()
                .push(thread);
        }
        let occupied: BTreeSet<_> = host
            .threads
            .iter()
            .map(|t| main_path(&t.checkout))
            .collect();
        for project in &host.projects {
            let main = main_path(&project.path);
            if project.hidden && !occupied.contains(&main) {
                continue;
            }
            let key = CopyRef {
                host: host.host.clone(),
                path: main.clone(),
            };
            let mut entry = project.clone();
            entry.path = main.clone();
            if main == project.path {
                entries.insert(key, (host, entry));
            } else {
                entries.entry(key).or_insert((host, entry));
            }
        }
        for path in occupied {
            let key = CopyRef {
                host: host.host.clone(),
                path: path.clone(),
            };
            entries.entry(key).or_insert_with(|| {
                (
                    host,
                    workspace::Project {
                        path,
                        ..Default::default()
                    },
                )
            });
        }
    }
    let entries: Vec<_> = entries.into_iter().collect();
    let indices: BTreeMap<_, _> = entries
        .iter()
        .enumerate()
        .map(|(i, (key, _))| (key.clone(), i))
        .collect();
    let forbidden: Vec<_> = links
        .unlinked
        .iter()
        .filter_map(|(a, b)| Some((*indices.get(a)?, *indices.get(b)?)))
        .collect();
    let mut parent: Vec<_> = (0..entries.len()).collect();
    let mut manual = Vec::new();
    let mut sets: Vec<_> = links.linked.iter().collect();
    sets.sort();
    for set in sets {
        let members: Vec<_> = set
            .iter()
            .filter_map(|key| indices.get(key).copied())
            .collect();
        for pair in members.windows(2) {
            merge(&mut parent, pair[0], pair[1], &forbidden);
        }
        if let Some(first) = set.first() {
            for i in members {
                manual.push((i, first));
            }
        }
    }
    let mut roots: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut names: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, (_, (_, project))) in entries.iter().enumerate() {
        let fingerprint = project
            .fingerprint
            .as_ref()
            .filter(|f| !f.roots.is_empty() && project.vcs.as_deref() != Some("folder"));
        if let Some(fingerprint) = fingerprint {
            for root in &fingerprint.roots {
                roots.entry(root).or_default().push(i);
            }
        } else {
            names
                .entry(name(&project.path).to_lowercase())
                .or_default()
                .push(i);
        }
    }
    for candidates in roots.values() {
        let mut representatives = Vec::new();
        for &candidate in candidates {
            if !representatives
                .iter()
                .any(|&rep| merge(&mut parent, rep, candidate, &forbidden))
            {
                representatives.push(candidate);
            }
        }
    }
    for candidates in names.values() {
        let mut representatives: Vec<(usize, Option<usize>)> = Vec::new();
        for &candidate in candidates {
            let mut joined = false;
            for (rep, alternate) in &mut representatives {
                let anchor = if entries[*rep].0.host != entries[candidate].0.host {
                    Some(*rep)
                } else {
                    *alternate
                };
                if anchor.is_some_and(|anchor| merge(&mut parent, anchor, candidate, &forbidden)) {
                    if alternate.is_none() && entries[*rep].0.host != entries[candidate].0.host {
                        *alternate = Some(candidate);
                    }
                    joined = true;
                    break;
                }
            }
            if !joined {
                representatives.push((candidate, None));
            }
        }
    }
    let mut grouped: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..entries.len() {
        grouped.entry(find(&mut parent, i)).or_default().push(i);
    }
    let mut result = Vec::new();
    for members in grouped.values() {
        let first = &entries[members[0]];
        let group_name = name(&first.0.path);
        let manual_id = manual
            .iter()
            .filter(|(i, _)| members.binary_search(i).is_ok())
            .map(|(_, key)| *key)
            .min();
        let root = members
            .iter()
            .flat_map(|&i| {
                let project = &entries[i].1.1;
                project
                    .fingerprint
                    .iter()
                    .filter(move |_| project.vcs.as_deref() != Some("folder"))
                    .flat_map(|f| &f.roots)
            })
            .min();
        let id = if let Some(key) = manual_id {
            format!("link:{}:{}", key.host.0, key.path.display())
        } else if let Some(root) = root {
            format!("git:{root}")
        } else {
            format!("name:{}", group_name.to_lowercase())
        };
        let mut copies = Vec::new();
        let mut threads = Vec::new();
        for &i in members {
            let (key, (host, project)) = &entries[i];
            if filter.is_some_and(|f| f != &key.host) {
                continue;
            }
            copies.push(Copy {
                host: key.host.clone(),
                host_name: host.name.clone(),
                path: key.path.clone(),
                branch: project.branch.clone(),
                online: host.online,
                vcs: project.vcs.clone(),
            });
            if let Some(attached) = threads_by_copy.get(key) {
                threads.extend(attached.iter().map(|t| (key.host.clone(), (*t).clone())));
            }
        }
        copies.sort_by(|a, b| {
            (&a.host_name, &a.path, &a.host).cmp(&(&b.host_name, &b.path, &b.host))
        });
        threads.sort_by(|a, b| {
            b.1.updated_at
                .cmp(&a.1.updated_at)
                .then_with(|| (&a.0, &a.1.id).cmp(&(&b.0, &b.1.id)))
        });
        let last_activity = threads
            .iter()
            .map(|(_, t)| t.updated_at.unwrap_or(0))
            .max()
            .unwrap_or(0);
        result.push(ProjectGroup {
            id: GroupId(id),
            name: group_name,
            copies,
            threads,
            last_activity,
        });
    }
    let mut counts = BTreeMap::new();
    for group in &result {
        *counts.entry(group.id.0.clone()).or_insert(0usize) += 1;
    }
    // IDs use the full group even when filtered; collisions append a copy suffix.
    for (group, members) in result.iter_mut().zip(grouped.values()) {
        if counts[&group.id.0] > 1 {
            let key = &entries[members[0]].0;
            group
                .id
                .0
                .push_str(&format!("#{}:{}", key.host.0, key.path.display()));
        }
    }
    result.retain(|group| !group.copies.is_empty());
    result.sort_by(|a, b| {
        b.last_activity
            .cmp(&a.last_activity)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use plait_protocol::host::git::Fingerprint;
    fn project(path: &str, roots: &[&str]) -> workspace::Project {
        workspace::Project {
            path: path.into(),
            vcs: Some("git".into()),
            fingerprint: Some(Fingerprint {
                roots: roots.iter().map(|s| (*s).into()).collect(),
                remote: None,
            }),
            ..Default::default()
        }
    }
    fn host(id: &str, projects: Vec<workspace::Project>) -> HostProjects {
        HostProjects {
            host: HostId(id.into()),
            name: id.into(),
            online: true,
            projects,
            threads: vec![],
            worktrees: BTreeMap::new(),
        }
    }
    fn key(host: &str, path: &str) -> CopyRef {
        CopyRef {
            host: HostId(host.into()),
            path: path.into(),
        }
    }
    #[test]
    fn roots_names_and_unlinks() {
        let a = host(
            "a",
            vec![
                project("/old", &["root"]),
                project("/clone", &["root"]),
                project("/empty", &[]),
            ],
        );
        let mut renamed = project("/new", &["other", "root"]);
        renamed.fingerprint.as_mut().unwrap().remote = Some("new remote".into());
        let b = host("b", vec![renamed, project("/EMPTY", &[])]);
        let groups = group(&[a.clone(), b.clone()], &ProjectLinks::default(), None);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups
                .iter()
                .find(|g| g.id.0 == "git:other")
                .unwrap()
                .copies
                .len(),
            3
        );
        let mut links = ProjectLinks::default();
        links.unlinked.insert((key("a", "/old"), key("b", "/new")));
        let split = group(&[a, b], &links, None);
        assert!(
            split
                .iter()
                .all(|g| !(g.copies.iter().any(|c| c.path == Path::new("/old"))
                    && g.copies.iter().any(|c| c.path == Path::new("/new"))))
        );
    }
    #[test]
    fn fallback_filter_and_stable_order() {
        let mut old = project("/x/Thing", &[]);
        old.fingerprint = None;
        let mut folder = project("/y/thing", &[]);
        folder.vcs = Some("folder".into());
        let a = host("a", vec![old]);
        let b = host("b", vec![folder]);
        let one = group(&[a.clone(), b.clone()], &ProjectLinks::default(), None);
        let two = group(&[b.clone(), a.clone()], &ProjectLinks::default(), None);
        assert_eq!(one[0].id, two[0].id);
        assert_eq!(one[0].copies.len(), 2);
        assert_eq!(
            group(&[a, b], &ProjectLinks::default(), Some(&HostId("a".into())))[0]
                .copies
                .len(),
            1
        );
    }
    #[test]
    fn hidden_main_with_worktree_thread_is_kept() {
        let mut main = project("/main", &["r"]);
        main.hidden = true;
        let mut tree = project("/tree", &["r"]);
        tree.main_checkout = Some("/main".into());
        let mut h = host("a", vec![main, tree]);
        h.threads.push(ThreadSummary {
            id: "t".into(),
            checkout: "/tree".into(),
            updated_at: Some(12),
            ..Default::default()
        });
        let groups = group(&[h], &ProjectLinks::default(), None);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].copies.len(), 1);
        assert_eq!(groups[0].threads.len(), 1);
        assert_eq!(groups[0].last_activity, 12);
    }
    #[test]
    fn missing_projects_and_chained_worktrees_keep_threads() {
        let mut tree = project("/w1", &["root"]);
        tree.main_checkout = Some("/main".into());
        let mut nested = project("/w2", &["root"]);
        nested.main_checkout = Some("/w1".into());
        let mut h = host("a", vec![tree, nested]);
        for path in ["/w2", "/legacy"] {
            h.threads.push(ThreadSummary {
                id: path.into(),
                checkout: path.into(),
                ..Default::default()
            });
        }
        let groups = group(&[h], &ProjectLinks::default(), None);
        let main = groups
            .iter()
            .find(|g| g.copies[0].path == Path::new("/main"))
            .unwrap();
        assert_eq!(main.id.0, "git:root");
        assert_eq!(main.threads[0].1.id, "/w2");
        let legacy = groups
            .iter()
            .find(|g| g.copies[0].path == Path::new("/legacy"))
            .unwrap();
        assert_eq!(legacy.id.0, "name:legacy");
        assert_eq!(legacy.threads.len(), 1);
    }

    #[test]
    fn cyclic_worktrees_resolve_without_losing_threads() {
        let mut a = project("/a", &["r"]);
        a.main_checkout = Some("/b".into());
        let mut b = project("/b", &["r"]);
        b.main_checkout = Some("/a".into());
        let mut h = host("a", vec![a, b]);
        h.threads.push(ThreadSummary {
            checkout: "/b".into(),
            ..Default::default()
        });
        let groups = group(&[h], &ProjectLinks::default(), None);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].threads.len(), 1);
    }

    #[test]
    fn unlinked_first_candidate_does_not_split_other_copies() {
        for roots in [&["r"][..], &[][..]] {
            let hosts = vec![
                host("a", vec![project("/fork/repo", roots)]),
                host("b", vec![project("/upstream/repo", roots)]),
                host("c", vec![project("/upstream/repo", roots)]),
            ];
            let mut links = ProjectLinks::default();
            links
                .unlinked
                .insert((key("a", "/fork/repo"), key("b", "/upstream/repo")));
            links
                .unlinked
                .insert((key("a", "/fork/repo"), key("c", "/upstream/repo")));
            let groups = group(&hosts, &links, None);
            assert_eq!(groups.len(), 2);
            assert!(groups.iter().any(|g| {
                g.copies
                    .iter()
                    .map(|c| c.host.0.as_str())
                    .collect::<Vec<_>>()
                    == ["b", "c"]
            }));
        }
    }

    #[test]
    fn conflicting_manual_links_are_deterministic() {
        let hosts = vec![
            host("a", vec![project("/a", &["r"])]),
            host("b", vec![project("/b", &["r"])]),
            host("c", vec![project("/c", &["r"])]),
        ];
        let mut links = ProjectLinks::default();
        links.linked.push(
            [key("a", "/a"), key("b", "/b"), key("c", "/c")]
                .into_iter()
                .collect(),
        );
        links.unlinked.insert((key("a", "/a"), key("c", "/c")));
        let groups = group(&hosts, &links, None);
        assert_eq!(groups.len(), 2);
        assert!(
            groups
                .iter()
                .all(|g| !(g.copies.iter().any(|c| c.host.0 == "a")
                    && g.copies.iter().any(|c| c.host.0 == "c")))
        );
        let reversed: Vec<_> = hosts.into_iter().rev().collect();
        assert_eq!(
            groups
                .iter()
                .map(|g| (&g.id, g.copies.len()))
                .collect::<Vec<_>>(),
            group(&reversed, &links, None)
                .iter()
                .map(|g| (&g.id, g.copies.len()))
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn same_host_clones_and_folders() {
        let mut folder = project("/a/thing", &[]);
        folder.vcs = Some("folder".into());
        let mut second = folder.clone();
        second.path = "/b/thing".into();
        let groups = group(
            &[host(
                "a",
                vec![
                    project("/one", &["r"]),
                    project("/two", &["r"]),
                    folder,
                    second,
                ],
            )],
            &ProjectLinks::default(),
            None,
        );
        assert_eq!(groups.len(), 3);
        assert_eq!(
            groups
                .iter()
                .find(|g| g.id.0 == "git:r")
                .unwrap()
                .copies
                .len(),
            2
        );
        assert_ne!(groups[1].id, groups[2].id);
    }

    #[test]
    fn fork_unlink_has_distinct_stable_ids() {
        let mut a = project("/upstream", &["root"]);
        a.fingerprint.as_mut().unwrap().remote = Some("upstream".into());
        let mut b = project("/fork", &["root"]);
        b.fingerprint.as_mut().unwrap().remote = Some("fork".into());
        let hosts = vec![host("a", vec![a]), host("b", vec![b])];
        assert_eq!(
            group(&hosts, &ProjectLinks::default(), None)[0]
                .copies
                .len(),
            2
        );
        let mut links = ProjectLinks::default();
        links
            .unlinked
            .insert((key("a", "/upstream"), key("b", "/fork")));
        let groups = group(&hosts, &links, None);
        assert_eq!(groups.len(), 2);
        assert_ne!(groups[0].id, groups[1].id);
        assert!(groups.iter().all(|g| g.id.0.starts_with("git:root#")));
        let filtered = group(&hosts, &links, Some(&HostId("a".into())));
        assert_eq!(
            filtered[0].id,
            groups
                .iter()
                .find(|g| g.copies[0].host.0 == "a")
                .unwrap()
                .id
        );
    }

    #[test]
    fn shared_root_scales() {
        let projects = (0..2000)
            .map(|i| project(&format!("/repo/{i}"), &["root"]))
            .collect();
        let hosts = [host("a", projects)];
        let started = std::time::Instant::now();
        let groups = group(&hosts, &ProjectLinks::default(), None);
        assert_eq!(groups[0].copies.len(), 2000);
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn hidden_empty_and_filter_threads() {
        let mut hidden = project("/hidden", &["h"]);
        hidden.hidden = true;
        let mut a = host("a", vec![hidden, project("/one", &["r"])]);
        let mut b = host("b", vec![project("/two", &["r"])]);
        for h in [&mut a, &mut b] {
            h.threads.push(ThreadSummary {
                id: h.host.0.clone(),
                checkout: h.projects.last().unwrap().path.clone(),
                ..Default::default()
            });
        }
        let groups = group(&[a, b], &ProjectLinks::default(), Some(&HostId("a".into())));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].copies.len(), 1);
        assert_eq!(groups[0].threads.len(), 1);
        assert_eq!(groups[0].threads[0].0.0, "a");
    }
    #[test]
    fn ids_stay_stable_when_inputs_are_shuffled() {
        let mut hosts = vec![
            host("a", vec![project("/one", &["r"]), project("/other", &[])]),
            host("b", vec![project("/two", &["r"]), project("/other", &[])]),
        ];
        let mut links = ProjectLinks::default();
        links
            .linked
            .push([key("a", "/one"), key("b", "/two")].into_iter().collect());
        links.linked.push(
            [key("a", "/other"), key("b", "/other")]
                .into_iter()
                .collect(),
        );
        let ids = |hosts: &[HostProjects], links: &ProjectLinks| {
            group(hosts, links, None)
                .into_iter()
                .map(|g| g.id)
                .collect::<BTreeSet<_>>()
        };
        let expected = ids(&hosts, &links);
        hosts.reverse();
        for host in &mut hosts {
            host.projects.reverse();
        }
        links.linked.reverse();
        assert_eq!(expected, ids(&hosts, &links));
    }
}
