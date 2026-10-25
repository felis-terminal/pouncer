//! Labels are prefix-free so a pick resolves on its last key, with no
//! confirmation key (docs/design.md "The optimal label algorithm").

#[must_use]
pub(crate) fn labels(count: usize, alphabet: &str) -> Vec<String> {
    let letters: Vec<char> = alphabet.chars().collect();
    let mut singles: Vec<String> = letters.iter().map(ToString::to_string).collect();
    let mut expanded: Vec<String> = Vec::new();

    // Grow the label set toward `count` by turning the last remaining
    // single-character label into a row of two-character labels. Consuming
    // from the end keeps the earliest matches on the shortest labels, and
    // dropping the prefix from `singles` is what keeps the set prefix-free.
    while singles.len() + expanded.len() < count {
        let Some(prefix) = singles.pop() else { break };
        let need = count - singles.len() - expanded.len();
        let mut two: Vec<String> = letters
            .iter()
            .map(|c| format!("{prefix}{c}"))
            .take(need)
            .collect();
        two.append(&mut expanded);
        expanded = two;
    }

    let mut result = singles;
    result.extend(expanded);
    result.truncate(count);
    result
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Assignment<'a> {
    pub(crate) alphabet: &'a str,
    pub(crate) reverse: bool,
    pub(crate) unique: bool,
}

/// `None` when the alphabet cannot form enough one- and two-character labels
/// for the distinct matches.
#[must_use]
pub(crate) fn assign(texts: &[&str], opts: &Assignment) -> Vec<Option<String>> {
    let mut slot_of_text: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut slot_of_match: Vec<usize> = Vec::with_capacity(texts.len());
    for &text in texts {
        let slot = if opts.unique {
            let next = slot_of_text.len();
            *slot_of_text.entry(text).or_insert(next)
        } else {
            slot_of_match.len()
        };
        slot_of_match.push(slot);
    }
    let slot_count = if opts.unique {
        slot_of_text.len()
    } else {
        texts.len()
    };

    let pool = labels(slot_count, opts.alphabet);
    slot_of_match
        .into_iter()
        .map(|slot| {
            let index = if opts.reverse {
                slot_count - 1 - slot
            } else {
                slot
            };
            pool.get(index).cloned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Assignment, assign, labels};

    const FOUR: Assignment<'static> = Assignment {
        alphabet: "abcd",
        reverse: false,
        unique: false,
    };

    fn some(labels: &[&str]) -> Vec<Option<String>> {
        labels.iter().map(|&l| Some(l.to_owned())).collect()
    }

    #[test]
    fn assigns_shortest_labels_to_first_matches_by_default() {
        assert_eq!(assign(&["x", "y", "z"], &FOUR), some(&["a", "b", "c"]));
    }

    #[test]
    fn reverse_gives_shortest_labels_to_last_matches() {
        let opts = Assignment {
            reverse: true,
            ..FOUR
        };
        assert_eq!(assign(&["x", "y", "z"], &opts), some(&["c", "b", "a"]));
    }

    #[test]
    fn non_unique_labels_every_occurrence_separately() {
        assert_eq!(assign(&["x", "y", "x"], &FOUR), some(&["a", "b", "c"]));
    }

    #[test]
    fn unique_shares_one_label_across_equal_texts() {
        let opts = Assignment {
            unique: true,
            ..FOUR
        };
        assert_eq!(assign(&["x", "y", "x"], &opts), some(&["a", "b", "a"]));
    }

    #[test]
    fn unique_and_reverse_combine() {
        let opts = Assignment {
            unique: true,
            reverse: true,
            ..FOUR
        };
        assert_eq!(assign(&["x", "y", "x"], &opts), some(&["b", "a", "b"]));
    }

    // Reference vectors taken from tmux-thumbs' `Alphabet::hints` tests,
    // pinning the exact expansion order we deliberately reuse.
    #[test]
    fn expands_overflow_to_two_char_labels_from_alphabet_end() {
        assert_eq!(labels(6, "abcd"), ["a", "b", "c", "da", "db", "dc"]);
    }

    #[test]
    fn fills_two_char_labels_when_count_exceeds_alphabet() {
        assert_eq!(
            labels(8, "abcd"),
            ["a", "b", "ca", "cb", "da", "db", "dc", "dd"]
        );
    }

    #[test]
    fn uses_only_two_char_labels_for_a_two_letter_alphabet() {
        assert_eq!(labels(8, "ab"), ["aa", "ab", "ba", "bb"]);
    }
}

/// The reference vectors in `tests` pin one alphabet; these assert the
/// invariants that make *any* label set usable (exactly `count` labels,
/// unique, prefix-free, at most two characters, every character in the
/// alphabet) for arbitrary alphabets and counts.
#[cfg(test)]
mod proptests {
    use super::{Assignment, assign, labels};
    use proptest::prelude::*;

    /// Distinct lowercase letters, 2..=10 of them, as an alphabet string.
    fn alphabet_strategy() -> impl Strategy<Value = String> {
        proptest::collection::hash_set(proptest::char::range('a', 'z'), 2..=10)
            .prop_map(|set| set.into_iter().collect::<String>())
    }

    proptest! {
        #[test]
        fn labels_are_unique_prefix_free_and_in_alphabet(
            alphabet in alphabet_strategy(),
            count in 0usize..200,
        ) {
            let k = alphabet.chars().count();
            let capacity = k * k; // one- plus two-character labels
            let want = count.min(capacity);
            let got = labels(count, &alphabet);

            // Exactly `count` labels whenever the alphabet can form that many.
            prop_assert_eq!(got.len(), want);

            // Unique.
            let unique: std::collections::HashSet<&String> = got.iter().collect();
            prop_assert_eq!(unique.len(), got.len());

            for label in &got {
                // At most two characters (expansion caps at two).
                prop_assert!((1..=2).contains(&label.chars().count()));
                // Every character comes from the alphabet.
                prop_assert!(label.chars().all(|c| alphabet.contains(c)));
            }

            // Prefix-free: no label is a prefix of a different label.
            for (i, a) in got.iter().enumerate() {
                for (j, b) in got.iter().enumerate() {
                    if i != j {
                        prop_assert!(!b.starts_with(a.as_str()));
                    }
                }
            }
        }
    }

    proptest! {
        #[test]
        fn assign_is_parallel_and_honours_unique(
            alphabet in alphabet_strategy(),
            // A small text alphabet so duplicates actually occur.
            texts in proptest::collection::vec("[a-c]{1,2}", 0..30),
            reverse in any::<bool>(),
            unique in any::<bool>(),
        ) {
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            let opts = Assignment { alphabet: &alphabet, reverse, unique };
            let got = assign(&refs, &opts);

            // One label per match, in order.
            prop_assert_eq!(got.len(), refs.len());

            if unique {
                // Equal texts always receive equal labels.
                let mut seen: std::collections::HashMap<&str, &Option<String>> =
                    std::collections::HashMap::new();
                for (t, l) in refs.iter().zip(&got) {
                    match seen.get(t) {
                        Some(prev) => prop_assert_eq!(*prev, l),
                        None => { seen.insert(t, l); }
                    }
                }
            }
        }
    }
}
