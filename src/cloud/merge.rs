//! Three-way merge of a project's variables: last-synced base, this computer
//! (local) and the cloud. Pure logic, no I/O.

use crate::master::Variables;

#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub name: String,
    /// `None` = removed locally.
    pub local: Option<String>,
    /// `None` = removed in the cloud.
    pub cloud: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct Merge {
    /// Every non-conflict variable.
    pub result: Variables,
    pub conflicts: Vec<Conflict>,
    /// All merged names: local order first, then cloud-only names.
    order: Vec<String>,
}

pub enum Keep {
    Local,
    Cloud,
}

fn get<'a>(list: &'a [(String, String)], name: &str) -> Option<&'a String> {
    list.iter().find(|(n, _)| n == name).map(|(_, v)| v)
}

pub fn merge(
    base: &[(String, String)],
    local: &[(String, String)],
    cloud: &[(String, String)],
) -> Merge {
    let mut order: Vec<String> = local.iter().map(|(n, _)| n.clone()).collect();
    for (n, _) in cloud {
        if get(local, n).is_none() {
            order.push(n.clone());
        }
    }
    let mut result = Vec::new();
    let mut conflicts = Vec::new();
    for name in &order {
        let (b, l, c) = (get(base, name), get(local, name), get(cloud, name));
        let take = if l == c || c == b {
            Some(l)
        } else if l == b {
            Some(c)
        } else {
            None
        };
        match take {
            Some(Some(v)) => result.push((name.clone(), v.clone())),
            Some(None) => {}
            None => conflicts.push(Conflict {
                name: name.clone(),
                local: l.cloned(),
                cloud: c.cloned(),
            }),
        }
    }
    Merge {
        result,
        conflicts,
        order,
    }
}

/// `choices[i]` is the value for `merge.conflicts[i]`; `None` removes it.
/// Panics if the lengths differ.
pub fn resolve(merge: Merge, choices: &[Option<String>]) -> Variables {
    assert_eq!(
        merge.conflicts.len(),
        choices.len(),
        "one choice per conflict required"
    );
    let mut out = Vec::new();
    for name in &merge.order {
        if let Some(i) = merge.conflicts.iter().position(|c| &c.name == name) {
            if let Some(v) = &choices[i] {
                out.push((name.clone(), v.clone()));
            }
        } else if let Some(v) = get(&merge.result, name) {
            out.push((name.clone(), v.clone()));
        }
    }
    out
}

pub fn choose_all(conflicts: &[Conflict], keep: Keep) -> Vec<Option<String>> {
    conflicts
        .iter()
        .map(|c| match keep {
            Keep::Local => c.local.clone(),
            Keep::Cloud => c.cloud.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(pairs: &[(&str, &str)]) -> Variables {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn only_local_changed() {
        let m = merge(&v(&[("A", "1")]), &v(&[("A", "2")]), &v(&[("A", "1")]));
        assert_eq!(m.result, v(&[("A", "2")]));
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn only_cloud_changed() {
        let m = merge(&v(&[("A", "1")]), &v(&[("A", "1")]), &v(&[("A", "3")]));
        assert_eq!(m.result, v(&[("A", "3")]));
    }

    #[test]
    fn both_added_different_names() {
        let m = merge(&v(&[]), &v(&[("B", "1")]), &v(&[("C", "2")]));
        assert_eq!(m.result, v(&[("B", "1"), ("C", "2")]));
    }

    #[test]
    fn both_changed_same_value_is_not_conflict() {
        let m = merge(&v(&[("A", "1")]), &v(&[("A", "2")]), &v(&[("A", "2")]));
        assert_eq!(m.result, v(&[("A", "2")]));
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn both_changed_different_values_conflict() {
        let m = merge(&v(&[("A", "1")]), &v(&[("A", "2")]), &v(&[("A", "3")]));
        assert_eq!(
            m.conflicts,
            vec![Conflict {
                name: "A".into(),
                local: Some("2".into()),
                cloud: Some("3".into())
            }]
        );
        assert!(m.result.is_empty());
    }

    #[test]
    fn removed_on_one_side_unchanged_on_other_is_removed() {
        let m = merge(&v(&[("A", "1")]), &v(&[]), &v(&[("A", "1")]));
        assert!(m.result.is_empty());
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn removed_on_one_side_changed_on_other_conflicts() {
        let m = merge(&v(&[("A", "1")]), &v(&[]), &v(&[("A", "5")]));
        assert_eq!(
            m.conflicts[0],
            Conflict {
                name: "A".into(),
                local: None,
                cloud: Some("5".into())
            }
        );
    }

    #[test]
    fn added_both_sides_different_values_conflict() {
        let m = merge(&v(&[]), &v(&[("A", "1")]), &v(&[("A", "2")]));
        assert_eq!(m.conflicts.len(), 1);
    }

    #[test]
    fn merge_after_lost_base_has_no_conflicts() {
        let m = merge(
            &v(&[("A", "1")]),
            &v(&[("A", "2"), ("B", "3")]),
            &v(&[("A", "2"), ("B", "3")]),
        );
        assert_eq!(m.result, v(&[("A", "2"), ("B", "3")]));
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn resolve_keeps_local_order() {
        let m = merge(
            &v(&[("A", "1"), ("B", "1")]),
            &v(&[("A", "2"), ("B", "1"), ("Z", "9")]),
            &v(&[("A", "3"), ("B", "1")]),
        );
        assert_eq!(
            resolve(m, &[Some("4".into())]),
            v(&[("A", "4"), ("B", "1"), ("Z", "9")])
        );
    }

    #[test]
    fn resolve_none_removes() {
        let m = merge(&v(&[("A", "1")]), &v(&[("A", "2")]), &v(&[("A", "3")]));
        let result = resolve(m, &[None]);
        assert!(!result.iter().any(|(n, _)| n == "A"));
    }

    #[test]
    fn choose_all_cloud() {
        assert_eq!(
            choose_all(
                &[Conflict {
                    name: "A".into(),
                    local: Some("2".into()),
                    cloud: None
                }],
                Keep::Cloud
            ),
            vec![None]
        );
    }
}
