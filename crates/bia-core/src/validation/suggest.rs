//! "Did you mean" suggestions for misspelt names.

/// The candidate closest to `name` by edit distance, if one is close enough to be a typo:
/// at most one edit per three characters, and at least one. Ties go to the candidate listed
/// first.
pub(super) fn closest<'c>(
    name: &str,
    candidates: impl IntoIterator<Item = &'c str>,
) -> Option<&'c str> {
    let threshold = (name.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .filter(|candidate| *candidate != name)
        .map(|candidate| (distance(name, candidate), candidate))
        .filter(|(distance, _)| *distance <= threshold)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate)
}

/// The suggestion text for a name close to `name`.
pub(super) fn did_you_mean<'c>(
    name: &str,
    candidates: impl IntoIterator<Item = &'c str>,
) -> Option<String> {
    closest(name, candidates).map(|found| format!("Did you mean '{found}'?"))
}

/// Optimal string alignment distance: insertions, deletions, substitutions and swaps of two
/// adjacent characters each count as one edit.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut before_previous = previous.clone();
    for i in 1..=a.len() {
        let mut current = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let substitution = usize::from(a[i - 1] != b[j - 1]);
            current[j] = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + substitution);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                current[j] = current[j].min(before_previous[j - 2] + 1);
            }
        }
        before_previous = std::mem::replace(&mut previous, current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_counts_single_edits() {
        assert_eq!(distance("emails", "emails"), 0);
        assert_eq!(distance("emals", "emails"), 1);
        assert_eq!(distance("emialS", "emails"), 2);
        assert_eq!(distance("htpt", "http"), 1);
        assert_eq!(distance("", "abc"), 3);
    }

    #[test]
    fn only_close_candidates_are_suggested() {
        let candidates = ["emails", "signups", "emails-dlq"];
        assert_eq!(closest("emals", candidates), Some("emails"));
        assert_eq!(closest("signup", candidates), Some("signups"));
        assert_eq!(closest("orders", candidates), None);
        assert_eq!(closest("emails", candidates), None);
        assert_eq!(closest("ab", ["xy"]), None);
    }
}
