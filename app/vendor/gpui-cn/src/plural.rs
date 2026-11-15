//! The one rule for a count and its noun: "1 file", "2 files", "0 files".

/// `count` and `noun`, with an `s` after the noun unless the count is one.
pub(crate) fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::counted;

    #[test]
    fn a_count_of_one_is_singular_and_the_rest_are_plural() {
        assert_eq!(counted(0, "model"), "0 models");
        assert_eq!(counted(1, "model"), "1 model");
        assert_eq!(counted(2, "model"), "2 models");
        assert_eq!(counted(25, "file"), "25 files");
    }
}
