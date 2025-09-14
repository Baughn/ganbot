//! Fuzzy matching utilities using Levenshtein distance for typo correction

use levenshtein::levenshtein;

#[derive(Debug, Clone)]
pub enum FuzzyResult<T> {
    Exact(T),
    Corrected {
        corrected: T,
        original: String,
    },
    Suggestions {
        candidates: Vec<T>,
        original: String,
    },
    NotFound {
        original: String,
    },
}

impl<T: std::fmt::Debug> FuzzyResult<T> {
    pub fn into_result(self) -> Result<T, String> {
        match self {
            FuzzyResult::Exact(item) => Ok(item),
            FuzzyResult::Corrected { corrected, .. } => Ok(corrected),
            FuzzyResult::Suggestions {
                candidates,
                original,
            } => {
                if candidates.is_empty() {
                    Err(format!("'{}' not found", original))
                } else {
                    let suggestions = candidates
                        .into_iter()
                        .take(5)
                        .map(|c| format!("{:?}", c))
                        .collect::<Vec<_>>()
                        .join(", ");
                    Err(format!("Did you mean: {}?", suggestions))
                }
            }
            FuzzyResult::NotFound { original } => Err(format!("'{}' not found", original)),
        }
    }

    pub fn with_correction_message(self, format_fn: impl Fn(&T) -> String) -> (T, Option<String>)
    where
        T: std::fmt::Debug,
    {
        match self {
            FuzzyResult::Exact(item) => (item, None),
            FuzzyResult::Corrected {
                corrected,
                original,
            } => {
                let message = format!("Corrected '{}' to '{}'", original, format_fn(&corrected));
                (corrected, Some(message))
            }
            _ => panic!("Cannot extract corrected item from non-corrected result"),
        }
    }
}

pub fn find_fuzzy_match<'a, T>(
    input: &str,
    candidates: impl IntoIterator<Item = (&'a str, T)>,
) -> FuzzyResult<T>
where
    T: Clone,
{
    let input_lower = input.to_lowercase();
    let mut distances: Vec<(usize, &str, T)> = candidates
        .into_iter()
        .map(|(candidate, value)| {
            let distance = levenshtein(&input_lower, &candidate.to_lowercase());
            (distance, candidate, value)
        })
        .collect();

    distances.sort_by_key(|(distance, _, _)| *distance);

    if distances.is_empty() {
        return FuzzyResult::NotFound {
            original: input.to_string(),
        };
    }

    let best_distance = distances[0].0;

    if best_distance == 0 {
        return FuzzyResult::Exact(distances[0].2.clone());
    }

    if best_distance == 1 {
        let matches_at_distance_1: Vec<_> = distances.iter().filter(|(d, _, _)| *d == 1).collect();

        if matches_at_distance_1.len() == 1 {
            return FuzzyResult::Corrected {
                corrected: matches_at_distance_1[0].2.clone(),
                original: input.to_string(),
            };
        } else {
            return FuzzyResult::Suggestions {
                candidates: matches_at_distance_1
                    .into_iter()
                    .map(|(_, _, value)| value.clone())
                    .collect(),
                original: input.to_string(),
            };
        }
    }

    if best_distance <= 3 {
        let close_matches: Vec<_> = distances
            .iter()
            .take_while(|(d, _, _)| *d <= 3)
            .take(5)
            .map(|(_, _, value)| value.clone())
            .collect();

        FuzzyResult::Suggestions {
            candidates: close_matches,
            original: input.to_string(),
        }
    } else {
        FuzzyResult::NotFound {
            original: input.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let candidates = vec![("fire", "fire"), ("water", "water"), ("earth", "earth")];
        let result = find_fuzzy_match("fire", candidates);

        match result {
            FuzzyResult::Exact(value) => assert_eq!(value, "fire"),
            _ => panic!("Expected exact match"),
        }
    }

    #[test]
    fn test_single_character_correction() {
        let candidates = vec![("fire", "fire"), ("water", "water"), ("earth", "earth")];
        let result = find_fuzzy_match("fir", candidates);

        match result {
            FuzzyResult::Corrected {
                corrected,
                original,
            } => {
                assert_eq!(corrected, "fire");
                assert_eq!(original, "fir");
            }
            other => panic!("Expected correction, got: {:?}", other),
        }
    }

    #[test]
    fn test_multiple_matches_at_distance_1() {
        // Test case where multiple candidates are at distance 1 from input
        let candidates = vec![("cat", "cat"), ("bat", "bat"), ("rat", "rat")];
        let result = find_fuzzy_match("at", candidates);

        match result {
            FuzzyResult::Suggestions {
                candidates,
                original,
            } => {
                assert_eq!(candidates.len(), 3);
                assert_eq!(original, "at");
            }
            other => panic!("Expected suggestions, got: {:?}", other),
        }
    }

    #[test]
    fn test_suggestions_for_distance_2_3() {
        let candidates = vec![("fire", "fire"), ("water", "water"), ("earth", "earth")];
        let result = find_fuzzy_match("wr", candidates); // "wr" should be distance 2-3 from all

        match result {
            FuzzyResult::Suggestions {
                candidates,
                original,
            } => {
                assert!(candidates.len() > 0);
                assert_eq!(original, "wr");
            }
            other => panic!("Expected suggestions, got: {:?}", other),
        }
    }

    #[test]
    fn test_not_found() {
        let candidates = vec![("fire", "fire"), ("water", "water"), ("earth", "earth")];
        let result = find_fuzzy_match("completely_different", candidates);

        match result {
            FuzzyResult::NotFound { original } => {
                assert_eq!(original, "completely_different");
            }
            _ => panic!("Expected not found"),
        }
    }

    #[test]
    fn test_case_insensitive() {
        let candidates = vec![("Fire", "fire"), ("WATER", "water"), ("Earth", "earth")];
        let result = find_fuzzy_match("fire", candidates);

        match result {
            FuzzyResult::Exact(value) => assert_eq!(value, "fire"),
            _ => panic!("Expected exact match with case insensitive"),
        }
    }
}
