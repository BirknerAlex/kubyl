//! Rows of the Flux lists: name, namespace, state with message, source and revision, suspended,
//! interval, last reconcile, age; filters by state, namespace and text; sorting.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::kinds::FluxKind;
use crate::model::{FluxObject, State, short_revision};

/// A row of a Flux list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub object: FluxObject,
    pub state: State,
    pub message: String,
    /// `GitRepository/podinfo`, the URL of a source, the image of an ImageRepository.
    pub source: String,
    /// Short revision (`master@sha1:3e0ff8a`, `6.15.0`).
    pub revision: String,
    /// `<kind>/<namespace>/<name>`: unique across the kinds of one list.
    pub key: Arc<str>,
}

impl Row {
    pub fn new(object: FluxObject) -> Row {
        let state = object.state();
        let message = object.message();
        let source = match &object.source {
            Some(source) => source.label(&object.namespace),
            None => crate::details::source_url(&object)
                .or_else(|| {
                    object
                        .raw
                        .pointer("/spec/image")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .or_else(|| {
                    object
                        .raw
                        .pointer("/spec/type")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_default(),
        };
        let revision = object
            .revision()
            .map(|r| short_revision(&r))
            .unwrap_or_default();
        let key: Arc<str> = format!(
            "{}/{}/{}",
            object.kind.kind(),
            object.namespace,
            object.name
        )
        .into();
        Row {
            state,
            message,
            source,
            revision,
            key,
            object,
        }
    }

    pub fn name(&self) -> &str {
        &self.object.name
    }

    pub fn namespace(&self) -> &str {
        &self.object.namespace
    }

    pub fn kind(&self) -> FluxKind {
        self.object.kind
    }
}

/// What a list shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filters {
    /// Empty: every state.
    pub states: BTreeSet<State>,
    pub namespace: Option<String>,
    /// Empty: every kind of the list.
    pub kinds: BTreeSet<FluxKind>,
    pub text: String,
}

impl Filters {
    /// Matches everything but the state chips (their counts follow the other filters).
    pub fn matches_scope(&self, row: &Row) -> bool {
        if self
            .namespace
            .as_ref()
            .is_some_and(|ns| ns != row.namespace())
        {
            return false;
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&row.kind()) {
            return false;
        }
        let text = self.text.trim().to_lowercase();
        text.is_empty()
            || [
                row.name(),
                row.namespace(),
                row.source.as_str(),
                row.revision.as_str(),
                row.message.as_str(),
                row.kind().kind(),
            ]
            .iter()
            .any(|field| field.to_lowercase().contains(&text))
    }

    pub fn matches(&self, row: &Row) -> bool {
        self.matches_scope(row) && (self.states.is_empty() || self.states.contains(&row.state))
    }
}

/// Rows per state (in [`State::ALL`] order).
pub fn state_counts<'a>(rows: impl IntoIterator<Item = &'a Row>) -> Vec<(State, usize)> {
    let mut counts: Vec<(State, usize)> = State::ALL.into_iter().map(|s| (s, 0)).collect();
    for row in rows {
        if let Some((_, n)) = counts.iter_mut().find(|(s, _)| *s == row.state) {
            *n += 1;
        }
    }
    counts
}

/// The namespaces of `rows`, sorted.
pub fn namespaces(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|r| r.namespace().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Sorts by a column (`name`, `namespace`, `state`, `source`, `revision`, `interval`, `last`,
/// `age`); problems first when sorting by state.
pub fn sort(rows: &mut [Row], column: &str, ascending: bool) {
    let rank = |s: State| match s {
        State::Stalled => 0,
        State::Failed => 1,
        State::Reconciling => 2,
        State::Unknown => 3,
        State::Suspended => 4,
        State::Ready => 5,
    };
    rows.sort_by(|a, b| {
        let order = match column {
            "namespace" => a.namespace().cmp(b.namespace()),
            "state" => rank(a.state).cmp(&rank(b.state)),
            "source" => a.source.cmp(&b.source),
            "revision" => a.revision.cmp(&b.revision),
            "interval" => a.object.interval.cmp(&b.object.interval),
            // Newest first when ascending: what changed last.
            "last" => b.object.last_reconcile().cmp(&a.object.last_reconcile()),
            "age" => b.object.created.cmp(&a.object.created),
            "kind" => a.kind().cmp(&b.kind()),
            _ => std::cmp::Ordering::Equal,
        }
        .then_with(|| a.name().cmp(b.name()))
        .then_with(|| a.namespace().cmp(b.namespace()));
        if ascending { order } else { order.reverse() }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use serde_json::Value;

    fn row(value: Value) -> Row {
        Row::new(FluxObject::parse(&Arc::new(value)).unwrap())
    }

    #[test]
    fn rows_filters_and_sorting() {
        let mut rows = vec![
            row(fixtures::kustomization_ready()),
            row(fixtures::kustomization_failed()),
            row(fixtures::kustomization_suspended()),
            row(fixtures::git_repository()),
            row(fixtures::image_repository()),
        ];
        assert_eq!(rows[0].source, "GitRepository/podinfo");
        assert_eq!(rows[0].revision, "master@sha1:3e0ff8a");
        assert_eq!(rows[3].source, "https://github.com/stefanprodan/podinfo");
        assert_eq!(rows[4].source, "ghcr.io/stefanprodan/podinfo");
        sort(&mut rows, "state", true);
        assert_eq!(rows[0].name(), "broken");
        let mut filters = Filters {
            states: [State::Ready].into(),
            ..Default::default()
        };
        assert_eq!(rows.iter().filter(|r| filters.matches(r)).count(), 3);
        filters.text = "PODINFO".into();
        filters.kinds = [FluxKind::GitRepository].into();
        assert_eq!(rows.iter().filter(|r| filters.matches(r)).count(), 1);
        let counts = state_counts(&rows);
        assert_eq!(counts[0], (State::Ready, 3));
        assert_eq!(namespaces(&rows), ["flux-demo"]);
        let filters = Filters {
            namespace: Some("other".into()),
            ..Default::default()
        };
        assert!(!filters.matches(&rows[0]));
    }
}
