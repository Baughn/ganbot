//! Provider-neutral URL extraction. Used by OpenRouter to forward image
//! URLs server-side, and by the OpenAI image path to download referenced
//! images client-side before uploading them as multipart input.

/// Extract URLs from a body of text.
pub fn extract_urls(text: &str) -> Vec<String> {
    let url_regex =
        regex::Regex::new(r"https?://[^\s]+").expect("URL regex must be valid at compile time");

    url_regex
        .find_iter(text)
        .filter_map(|m| {
            let mut candidate = m.as_str();

            const TRAILING_PUNCT: &[char] =
                &[',', '.', ';', '!', '?', ')', ']', '}', '"', '\'', ':', '>'];

            while !candidate.is_empty() && candidate.ends_with(TRAILING_PUNCT) {
                candidate = &candidate[..candidate.len() - 1];
            }

            if !candidate.is_empty() && url::Url::parse(candidate).is_ok() {
                Some(candidate.to_string())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_urls_handles_common_trailing_characters() {
        let text = "Look at https://brage.info/vacation.jpg, and also https://example.com/test.";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://brage.info/vacation.jpg".to_string(),
                "https://example.com/test".to_string()
            ]
        );
    }

    #[test]
    fn extract_urls_handles_parentheses_and_quotes() {
        let text = "(https://brage.info/vacation.jpg) and \"https://example.com/next\"";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://brage.info/vacation.jpg".to_string(),
                "https://example.com/next".to_string()
            ]
        );
    }
}
