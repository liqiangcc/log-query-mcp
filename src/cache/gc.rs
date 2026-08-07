use std::collections::{HashMap, HashSet};

use thiserror::Error;

use super::generation::{CacheFileId, CacheSourceId, GenerationKey};

#[derive(Debug, Clone)]
pub(crate) struct GcEntry {
    pub key: GenerationKey,
    pub data_len: u64,
    pub created_at_unix_millis: u64,
    pub is_current: bool,
    pub is_pinned: bool,
}

impl GcEntry {
    fn protected(&self) -> bool {
        self.is_current || self.is_pinned
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GcPlan {
    pub removals: Vec<GenerationKey>,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub protected_generations: usize,
}

pub(crate) fn plan_gc(
    entries: &[GcEntry],
    max_bytes: u64,
    max_bytes_per_source: u64,
    retention_millis: u64,
    max_generations_per_file: usize,
    now_unix_millis: u64,
) -> Result<GcPlan, GcPlanError> {
    let bytes_before = total_bytes(entries, &HashSet::new());
    let mut removals = HashSet::new();

    let retention_cutoff = now_unix_millis.saturating_sub(retention_millis);
    for entry in entries {
        if !entry.protected() && entry.created_at_unix_millis < retention_cutoff {
            removals.insert(entry.key.clone());
        }
    }

    enforce_generation_limit(entries, &mut removals, max_generations_per_file)?;
    enforce_source_quota(entries, &mut removals, max_bytes_per_source)?;
    enforce_global_quota(entries, &mut removals, max_bytes)?;

    let bytes_after = total_bytes(entries, &removals);
    let mut ordered_removals = removals.into_iter().collect::<Vec<_>>();
    ordered_removals.sort();

    Ok(GcPlan {
        removals: ordered_removals,
        bytes_before,
        bytes_after,
        protected_generations: entries.iter().filter(|entry| entry.protected()).count(),
    })
}

fn enforce_generation_limit(
    entries: &[GcEntry],
    removals: &mut HashSet<GenerationKey>,
    max_generations_per_file: usize,
) -> Result<(), GcPlanError> {
    let mut groups: HashMap<(CacheSourceId, CacheFileId), Vec<&GcEntry>> = HashMap::new();
    for entry in entries {
        groups
            .entry((entry.key.source_id.clone(), entry.key.file_id.clone()))
            .or_default()
            .push(entry);
    }

    for group in groups.values_mut() {
        group.sort_by_key(|entry| entry.created_at_unix_millis);
        while group
            .iter()
            .filter(|entry| !removals.contains(&entry.key))
            .count()
            > max_generations_per_file
        {
            let Some(candidate) = group
                .iter()
                .find(|entry| !entry.protected() && !removals.contains(&entry.key))
            else {
                return Err(GcPlanError::LimitExceeded);
            };
            removals.insert(candidate.key.clone());
        }
    }
    Ok(())
}

fn enforce_source_quota(
    entries: &[GcEntry],
    removals: &mut HashSet<GenerationKey>,
    max_bytes_per_source: u64,
) -> Result<(), GcPlanError> {
    let mut sources: HashMap<CacheSourceId, Vec<&GcEntry>> = HashMap::new();
    for entry in entries {
        sources
            .entry(entry.key.source_id.clone())
            .or_default()
            .push(entry);
    }

    for source_entries in sources.values_mut() {
        source_entries.sort_by_key(|entry| entry.created_at_unix_millis);
        while source_entries
            .iter()
            .filter(|entry| !removals.contains(&entry.key))
            .map(|entry| entry.data_len)
            .sum::<u64>()
            > max_bytes_per_source
        {
            let Some(candidate) = source_entries
                .iter()
                .find(|entry| !entry.protected() && !removals.contains(&entry.key))
            else {
                return Err(GcPlanError::LimitExceeded);
            };
            removals.insert(candidate.key.clone());
        }
    }
    Ok(())
}

fn enforce_global_quota(
    entries: &[GcEntry],
    removals: &mut HashSet<GenerationKey>,
    max_bytes: u64,
) -> Result<(), GcPlanError> {
    let mut ordered = entries.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|entry| entry.created_at_unix_millis);

    while total_bytes(entries, removals) > max_bytes {
        let Some(candidate) = ordered
            .iter()
            .find(|entry| !entry.protected() && !removals.contains(&entry.key))
        else {
            return Err(GcPlanError::LimitExceeded);
        };
        removals.insert(candidate.key.clone());
    }
    Ok(())
}

fn total_bytes(entries: &[GcEntry], removals: &HashSet<GenerationKey>) -> u64 {
    entries
        .iter()
        .filter(|entry| !removals.contains(&entry.key))
        .map(|entry| entry.data_len)
        .sum()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum GcPlanError {
    #[error("cache limits cannot be satisfied without deleting protected generations")]
    LimitExceeded,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::generation::{CacheFileId, CacheSourceId, GenerationId};

    fn entry(
        source: &CacheSourceId,
        file: &CacheFileId,
        created: u64,
        current: bool,
        pinned: bool,
    ) -> GcEntry {
        GcEntry {
            key: GenerationKey::new(source.clone(), file.clone(), GenerationId::new()),
            data_len: 10,
            created_at_unix_millis: created,
            is_current: current,
            is_pinned: pinned,
        }
    }

    #[test]
    fn pinned_generation_is_not_selected() {
        let source = CacheSourceId::new();
        let file = CacheFileId::new();
        let pinned = entry(&source, &file, 1, false, true);
        let removable = entry(&source, &file, 2, false, false);
        let current = entry(&source, &file, 3, true, false);
        let entries = vec![pinned.clone(), removable.clone(), current];

        let plan = plan_gc(&entries, 20, 20, 0, 2, 3).expect("plan");
        assert!(!plan.removals.contains(&pinned.key));
        assert!(plan.removals.contains(&removable.key));
    }

    #[test]
    fn quota_failure_is_explicit_when_everything_is_protected() {
        let source = CacheSourceId::new();
        let file = CacheFileId::new();
        let entries = vec![entry(&source, &file, 1, true, false)];

        assert_eq!(
            plan_gc(&entries, 5, 5, 0, 2, 1),
            Err(GcPlanError::LimitExceeded)
        );
    }
}
