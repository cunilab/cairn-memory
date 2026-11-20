use crate::legacy::{Memory, MemoryKind, RecallFilter, StoreError};
use std::collections::{HashMap, HashSet};

pub const SEMANTIC_CANDIDATES: usize = 512;
const RRF_K: f32 = 60.0;
const PINNED_WEIGHT: f32 = 2.0;
const RULE_DECISION_WEIGHT: f32 = 3.0;
const NOTE_WEIGHT: f32 = 2.0;
const CAPTURE_WEIGHT: f32 = 1.0;
const TITLE_WEIGHT: usize = 8;
const ENTITY_EXACT_WEIGHT: f32 = 4.0;

/// Inputs must already satisfy authorization, lifecycle and context filters.
pub fn hybrid_rank(
    memories: &[Memory],
    query: &str,
    filter: &RecallFilter,
    lexical: Option<Vec<(String, f32)>>,
    mut semantic: Vec<(String, f32)>,
) -> Result<Vec<(String, f32)>, StoreError> {
    if query.trim().is_empty() {
        return Err(StoreError::Validation("query must not be empty".to_owned()));
    }
    let by_id = memories
        .iter()
        .map(|m| (m.memory_id.as_str(), m))
        .collect::<HashMap<_, _>>();
    let terms = tokenize(query);
    let lexical = lexical.unwrap_or_else(|| {
        memories
            .iter()
            .map(|m| (m.memory_id.clone(), lexical_score(m, &terms)))
            .collect()
    });
    let entities = memories
        .iter()
        .map(|m| (m.memory_id.clone(), entity_score(m, query, filter)))
        .collect();
    semantic
        .retain(|(id, score)| by_id.contains_key(id.as_str()) && score.is_finite() && *score > 0.0);
    sort_channel(&mut semantic);
    semantic.truncate(SEMANTIC_CANDIDATES);
    let mut scores = HashMap::new();
    for channel in [lexical, entities, semantic] {
        add_channel(&mut scores, channel, &by_id);
    }
    // Expand one hop from the fused seeds, never from newly discovered neighbours.
    let mut seeds = scores
        .iter()
        .map(|(id, score)| (id.clone(), *score))
        .collect::<Vec<_>>();
    sort_channel(&mut seeds);
    seeds.truncate(SEMANTIC_CANDIDATES);
    let mut neighbours = HashMap::<String, f32>::new();
    let mut sources: HashMap<(Option<&str>, &str), Vec<&str>> = HashMap::new();
    let mut incoming: HashMap<&str, Vec<&str>> = HashMap::new();
    for m in memories {
        for id in &m.source_event_ids {
            sources
                .entry((m.fields.author.as_deref(), id))
                .or_default()
                .push(&m.memory_id);
        }
        for id in &m.fields.evidence {
            if by_id.contains_key(id.as_str()) {
                incoming.entry(id).or_default().push(&m.memory_id);
            }
        }
    }
    let mut typed_out: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut typed_in: HashMap<&str, Vec<&str>> = HashMap::new();
    for memory in memories {
        for link in &memory.links {
            let Some(target) = by_id.get(link.target_id.as_str()) else {
                continue;
            };
            if !crate::legacy::link_is_readable(memory, target) {
                continue;
            }
            typed_out
                .entry(memory.memory_id.as_str())
                .or_default()
                .push(link.target_id.as_str());
            typed_in
                .entry(link.target_id.as_str())
                .or_default()
                .push(memory.memory_id.as_str());
        }
    }
    for (id, score) in seeds {
        let Some(seed) = by_id.get(id.as_str()) else {
            continue;
        };
        let mut linked = HashSet::new();
        for source in &seed.source_event_ids {
            if let Some(ids) = sources.get(&(seed.fields.author.as_deref(), source.as_str())) {
                linked.extend(ids.iter().copied());
            }
        }
        linked.extend(
            seed.fields
                .evidence
                .iter()
                .map(String::as_str)
                .filter(|id| by_id.contains_key(id)),
        );
        if let Some(ids) = incoming.get(id.as_str()) {
            linked.extend(ids.iter().copied());
        }
        if let Some(ids) = typed_out.get(id.as_str()) {
            linked.extend(ids.iter().copied());
        }
        if let Some(ids) = typed_in.get(id.as_str()) {
            linked.extend(ids.iter().copied());
        }
        linked.remove(id.as_str());
        for target in linked {
            *neighbours.entry(target.to_owned()).or_default() += score;
        }
    }
    let mut links = neighbours.into_iter().collect::<Vec<_>>();
    sort_channel(&mut links);
    links.truncate(SEMANTIC_CANDIDATES);
    add_channel(&mut scores, links, &by_id);
    let mut results = scores
        .into_iter()
        .map(|(id, score)| {
            let m = by_id[id.as_str()];
            let weight = match m.kind {
                MemoryKind::Rule | MemoryKind::Decision => RULE_DECISION_WEIGHT,
                MemoryKind::Capture => CAPTURE_WEIGHT,
                _ => NOTE_WEIGHT,
            };
            (
                id,
                score * weight * if m.fields.pinned { PINNED_WEIGHT } else { 1.0 },
            )
        })
        .collect::<Vec<_>>();
    sort_channel(&mut results);
    Ok(results)
}

fn add_channel(
    scores: &mut HashMap<String, f32>,
    mut channel: Vec<(String, f32)>,
    eligible: &HashMap<&str, &Memory>,
) {
    channel.retain(|(id, score)| {
        eligible.contains_key(id.as_str()) && score.is_finite() && *score > 0.0
    });
    sort_channel(&mut channel);
    let mut seen = HashSet::new();
    channel.retain(|(id, _)| seen.insert(id.clone()));
    for (rank, (id, _)) in channel.into_iter().enumerate() {
        *scores.entry(id).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
    }
}

fn sort_channel(channel: &mut [(String, f32)]) {
    channel.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
}

pub(crate) fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 2)
        .map(str::to_lowercase)
        .collect()
}

fn lexical_score(m: &Memory, terms: &[String]) -> f32 {
    let title = m.fields.title.to_lowercase();
    let text = format!(
        "{} {} {} {} {} {} {} {} {} {} {} {}",
        m.content,
        m.fields.summary,
        m.fields.details.as_deref().unwrap_or_default(),
        m.fields.reason,
        m.context.repository.as_deref().unwrap_or_default(),
        m.context.branch.as_deref().unwrap_or_default(),
        m.context.worktree.as_deref().unwrap_or_default(),
        m.context.files.join(" "),
        m.context.symbols.join(" "),
        m.context.commands.join(" "),
        m.context.tests.join(" "),
        m.context.services.join(" ")
    )
    .to_lowercase();
    terms
        .iter()
        .map(|t| (TITLE_WEIGHT * title.matches(t).count() + text.matches(t).count()) as f32)
        .sum()
}

fn entity_score(m: &Memory, query: &str, filter: &RecallFilter) -> f32 {
    let values = m
        .fields
        .applies_to
        .iter()
        .chain(&m.fields.evidence)
        .chain(&m.fields.tags)
        .chain(&m.fields.entities)
        .chain(&m.context.files)
        .chain(&m.context.symbols)
        .chain(&m.context.commands)
        .chain(&m.context.tests)
        .chain(&m.context.services);
    let query = query.trim().to_lowercase();
    let terms = tokenize(&query);
    let context = filter.context.as_ref();
    values
        .map(|value| {
            let value = value.to_lowercase();
            let requested = context.is_some_and(|c| {
                c.files
                    .iter()
                    .chain(&c.symbols)
                    .chain(&c.commands)
                    .any(|s| s.to_lowercase() == value)
            });
            if requested || value == query {
                ENTITY_EXACT_WEIGHT
            } else {
                terms.iter().filter(|t| value.contains(t.as_str())).count() as f32
            }
        })
        .sum()
}

#[cfg(test)]
mod tests;
