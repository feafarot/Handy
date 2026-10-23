//! Works out which words are worth adding to the dictionary.
//!
//! Two sources feed this: the user correcting a dictation after it was
//! pasted (`extract_corrections`, a port of OpenWhispr's correction learner)
//! and an LLM naming the uncommon terms it saw (`filter_terms`). Both are
//! pure functions over text so they can be tested without a window or a
//! model.

use std::collections::HashSet;

use once_cell::sync::Lazy;
use strsim::levenshtein;

/// Everyday words that never belong in the dictionary. Swapping one of them
/// for another ("why" -> "what") is a content edit, not vocabulary; an LLM
/// listing one as a "term" is noise.
static COMMON_WORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    "the and for not with you this but his from they say her she will one all would \
     there their what out about who get which when make can like time just him know \
     take into year your good some could them see other than then now look only come \
     over think also back after use two how our work first well way even new want \
     because any these give day most are was were been has had did does said went \
     made got came took saw knew thought where why here very much many still too again \
     off down never every own same another both each few more less last next while \
     before through under between should might must being have that its yes okay \
     hello thanks thank please sorry today tomorrow yesterday morning evening night \
     week month meeting call email message note notes update updates team project \
     file files folder code test tests build release version user users data \
     something anything nothing everything someone anyone everyone"
        .split_whitespace()
        .collect()
});

/// The longest word the dictionary keeps, matching the settings UI.
pub const MAX_WORD_CHARS: usize = 50;

/// Trims, collapses whitespace and strips characters that would break a
/// prompt. `None` when nothing usable is left.
pub fn normalize_word(word: &str) -> Option<String> {
    let cleaned: String = word
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '<' | '>' | '"' | '\'' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}'
            )
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '&' && c != '#' && c != '+')
        .to_string();
    if cleaned.is_empty() || cleaned.chars().count() > MAX_WORD_CHARS {
        return None;
    }
    Some(cleaned)
}

/// The case-insensitive identity of a word, used to spot duplicates.
pub fn word_key(word: &str) -> String {
    word.to_lowercase()
}

fn is_common(word: &str) -> bool {
    COMMON_WORDS.contains(word.to_lowercase().as_str())
}

/// Words of `text`, punctuation stripped from both ends, in order.
fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// The part of `field` that holds the pasted `original`. The field may
/// contain a whole document; only the dictation's own region is diffed.
fn edited_region(original: &str, field: &str) -> String {
    if field.chars().count() <= original.chars().count() * 3 / 2 {
        return field.to_string();
    }
    if field.contains(original) {
        return original.to_string();
    }

    // Sliding window over the field's words: the window with the most words
    // in common with the original is where the dictation went.
    let original_words = tokenize(original);
    let field_words = tokenize(field);
    let window = original_words.len();
    if window == 0 || field_words.len() <= window {
        return field.to_string();
    }

    let original_lower: Vec<String> = original_words.iter().map(|w| w.to_lowercase()).collect();
    let field_lower: Vec<String> = field_words.iter().map(|w| w.to_lowercase()).collect();
    let mut best_start = 0;
    let mut best_score = 0;
    for start in 0..=field_words.len() - window {
        let matches = (0..window)
            .filter(|&i| field_lower[start + i] == original_lower[i])
            .count();
        if matches > best_score {
            best_score = matches;
            best_start = start;
        }
    }
    // Less than 30% overlap: the dictation is not recognizably there.
    if best_score * 10 < window * 3 {
        return field.to_string();
    }
    field_words[best_start..best_start + window].join(" ")
}

/// `(original word, edited word)` pairs where one word was replaced by
/// another, found by aligning the two word lists on their longest common
/// subsequence.
fn substitutions(original: &[String], edited: &[String]) -> Vec<(String, String)> {
    let m = original.len();
    let n = edited.len();
    let eq = |i: usize, j: usize| original[i].to_lowercase() == edited[j].to_lowercase();

    let mut lcs = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            lcs[i][j] = if eq(i - 1, j - 1) {
                lcs[i - 1][j - 1] + 1
            } else {
                lcs[i - 1][j].max(lcs[i][j - 1])
            };
        }
    }

    // Walk back: (Some, Some) is a match, (Some, None) a deletion,
    // (None, Some) an insertion.
    let mut aligned: Vec<(Option<&String>, Option<&String>)> = Vec::new();
    let (mut i, mut j) = (m, n);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && eq(i - 1, j - 1) {
            aligned.push((Some(&original[i - 1]), Some(&edited[j - 1])));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || lcs[i][j - 1] >= lcs[i - 1][j]) {
            aligned.push((None, Some(&edited[j - 1])));
            j -= 1;
        } else {
            aligned.push((Some(&original[i - 1]), None));
            i -= 1;
        }
    }
    aligned.reverse();

    // A deletion directly followed by an insertion is one word replaced by
    // another.
    aligned
        .windows(2)
        .filter_map(|pair| match (pair[0], pair[1]) {
            ((Some(from), None), (None, Some(to))) => Some((from.clone(), to.clone())),
            _ => None,
        })
        .collect()
}

/// The words the user typed in place of what was dictated, when `field` is
/// what the pasted `original` became. Words already in `known` (any case)
/// are not reported again.
///
/// Edits that are not spelling corrections are ignored: replacements of
/// everyday words, very short words, words that look nothing like what they
/// replaced, and edits that rewrote more than half the dictation.
pub fn extract_corrections(original: &str, field: &str, known: &[String]) -> Vec<String> {
    if original.trim().is_empty() || field.trim().is_empty() || original == field {
        return Vec::new();
    }
    let region = edited_region(original, field);
    if region == original {
        return Vec::new();
    }
    let original_words = tokenize(original);
    let edited_words = tokenize(&region);
    if original_words.is_empty() || edited_words.is_empty() {
        return Vec::new();
    }

    let subs = substitutions(&original_words, &edited_words);
    if subs.len() * 2 > original_words.len() {
        return Vec::new();
    }

    let known: HashSet<String> = known.iter().map(|w| word_key(w)).collect();
    let mut seen = HashSet::new();
    let mut corrections = Vec::new();
    for (from, to) in subs {
        let key = word_key(&to);
        if known.contains(&key) || seen.contains(&key) || word_key(&from) == key {
            continue;
        }
        if to.chars().count() < 3 || is_common(&to) {
            continue;
        }
        // 0.65 keeps phonetic corrections ("Shunade" -> "Sinead": 4 of 7
        // characters) and drops unrelated replacements.
        let distance = levenshtein(&from.to_lowercase(), &key);
        let longest = from.chars().count().max(to.chars().count());
        if longest == 0 || distance as f64 / longest as f64 > 0.65 {
            continue;
        }
        seen.insert(key);
        corrections.push(to);
    }
    corrections
}

/// The usable terms among what an LLM (or a speaker list) proposed: cleaned,
/// deduplicated, without everyday words, never more than four words long,
/// and, when `spoken_in` is given, only terms that actually occur in it.
pub fn filter_terms(candidates: &[String], spoken_in: Option<&str>, limit: usize) -> Vec<String> {
    let haystack = spoken_in.map(|text| text.to_lowercase());
    let mut seen = HashSet::new();
    let mut terms = Vec::new();
    for candidate in candidates {
        let Some(term) = normalize_word(candidate) else {
            continue;
        };
        if term.chars().count() < 2 || term.split_whitespace().count() > 4 {
            continue;
        }
        if term
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_punctuation())
        {
            continue;
        }
        if term.split_whitespace().all(is_common) {
            continue;
        }
        if let Some(haystack) = &haystack {
            if !haystack.contains(&term.to_lowercase()) {
                continue;
            }
        }
        if !seen.insert(word_key(&term)) {
            continue;
        }
        terms.push(term);
        if terms.len() >= limit {
            break;
        }
    }
    terms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn learns_a_misheard_name() {
        let original = "I talked to Shunade about the rollout";
        let edited = "I talked to Sinead about the rollout";
        assert_eq!(extract_corrections(original, edited, &[]), vec!["Sinead"]);
    }

    #[test]
    fn ignores_content_edits_and_common_words() {
        let original = "why did the build fail";
        let edited = "what did the build fail";
        assert!(extract_corrections(original, edited, &[]).is_empty());
    }

    #[test]
    fn ignores_unrelated_replacements() {
        let original = "send it to the designer tomorrow";
        let edited = "send it to the Kubernetes tomorrow";
        assert!(extract_corrections(original, edited, &[]).is_empty());
    }

    #[test]
    fn ignores_rewrites() {
        let original = "one two three four";
        let edited = "uno dos tres cuatro";
        assert!(extract_corrections(original, edited, &[]).is_empty());
    }

    #[test]
    fn finds_the_dictation_inside_a_longer_document() {
        let original = "deploy the new grafana dashboard today";
        let before = "Lots of unrelated text that was already in the editor before. ";
        let after = "\n\nAnd a paragraph typed afterwards about something else entirely.";

        // Only the case changed: same word, no correction.
        let edited = format!("{before}deploy the new Grafana dashboard today{after}");
        assert!(extract_corrections(original, &edited, &[]).is_empty());

        let edited = format!("{before}deploy the new Grafarna dashboard today{after}");
        assert_eq!(
            extract_corrections(original, &edited, &[]),
            vec!["Grafarna"]
        );
    }

    #[test]
    fn skips_known_words() {
        let original = "ping Shunade";
        let edited = "ping Sinead";
        assert!(extract_corrections(original, edited, &strings(&["sinead"])).is_empty());
    }

    #[test]
    fn normalizes_words() {
        assert_eq!(normalize_word("  <Handy>  "), Some("Handy".to_string()));
        assert_eq!(
            normalize_word("\"ChargeBee\","),
            Some("ChargeBee".to_string())
        );
        assert_eq!(normalize_word("C#"), Some("C#".to_string()));
        assert_eq!(normalize_word("   "), None);
        assert_eq!(normalize_word(&"x".repeat(60)), None);
    }

    #[test]
    fn filters_llm_terms() {
        let output = "We moved the Kubernetes cluster to Hetzner, Sinead approved it.";
        let terms = filter_terms(
            &strings(&[
                "Kubernetes",
                "the",
                "Hetzner",
                "Sinead",
                "kubernetes",
                "AWS",
                "12",
            ]),
            Some(output),
            10,
        );
        assert_eq!(terms, strings(&["Kubernetes", "Hetzner", "Sinead"]));
    }

    #[test]
    fn term_limit_holds() {
        let terms = filter_terms(&strings(&["Alpha", "Beta", "Gamma"]), None, 2);
        assert_eq!(terms, strings(&["Alpha", "Beta"]));
    }
}
